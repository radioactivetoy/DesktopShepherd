//! Everything that talks to Windows: reading the desktop folders, opening items, hiding the
//! system desktop icons, start-with-Windows, single instance and CPU priority.
//! Each call is short and failure-tolerant: a failure is logged and ignored, never a crash.

use crate::model::{Category, DeskItem};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The folders whose contents show up on the desktop: the user's and the shared (Public) one.
pub fn desktop_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(d) = dirs::desktop_dir() {
        v.push(d);
    }
    if let Ok(p) = std::env::var("PUBLIC") {
        let d = PathBuf::from(p).join("Desktop");
        if d.is_dir() {
            v.push(d);
        }
    }
    v
}

pub fn item_from_path(path: &Path) -> Option<DeskItem> {
    let meta = std::fs::metadata(path).ok()?;
    let is_dir = meta.is_dir();
    let file_name = path.file_name()?.to_string_lossy().to_string();
    let ext = if is_dir {
        String::new()
    } else {
        path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
    };
    // Like Explorer with known extensions hidden: shortcuts and files show their plain name.
    let name = if is_dir || ext.is_empty() {
        file_name
    } else {
        path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(file_name)
    };
    Some(DeskItem {
        path: path.to_string_lossy().to_string(),
        name,
        category: Category::of(&ext, is_dir),
        ext,
        is_dir,
        on_desktop: false,
        size: if is_dir { 0 } else { meta.len() },
        modified: meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0),
    })
}

fn is_hidden(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if let Ok(m) = std::fs::symlink_metadata(path) {
            const HIDDEN: u32 = 0x2;
            const SYSTEM: u32 = 0x4;
            return m.file_attributes() & (HIDDEN | SYSTEM) != 0;
        }
    }
    path.file_name().map(|n| n.to_string_lossy().starts_with('.')).unwrap_or(false)
}

/// Reads the desktop folders plus any extra paths boxes refer to (files dropped in from
/// elsewhere). Returns items keyed by lower-case path.
pub fn scan(extra: &[String]) -> HashMap<String, DeskItem> {
    let mut out = HashMap::new();
    for dir in desktop_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if is_hidden(&p) {
                continue;
            }
            if let Some(mut it) = item_from_path(&p) {
                it.on_desktop = true;
                out.insert(it.path.to_lowercase(), it);
            }
        }
    }
    for p in extra {
        let key = p.to_lowercase();
        if out.contains_key(&key) {
            continue;
        }
        if let Some(it) = item_from_path(Path::new(p)) {
            out.insert(key, it);
        }
    }
    out
}

/// Opens a file, folder or shortcut the way a double-click on the desktop would, on its own
/// thread so a slow program start never freezes the boxes.
pub fn open_item(path: &str) {
    let path = path.to_string();
    std::thread::spawn(move || {
        #[cfg(windows)]
        unsafe {
            use windows::core::{w, HSTRING, PCWSTR};
            use windows::Win32::Foundation::HWND;
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
            use windows::Win32::UI::Shell::ShellExecuteW;
            use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
            let file = HSTRING::from(path.as_str());
            let dir = Path::new(&path).parent().map(|p| HSTRING::from(p.as_os_str())).unwrap_or_default();
            let r = ShellExecuteW(
                HWND::default(),
                w!("open"),
                PCWSTR(file.as_ptr()),
                PCWSTR::null(),
                PCWSTR(dir.as_ptr()),
                SW_SHOWNORMAL,
            );
            if (r.0 as isize) <= 32 {
                crate::storage::log_line(&format!("could not open {path}"));
            }
        }
        #[cfg(not(windows))]
        {
            let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
        }
    });
}

pub fn show_in_folder(path: &str) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(format!("/select,{path}")).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
}

pub fn open_folder(path: &Path) {
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(path).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
}

