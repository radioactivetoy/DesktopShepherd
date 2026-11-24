//! Box windows drawn by Windows' composition system (the "DirectX" boxes).
//!
//! Each box is a plain Win32 window without its own surface. Windows composes it from layers:
//!   1. the frosted desktop behind it (Windows' own backdrop blur, on the GPU), faded in by the
//!      box's blur amount;
//!   2. the box's tint colour;
//!   3. the contents (icons, text, tabs), drawn by egui with Direct3D 11 into a transparent
//!      swap chain;
//! all clipped to a rounded rectangle. The window is truly see-through: Windows blends it with
//! whatever is behind, live, and moves it without any redrawing.
//!
//! Input arrives in the window procedure, is turned into egui events and wakes the app; on the
//! app's next pass every box runs one egui frame (sharing one egui context, one viewport per
//! box) and draws it.

#![cfg(windows)]

use crate::d3d::Renderer;
use egui::{self, Pos2, Rect, Vec2, ViewportCommand, ViewportId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use windows::core::{w, Interface, Result, HSTRING};
use windows::Foundation::Numerics::Vector2;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_HOSTBACKDROPBRUSH};
use windows::Win32::Graphics::Dxgi::IDXGISwapChain1;
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::WinRT::Composition::{ICompositorDesktopInterop, ICompositorInterop};
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, DispatcherQueueOptions, RoInitialize, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT,
    RO_INIT_SINGLETHREADED,
};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT, VK_BACK, VK_CONTROL, VK_DELETE,
    VK_DOWN, VK_END, VK_ESCAPE, VK_HOME, VK_LEFT, VK_MENU, VK_RETURN, VK_RIGHT, VK_SHIFT, VK_TAB, VK_UP,
};
use windows::Win32::UI::Shell::{DragAcceptFiles, DragFinish, DragQueryFileW, HDROP};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::UI::Color;
use windows::UI::Composition::{
    CompositionColorBrush, CompositionRoundedRectangleGeometry, Compositor, ContainerVisual, SpriteVisual,
};
use windows::UI::Composition::Desktop::DesktopWindowTarget;

/// What a box window has received since its last frame.
#[derive(Default)]
struct Shared {
    events: Vec<egui::Event>,
    focused: bool,
    tracking: bool,
    dropped: Vec<std::path::PathBuf>,
    cursor: egui::CursorIcon,
    no_activate: bool,
    /// A box window (not a menu or settings panel): it must stay behind other programs' windows.
    is_box: bool,
    /// Something happened to the window (input, move, resize, DPI): draw it on the next pass.
    dirty: bool,
    /// Panels only: the close button (or Alt+F4) was used; boxes never close.
    can_close: bool,
    close: bool,
    /// Panels only: smallest size the person can resize to (pixels, 0 = none).
    min_size: [i32; 2],
    /// Windows' own move/resize loop is running (the person drags the window's edge).
    sizing: bool,
    /// First half of a two-part character (see WM_CHAR).
    high_surrogate: Option<u16>,
    /// Where the input method's windows were last placed (client pixels), to move them only
    /// when the text cursor moves.
    ime_at: Option<(i32, i32)>,
    /// The primary button went down in this window and no release has been passed on yet.
    primary_down: bool,
}

/// Places the input method's composition and candidate windows at the text cursor of the
/// text field being typed in (egui reports it), so they appear next to the text.
fn place_ime(hwnd: HWND, shared: &Mutex<Shared>, ime: Option<&egui::output::IMEOutput>, ppp: f32) {
    use windows::Win32::UI::Input::Ime::{
        ImmGetContext, ImmReleaseContext, ImmSetCandidateWindow, ImmSetCompositionWindow, CANDIDATEFORM, CFS_CANDIDATEPOS,
        CFS_FORCE_POSITION, CFS_POINT, COMPOSITIONFORM,
    };
    let Some(ime) = ime else {
        if let Ok(mut s) = shared.lock() {
            s.ime_at = None;
        }
        return;
    };
    let at = ((ime.cursor_rect.min.x * ppp).round() as i32, (ime.cursor_rect.max.y * ppp).round() as i32);
    let changed = shared.lock().map(|mut s| s.ime_at.replace(at) != Some(at)).unwrap_or(false);
    if !changed {
        return;
    }
    unsafe {
        let himc = ImmGetContext(hwnd);
        if himc.is_invalid() {
            return;
        }
        let pt = POINT { x: at.0, y: at.1 };
        let cf = COMPOSITIONFORM { dwStyle: CFS_POINT | CFS_FORCE_POSITION, ptCurrentPos: pt, ..Default::default() };
        let _ = ImmSetCompositionWindow(himc, &cf);
        let cand = CANDIDATEFORM { dwIndex: 0, dwStyle: CFS_CANDIDATEPOS, ptCurrentPos: pt, ..Default::default() };
        let _ = ImmSetCandidateWindow(himc, &cand);
        let _ = ImmReleaseContext(hwnd, himc);
    }
}

/// Reads text from the clipboard (for Ctrl+V in text fields).
fn clipboard_get(h: HWND) -> Option<String> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        OpenClipboard(h).ok()?;
        let text = (|| {
            let data = GetClipboardData(CF_UNICODETEXT).ok()?;
            let mem = HGLOBAL(data.0);
            let p = GlobalLock(mem) as *const u16;
            if p.is_null() {
                return None;
            }
            let mut n = 0usize;
            while n < 1 << 24 && *p.add(n) != 0 {
                n += 1;
            }
            let s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
            let _ = GlobalUnlock(mem);
            Some(s)
        })();
        let _ = CloseClipboard();
        text
    }
}

/// Puts text on the clipboard (for Ctrl+C / Ctrl+X in text fields).
fn clipboard_set(h: HWND, text: &str) {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::Foundation::GlobalFree;
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    const CF_UNICODETEXT: u32 = 13;
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        if OpenClipboard(h).is_err() {
            return;
        }
        let _ = EmptyClipboard();
        if let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) {
            let p = GlobalLock(mem) as *mut u16;
            if p.is_null() {
                let _ = GlobalFree(mem);
            } else {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
                let _ = GlobalUnlock(mem);
                // On success the clipboard owns the memory; otherwise free it.
                if SetClipboardData(CF_UNICODETEXT, HANDLE(mem.0)).is_err() {
                    let _ = GlobalFree(mem);
                }
            }
        }
        let _ = CloseClipboard();
    }
}

static WAKE: OnceLock<egui::Context> = OnceLock::new();

