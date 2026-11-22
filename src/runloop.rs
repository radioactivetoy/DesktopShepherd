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
/// Explorer (re)started: Windows broadcasts "TaskbarCreated" (its message number is kept here).
static TASKBAR_CREATED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static EXPLORER_RESTARTED: AtomicBool = AtomicBool::new(false);

/// True once after Explorer restarted (a new desktop and taskbar).
pub fn take_explorer_restarted() -> bool {
    EXPLORER_RESTARTED.swap(false, Ordering::SeqCst)
}

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
    ("Ctrl+Alt+D", 0x0002 | 0x0001, 0x44),
    ("Ctrl+Shift+H", 0x0002 | 0x0004, 0x48),
];

/// The hide/show-all-boxes shortcut was pressed.
static HIDE_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Page switches asked for with Ctrl+Alt+PageDown (+1) / PageUp (-1) since the last pass.
static PAGE_STEP: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

/// Page switches asked for since the last call (+ next, - previous).
pub fn take_page_step() -> i32 {
    PAGE_STEP.swap(0, Ordering::SeqCst)
}

/// Registers the page shortcuts (Ctrl+Alt+PageDown / PageUp). Call once, from the app's thread.
pub fn set_page_hotkeys() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, HOT_KEY_MODIFIERS};
    let h = WAKE_HWND.load(Ordering::SeqCst);
    if h == 0 {
        return;
    }
    unsafe {
        let hwnd = HWND(h as *mut _);
        let _ = RegisterHotKey(hwnd, 3, HOT_KEY_MODIFIERS(0x0002 | 0x0001 | 0x4000), 0x22);
        let _ = RegisterHotKey(hwnd, 4, HOT_KEY_MODIFIERS(0x0002 | 0x0001 | 0x4000), 0x21);
    }
}

/// True once after each press of the hide/show-all-boxes shortcut.
pub fn take_hide_request() -> bool {
    HIDE_REQUESTED.swap(false, Ordering::SeqCst)
}

/// (Re)registers the quick-search shortcut with Windows (an ordinary hotkey, no hook). Returns
/// false if another program already uses it. Call from the app's thread.
pub fn set_search_hotkey(index: usize) -> bool {
    set_hotkey(1, "quick search", index)
}

/// (Re)registers the hide/show-all-boxes shortcut (index into SEARCH_HOTKEYS, 0 = off).
pub fn set_hide_hotkey(index: usize) -> bool {
    set_hotkey(2, "hide boxes", index)
}

fn set_hotkey(id: i32, what: &str, index: usize) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS};
    let h = WAKE_HWND.load(Ordering::SeqCst);
    if h == 0 {
        return false;
    }
    let hwnd = HWND(h as *mut _);
    unsafe {
        let _ = UnregisterHotKey(hwnd, id);
        let Some(&(label, mods, vk)) = SEARCH_HOTKEYS.get(index).filter(|_| index > 0) else { return true };
        // 0x4000 = MOD_NOREPEAT: holding the keys doesn't repeat.
        let ok = RegisterHotKey(hwnd, id, HOT_KEY_MODIFIERS(mods | 0x4000), vk).is_ok();
        if !ok {
            crate::storage::log_line(&format!("{what} shortcut {label} is already used by another program"));
        }
        ok
    }
}

/// Keys borrowed while the desktop has the focus (type to jump, keyboard in boxes): modifier
/// keys and virtual-key code of each press since the last pass.
static TYPED: Mutex<Vec<(u32, u32)>> = Mutex::new(Vec::new());
/// The typing keys are currently borrowed.
/// Keys borrowed now: 0 = none, 1 = Enter, Esc, arrows, Delete, F2, Ctrl keys, 3 = also letters and digits.
static TYPING_STATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
/// Hotkey ids of the typing keys start here (1 = quick search).
const TYPING_ID: i32 = 100;

/// The keys borrowed while the desktop has the focus, as (modifiers, key): digits and letters
/// (type to jump), Enter, Esc, arrows, Delete, F2, and Ctrl+A / C / X / V.
fn typing_keys() -> impl Iterator<Item = (u32, u32)> {
    (0x30..=0x39u32)
        .chain(0x41..=0x5Au32)
        .chain([0x0D, 0x1B, 0x25, 0x26, 0x27, 0x28, 0x2E, 0x71])
        .map(|vk| (0u32, vk))
        .chain([0x41u32, 0x43, 0x56, 0x58].map(|vk| (0x0002u32, vk)))
}