/// Windows' own desktop icon spacing (registry, in twips) converted to pixels and clamped to
/// 16..500, as the original does (function 0x1801be040). Default Windows value -1125 -> 75 px.
pub fn icon_spacing() -> (f32, f32) {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        let key = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey("Control Panel\\Desktop\\WindowMetrics");
        let read = |name: &str| -> f32 {
            key.as_ref()
                .ok()
                .and_then(|k| k.get_value::<String, _>(name).ok())
                .and_then(|s| s.trim().parse::<i32>().ok())
                .map(|twips| (-twips / 15).clamp(16, 500) as f32)
                .unwrap_or(75.0)
        };
        (read("IconSpacing"), read("IconVerticalSpacing"))
    }
    #[cfg(not(windows))]
    {
        (75.0, 75.0)
    }
}

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const RUN_NAME: &str = "DeskBoxes";

pub fn set_start_with_windows(on: bool) {
    #[cfg(windows)]
    {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
        let Ok(key) = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(RUN_KEY, KEY_SET_VALUE) else {
            return;
        };
        if on {
            if let Ok(exe) = std::env::current_exe() {
                // "--boot" makes the start wait longer for the desktop, as the original does.
                let _ = key.set_value(RUN_NAME, &format!("\"{}\" --boot", exe.display()));
            }
        } else {
            let _ = key.delete_value(RUN_NAME);
        }
    }
    #[cfg(not(windows))]
    {
        let _ = on;
    }
}

/// Returns false when another copy is already running.
pub fn claim_single_instance() -> bool {
    #[cfg(windows)]
    unsafe {
        use windows::core::w;
        use windows::Win32::Foundation::{GetLastError, BOOL, ERROR_ALREADY_EXISTS};
        use windows::Win32::System::Threading::CreateMutexW;
        match CreateMutexW(None, BOOL(0), w!("Local\\DeskBoxes.SingleInstance")) {
            Ok(h) => {
                let exists = GetLastError() == ERROR_ALREADY_EXISTS;
                // Keep the handle for the whole run (it closes when the process ends).
                let _ = h;
                !exists
            }
            Err(_) => true,
        }
    }
    #[cfg(not(windows))]
    {
        true
    }
}

pub fn set_low_priority(on: bool) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{
            GetCurrentProcess, SetPriorityClass, BELOW_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
        };
        let _ = SetPriorityClass(
            GetCurrentProcess(),
            if on { BELOW_NORMAL_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS },
        );
    }
    #[cfg(not(windows))]
    {
        let _ = on;
    }
}

/// Listens for a polite "please quit" request (scripts\stop.ps1 sets the named event
/// `Local\DeskBoxes.Quit`) and calls `on_quit` once when it arrives. The helper thread sleeps
/// inside Windows until then, so it costs no CPU. Call once, from the worker.
pub fn watch_quit_signal(on_quit: impl Fn() + Send + 'static) {
    #[cfg(windows)]
    unsafe {
        use windows::core::w;
        use windows::Win32::Foundation::{BOOL, HANDLE};
        use windows::Win32::System::Threading::{CreateEventW, ResetEvent, WaitForSingleObject, INFINITE};
        // Manual-reset event, not signalled at the start.
        let ev = match CreateEventW(None, BOOL(1), BOOL(0), w!("Local\\DeskBoxes.Quit")) {
            Ok(h) => h,
            Err(e) => {
                storage_log(&format!("quit signal: could not create event: {e}"));
                return;
            }
        };
        // Clear any request left over from before this start.
        let _ = ResetEvent(ev);
        // Raw handles are not Send; pass the number and rebuild it in the thread.
        // The handle is kept open for the whole run on purpose.
        let raw = ev.0 as isize;
        let spawned = std::thread::Builder::new().name("quit-signal".into()).spawn(move || {
            let h = HANDLE(raw as *mut _);
            if WaitForSingleObject(h, INFINITE).0 == 0 {
                storage_log("quit signal received");
                on_quit();
            }
        });
        if let Err(e) = spawned {
            storage_log(&format!("quit signal: could not start thread: {e}"));
        }
    }
    #[cfg(not(windows))]
    {
        let _ = on_quit;
    }
}