/// Shortest time between two passes started by box input. Mouse messages arrive up to ~1000 a
/// second; grouping them into at most one pass per 10 ms keeps boxes smooth (100 per second)
/// while each pass of the GL main window costs far more than the box drawing itself.
pub const MIN_PASS_GAP: Duration = Duration::from_millis(16);

fn wake() {
    if let Some(c) = WAKE.get() {
        c.request_repaint_after(MIN_PASS_GAP);
    }
}

fn modifiers() -> egui::Modifiers {
    unsafe {
        let down = |vk: i32| (GetKeyState(vk) as u16 & 0x8000) != 0;
        let ctrl = down(VK_CONTROL.0 as i32);
        egui::Modifiers { alt: down(VK_MENU.0 as i32), ctrl, shift: down(VK_SHIFT.0 as i32), mac_cmd: false, command: ctrl }
    }
}

fn key_of(vk: u32) -> Option<egui::Key> {
    use egui::Key;
    let k = match vk {
        x if x == VK_RETURN.0 as u32 => Key::Enter,
        x if x == VK_ESCAPE.0 as u32 => Key::Escape,
        x if x == VK_BACK.0 as u32 => Key::Backspace,
        x if x == VK_DELETE.0 as u32 => Key::Delete,
        x if x == VK_TAB.0 as u32 => Key::Tab,
        x if x == VK_LEFT.0 as u32 => Key::ArrowLeft,
        x if x == VK_RIGHT.0 as u32 => Key::ArrowRight,
        x if x == VK_UP.0 as u32 => Key::ArrowUp,
        x if x == VK_DOWN.0 as u32 => Key::ArrowDown,
        x if x == VK_HOME.0 as u32 => Key::Home,
        x if x == VK_END.0 as u32 => Key::End,
        0x21 => Key::PageUp,
        0x22 => Key::PageDown,
        0x20 => Key::Space,
        0x41 => Key::A,
        0x43 => Key::C,
        0x56 => Key::V,
        0x58 => Key::X,
        0x5A => Key::Z,
        _ => return None,
    };
    Some(k)
}

fn ppp_of(h: HWND) -> f32 {
    unsafe { (GetDpiForWindow(h).max(96) as f32) / 96.0 }
}

