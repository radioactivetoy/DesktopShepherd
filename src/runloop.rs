//! The program's own main loop (replaces eframe and its invisible GL window).
//!
//! * One hidden window (never shown) receives wake-ups from other threads and Windows'
//!   shutdown notice.
//! * The loop sleeps in `MsgWaitForMultipleObjectsEx` until a message arrives or the next
//!   pass is due: idle = 0 % CPU. Box, menu and settings windows get their messages here too.
//! * A "root" egui context is kept only for scheduling: everything that used to call
//!   `ctx.request_repaint()` on eframe's context still does, and its repaint callback sets
//!   when the next pass runs. The root context draws nothing (except in `--screenshot`).

#![cfg(windows)]

use crate::app::DeskApp;
use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::INFINITE;
use windows::Win32::UI::WindowsAndMessaging::*;

/// The hidden window that receives wake-ups (0 until it exists).
static WAKE_HWND: AtomicIsize = AtomicIsize::new(0);
/// When the next pass is due, if anyone asked.
static DEADLINE: Mutex<Option<Instant>> = Mutex::new(None);
/// Quit was chosen (tray, settings, menu).
static QUIT: AtomicBool = AtomicBool::new(false);
/// Windows reported a display change (monitor added, removed, switched off, resolution).
static DISPLAY_CHANGED: AtomicBool = AtomicBool::new(false);

/// The quick-search shortcut was pressed.
static SEARCH_REQUESTED: AtomicBool = AtomicBool::new(false);

/// True once after each press of the quick-search shortcut.
pub fn take_search_request() -> bool {
    SEARCH_REQUESTED.swap(false, Ordering::SeqCst)
}

/// Shortcuts offered for quick search: label, modifier keys, key. Index 0 = off.
pub const SEARCH_HOTKEYS: &[(&str, u32, u32)] = &[
    ("Off", 0, 0),
    ("Ctrl+Shift+Space", 0x0002 | 0x0004, 0x20),
    ("Ctrl+Alt+Space", 0x0002 | 0x0001, 0x20),
    ("Win+Shift+Space", 0x0008 | 0x0004, 0x20),
    ("Ctrl+Alt+F", 0x0002 | 0x0001, 0x46),
];

/// (Re)registers the quick-search shortcut with Windows (an ordinary hotkey, no hook). Returns
/// false if another program already uses it. Call from the app's thread.
pub fn set_search_hotkey(index: usize) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS};
    let h = WAKE_HWND.load(Ordering::SeqCst);
    if h == 0 {
        return false;
    }
    let hwnd = HWND(h as *mut _);
    unsafe {
        let _ = UnregisterHotKey(hwnd, 1);
        let Some(&(label, mods, vk)) = SEARCH_HOTKEYS.get(index).filter(|_| index > 0) else { return true };
        // 0x4000 = MOD_NOREPEAT: holding the keys doesn't repeat.
        let ok = RegisterHotKey(hwnd, 1, HOT_KEY_MODIFIERS(mods | 0x4000), vk).is_ok();
        if !ok {
            crate::storage::log_line(&format!("quick search shortcut {label} is already used by another program"));
        }
        ok
    }
}

/// Keys borrowed while the desktop has the focus (type to jump to an icon): virtual-key codes
/// pressed since the last pass.
static TYPED: Mutex<Vec<u32>> = Mutex::new(Vec::new());
/// The typing keys are currently borrowed.
static TYPING_ON: AtomicBool = AtomicBool::new(false);
/// Hotkey ids of the typing keys start here (1 = quick search).
const TYPING_ID: i32 = 100;

/// The keys borrowed for typing on the desktop: digits, letters and Enter.
fn typing_keys() -> impl Iterator<Item = u32> {
    (0x30..=0x39u32).chain(0x41..=0x5Au32).chain(std::iter::once(0x0Du32))
}

/// Borrows (true) or gives back (false) the letter, digit and Enter keys. Only ever on while
/// the desktop itself has the focus: then those keys would go to Explorer's hidden icon list.
/// Ordinary hotkeys, no hook. Call from the app's thread.
pub fn set_typing_keys(on: bool) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS};
    let h = WAKE_HWND.load(Ordering::SeqCst);
    if h == 0 || TYPING_ON.swap(on, Ordering::SeqCst) == on {
        return;
    }
    let hwnd = HWND(h as *mut _);
    unsafe {
        for (i, vk) in typing_keys().enumerate() {
            let id = TYPING_ID + i as i32;
            if on {
                // 0x4000 = MOD_NOREPEAT. A key another program already uses is simply skipped.
                let _ = RegisterHotKey(hwnd, id, HOT_KEY_MODIFIERS(0x4000), vk);
            } else {
                let _ = UnregisterHotKey(hwnd, id);
            }
        }
    }
}

/// Keys typed on the desktop since the last call (virtual-key codes).
pub fn take_typed() -> Vec<u32> {
    TYPED.lock().map(|mut t| std::mem::take(&mut *t)).unwrap_or_default()
}