/// The main monitor's size in screen points (pixels divided by the display scale).
pub fn primary_screen_points() -> [f32; 2] {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::HiDpi::GetDpiForSystem;
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        let scale = (GetDpiForSystem().max(96) as f32) / 96.0;
        let w = GetSystemMetrics(SM_CXSCREEN).max(800) as f32;
        let h = GetSystemMetrics(SM_CYSCREEN).max(600) as f32;
        // The window system made this process DPI-aware, so metrics are real pixels.
        [w / scale, h / scale]
    }
    #[cfg(not(windows))]
    {
        [1920.0, 1080.0]
    }
}

/// The usable area (without the taskbar) of the monitor under a point, in screen points.
pub fn work_area_at(p: egui::Pos2, ppp: f32) -> egui::Rect {
    use egui::{Pos2, Rect};
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
        let hm = MonitorFromPoint(POINT { x: (p.x * ppp) as i32, y: (p.y * ppp) as i32 }, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(hm, &mut mi).as_bool() {
            let w = mi.rcWork;
            return Rect::from_min_max(
                Pos2::new(w.left as f32 / ppp, w.top as f32 / ppp),
                Pos2::new(w.right as f32 / ppp, w.bottom as f32 / ppp),
            );
        }
    }
    let [w, h] = primary_screen_points();
    Rect::from_min_max(Pos2::ZERO, Pos2::new(w, h - 48.0))
}

/// Prepares a box window so it behaves like part of the desktop: clicking it does not activate
/// it, so Windows never brings it in front of your apps (WM_MOUSEACTIVATE -> MA_NOACTIVATE).
/// Boxes that need typing (widgets) may still take focus; send_to_bottom puts any box back
/// when it loses focus. Nothing here fights Explorer: there is no hook in Explorer and no
/// repeated z-order forcing (an earlier version forced every move to the bottom, which could
/// clash with Explorer keeping its own desktop at the bottom). Returns the window handle.
pub fn prepare_box(title: &str, no_activate: bool) -> Option<isize> {
    #[cfg(windows)]
    unsafe {
        use windows::core::HSTRING;
        use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
        use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
        use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, MA_NOACTIVATE, WM_MOUSEACTIVATE};

        unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, data: usize) -> LRESULT {
            if msg == WM_MOUSEACTIVATE && data == 1 {
                return LRESULT(MA_NOACTIVATE as isize);
            }
            DefSubclassProc(hwnd, msg, wp, lp)
        }

        let t = HSTRING::from(title);
        let hwnd = FindWindowW(None, &t).ok()?;
        if hwnd.0.is_null() {
            return None;
        }
        let _ = SetWindowSubclass(hwnd, Some(proc), 0xDE5B, if no_activate { 1 } else { 0 });
        register_box(hwnd.0 as isize);
        send_to_bottom(hwnd.0 as isize);
        Some(hwnd.0 as isize)
    }
    #[cfg(not(windows))]
    {
        let _ = (title, no_activate);
        Some(0)
    }
}

/// One display: its Windows device name (e.g. \\.\DISPLAY2), whether it is the main one, and
/// its area in screen points.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    pub name: String,
    pub primary: bool,
    pub rect: egui::Rect,
}