unsafe extern "system" fn wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let ptr = GetWindowLongPtrW(h, GWLP_USERDATA) as *const Mutex<Shared>;
    if ptr.is_null() {
        return DefWindowProcW(h, m, wp, lp);
    }
    let shared = &*ptr;
    let ppp = ppp_of(h);
    let pos = || {
        let x = (lp.0 & 0xFFFF) as u16 as i16 as f32;
        let y = ((lp.0 >> 16) & 0xFFFF) as u16 as i16 as f32;
        Pos2::new(x / ppp, y / ppp)
    };
    let push = |e: egui::Event| {
        if let Ok(mut s) = shared.lock() {
            s.events.push(e);
            s.dirty = true;
        }
        wake();
    };
    let button = |b: egui::PointerButton, pressed: bool| {
        push(egui::Event::PointerButton { pos: pos(), button: b, pressed, modifiers: modifiers() });
    };
    match m {
        WM_MOUSEMOVE => {
            let start_tracking = shared.lock().map(|mut s| !std::mem::replace(&mut s.tracking, true)).unwrap_or(false);
            if start_tracking {
                let mut t = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: h, dwHoverTime: 0 };
                let _ = TrackMouseEvent(&mut t);
            }
            push(egui::Event::PointerMoved(pos()));
            LRESULT(0)
        }
        0x02A3 /* WM_MOUSELEAVE */ => {
            if let Ok(mut s) = shared.lock() {
                s.tracking = false;
            }
            push(egui::Event::PointerGone);
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            // An icon box is part of the desktop: a click gives the keyboard to the desktop.
            // (Not while the box itself has the keyboard: an icon's name is being edited.)
            if shared.lock().map(|s| s.no_activate).unwrap_or(false) && GetForegroundWindow() != h {
                crate::desktop::focus_desktop();
            }
            SetCapture(h);
            if let Ok(mut s) = shared.lock() {
                s.primary_down = true;
            }
            button(egui::PointerButton::Primary, true);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            // Cleared first: ReleaseCapture sends WM_CAPTURECHANGED right away (see below).
            if let Ok(mut s) = shared.lock() {
                s.primary_down = false;
            }
            let _ = ReleaseCapture();
            button(egui::PointerButton::Primary, false);
            LRESULT(0)
        }
        // The mouse was taken from us (another window grabbed it, a menu or dialog opened, Alt+Tab, a
        // modal loop): the button-up will never arrive here. Tell egui the button was released and
        // the pointer left, or it keeps a drag / press stuck.
        WM_CAPTURECHANGED | WM_CANCELMODE => {
            let was_down = shared.lock().map(|mut s| std::mem::replace(&mut s.primary_down, false)).unwrap_or(false);
            if was_down {
                // Where the pointer is now (lp holds a window handle in WM_CAPTURECHANGED).
                let mut p = POINT::default();
                let at = if GetCursorPos(&mut p).is_ok() && ScreenToClient(h, &mut p).as_bool() {
                    Pos2::new(p.x as f32 / ppp, p.y as f32 / ppp)
                } else {
                    Pos2::ZERO
                };
                push(egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: modifiers() });
                push(egui::Event::PointerGone);
                if m == WM_CANCELMODE {
                    let _ = ReleaseCapture();
                }
            }
            if m == WM_CAPTURECHANGED {
                LRESULT(0)
            } else {
                DefWindowProcW(h, m, wp, lp)
            }
        }
        WM_RBUTTONDOWN => {
            button(egui::PointerButton::Secondary, true);
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            button(egui::PointerButton::Secondary, false);
            LRESULT(0)
        }
        WM_MBUTTONDOWN => {
            button(egui::PointerButton::Middle, true);
            LRESULT(0)
        }
        WM_MBUTTONUP => {
            button(egui::PointerButton::Middle, false);
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let d = ((wp.0 >> 16) & 0xFFFF) as u16 as i16 as f32 / 120.0;
            push(egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Line, delta: Vec2::new(0.0, d), modifiers: modifiers() });
            LRESULT(0)
        }
        WM_CHAR => {
            // Typed text, including what an input method (Chinese, Japanese, ...) produces.
            // Characters outside the basic range (emoji, rare characters) arrive in two halves.
            let unit = wp.0 as u32;
            let text = if (0xD800..0xDC00).contains(&unit) {
                if let Ok(mut s) = shared.lock() {
                    s.high_surrogate = Some(unit as u16);
                }
                None
            } else if (0xDC00..0xE000).contains(&unit) {
                let high = shared.lock().ok().and_then(|mut s| s.high_surrogate.take());
                high.and_then(|hi| char::decode_utf16([hi, unit as u16]).next()).and_then(|r| r.ok())
            } else {
                char::from_u32(unit)
            };
            if let Some(c) = text {
                if !c.is_control() {
                    push(egui::Event::Text(c.to_string()));
                }
            }
            LRESULT(0)
        }
        WM_KEYDOWN | WM_KEYUP => {
            if let Some(key) = key_of(wp.0 as u32) {
                let pressed = m == WM_KEYDOWN;
                let repeat = pressed && (lp.0 >> 30) & 1 == 1;
                let mods = modifiers();
                if pressed && mods.command {
                    match key {
                        egui::Key::C => push(egui::Event::Copy),
                        egui::Key::X => push(egui::Event::Cut),
                        egui::Key::V => {
                            if let Some(text) = clipboard_get(h) {
                                push(egui::Event::Paste(text.replace("\r\n", "\n")));
                            }
                        }
                        _ => {}
                    }
                }
                push(egui::Event::Key { key, physical_key: None, pressed, repeat, modifiers: mods });
            }
            LRESULT(0)
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            let f = m == WM_SETFOCUS;
            if let Ok(mut s) = shared.lock() {
                s.focused = f;
            }
            push(egui::Event::WindowFocused(f));
            LRESULT(0)
        }
        // No visible frame: the whole window is the box (kept harmless for old frame tricks).
        WM_NCCALCSIZE if wp.0 != 0 => LRESULT(0),
        WM_WINDOWPOSCHANGING if lp.0 != 0 => {
            // A widget box takes the keyboard when clicked, and Windows lifts a window that becomes
            // active to the top. Boxes stay behind other windows: drop the z-order part of any move
            // that isn't our own restacking (which always passes SWP_NOACTIVATE).
            if shared.lock().map(|s| s.is_box).unwrap_or(false) {
                let pos = &mut *(lp.0 as *mut WINDOWPOS);
                if (pos.flags & SWP_NOZORDER).0 == 0 && (pos.flags & SWP_NOACTIVATE).0 == 0 {
                    pos.flags |= SWP_NOZORDER;
                }
            }
            DefWindowProcW(h, m, wp, lp)
        }
        WM_ACTIVATE => {
            // Safety net: a box that became active is sent back just above the desktop (it keeps
            // the keyboard; the move is asynchronous and never activates anything).
            if (wp.0 & 0xFFFF) != 0 && shared.lock().map(|s| s.is_box).unwrap_or(false) {
                crate::desktop::send_to_bottom(h.0 as isize);
            }
            DefWindowProcW(h, m, wp, lp)
        }
        WM_MOUSEACTIVATE => {
            // Part of the desktop: clicking a box does not bring it in front of apps.
            if shared.lock().map(|s| s.no_activate).unwrap_or(true) {
                LRESULT(MA_NOACTIVATE as isize)
            } else {
                DefWindowProcW(h, m, wp, lp)
            }
        }
        WM_SETCURSOR => {
            if (lp.0 & 0xFFFF) as u32 == HTCLIENT {
                let icon = shared.lock().map(|s| s.cursor).unwrap_or_default();
                let id = match icon {
                    egui::CursorIcon::ResizeHorizontal | egui::CursorIcon::ResizeEast | egui::CursorIcon::ResizeWest => IDC_SIZEWE,
                    egui::CursorIcon::ResizeVertical | egui::CursorIcon::ResizeSouth | egui::CursorIcon::ResizeNorth => IDC_SIZENS,
                    egui::CursorIcon::ResizeSouthEast | egui::CursorIcon::ResizeNwSe => IDC_SIZENWSE,
                    egui::CursorIcon::Text => IDC_IBEAM,
                    egui::CursorIcon::PointingHand => IDC_HAND,
                    _ => IDC_ARROW,
                };
                if let Ok(c) = LoadCursorW(None, id) {
                    SetCursor(c);
                }
                return LRESULT(1);
            }
            DefWindowProcW(h, m, wp, lp)
        }
        WM_DROPFILES => {
            let drop = HDROP(wp.0 as *mut _);
            let n = DragQueryFileW(drop, u32::MAX, None);
            let mut files = Vec::new();
            for i in 0..n {
                let len = DragQueryFileW(drop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(drop, i, Some(&mut buf));
                files.push(std::path::PathBuf::from(String::from_utf16_lossy(&buf[..len])));
            }
            DragFinish(drop);
            if let Ok(mut s) = shared.lock() {
                s.dropped.extend(files);
                s.dirty = true;
            }
            wake();
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_CLOSE => {
            // Boxes are never closed by Alt+F4; a panel tells the app, which closes it.
            if let Ok(mut s) = shared.lock() {
                if s.can_close {
                    s.close = true;
                    s.dirty = true;
                }
            }
            wake();
            LRESULT(0)
        }
        WM_GETMINMAXINFO => {
            let min = shared.lock().map(|s| s.min_size).unwrap_or([0, 0]);
            if min[0] > 0 && lp.0 != 0 {
                let info = &mut *(lp.0 as *mut MINMAXINFO);
                info.ptMinTrackSize = POINT { x: min[0], y: min[1] };
            }
            LRESULT(0)
        }
        0x0231 /* WM_ENTERSIZEMOVE */ | 0x0232 /* WM_EXITSIZEMOVE */ => {
            if let Ok(mut s) = shared.lock() {
                s.sizing = m == 0x0231;
            }
            DefWindowProcW(h, m, wp, lp)
        }
        WM_DPICHANGED | WM_SIZE | WM_MOVE => {
            let live = match shared.lock() {
                Ok(mut s) => {
                    s.dirty = true;
                    m == WM_SIZE && s.sizing && s.can_close
                }
                Err(_) => false,
            };
            wake();
            // A panel being resized by its edge: our loop is paused (Windows runs its own), so
            // draw now, or the content would only follow when the mouse is released.
            if live {
                crate::runloop::pass_now();
            }
            DefWindowProcW(h, m, wp, lp)
        }
        _ => DefWindowProcW(h, m, wp, lp),
    }
}

