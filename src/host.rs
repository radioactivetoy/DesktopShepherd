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
use eframe::egui::{self, Pos2, Rect, Vec2, ViewportCommand, ViewportId};
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
    /// Something happened to the window (input, move, resize, DPI): draw it on the next pass.
    dirty: bool,
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
            SetCapture(h);
            button(egui::PointerButton::Primary, true);
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let _ = ReleaseCapture();
            button(egui::PointerButton::Primary, false);
            LRESULT(0)
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
            if let Some(c) = char::from_u32(wp.0 as u32) {
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
        WM_CLOSE => LRESULT(0), // never closed by Alt+F4
        WM_DPICHANGED | WM_SIZE | WM_MOVE => {
            if let Ok(mut s) = shared.lock() {
                s.dirty = true;
            }
            wake();
            DefWindowProcW(h, m, wp, lp)
        }
        _ => DefWindowProcW(h, m, wp, lp),
    }
}

/// One box window.
pub struct Host {
    pub hwnd: HWND,
    shared: Arc<Mutex<Shared>>,
    swapchain: IDXGISwapChain1,
    size: [u32; 2],
    _target: DesktopWindowTarget,
    container: ContainerVisual,
    backdrop: SpriteVisual,
    tint: SpriteVisual,
    tint_brush: CompositionColorBrush,
    content: SpriteVisual,
    geo: CompositionRoundedRectangleGeometry,
    /// When egui asked to draw this box again (a timer, an animation), if it did.
    next: Option<std::time::Instant>,
    /// The look and size last given to the composition layers (set again only on change).
    applied: Option<([u8; 4], u32, u32, [u32; 2])>,
}

impl Drop for Host {
    fn drop(&mut self) {
        unsafe {
            let p = GetWindowLongPtrW(self.hwnd, GWLP_USERDATA) as *const Mutex<Shared>;
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            crate::desktop::unregister_box(self.hwnd.0 as isize);
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
    _controller: windows::System::DispatcherQueueController,
    compositor: Compositor,
    renderer: Renderer,
    pub ctx: egui::Context,
    hosts: HashMap<u64, Host>,
    start: std::time::Instant,
    /// The graphics device was lost (driver reset, sleep/resume...): rebuild before drawing.
    pub lost: bool,
    /// A drawing error was already written to the log (avoid filling it).
    logged_error: bool,
}

impl HostSystem {
    /// Sets up Windows' composition system and the GPU renderer. Fails cleanly (the app then
    /// keeps its previous way of drawing boxes).
    pub fn new(root_ctx: &egui::Context) -> Result<Self> {
        unsafe {
            let _ = RoInitialize(RO_INIT_SINGLETHREADED);
            let controller = CreateDispatcherQueueController(DispatcherQueueOptions {
                dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
                threadType: DQTYPE_THREAD_CURRENT,
                apartmentType: DQTAT_COM_NONE,
            })?;
            let inst = GetModuleHandleW(None)?;
            let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: inst.into(), lpszClassName: w!("DeskBoxesBox"), ..Default::default() };
            RegisterClassW(&wc);
            let compositor = Compositor::new()?;
            let renderer = Renderer::new()?;
            let _ = WAKE.set(root_ctx.clone());
            let ctx = egui::Context::default();
            {
                let root = root_ctx.clone();
                ctx.set_request_repaint_callback(move |info| {
                    root.request_repaint_after(info.delay.max(MIN_PASS_GAP));
                });
            }
            Ok(Self { _controller: controller, compositor, renderer, ctx, hosts: HashMap::new(), start: std::time::Instant::now(), lost: false, logged_error: false })
        }
    }

    fn create(&self, id: u64, rect_px: [i32; 4]) -> Result<Host> {
        unsafe {
            let inst = GetModuleHandleW(None)?;
            let title = HSTRING::from(format!("DeskBoxes box {id}"));
            let hwnd = CreateWindowExW(
                WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW | WS_EX_ACCEPTFILES,
                w!("DeskBoxesBox"),
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
            let shared = Arc::new(Mutex::new(Shared { no_activate: true, ..Default::default() }));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, Arc::into_raw(shared.clone()) as isize);
            DragAcceptFiles(hwnd, true);
            let on = BOOL(1);
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_HOSTBACKDROPBRUSH, &on as *const _ as *const _, 4);

            let c = &self.compositor;
            let target = c.cast::<ICompositorDesktopInterop>()?.CreateDesktopWindowTarget(hwnd, false)?;
            let size = Vector2 { X: rect_px[2] as f32, Y: rect_px[3] as f32 };
            let container = c.CreateContainerVisual()?;
            let backdrop = c.CreateSpriteVisual()?;
            backdrop.SetBrush(&c.CreateHostBackdropBrush()?)?;
            let tint_brush = c.CreateColorBrushWithColor(Color { A: 0, R: 0, G: 0, B: 0 })?;
            let tint = c.CreateSpriteVisual()?;
            tint.SetBrush(&tint_brush)?;
            let swapchain = self.renderer.create_swapchain(rect_px[2] as u32, rect_px[3] as u32)?;
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
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            crate::desktop::register_box(hwnd.0 as isize);
            crate::desktop::send_to_bottom(hwnd.0 as isize);
            Ok(Host {
                hwnd,
                shared,
                swapchain,
                size: [rect_px[2].max(1) as u32, rect_px[3].max(1) as u32],
                _target: target,
                container,
                backdrop,
                tint,
                tint_brush,
                content,
                geo,
                next: None,
                applied: None,
            })
        }
    }