/// All displays, ordered by role: the main monitor first, then the others left to right
/// (then top to bottom). Index = role (0 main, 1 second, ...).
pub fn monitors() -> Vec<Monitor> {
    use egui::{Pos2, Rect};
    #[allow(unused_mut)]
    let mut out: Vec<Monitor> = Vec::new();
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
        use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};
        use windows::Win32::UI::HiDpi::GetDpiForSystem;
        unsafe extern "system" fn cb(hm: HMONITOR, _dc: HDC, _r: *mut RECT, lp: LPARAM) -> BOOL {
            let list = &mut *(lp.0 as *mut Vec<(String, bool, RECT)>);
            let mut mi = MONITORINFOEXW::default();
            mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if GetMonitorInfoW(hm, &mut mi as *mut _ as *mut MONITORINFO).as_bool() {
                let len = mi.szDevice.iter().position(|c| *c == 0).unwrap_or(mi.szDevice.len());
                let name = String::from_utf16_lossy(&mi.szDevice[..len]);
                list.push((name, mi.monitorInfo.dwFlags & 1 != 0, mi.monitorInfo.rcMonitor));
            }
            BOOL(1)
        }
        let mut raw: Vec<(String, bool, RECT)> = Vec::new();
        let _ = EnumDisplayMonitors(HDC::default(), None, Some(cb), LPARAM(&mut raw as *mut _ as isize));
        let scale = (GetDpiForSystem().max(96) as f32) / 96.0;
        for (name, primary, r) in raw {
            out.push(Monitor {
                name,
                primary,
                rect: Rect::from_min_max(
                    Pos2::new(r.left as f32 / scale, r.top as f32 / scale),
                    Pos2::new(r.right as f32 / scale, r.bottom as f32 / scale),
                ),
            });
        }
    }
    if out.is_empty() {
        let [w, h] = primary_screen_points();
        out.push(Monitor { name: "main".into(), primary: true, rect: Rect::from_min_size(Pos2::ZERO, egui::Vec2::new(w, h)) });
    }
    out.sort_by(|a, b| {
        (!a.primary, a.rect.min.x as i64, a.rect.min.y as i64).cmp(&(!b.primary, b.rect.min.x as i64, b.rect.min.y as i64))
    });
    out
}

/// Windows' display scale for this computer (1.0 = 100 %, 1.25 = 125 %).
pub fn system_scale() -> f32 {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::HiDpi::GetDpiForSystem;
        (GetDpiForSystem().max(96) as f32) / 96.0
    }
    #[cfg(not(windows))]
    {
        1.0
    }
}

/// The mouse position on screen, in points.
pub fn cursor_points(ppp: f32) -> Option<egui::Pos2> {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut p = POINT::default();
        GetCursorPos(&mut p).ok()?;
        Some(egui::Pos2::new(p.x as f32 / ppp, p.y as f32 / ppp))
    }
    #[cfg(not(windows))]
    {
        let _ = ppp;
        None
    }
}

/// Every box window, so they can follow the desktop when "Show desktop" (Win+D) lifts it.
static BOX_HWNDS: std::sync::Mutex<Vec<isize>> = std::sync::Mutex::new(Vec::new());

pub fn register_box(hwnd: isize) {
    if let Ok(mut v) = BOX_HWNDS.lock() {
        if !v.contains(&hwnd) {
            v.push(hwnd);
        }
    }
}

pub fn unregister_box(hwnd: isize) {
    if let Ok(mut v) = BOX_HWNDS.lock() {
        v.retain(|h| *h != hwnd);
    }
}

/// The top-level window that shows the desktop (Progman, or the WorkerW holding the icons).
#[cfg(windows)]
fn desktop_window() -> Option<windows::Win32::Foundation::HWND> {
    use windows::core::w;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetAncestor, GA_ROOT};
    unsafe {
        if let Some(lv) = icons_view::list_view() {
            let root = GetAncestor(lv, GA_ROOT);
            if !root.0.is_null() {
                return Some(root);
            }
        }
        FindWindowW(w!("Progman"), None).ok().filter(|h| !h.0.is_null())
    }
}