/// One box window.
/// The glass layers of a box (Windows' composition system).
struct Glass {
    _target: DesktopWindowTarget,
    container: ContainerVisual,
    backdrop: SpriteVisual,
    tint: SpriteVisual,
    tint_brush: CompositionColorBrush,
    content: SpriteVisual,
    geo: CompositionRoundedRectangleGeometry,
}

pub struct Host {
    pub hwnd: HWND,
    shared: Arc<Mutex<Shared>>,
    swapchain: IDXGISwapChain1,
    size: [u32; 2],
    /// None = no-glass fallback: an ordinary window with a solid background drawn by egui.
    glass: Option<Glass>,
    /// When egui asked to draw this box again (a timer, an animation), if it did.
    next: Option<std::time::Instant>,
    /// The look and size last given to the composition layers (set again only on change).
    applied: Option<([u8; 4], u32, u32, [u32; 2])>,
    /// The whole box's opacity last asked for (fading when the mouse is away).
    opacity: f32,
}

impl Drop for Host {
    fn drop(&mut self) {
        unsafe {
            let p = GetWindowLongPtrW(self.hwnd, GWLP_USERDATA) as *const Mutex<Shared>;
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            crate::desktop::unregister_box(self.hwnd.0 as isize);
            crate::shadow::destroy(self.hwnd.0 as isize);
            let _ = DestroyWindow(self.hwnd);
            if !p.is_null() {
                drop(Arc::from_raw(p));
            }
        }
    }
}

/// Window commands from egui (move, resize, focus), for boxes and panels.
fn apply_to(hwnd: HWND, cmd: &ViewportCommand) {
    unsafe {
        let ppp = ppp_of(hwnd);
        match cmd {
            ViewportCommand::InnerSize(v) => {
                let _ = SetWindowPos(hwnd, None, 0, 0, (v.x * ppp).round() as i32, (v.y * ppp).round() as i32, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
            ViewportCommand::OuterPosition(p) => {
                // Positions are app screen points (main display scale); sizes are the window's own.
                let sys = crate::desktop::system_scale();
                let _ = SetWindowPos(hwnd, None, (p.x * sys).round() as i32, (p.y * sys).round() as i32, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
            ViewportCommand::Focus => {
                let _ = SetForegroundWindow(hwnd);
                let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(hwnd);
            }
            _ => {}
        }
    }
}

/// The two kinds of ordinary windows drawn like the boxes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PanelKind {
    /// A right-click menu: borderless, always on top, rounded, closes when it loses focus.
    Menu,
    /// The settings window: a normal window with a title bar, resizable, in the taskbar.
    Settings,
}

/// A menu or settings window: an ordinary window with its own swap chain (no composition
/// glass), drawn by the same renderer and egui context as the boxes.
pub struct Panel {
    pub hwnd: HWND,
    shared: Arc<Mutex<Shared>>,
    swapchain: IDXGISwapChain1,
    size: [u32; 2],
    next: Option<std::time::Instant>,
    /// Not drawn yet.
    fresh: bool,
}

impl Panel {
    fn create(renderer: &Renderer, kind: PanelKind, title: &str, r: [i32; 4]) -> Result<Panel> {
        unsafe {
            let inst = GetModuleHandleW(None)?;
            let (ex, style) = match kind {
                PanelKind::Menu => (WS_EX_TOPMOST | WS_EX_TOOLWINDOW, WS_POPUP),
                PanelKind::Settings => (WINDOW_EX_STYLE(0), WS_OVERLAPPEDWINDOW),
            };
            let hwnd = CreateWindowExW(ex, w!("DesktopShepherdPanel"), &HSTRING::from(title), style, r[0], r[1], r[2].max(1), r[3].max(1), None, None, inst, None)?;
            let s = ppp_of(hwnd);
            let shared = Arc::new(Mutex::new(Shared {
                // A new window starts with no mouse over it: forget where the mouse was in the
                // last window that used this id (otherwise a menu row shows as hovered).
                events: vec![egui::Event::PointerGone],
                dirty: true,
                can_close: true,
                min_size: if kind == PanelKind::Settings { [(640.0 * s) as i32, (420.0 * s) as i32] } else { [0, 0] },
                ..Default::default()
            }));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Arc::into_raw(shared.clone()) as isize);
            {
                use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND};
                let dark = BOOL(if crate::theme::is_light() { 0 } else { 1 }); // title bar matches the window
                let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, 4);
                if kind == PanelKind::Menu {
                    let round = DWMWCP_ROUND;
                    let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const _, std::mem::size_of_val(&round) as u32);
                }
            }
            let mut cr = RECT::default();
            let _ = GetClientRect(hwnd, &mut cr);
            let size = [(cr.right - cr.left).max(1) as u32, (cr.bottom - cr.top).max(1) as u32];
            let swapchain = match renderer.create_swapchain_for_hwnd(hwnd, size[0], size[1]) {
                Ok(s) => s,
                Err(e) => {
                    let p = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Mutex<Shared>;
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    let _ = DestroyWindow(hwnd);
                    if !p.is_null() {
                        drop(Arc::from_raw(p));
                    }
                    return Err(e);
                }
            };
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(hwnd);
            Ok(Panel { hwnd, shared, swapchain, size, next: None, fresh: true })
        }
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        unsafe {
            let p = GetWindowLongPtrW(self.hwnd, GWLP_USERDATA) as *const Mutex<Shared>;
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
            if !p.is_null() {
                drop(Arc::from_raw(p));
            }
        }
    }
}

/// How a box should look this frame (glass layers).
pub struct Look {
    pub tint: [u8; 4],
    pub corner_points: f32,
    pub blur: f32,
    pub no_activate: bool,
}

pub struct HostSystem {
    _controller: Option<windows::System::DispatcherQueueController>,
    compositor: Option<Compositor>,
    renderer: Renderer,
    pub ctx: egui::Context,
    hosts: HashMap<u64, Host>,
    /// Menu and settings windows (see `panel`).
    panels: HashMap<u64, Panel>,
    start: std::time::Instant,
    /// The graphics device was lost (driver reset, sleep/resume...): rebuild before drawing.
    pub lost: bool,
    /// A drawing error was already written to the log (avoid filling it).
    logged_error: bool,
    /// Boxes whose window could not be created (logged once each).
    create_failed: std::collections::HashSet<u64>,
}