/// True once after each display change (see WM_DISPLAYCHANGE below).
pub fn take_display_changed() -> bool {
    DISPLAY_CHANGED.swap(false, Ordering::SeqCst)
}
const WM_APP_WAKE: u32 = WM_APP + 1;

thread_local! {
    /// The app, reachable from the hidden window's procedure for the shutdown save.
    static APP: RefCell<Option<DeskApp>> = const { RefCell::new(None) };
    /// The root (scheduling) context, for `pass_now`.
    static ROOT: RefCell<Option<egui::Context>> = const { RefCell::new(None) };
}

/// Runs one pass right away, from inside a window procedure. Used while the person resizes
/// the settings window: Windows then runs its own message loop and ours is paused, so without
/// this the window would only redraw when the mouse is released. Does nothing if a pass is
/// already running (no re-entry).
pub fn pass_now() {
    let Some(root) = ROOT.with(|r| r.borrow().clone()) else { return };
    APP.with(|a| {
        if let Ok(mut a) = a.try_borrow_mut() {
            if let Some(app) = a.as_mut() {
                let _ = root.run(egui::RawInput::default(), |ctx| app.update(ctx));
            }
        }
    });
}

fn post_wake() {
    let h = WAKE_HWND.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(HWND(h as *mut _), WM_APP_WAKE, WPARAM(0), LPARAM(0));
        }
    }
}

/// Asks for a pass after `delay` (keeps the earliest request). Safe from any thread.
pub fn wake_after(delay: Duration) {
    let t = Instant::now() + delay.min(Duration::from_secs(86_400));
    if let Ok(mut d) = DEADLINE.lock() {
        if d.map(|old| t < old).unwrap_or(true) {
            *d = Some(t);
            drop(d);
            post_wake();
        }
    }
}

/// Ends the program cleanly after the current pass (exit code 0: the supervisor stops too).
pub fn request_quit() {
    QUIT.store(true, Ordering::SeqCst);
    post_wake();
}

/// Gives the keyboard focus to the hidden main window (before showing Windows' menu).
pub fn bring_main_to_front() {
    let h = WAKE_HWND.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = SetForegroundWindow(HWND(h as *mut _));
        }
    }
}

/// The hidden main window (owner of Windows' right-click menu, see shellmenu.rs); 0 if none.
pub fn main_hwnd() -> isize {
    WAKE_HWND.load(Ordering::SeqCst)
}

unsafe extern "system" fn main_wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // Windows' right-click menu, while open, draws its submenus through its owner (us).
    if let Some(r) = crate::shellmenu::handle_menu_msg(m, wp, lp) {
        return r;
    }
    match m {
        WM_APP_WAKE => LRESULT(0),
        WM_HOTKEY => {
            let id = wp.0 as i32;
            if id >= TYPING_ID {
                let vk = ((lp.0 >> 16) & 0xFFFF) as u32;
                if crate::desktop::foreground_is_desktop() {
                    if let Ok(mut t) = TYPED.lock() {
                        t.push(vk);
                    }
                    wake_after(Duration::ZERO);
                } else {
                    // Another window came to the front before we gave the keys back: give them
                    // back now and pass this key on, so the person's typing is never lost.
                    set_typing_keys(false);
                    use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP};
                    keybd_event(vk as u8, 0, Default::default(), 0);
                    keybd_event(vk as u8, 0, KEYEVENTF_KEYUP, 0);
                    wake_after(Duration::ZERO);
                }
                return LRESULT(0);
            }
            SEARCH_REQUESTED.store(true, Ordering::SeqCst);
            wake_after(Duration::ZERO);
            LRESULT(0)
        }
        // Monitors changed: the app re-places the boxes and puts them back under the windows
        // (Windows rearranges windows when a monitor goes away).
        WM_DISPLAYCHANGE => {
            DISPLAY_CHANGED.store(true, Ordering::SeqCst);
            wake_after(Duration::ZERO);
            LRESULT(0)
        }
        // Windows is shutting down or the person signs out: allow it, and save first.
        WM_QUERYENDSESSION => LRESULT(1),
        WM_ENDSESSION => {
            if wp.0 != 0 {
                APP.with(|a| {
                    if let Ok(mut a) = a.try_borrow_mut() {
                        if let Some(app) = a.as_mut() {
                            app.shutdown();
                        }
                    }
                });
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(h, m, wp, lp),
    }
}