/// One request to put a box behind all windows but just above the desktop (asynchronous:
/// never waits on anyone). Normally the desktop is at the very bottom, so this is "behind
/// every app"; after Win+D the desktop sits on top, and the boxes go on top with it. Windows
/// of this program above the desktop are skipped when choosing the spot.
pub fn send_to_bottom(hwnd: isize) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::Threading::GetCurrentProcessId;
        use windows::Win32::UI::WindowsAndMessaging::{
            GetWindow, GetWindowLongW, GetWindowThreadProcessId, SetWindowPos, GWL_EXSTYLE, GW_HWNDPREV, HWND_BOTTOM, HWND_TOP, WS_EX_TOPMOST,
            SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        };
        let me = GetCurrentProcessId();
        let after = match desktop_window() {
            None => HWND_BOTTOM,
            Some(desk) => {
                // The nearest window above the desktop that isn't ours: the box goes just below it.
                let mut w = GetWindow(desk, GW_HWNDPREV).unwrap_or_default();
                let mut guard = 0;
                while !w.0.is_null() && guard < 4096 {
                    let mut pid = 0u32;
                    GetWindowThreadProcessId(w, Some(&mut pid));
                    // Skip our own windows and always-on-top ones (the taskbar...): going below a
                    // topmost window could make a box topmost too.
                    let topmost = (GetWindowLongW(w, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0) != 0;
                    if pid != me && !topmost {
                        break;
                    }
                    w = GetWindow(w, GW_HWNDPREV).unwrap_or_default();
                    guard += 1;
                }
                if w.0.is_null() { HWND_TOP } else { w }
            }
        };
        let _ = SetWindowPos(
            HWND(hwnd as *mut _),
            after,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
        );
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
    }
}