impl HostSystem {
    /// Sets up Windows' composition system and the GPU renderer. Fails cleanly (the app then
    /// keeps its previous way of drawing boxes).
    pub fn new(root_ctx: &egui::Context) -> Result<Self> {
        unsafe {
            let _ = RoInitialize(RO_INIT_SINGLETHREADED);
            let inst = GetModuleHandleW(None)?;
            let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: inst.into(), lpszClassName: w!("DesktopShepherdBox"), ..Default::default() };
            RegisterClassW(&wc);
            // Glass needs Windows' composition system. Without it (or with
            // DESKTOPSHEPHERD_NO_COMPOSITION=1, for testing) boxes still work, with solid backgrounds.
            let composition = if std::env::var_os("DESKTOPSHEPHERD_NO_COMPOSITION").is_some() {
                None
            } else {
                let made = (|| -> Result<(windows::System::DispatcherQueueController, Compositor)> {
                    let controller = CreateDispatcherQueueController(DispatcherQueueOptions {
                        dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
                        threadType: DQTYPE_THREAD_CURRENT,
                        apartmentType: DQTAT_COM_NONE,
                    })?;
                    Ok((controller, Compositor::new()?))
                })();
                match made {
                    Ok(c) => Some(c),
                    Err(e) => {
                        crate::storage::log_line(&format!("Windows composition unavailable ({e:?}); boxes without glass"));
                        None
                    }
                }
            };
            if composition.is_none() {
                crate::storage::log_line("boxes drawn without glass (solid backgrounds)");
            }
            let (controller, compositor) = match composition {
                Some((c, comp)) => (Some(c), Some(comp)),
                None => (None, None),
            };
            let renderer = Renderer::new()?;
            let _ = WAKE.set(root_ctx.clone());
            let ctx = egui::Context::default();
            {
                let root = root_ctx.clone();
                ctx.set_request_repaint_callback(move |info| {
                    root.request_repaint_after(info.delay.max(MIN_PASS_GAP));
                });
            }
            let pc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: inst.into(),
                lpszClassName: w!("DesktopShepherdPanel"),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                hIcon: crate::appicon::big(),
                hIconSm: crate::appicon::small(),
                ..Default::default()
            };
            RegisterClassExW(&pc);
            Ok(Self { _controller: controller, compositor, renderer, ctx, hosts: HashMap::new(), panels: HashMap::new(), start: std::time::Instant::now(), lost: false, logged_error: false, create_failed: Default::default() })
        }
    }

    fn create(&self, id: u64, rect_px: [i32; 4]) -> Result<Host> {
        unsafe {
            let inst = GetModuleHandleW(None)?;
            let title = HSTRING::from(format!("DesktopShepherd box {id}"));
            // With glass the window has no surface of its own (composition draws it); without,
            // it is an ordinary window with its own swap chain.
            let ex = if self.compositor.is_some() { WS_EX_NOREDIRECTIONBITMAP } else { WINDOW_EX_STYLE(0) };
            let hwnd = CreateWindowExW(
                ex | WS_EX_TOOLWINDOW | WS_EX_ACCEPTFILES,
                w!("DesktopShepherdBox"),
                &title,
                WS_POPUP,
                rect_px[0],
                rect_px[1],
                rect_px[2].max(1),
                rect_px[3].max(1),
                None,
                None,
                inst,
                None,
            )?;
            let shared = Arc::new(Mutex::new(Shared { no_activate: true, is_box: true, ..Default::default() }));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Arc::into_raw(shared.clone()) as isize);
            DragAcceptFiles(hwnd, true);
            let made = match &self.compositor {
                Some(c) => Self::glass_layers(c, &self.renderer, hwnd, rect_px).map(|(sc, g)| (sc, Some(g))),
                None => self.renderer.create_swapchain_for_hwnd(hwnd, rect_px[2].max(1) as u32, rect_px[3].max(1) as u32).map(|sc| (sc, None)),
            };
            let (swapchain, glass) = match made {
                Ok(m) => m,
                Err(e) => {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    let _ = DestroyWindow(hwnd);
                    drop(Arc::from_raw(Arc::as_ptr(&shared)));
                    return Err(e);
                }
            };
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            crate::desktop::register_box(hwnd.0 as isize);
            crate::desktop::send_to_bottom(hwnd.0 as isize);
            Ok(Host {
                hwnd,
                shared,
                swapchain,
                size: [rect_px[2].max(1) as u32, rect_px[3].max(1) as u32],
                glass,
                next: None,
                applied: None, opacity: 1.0,
            })
        }
    }

    /// The glass layers for a box window: frosted backdrop, tint, and the contents' swap chain,
    /// clipped to a rounded rectangle.
    unsafe fn glass_layers(c: &Compositor, renderer: &Renderer, hwnd: HWND, rect_px: [i32; 4]) -> Result<(IDXGISwapChain1, Glass)> {
        let on = BOOL(1);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_HOSTBACKDROPBRUSH, &on as *const _ as *const _, 4);
        let target = c.cast::<ICompositorDesktopInterop>()?.CreateDesktopWindowTarget(hwnd, false)?;
        let size = Vector2 { X: rect_px[2] as f32, Y: rect_px[3] as f32 };
        let container = c.CreateContainerVisual()?;
        let backdrop = c.CreateSpriteVisual()?;
        backdrop.SetBrush(&c.CreateHostBackdropBrush()?)?;
        let tint_brush = c.CreateColorBrushWithColor(Color { A: 0, R: 0, G: 0, B: 0 })?;
        let tint = c.CreateSpriteVisual()?;
        tint.SetBrush(&tint_brush)?;
        let swapchain = renderer.create_swapchain(rect_px[2] as u32, rect_px[3] as u32)?;
        let surface = c.cast::<ICompositorInterop>()?.CreateCompositionSurfaceForSwapChain(&swapchain)?;
        let content = c.CreateSpriteVisual()?;
        content.SetBrush(&c.CreateSurfaceBrushWithSurface(&surface)?)?;
        for v in [&backdrop, &tint, &content] {
            v.SetSize(size)?;
        }
        container.SetSize(size)?;
        let kids = container.Children()?;
        kids.InsertAtTop(&backdrop)?;
        kids.InsertAtTop(&tint)?;
        kids.InsertAtTop(&content)?;
        let geo = c.CreateRoundedRectangleGeometry()?;
        geo.SetSize(size)?;
        container.SetClip(&c.CreateGeometricClipWithGeometry(&geo)?)?;
        target.SetRoot(&container)?;
        Ok((swapchain, Glass { _target: target, container, backdrop, tint, tint_brush, content, geo }))
    }

    /// Boxes have Windows' frosted glass (false = no-glass fallback: solid backgrounds).
    pub fn has_glass(&self) -> bool {
        self.compositor.is_some()
    }

    /// After the graphics device was lost: a new device, new box windows and a fresh egui
    /// context (so every texture is sent again). Returns false if the GPU is still unavailable.
    pub fn recover(&mut self, root_ctx: &egui::Context) -> bool {
        match Renderer::new() {
            Ok(r) => {
                self.hosts.clear();
                self.panels.clear();
                self.renderer = r;
                let ctx = egui::Context::default();
                let root = root_ctx.clone();
                ctx.set_request_repaint_callback(move |info| root.request_repaint_after(info.delay.max(MIN_PASS_GAP)));
                self.ctx = ctx;
                self.lost = false;
                self.logged_error = false;
                crate::storage::log_line("graphics device was reset; boxes rebuilt");
                true
            }
            Err(_) => false,
        }
    }

    /// The earliest time any box asked to be drawn again (a clock, a tooltip, an animation).
    /// The app re-arms its own wake-up with it after every pass: egui forgets a pending wake-up
    /// when another pass runs in between, and skipped boxes would not ask again.
    pub fn next_due(&self) -> Option<std::time::Instant> {
        self.hosts.values().filter_map(|h| h.next).chain(self.panels.values().filter_map(|p| p.next)).min()
    }

    /// Removes the windows of boxes that no longer exist.
    pub fn retain(&mut self, ids: &[u64]) {
        self.hosts.retain(|id, _| ids.contains(id));
    }

    pub fn hwnd(&self, id: u64) -> Option<isize> {
        self.hosts.get(&id).map(|h| h.hwnd.0 as isize)
    }

    /// A box that isn't shown (another page, or all boxes hidden) gives back its drawing memory:
    /// its picture shrinks to a few pixels and grows back on its next frame.
    pub fn release_hidden(&mut self, id: u64) {
        if let Some(h) = self.hosts.get_mut(&id) {
            h.next = None; // a hidden box has no timer (it would keep the app waking up)
            if h.size != [8, 8] && self.renderer.resize(&h.swapchain, 8, 8).is_ok() {
                h.size = [8, 8];
                h.applied = None; // the glass layers are re-sized when it shows again
            }
        }
    }

    /// A box nobody can see (under a full-screen or maximized window, screen off, session locked):
    /// no timer, so it doesn't wake the app. Its picture stays as it is; it is redrawn (forced) when
    /// it can be seen again.
    pub fn set_occluded(&mut self, id: u64) {
        if let Some(h) = self.hosts.get_mut(&id) {
            h.next = None;
        }
    }

    /// Fades the whole box (glass, text and icons) to `a` (0..1) over a short animation, done by
    /// Windows' compositor: nothing is redrawn. Without glass it does nothing.
    pub fn set_box_opacity(&mut self, id: u64, a: f32) {
        let Some(comp) = self.compositor.as_ref() else { return };
        let Some(h) = self.hosts.get_mut(&id) else { return };
        if (h.opacity - a).abs() < 0.01 {
            return;
        }
        h.opacity = a;
        crate::shadow::set_faded(h.hwnd.0 as isize, a < 0.5);
        let Some(g) = &h.glass else { return };
        let anim = (|| -> windows::core::Result<()> {
            let an = comp.CreateScalarKeyFrameAnimation()?;
            an.InsertKeyFrame(1.0, a)?;
            // Fading out slowly, back in quickly.
            let ms: i64 = if a < 1.0 { 600 } else { 150 };
            an.SetDuration(windows::Foundation::TimeSpan { Duration: ms * 10_000 })?;
            g.container.StartAnimation(&windows::core::HSTRING::from("Opacity"), &an)
        })();
        if anim.is_err() {
            let _ = g.container.SetOpacity(a);
        }
    }

    /// A panel window's handle (menu, settings, search…), if it is open.
    pub fn panel_hwnd(&self, id: u64) -> Option<isize> {
        self.panels.get(&id).map(|p| p.hwnd.0 as isize)
    }

    /// Applies a command (move, resize, focus) to a box window.
    pub fn apply(&self, id: u64, cmd: &ViewportCommand) {
        let Some(hwnd) = self.hosts.get(&id).map(|h| h.hwnd).or_else(|| self.panels.get(&id).map(|p| p.hwnd)) else { return };
        apply_to(hwnd, cmd);
    }

    /// Runs one egui frame for a menu or settings window (creating it if needed) and draws it
    /// with the same renderer and egui context as the boxes. Like boxes, it draws only when it
    /// has a reason to (`force`, input, resize, its egui timer, or first show). `rect_px` is the
    /// window's place on screen (pixels): a menu is kept there (`keep_rect`); a settings window
    /// only starts there and the person moves it. Returns true when the window was drawn.
    #[allow(clippy::too_many_arguments)]
    pub fn panel(&mut self, id: u64, kind: PanelKind, title: &str, rect_px: [i32; 4], keep_rect: bool, force: bool, mut ui: impl FnMut(&egui::Context)) -> bool {
        if !self.panels.contains_key(&id) {
            match Panel::create(&self.renderer, kind, title, rect_px) {
                Ok(p) => {
                    self.panels.insert(id, p);
                }
                Err(e) => {
                    crate::storage::log_line(&format!("could not create {title} window: {e:?}"));
                    return false;
                }
            }
        }
        let vid = ViewportId::from_hash_of(("desktopshepherd-panel", id));
        let time = self.start.elapsed().as_secs_f64();
        let Some(p) = self.panels.get_mut(&id) else { return false };
        unsafe {
            if keep_rect {
                let mut wr = RECT::default();
                let _ = GetWindowRect(p.hwnd, &mut wr);
                if [wr.left, wr.top, wr.right - wr.left, wr.bottom - wr.top] != rect_px {
                    let _ = SetWindowPos(p.hwnd, None, rect_px[0], rect_px[1], rect_px[2].max(1), rect_px[3].max(1), SWP_NOZORDER | SWP_NOACTIVATE);
                }
            }
            let ppp = ppp_of(p.hwnd);
            let mut cr = RECT::default();
            let _ = GetClientRect(p.hwnd, &mut cr);
            let (w, ht) = ((cr.right - cr.left).max(1) as u32, (cr.bottom - cr.top).max(1) as u32);
            let now = std::time::Instant::now();
            let has_input = p.shared.lock().map(|s| s.dirty).unwrap_or(true);
            let due = p.next.map(|t| t <= now).unwrap_or(false);
            let resized = [w, ht] != p.size;
            if !(force || has_input || due || resized || p.fresh) {
                return false;
            }
            p.fresh = false;
            if resized && self.renderer.resize(&p.swapchain, w, ht).is_ok() {
                p.size = [w, ht];
            }
            let (events, focused, close) = match p.shared.lock() {
                Ok(mut s) => {
                    s.dirty = false;
                    (std::mem::take(&mut s.events), s.focused, std::mem::take(&mut s.close))
                }
                Err(_) => (Vec::new(), false, false),
            };
            let mut origin = POINT::default();
            let _ = windows::Win32::Graphics::Gdi::ClientToScreen(p.hwnd, &mut origin);
            let inner = Rect::from_min_size(Pos2::new(origin.x as f32 / ppp, origin.y as f32 / ppp), Vec2::new(w as f32 / ppp, ht as f32 / ppp));
            let mut wr = RECT::default();
            let _ = GetWindowRect(p.hwnd, &mut wr);
            let outer = Rect::from_min_max(Pos2::new(wr.left as f32 / ppp, wr.top as f32 / ppp), Pos2::new(wr.right as f32 / ppp, wr.bottom as f32 / ppp));
            let mut viewports = egui::ViewportIdMap::default();
            viewports.insert(
                vid,
                egui::ViewportInfo {
                    native_pixels_per_point: Some(ppp),
                    inner_rect: Some(inner),
                    outer_rect: Some(outer),
                    focused: Some(focused),
                    minimized: Some(false),
                    maximized: Some(false),
                    fullscreen: Some(false),
                    events: if close { vec![egui::ViewportEvent::Close] } else { vec![] },
                    ..Default::default()
                },
            );
            let raw = egui::RawInput {
                viewport_id: vid,
                viewports,
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, inner.size())),
                max_texture_side: Some(8192),
                time: Some(time),
                modifiers: modifiers(),
                events,
                focused,
                ..Default::default()
            };
            // Menus and settings windows may be light (the boxes share this context and stay as they are).
            let light = crate::theme::is_light();
            let dark = light.then(|| self.ctx.style().visuals.clone());
            if let Some(d) = &dark {
                self.ctx.set_visuals(crate::theme::panel_visuals(d));
            }
            let out = self.ctx.run(raw, |c| ui(c));
            if let Some(d) = dark {
                self.ctx.set_visuals(d);
            }
            if let Ok(mut s) = p.shared.lock() {
                s.cursor = out.platform_output.cursor_icon;
            }
            place_ime(p.hwnd, &p.shared, out.platform_output.ime.as_ref(), ppp);
            // Copy / cut in text fields.
            #[allow(deprecated)]
            let mut copied = out.platform_output.copied_text.clone();
            for c in &out.platform_output.commands {
                if let egui::OutputCommand::CopyText(t) = c {
                    copied = t.clone();
                }
            }
            if !copied.is_empty() {
                clipboard_set(p.hwnd, &copied);
            }
            let cmds: Vec<ViewportCommand> = out.viewport_output.get(&vid).map(|v| v.commands.clone()).unwrap_or_default();
            let delay = out.viewport_output.get(&vid).map(|v| v.repaint_delay).unwrap_or(Duration::MAX);
            p.next = if delay < Duration::from_secs(86_400) { Some(now + delay) } else { None };
            self.renderer.update_textures(&out.textures_delta);
            let prims = self.ctx.tessellate(out.shapes, out.pixels_per_point);
            if let Err(e) = self.renderer.render(&p.swapchain, p.size, out.pixels_per_point, &prims) {
                use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET};
                if e.code() == DXGI_ERROR_DEVICE_REMOVED || e.code() == DXGI_ERROR_DEVICE_RESET {
                    self.lost = true;
                }
                if !self.logged_error {
                    self.logged_error = true;
                    crate::storage::log_line(&format!("{title} drawing failed: {e:?}"));
                }
            }
            for c in &cmds {
                apply_to(p.hwnd, c);
            }
        }
        true
    }

    /// Closes a menu or settings window (nothing happens if it isn't open).
    pub fn close_panel(&mut self, id: u64) {
        self.panels.remove(&id);
    }

    /// Runs one egui frame for a box (creating its window if needed) and draws it — but only
    /// when the box has a reason to: `force` (shared data changed), input or a move/resize, a
    /// look change, or a redraw time egui asked for. Otherwise nothing runs and nothing is
    /// presented. Returns true when the box was drawn.
    pub fn frame(&mut self, id: u64, rect_points: [f32; 4], look: &Look, force: bool, mut ui: impl FnMut(&egui::Context)) -> bool {
        if !self.hosts.contains_key(&id) {
            let s = crate::desktop::system_scale();
            let r = [
                (rect_points[0] * s).round() as i32,
                (rect_points[1] * s).round() as i32,
                (rect_points[2] * s).round() as i32,
                (rect_points[3] * s).round() as i32,
            ];
            match self.create(id, r) {
                Ok(h) => {
                    // On a monitor with a different scale than the main one, the window now has
                    // that monitor's scale: give it its size in its own points.
                    let own = ppp_of(h.hwnd);
                    if (own - s).abs() > 0.01 {
                        apply_to(h.hwnd, &ViewportCommand::InnerSize(Vec2::new(rect_points[2], rect_points[3])));
                    }
                    self.hosts.insert(id, h);
                }
                Err(e) => {
                    if self.create_failed.insert(id) {
                        crate::storage::log_line(&format!("could not create box window {id}: {e:?}"));
                    }
                    return false;
                }
            }
        }
        let vid = ViewportId::from_hash_of(("desktopshepherd-fence", id));
        let time = self.start.elapsed().as_secs_f64();
        let Some(h) = self.hosts.get_mut(&id) else { return false };
        unsafe {
            let ppp = ppp_of(h.hwnd);
            let mut wr = RECT::default();
            let _ = GetWindowRect(h.hwnd, &mut wr);
            let (w, ht) = ((wr.right - wr.left).max(1) as u32, (wr.bottom - wr.top).max(1) as u32);
            let now = std::time::Instant::now();
            let applied = (look.tint, (look.corner_points * ppp).to_bits(), look.blur.to_bits(), [w, ht]);
            let has_input = h.shared.lock().map(|s| s.dirty).unwrap_or(true);
            let due = h.next.map(|t| t <= now).unwrap_or(false);
            let changed = h.applied != Some(applied);
            if !(force || has_input || due || changed) {
                return false;
            }
            if [w, ht] != h.size {
                if self.renderer.resize(&h.swapchain, w, ht).is_ok() {
                    h.size = [w, ht];
                }
            }
            if changed {
                // Each change makes Windows recompose the box: only send what changed.
                if let Some(g) = &h.glass {
                    let size = Vector2 { X: w as f32, Y: ht as f32 };
                    let _ = g.container.SetSize(size);
                    let _ = g.backdrop.SetSize(size);
                    let _ = g.tint.SetSize(size);
                    let _ = g.content.SetSize(size);
                    let _ = g.geo.SetSize(size);
                    let r = look.corner_points * ppp;
                    let _ = g.geo.SetCornerRadius(Vector2 { X: r, Y: r });
                    let _ = g.tint_brush.SetColor(Color { A: look.tint[3], R: look.tint[0], G: look.tint[1], B: look.tint[2] });
                    let _ = g.backdrop.SetOpacity(look.blur.clamp(0.0, 1.0));
                    let _ = g.backdrop.SetIsVisible(look.blur > 0.0);
                }
                h.applied = Some(applied);
            }

            let (events, focused, dropped) = match h.shared.lock() {
                Ok(mut s) => {
                    s.no_activate = look.no_activate;
                    s.dirty = false;
                    (std::mem::take(&mut s.events), s.focused, std::mem::take(&mut s.dropped))
                }
                Err(_) => (Vec::new(), false, Vec::new()),
            };
            // Position in the app's screen points (pixels / the main display scale), so boxes on
            // monitors with different scaling still line up with the monitor list; size in this
            // box's own points (its monitor's scale), which is what egui draws with.
            let sys = crate::desktop::system_scale();
            let outer = Rect::from_min_size(Pos2::new(wr.left as f32 / sys, wr.top as f32 / sys), Vec2::new(w as f32 / ppp, ht as f32 / ppp));
            let mut viewports = egui::ViewportIdMap::default();
            viewports.insert(
                vid,
                egui::ViewportInfo {
                    native_pixels_per_point: Some(ppp),
                    inner_rect: Some(outer),
                    outer_rect: Some(outer),
                    focused: Some(focused),
                    minimized: Some(false),
                    maximized: Some(false),
                    fullscreen: Some(false),
                    ..Default::default()
                },
            );
            let raw = egui::RawInput {
                viewport_id: vid,
                viewports,
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, outer.size())),
                max_texture_side: Some(8192),
                time: Some(time),
                modifiers: modifiers(),
                events,
                focused,
                dropped_files: dropped
                    .into_iter()
                    .map(|p| egui::DroppedFile { path: Some(p), ..Default::default() })
                    .collect(),
                ..Default::default()
            };
            let out = self.ctx.run(raw, |c| ui(c));
            if let Ok(mut s) = h.shared.lock() {
                s.cursor = out.platform_output.cursor_icon;
            }
            place_ime(h.hwnd, &h.shared, out.platform_output.ime.as_ref(), ppp);
            // Copy / cut in a widget's text (the sticky note).
            #[allow(deprecated)]
            let mut copied = out.platform_output.copied_text.clone();
            for c in &out.platform_output.commands {
                if let egui::OutputCommand::CopyText(t) = c {
                    copied = t.clone();
                }
            }
            if !copied.is_empty() {
                clipboard_set(h.hwnd, &copied);
            }
            let cmds: Vec<ViewportCommand> = out.viewport_output.get(&vid).map(|v| v.commands.clone()).unwrap_or_default();
            // egui's own "draw me again in …" for this box (tooltips, animations, a clock).
            let delay = out.viewport_output.get(&vid).map(|v| v.repaint_delay).unwrap_or(Duration::MAX);
            if let Some(h) = self.hosts.get_mut(&id) {
                h.next = if delay < Duration::from_secs(86_400) { Some(now + delay) } else { None };
            }
            self.renderer.update_textures(&out.textures_delta);
            let prims = self.ctx.tessellate(out.shapes, out.pixels_per_point);
            let size_px = self.hosts.get(&id).map(|h| h.size).unwrap_or([w, ht]);
            if let Some(h) = self.hosts.get(&id) {
                if let Err(e) = self.renderer.render(&h.swapchain, size_px, out.pixels_per_point, &prims) {
                    use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET};
                    if e.code() == DXGI_ERROR_DEVICE_REMOVED || e.code() == DXGI_ERROR_DEVICE_RESET {
                        self.lost = true;
                    }
                    if !self.logged_error {
                        self.logged_error = true;
                        crate::storage::log_line(&format!("box drawing failed: {e:?}"));
                    }
                }
            }
            for c in &cmds {
                self.apply(id, c);
            }
        }
        true
    }
}