/// Borrows (`on`) or gives back the desktop keys; `letters` also borrows letters and digits (type to
/// jump / quick search; left alone when typing on the desktop "does nothing"). Only ever on while
/// the desktop itself has the focus: then those keys would go to Explorer's hidden icon list.
/// Ordinary hotkeys, no hook. Call from the app's thread.
pub fn set_typing_keys(on: bool, letters: bool) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, HOT_KEY_MODIFIERS};
    let h = WAKE_HWND.load(Ordering::SeqCst);
    let want: u8 = if on { 1 | if letters { 2 } else { 0 } } else { 0 };
    if h == 0 {
        return;
    }
    let was = TYPING_STATE.swap(want, Ordering::SeqCst);
    if was == want {
        return;
    }
    let on = want != 0;
    if std::env::var_os("DESKTOPSHEPHERD_DEBUG_KEYS").is_some() {
        crate::storage::log_line(&format!("typing keys {}", if on { "borrowed" } else { "given back" }));
    }
    let hwnd = HWND(h as *mut _);
    unsafe {
        // Start from nothing borrowed, then borrow what is wanted now.
        if was != 0 {
            for (i, _) in typing_keys().enumerate() {
                let _ = UnregisterHotKey(hwnd, TYPING_ID + i as i32);
            }
        }
        for (i, (mods, vk)) in typing_keys().enumerate() {
            let id = TYPING_ID + i as i32;
            let is_letter = mods == 0 && (matches!(vk, 0x30..=0x39) || matches!(vk, 0x41..=0x5A));
            if is_letter && !letters {
                continue;
            }
            if on {
                // 0x4000 = MOD_NOREPEAT. A key another program already uses is simply skipped.
                if RegisterHotKey(hwnd, id, HOT_KEY_MODIFIERS(mods | 0x4000), vk).is_err() {
                    // Logged once per key: another program already uses it.
                    static WARNED: Mutex<Vec<(u32, u32)>> = Mutex::new(Vec::new());
                    if let Ok(mut w) = WARNED.lock() {
                        if !w.contains(&(mods, vk)) {
                            w.push((mods, vk));
                            crate::storage::log_line(&format!("desktop key {vk:#x} (modifiers {mods}) is used by another program; DesktopShepherd leaves it alone"));
                        }
                    }
                }
            }
        }
    }
}

/// Keys pressed on the desktop since the last call: (modifiers, virtual-key code).
pub fn take_typed() -> Vec<(u32, u32)> {
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

/// The hidden main window (clipboard owner for copied files); 0 if none.
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
                let mods = (lp.0 & 0xFFFF) as u32;
                if crate::desktop::foreground_is_desktop() {
                    if let Ok(mut t) = TYPED.lock() {
                        t.push((mods, vk));
                    }
                    wake_after(Duration::ZERO);
                } else {
                    // Another window came to the front before we gave the keys back: give them
                    // back now and pass this key on, so the person's typing is never lost.
                    set_typing_keys(false, false);
                    use windows::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_KEYUP};
                    if mods & 0x0002 != 0 {
                        keybd_event(0x11, 0, Default::default(), 0);
                    }
                    keybd_event(vk as u8, 0, Default::default(), 0);
                    keybd_event(vk as u8, 0, KEYEVENTF_KEYUP, 0);
                    if mods & 0x0002 != 0 {
                        keybd_event(0x11, 0, KEYEVENTF_KEYUP, 0);
                    }
                    wake_after(Duration::ZERO);
                }
                return LRESULT(0);
            }
            if id == 3 || id == 4 {
                PAGE_STEP.fetch_add(if id == 3 { 1 } else { -1 }, Ordering::SeqCst);
            } else if id == 2 {
                HIDE_REQUESTED.store(true, Ordering::SeqCst);
            } else {
                SEARCH_REQUESTED.store(true, Ordering::SeqCst);
            }
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
        _ if m != 0 && m == TASKBAR_CREATED.load(Ordering::SeqCst) => {
            EXPLORER_RESTARTED.store(true, Ordering::SeqCst);
            wake_after(Duration::ZERO);
            DefWindowProcW(h, m, wp, lp)
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
        let wc = WNDCLASSW { lpfnWndProc: Some(main_wndproc), hInstance: inst.into(), lpszClassName: w!("DesktopShepherdMain"), ..Default::default() };
        RegisterClassW(&wc);
        // A plain hidden top-level window (not message-only: those don't get the shutdown notice).
        let Ok(hwnd) = CreateWindowExW(WS_EX_TOOLWINDOW, w!("DesktopShepherdMain"), w!("DesktopShepherd"), WS_POPUP, 0, 0, 0, 0, None, None, inst, None) else {
            crate::storage::log_line("could not create the main window");
            return crate::EXIT_START_FAILED;
        };
        WAKE_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        {
            use windows::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW;
            TASKBAR_CREATED.store(RegisterWindowMessageW(windows::core::w!("TaskbarCreated")), Ordering::SeqCst);
        }

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