/// True when the window in front is the desktop (Progman / WorkerW), e.g. right after a click on
/// an empty part of the desktop. Only then may the app borrow the letter keys (see runloop.rs).
pub fn foreground_is_desktop() -> bool {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GetClassNameW, GetForegroundWindow, GA_ROOT};
        let class_of = |h: windows::Win32::Foundation::HWND| {
            let mut buf = [0u16; 32];
            let n = GetClassNameW(h, &mut buf);
            String::from_utf16_lossy(&buf[..n.max(0) as usize])
        };
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            return false;
        }
        let c = class_of(fg);
        let root = class_of(GetAncestor(fg, GA_ROOT));
        c == "Progman" || c == "WorkerW" || root == "Progman" || root == "WorkerW"
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Watches which window comes to the front (an ordinary notification from Windows, nothing is
/// put inside Explorer). When the desktop comes forward ("Show desktop", Win+D, or a click on
/// it), every box is placed just above it so the boxes stay visible. Call once from the UI
/// thread (the notifications arrive through its message loop).
pub fn watch_show_desktop() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
        use windows::Win32::UI::WindowsAndMessaging::{
            GetClassNameW, IsWindow, EVENT_SYSTEM_FOREGROUND, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS,
        };
        static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            return;
        }
        unsafe extern "system" fn cb(_h: HWINEVENTHOOK, _ev: u32, hwnd: HWND, _o: i32, _c: i32, _t: u32, _ms: u32) {
            // Any window came to the front: the app re-checks whether the desktop has the focus.
            crate::runloop::wake_after(std::time::Duration::ZERO);
            let mut buf = [0u16; 32];
            let n = GetClassNameW(hwnd, &mut buf);
            let class = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
            if class != "Progman" && class != "WorkerW" {
                return;
            }
            let boxes: Vec<isize> = match BOX_HWNDS.lock() {
                Ok(mut v) => {
                    v.retain(|h| IsWindow(HWND(*h as *mut _)).as_bool());
                    v.clone()
                }
                Err(_) => return,
            };
            for h in &boxes {
                send_to_bottom(*h);
            }
            // Show desktop lifts the desktop 70-100 ms after it takes the focus (measured with
            // tools\measure_show_desktop_gap.ps1). The guard thread watches closely for a moment
            // and puts the boxes back the instant the desktop covers them.
            if let Some(tx) = GUARD.get() {
                if let Ok(tx) = tx.lock() {
                    let _ = tx.send(());
                }
            }
        }
        // One long-lived guard thread, asleep (no CPU) until the desktop comes forward.
        {
            let (tx, rx) = std::sync::mpsc::channel::<()>();
            let _ = GUARD.set(std::sync::Mutex::new(tx));
            let _ = std::thread::Builder::new().name("show-desktop-guard".into()).spawn(move || {
                use std::time::{Duration, Instant};
                while rx.recv().is_ok() {
                    let mut until = Instant::now() + Duration::from_millis(GUARD_WINDOW_MS);
                    while Instant::now() < until {
                        // Another desktop click meanwhile: keep watching a little longer.
                        while rx.try_recv().is_ok() {
                            until = Instant::now() + Duration::from_millis(GUARD_WINDOW_MS);
                        }
                        let boxes: Vec<isize> = BOX_HWNDS.lock().map(|v| v.clone()).unwrap_or_default();
                        if boxes.first().map(|h| box_below_desktop(*h)).unwrap_or(false) {
                            for h in &boxes {
                                send_to_bottom(*h);
                            }
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
            });
        }
        let hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(cb),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        storage_log(&format!("show-desktop watch: {:?}", hook.0));

        // When the desktop already has the focus (e.g. right after a click on it), Win+D lifts it
        // without any change of focused window, so the notice above never comes. Explorer then
        // only moves the keyboard focus to the desktop's icon list: watch for that too.
        unsafe extern "system" fn focus_cb(_h: HWINEVENTHOOK, _ev: u32, hwnd: HWND, _o: i32, _c: i32, _t: u32, _ms: u32) {
            use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GA_ROOT};
            let class_of = |h: HWND| {
                let mut buf = [0u16; 32];
                let n = GetClassNameW(h, &mut buf);
                String::from_utf16_lossy(&buf[..n.max(0) as usize])
            };
            let class = class_of(hwnd);
            if class != "SysListView32" && class != "SHELLDLL_DefView" {
                return;
            }
            let root = class_of(GetAncestor(hwnd, GA_ROOT));
            if root != "Progman" && root != "WorkerW" {
                return; // a list view in some other program
            }
            if let Some(tx) = GUARD.get() {
                if let Ok(tx) = tx.lock() {
                    let _ = tx.send(());
                }
            }
        }
        let focus_hook = SetWinEventHook(
            windows::Win32::UI::WindowsAndMessaging::EVENT_OBJECT_FOCUS,
            windows::Win32::UI::WindowsAndMessaging::EVENT_OBJECT_FOCUS,
            None,
            Some(focus_cb),
            0,
            0,
            WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
        );
        storage_log(&format!("show-desktop focus watch: {:?}", focus_hook.0));
    }
}

/// Wakes the show-desktop guard thread.
#[cfg(windows)]
static GUARD: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Sender<()>>> = std::sync::OnceLock::new();
/// How long the guard watches after the desktop comes forward.
#[cfg(windows)]
const GUARD_WINDOW_MS: u64 = 1000;

/// True when the box window sits below the desktop window in the stack (so it is covered).
/// Only reads the window list; never sends anything to Explorer.
#[cfg(windows)]
fn box_below_desktop(hwnd: isize) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindow, GW_HWNDPREV};
    let Some(desk) = desktop_window() else { return false };
    unsafe {
        let mut w = GetWindow(HWND(hwnd as *mut _), GW_HWNDPREV).unwrap_or_default();
        let mut guard = 0;
        while !w.0.is_null() && guard < 8192 {
            if w == desk {
                return true;
            }
            w = GetWindow(w, GW_HWNDPREV).unwrap_or_default();
            guard += 1;
        }
    }
    false
}

#[cfg(windows)]
fn storage_log(msg: &str) {
    crate::storage::log_line(msg);
}