/// Moves a box window with the mouse from a small helper thread until the button is released
/// (the window is truly see-through, so moving needs no redrawing at all).
/// `snap` (pixels): edges jump onto other boxes and the screen edges; holding Alt places the
/// box freely.
pub fn move_with_mouse(hwnd: isize, grab: (i32, i32), snap: Option<crate::snap::Snap>) {
    let _ = std::thread::Builder::new().name("box-move".into()).spawn(move || unsafe {
        let h = HWND(hwnd as *mut _);
        let mut last = (i32::MIN, i32::MIN);
        let mut wr = RECT::default();
        let _ = GetWindowRect(h, &mut wr);
        let (w, ht) = ((wr.right - wr.left) as f32, (wr.bottom - wr.top) as f32);
        loop {
            let mut p = POINT::default();
            if GetCursorPos(&mut p).is_ok() {
                let (mut x, mut y) = (p.x - grab.0, p.y - grab.1);
                let alt = (windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState(VK_MENU.0 as i32) as u16 & 0x8000) != 0;
                if let (Some(s), false) = (snap.as_ref(), alt) {
                    let (sx, sy) = s.snap_move(crate::snap::R::new(x as f32, y as f32, x as f32 + w, y as f32 + ht));
                    x = sx.round() as i32;
                    y = sy.round() as i32;
                }
                if (x, y) != last {
                    let _ = SetWindowPos(h, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
                    last = (x, y);
                    crate::shadow::follow(hwnd, x, y);
                }
            }
            if !crate::ghost::primary_button_down() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        wake();
    });
}

#[allow(dead_code)]
fn screen_to_client(h: HWND, p: POINT) -> POINT {
    let mut q = p;
    unsafe {
        let _ = ScreenToClient(h, &mut q);
    }
    q
}