    /// After the graphics device was lost: a new device, new box windows and a fresh egui
    /// context (so every texture is sent again). Returns false if the GPU is still unavailable.
    pub fn recover(&mut self, root_ctx: &egui::Context) -> bool {
        match Renderer::new() {
            Ok(r) => {
                self.hosts.clear();
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
        self.hosts.values().filter_map(|h| h.next).min()
    }

    /// Removes the windows of boxes that no longer exist.
    pub fn retain(&mut self, ids: &[u64]) {
        self.hosts.retain(|id, _| ids.contains(id));
    }

    #[allow(dead_code)]
    pub fn hwnd(&self, id: u64) -> Option<isize> {
        self.hosts.get(&id).map(|h| h.hwnd.0 as isize)
    }

    /// Applies a command (move, resize, focus) to a box window.
    pub fn apply(&self, id: u64, cmd: &ViewportCommand) {
        let Some(h) = self.hosts.get(&id) else { return };
        unsafe {
            let ppp = ppp_of(h.hwnd);
            match cmd {
                ViewportCommand::InnerSize(v) => {
                    let _ = SetWindowPos(h.hwnd, None, 0, 0, (v.x * ppp).round() as i32, (v.y * ppp).round() as i32, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE);
                }
                ViewportCommand::OuterPosition(p) => {
                    let _ = SetWindowPos(h.hwnd, None, (p.x * ppp).round() as i32, (p.y * ppp).round() as i32, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                }
                ViewportCommand::Focus => {
                    let _ = SetForegroundWindow(h.hwnd);
                    let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(h.hwnd);
                }
                _ => {}
            }
        }
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
                    self.hosts.insert(id, h);
                }
                Err(e) => {
                    crate::storage::log_line(&format!("could not create box window: {e:?}"));
                    return false;
                }
            }
        }
        let vid = ViewportId::from_hash_of(("deskboxes-fence", id));
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
                let size = Vector2 { X: w as f32, Y: ht as f32 };
                let _ = h.container.SetSize(size);
                let _ = h.backdrop.SetSize(size);
                let _ = h.tint.SetSize(size);
                let _ = h.content.SetSize(size);
                let _ = h.geo.SetSize(size);
                let r = look.corner_points * ppp;
                let _ = h.geo.SetCornerRadius(Vector2 { X: r, Y: r });
                let _ = h.tint_brush.SetColor(Color { A: look.tint[3], R: look.tint[0], G: look.tint[1], B: look.tint[2] });
                let _ = h.backdrop.SetOpacity(look.blur.clamp(0.0, 1.0));
                let _ = h.backdrop.SetIsVisible(look.blur > 0.0);
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
            let outer = Rect::from_min_size(Pos2::new(wr.left as f32 / ppp, wr.top as f32 / ppp), Vec2::new(w as f32 / ppp, ht as f32 / ppp));
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
pub fn move_with_mouse(hwnd: isize, grab: (i32, i32)) {
    let _ = std::thread::Builder::new().name("box-move".into()).spawn(move || unsafe {
        let h = HWND(hwnd as *mut _);
        let mut last = (i32::MIN, i32::MIN);
        loop {
            let mut p = POINT::default();
            if GetCursorPos(&mut p).is_ok() {
                let (x, y) = (p.x - grab.0, p.y - grab.1);
                if (x, y) != last {
                    let _ = SetWindowPos(h, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS);
                    last = (x, y);
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