/// Where a box's drawing area really is on screen right now (points). Asked from Windows
/// directly so the glass wallpaper stays lined up while the box is being dragged.
pub fn client_rect_points(hwnd: isize, ppp: f32) -> Option<egui::Rect> {
    use egui::{Pos2, Rect};
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::{HWND, POINT, RECT};
        use windows::Win32::Graphics::Gdi::ClientToScreen;
        use windows::Win32::UI::WindowsAndMessaging::GetClientRect;
        let h = HWND(hwnd as *mut _);
        let mut r = RECT::default();
        GetClientRect(h, &mut r).ok()?;
        let mut p = POINT { x: 0, y: 0 };
        if !ClientToScreen(h, &mut p).as_bool() {
            return None;
        }
        return Some(Rect::from_min_size(
            Pos2::new(p.x as f32 / ppp, p.y as f32 / ppp),
            egui::Vec2::new((r.right - r.left) as f32 / ppp, (r.bottom - r.top) as f32 / ppp),
        ));
    }
    #[cfg(not(windows))]
    {
        let _ = (hwnd, ppp);
        None
    }
}
/// Whether Explorer's desktop is up (Progman/WorkerW holding SHELLDLL_DefView).
pub fn desktop_ready() -> bool {
    #[cfg(windows)]
    {
        icons_view::list_view().is_some()
    }
    #[cfg(not(windows))]
    {
        true
    }
}

/// Waits for the desktop like the original (0x1802b00a0): up to `tries` checks 50 ms apart —
/// 120 tries (6 s) normally, 600 (30 s) when started with Windows (0x18019f600 / 0x18018c1b0).
pub fn wait_for_desktop(tries: u32) {
    for _ in 0..tries {
        if desktop_ready() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

/// Shows the system desktop icons unconditionally (used by the supervisor after repeated
/// crashes, when the app had been set to hide them).
pub fn force_show_desktop_icons() {
    set_icons_window(true);
}

/// If we hid the system icons and Explorer restarted (it shows them again), hide them again.
pub fn reapply_icon_hiding() {
    if WE_HID_ICONS.load(std::sync::atomic::Ordering::SeqCst) {
        set_icons_window(false);
    }
}

/// Asks Windows 11 to round a window's corners natively (radius 0 = square, ≤4 = small) and
/// to skip its own border, since boxes draw theirs. Harmless on Windows 10 (ignored).
pub fn round_window(title: &str, radius: f32) -> bool {
    #[cfg(windows)]
    unsafe {
        use windows::core::HSTRING;
        use windows::Win32::Graphics::Dwm::{
            DwmSetWindowAttribute, DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND,
            DWMWCP_ROUNDSMALL,
        };
        use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
        let t = HSTRING::from(title);
        let Ok(hwnd) = FindWindowW(None, &t) else { return false };
        if hwnd.0.is_null() {
            return false;
        }
        let pref = if radius < 1.0 {
            DWMWCP_DONOTROUND
        } else if radius <= 4.0 {
            DWMWCP_ROUNDSMALL
        } else {
            DWMWCP_ROUND
        };
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of_val(&pref) as u32,
        );
        let none: u32 = 0xFFFF_FFFE; // DWMWA_COLOR_NONE
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_BORDER_COLOR, &none as *const _ as *const _, 4);
        true
    }
    #[cfg(not(windows))]
    {
        let _ = (title, radius);
        true
    }
}

/// The whole desktop across all monitors, in pixels: x, y, width, height.
pub fn virtual_screen() -> [f32; 4] {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
        };
        [
            GetSystemMetrics(SM_XVIRTUALSCREEN) as f32,
            GetSystemMetrics(SM_YVIRTUALSCREEN) as f32,
            GetSystemMetrics(SM_CXVIRTUALSCREEN).max(640) as f32,
            GetSystemMetrics(SM_CYVIRTUALSCREEN).max(480) as f32,
        ]
    }
    #[cfg(not(windows))]
    {
        [0.0, 0.0, 1920.0, 1080.0]
    }
}

