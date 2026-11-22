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
const WM_APP_WAKE: u32 = WM_APP + 1;

thread_local! {
    /// The app, reachable from the hidden window's procedure for the shutdown save.
    static APP: RefCell<Option<DeskApp>> = const { RefCell::new(None) };
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

unsafe extern "system" fn main_wndproc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match m {
        WM_APP_WAKE => LRESULT(0),
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
        let Some(app) = DeskApp::new(&root, None) else {
            WAKE_HWND.store(0, Ordering::SeqCst);
            let _ = DestroyWindow(hwnd);
            return crate::EXIT_START_FAILED;
        };
        APP.with(|a| *a.borrow_mut() = Some(app));
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