fn set_dpi_awareness() {
    unsafe {
        use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// A root egui context whose repaint requests wake this loop.
fn root_context() -> egui::Context {
    let root = egui::Context::default();
    root.set_request_repaint_callback(|info| wake_after(info.delay));
    root
}

/// Runs the app until it quits. Returns the process exit code.
pub fn run(screenshot: Option<PathBuf>) -> i32 {
    set_dpi_awareness();
    if let Some(path) = screenshot {
        return run_screenshot(path);
    }
    unsafe {
        let Ok(inst) = GetModuleHandleW(None) else { return crate::EXIT_START_FAILED };
        let wc = WNDCLASSW { lpfnWndProc: Some(main_wndproc), hInstance: inst.into(), lpszClassName: w!("DeskBoxesMain"), ..Default::default() };
        RegisterClassW(&wc);
        // A plain hidden top-level window (not message-only: those don't get the shutdown notice).
        let Ok(hwnd) = CreateWindowExW(WS_EX_TOOLWINDOW, w!("DeskBoxesMain"), w!("DeskBoxes"), WS_POPUP, 0, 0, 0, 0, None, None, inst, None) else {
            crate::storage::log_line("could not create the main window");
            return crate::EXIT_START_FAILED;
        };
        WAKE_HWND.store(hwnd.0 as isize, Ordering::SeqCst);

        let root = root_context();
        // The root context only schedules and never draws text: no fonts (saves their memory).
        root.set_fonts(egui::FontDefinitions::empty());
        let Some(app) = DeskApp::new(&root, None) else {
            WAKE_HWND.store(0, Ordering::SeqCst);
            let _ = DestroyWindow(hwnd);
            return crate::EXIT_START_FAILED;
        };
        APP.with(|a| *a.borrow_mut() = Some(app));
        ROOT.with(|r| *r.borrow_mut() = Some(root.clone()));
        // scripts\stop.ps1 asks for a clean quit through a named event.
        crate::desktop::watch_quit_signal(request_quit);
        wake_after(Duration::ZERO);

        let mut next_pass: Option<Instant> = None;
        'outer: loop {
            // Sleep until a message arrives or the next pass is due.
            let deadline = [DEADLINE.lock().ok().and_then(|d| *d), next_pass].into_iter().flatten().min();
            let timeout = match deadline {
                Some(t) => t.saturating_duration_since(Instant::now()).as_millis().min(u32::MAX as u128 - 1) as u32,
                None => INFINITE,
            };
            if timeout > 0 {
                MsgWaitForMultipleObjectsEx(None, timeout, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
            }
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    break 'outer;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            if QUIT.load(Ordering::SeqCst) {
                break;
            }
            let now = Instant::now();
            let asked = match DEADLINE.lock() {
                Ok(mut d) if d.map(|t| t <= now).unwrap_or(false) => {
                    *d = None;
                    true
                }
                _ => false,
            };
            let scheduled = next_pass.map(|t| t <= now).unwrap_or(false);
            if !(asked || scheduled) {
                continue;
            }
            next_pass = None;
            let out = root.run(egui::RawInput::default(), |ctx| {
                APP.with(|a| {
                    if let Ok(mut a) = a.try_borrow_mut() {
                        if let Some(app) = a.as_mut() {
                            app.update(ctx);
                        }
                    }
                });
            });
            let delay = out.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.repaint_delay).unwrap_or(Duration::MAX);
            if delay < Duration::from_secs(86_400) {
                next_pass = Some(now + delay);
            }
        }
        APP.with(|a| {
            if let Some(mut app) = a.borrow_mut().take() {
                app.shutdown();
            }
        });
        WAKE_HWND.store(0, Ordering::SeqCst);
        let _ = DestroyWindow(hwnd);
    }
    0
}

/// `--screenshot out.png`: draws the settings view (Layout page) into an off-screen picture
/// with the DirectX renderer and saves it. No window is created at all, so it never touches
/// the live desktop.
fn run_screenshot(path: PathBuf) -> i32 {
    const W: usize = 1100;
    const H: usize = 680;
    let renderer = match crate::d3d::Renderer::new() {
        Ok(r) => r,
        Err(e) => {
            crate::storage::log_line(&format!("screenshot: no graphics device: {e:?}"));
            return crate::EXIT_START_FAILED;
        }
    };
    let mut renderer = renderer;
    let root = root_context();
    let Some(mut app) = DeskApp::new(&root, Some(path.clone())) else { return crate::EXIT_START_FAILED };
    let start = Instant::now();
    for frame in 0..3000u32 {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(W as f32, H as f32))),
            time: Some(start.elapsed().as_secs_f64()),
            max_texture_side: Some(8192),
            ..Default::default()
        };
        let out = root.run(raw, |ctx| app.update(ctx));
        renderer.update_textures(&out.textures_delta);
        let ready = frame >= 120 && (!app.icons_busy() || frame >= 600);
        if ready {
            let prims = root.tessellate(out.shapes, out.pixels_per_point);
            match renderer.render_offscreen([W as u32, H as u32], out.pixels_per_point, &prims) {
                Ok(rgba) => save_png(&path, W, H, rgba),
                Err(e) => crate::storage::log_line(&format!("screenshot failed: {e:?}")),
            }
            return 0;
        }
        std::thread::sleep(Duration::from_millis(16));
    }
    0
}

fn save_png(path: &PathBuf, w: usize, h: usize, rgba: Vec<u8>) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, rgba) {
        if let Err(e) = buf.save(path) {
            crate::storage::log_line(&format!("screenshot save failed: {e}"));
        }
    }
}