// ---- Hiding Windows' own desktop icons ---------------------------------------------------
// The icons live in a list view (SysListView32) inside SHELLDLL_DefView, whose parent is
// Progman or one of the WorkerW windows (same lookup as the original, function 0x1802b01f0).

#[cfg(windows)]
mod icons_view {
    use windows::core::w;
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, FindWindowExW, FindWindowW};

    unsafe extern "system" fn find_worker(hwnd: HWND, lparam: LPARAM) -> BOOL {
        if let Ok(def) = FindWindowExW(hwnd, HWND::default(), w!("SHELLDLL_DefView"), None) {
            if !def.0.is_null() {
                *(lparam.0 as *mut HWND) = def;
                return BOOL(0);
            }
        }
        BOOL(1)
    }

    pub fn list_view() -> Option<HWND> {
        unsafe {
            let mut def = HWND::default();
            if let Ok(progman) = FindWindowW(w!("Progman"), None) {
                if let Ok(d) = FindWindowExW(progman, HWND::default(), w!("SHELLDLL_DefView"), None) {
                    def = d;
                }
            }
            if def.0.is_null() {
                let _ = EnumWindows(Some(find_worker), LPARAM(&mut def as *mut HWND as isize));
            }
            if def.0.is_null() {
                return None;
            }
            FindWindowExW(def, HWND::default(), w!("SysListView32"), None).ok().filter(|h| !h.0.is_null())
        }
    }
}

static WE_HID_ICONS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Gives the desktop icons back, but only if this program hid them (so Windows' own
/// "Show desktop icons" choice is never overridden).
pub fn restore_desktop_icons() {
    if WE_HID_ICONS.swap(false, std::sync::atomic::Ordering::SeqCst) {
        set_icons_window(true);
    }
}

pub fn set_desktop_icons_visible(visible: bool) {
    if visible {
        restore_desktop_icons();
    } else {
        WE_HID_ICONS.store(true, std::sync::atomic::Ordering::SeqCst);
        set_icons_window(false);
    }
}

fn set_icons_window(visible: bool) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{IsWindowVisible, ShowWindowAsync, SW_HIDE, SW_SHOW};
        if let Some(h) = icons_view::list_view() {
            // Asynchronous: never waits for Explorer, even if it is busy.
            if IsWindowVisible(h).as_bool() != visible {
                let _ = ShowWindowAsync(h, if visible { SW_SHOW } else { SW_HIDE });
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = visible;
    }
}

/// A window's rectangle on screen, in pixels: left, top, right, bottom.
pub fn window_rect_px(hwnd: isize) -> Option<[i32; 4]> {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::{HWND, RECT};
        use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
        let mut r = RECT::default();
        GetWindowRect(HWND(hwnd as *mut _), &mut r).ok()?;
        Some([r.left, r.top, r.right, r.bottom])
    }
    #[cfg(not(windows))]
    {
        let _ = hwnd;
        None
    }
}

/// The usable area (without the taskbar) of every monitor, in pixels: left, top, right, bottom.
pub fn work_areas_px() -> Vec<[i32; 4]> {
    #[allow(unused_mut)]
    let mut out: Vec<[i32; 4]> = Vec::new();
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
        use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO};
        unsafe extern "system" fn cb(hm: HMONITOR, _dc: HDC, _r: *mut RECT, lp: LPARAM) -> BOOL {
            let list = &mut *(lp.0 as *mut Vec<[i32; 4]>);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            if GetMonitorInfoW(hm, &mut mi).as_bool() {
                let w = mi.rcWork;
                list.push([w.left, w.top, w.right, w.bottom]);
            }
            BOOL(1)
        }
        let _ = EnumDisplayMonitors(HDC::default(), None, Some(cb), LPARAM(&mut out as *mut _ as isize));
    }
    out
}
