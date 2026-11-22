//! Windows' own right-click menu for desktop items: the same menu Explorer shows ("Open with",
//! "Send to", and what other programs add: 7-Zip, the antivirus, …), drawn dark, with
//! DesktopShepherd' own entries ("Move to ▸", "Remove from this box") at the top.
//!
//! It runs in a short-lived helper process (`desktopshepherd.exe --shell-menu …`), not in the main
//! app: other programs' menu add-ons are loaded there and leave with it, so they can neither
//! crash DesktopShepherd nor stay in its memory. The helper's exit code is the DesktopShepherd entry chosen
//! (0 = none, or a Windows command that the helper ran itself).

#![cfg(windows)]

use std::cell::RefCell;
use windows::core::{w, Interface, HSTRING, PCSTR, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    IContextMenu, IContextMenu2, IContextMenu3, IShellFolder, SHBindToParent, SHParseDisplayName, CMF_NORMAL, CMINVOKECOMMANDINFO,
    CMINVOKECOMMANDINFOEX,
};
use windows::Win32::UI::WindowsAndMessaging::*;

thread_local! {
    /// The menu being shown: its owner window forwards drawing messages to it, so submenus like
    /// "Send to" and "Open with" fill in and show their icons.
    static OPEN: RefCell<Option<IContextMenu>> = const { RefCell::new(None) };
}

/// Lets the open menu handle its own drawing messages (called by the owner window).
pub fn handle_menu_msg(msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    if !matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
        return None;
    }
    OPEN.with(|o| {
        let o = o.try_borrow().ok()?;
        let cm = o.as_ref()?;
        unsafe {
            if let Ok(cm3) = cm.cast::<IContextMenu3>() {
                let mut result = LRESULT(0);
                if cm3.HandleMenuMsg2(msg, wp, lp, Some(&mut result)).is_ok() {
                    return Some(result);
                }
            } else if let Ok(cm2) = cm.cast::<IContextMenu2>() {
                if cm2.HandleMenuMsg(msg, wp, lp).is_ok() {
                    return Some(LRESULT(0));
                }
            }
        }
        None
    })
}

/// One DesktopShepherd entry at the top of the menu: its number (returned when chosen) and label.
/// `sub` = put it in the "Move to" submenu.
pub struct Extra {
    pub id: u32,
    pub label: String,
    pub sub: bool,
}

/// Results of helpers that finished (exit code, the item the menu was for), for the app.
static DONE: std::sync::Mutex<Vec<(u32, String)>> = std::sync::Mutex::new(Vec::new());

/// Entries chosen in helper menus since the last call: (entry number, item path).
pub fn take_done() -> Vec<(u32, String)> {
    DONE.lock().map(|mut d| std::mem::take(&mut *d)).unwrap_or_default()
}

/// Starts the helper for `paths` (the first is the clicked item) with `extras` on top.
/// Returns false if it couldn't start (the caller then shows its own menu).
pub fn spawn(paths: &[String], extras: &[Extra]) -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--shell-menu");
    for p in paths {
        cmd.arg("--path").arg(p);
    }
    for e in extras {
        cmd.arg(if e.sub { "--sub" } else { "--item" }).arg(format!("{}|{}", e.id, e.label));
    }
    // The person just clicked in DesktopShepherd: let the helper take the focus (its menu needs it).
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
    let Ok(mut child) = cmd.spawn() else { return false };
    let clicked = paths.first().cloned().unwrap_or_default();
    let _ = std::thread::Builder::new().name("menu-helper-wait".into()).spawn(move || {
        if let Ok(status) = child.wait() {
            let code = status.code().unwrap_or(0).max(0) as u32;
            if code > 0 {
                if let Ok(mut d) = DONE.lock() {
                    d.push((code, clicked));
                }
                crate::runloop::wake_after(std::time::Duration::ZERO);
            }
        }
    });
    true
}

unsafe extern "system" fn owner_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if let Some(r) = handle_menu_msg(m, wp, lp) {
        return r;
    }
    DefWindowProcW(h, m, wp, lp)
}

/// Dark menus like the rest of DesktopShepherd (uxtheme's SetPreferredAppMode / FlushMenuThemes,
/// found by number; silently skipped if this Windows doesn't have them).
unsafe fn dark_menus() {
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    let Ok(ux) = LoadLibraryW(w!("uxtheme.dll")) else { return };
    if let Some(set_mode) = GetProcAddress(ux, PCSTR(135usize as *const u8)) {
        let set_mode: extern "system" fn(i32) -> i32 = std::mem::transmute(set_mode);
        set_mode(2); // ForceDark
    }
    if let Some(flush) = GetProcAddress(ux, PCSTR(136usize as *const u8)) {
        let flush: extern "system" fn() = std::mem::transmute(flush);
        flush();
    }
}

/// The helper process (`--shell-menu`): shows the menu at the mouse, runs a Windows command
/// itself or returns the DesktopShepherd entry chosen as its exit code.
pub fn run_helper(args: &[String]) -> i32 {
    let mut paths = Vec::new();
    let mut extras = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--path" => paths.extend(it.next().cloned()),
            "--item" | "--sub" => {
                if let Some((id, label)) = it.next().and_then(|v| v.split_once('|')) {
                    if let Ok(id) = id.parse::<u32>() {
                        extras.push(Extra { id, label: label.to_string(), sub: a == "--sub" });
                    }
                }
            }
            _ => {}
        }
    }
    unsafe {
        use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        dark_menus();
        let Ok(inst) = GetModuleHandleW(None) else { return 0 };
        let wc = WNDCLASSW { lpfnWndProc: Some(owner_proc), hInstance: inst.into(), lpszClassName: w!("DesktopShepherdMenuOwner"), ..Default::default() };
        RegisterClassW(&wc);
        let Ok(owner) = CreateWindowExW(WS_EX_TOOLWINDOW, w!("DesktopShepherdMenuOwner"), w!("DesktopShepherd menu"), WS_POPUP, 0, 0, 0, 0, None, None, inst, None) else {
            return 0;
        };
        let chosen = match show_inner(owner, &paths, &extras) {
            Ok(c) => c,
            Err(e) => {
                crate::storage::log_line(&format!("Windows menu could not be shown: {e:?}"));
                0
            }
        };
        if chosen == 0 {
            // A Windows command may have opened a window of this process (Properties, Open
            // with…): stay until those are closed.
            wait_for_own_windows(owner);
        }
        let _ = DestroyWindow(owner);
        chosen as i32
    }
}

/// Keeps the helper alive (pumping messages) while it shows windows of its own. A command
/// may take a few seconds to open its window (Properties loads its pages the first time): wait up
/// to 10 s for one to appear, then until every one is closed.
unsafe fn wait_for_own_windows(owner: HWND) {
    use std::time::{Duration, Instant};
    use windows::Win32::UI::WindowsAndMessaging::{MsgWaitForMultipleObjects, QS_ALLINPUT};
    let start = Instant::now();
    let mut seen = false;
    let mut idle_since: Option<Instant> = None;
    while start.elapsed() < Duration::from_secs(3600) {
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        if own_visible_windows(owner) == 0 {
            let since = *idle_since.get_or_insert_with(Instant::now);
            // Nothing opened within 10 s, or everything that opened has been closed for 1 s.
            if (!seen && start.elapsed() >= Duration::from_secs(10)) || (seen && since.elapsed() >= Duration::from_secs(1)) {
                return;
            }
        } else {
            seen = true;
            idle_since = None;
        }
        // Sleep until a message arrives (a dialog of ours stays responsive) or 250 ms pass.
        let _ = MsgWaitForMultipleObjects(None, false, 250, QS_ALLINPUT);
    }
}

unsafe fn own_visible_windows(owner: HWND) -> usize {
    use windows::Win32::Foundation::BOOL;
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        let st = &mut *(lp.0 as *mut (u32, HWND, usize));
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        if pid == st.0 && h != st.1 && IsWindowVisible(h).as_bool() {
            st.2 += 1;
        }
        BOOL(1)
    }
    let mut st = (windows::Win32::System::Threading::GetCurrentProcessId(), owner, 0usize);
    let _ = EnumWindows(Some(cb), LPARAM(&mut st as *mut _ as isize));
    st.2
}

/// A shell ID allocated by Windows (freed on drop).
struct Pidl(*mut ITEMIDLIST);

impl Drop for Pidl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CoTaskMemFree(Some(self.0 as *const _)) };
        }
    }
}

/// Builds the menu (DesktopShepherd entries, then Windows' items for `paths` that share the first
/// one's folder), shows it at the mouse and handles the choice. Returns the DesktopShepherd entry
/// chosen, or 0.
unsafe fn show_inner(owner: HWND, paths: &[String], extras: &[Extra]) -> windows::core::Result<u32> {
    let mut items: Vec<(Pidl, String)> = Vec::new();
    for p in paths {
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        if SHParseDisplayName(&HSTRING::from(p.as_str()), None, &mut pidl, 0, None).is_ok() && !pidl.is_null() {
            items.push((Pidl(pidl), p.clone()));
        }
    }
    let Some((first, first_path)) = items.first() else { return Ok(0) };
    let parent = |p: &str| std::path::Path::new(p).parent().map(|x| x.to_string_lossy().to_lowercase());
    let mut last: *mut ITEMIDLIST = std::ptr::null_mut();
    let folder: IShellFolder = SHBindToParent(first.0, Some(&mut last))?;
    let mut children: Vec<*const ITEMIDLIST> = vec![last as *const _];
    for (pidl, p) in items.iter().skip(1) {
        if parent(p) == parent(first_path) {
            let mut l: *mut ITEMIDLIST = std::ptr::null_mut();
            if SHBindToParent::<IShellFolder>(pidl.0, Some(&mut l)).is_ok() && !l.is_null() {
                children.push(l as *const _);
            }
        }
    }
    let cm: IContextMenu = folder.GetUIObjectOf(owner, &children, None)?;
    let menu = CreatePopupMenu()?;
    const FIRST: u32 = 1;
    const LAST: u32 = 0x7FFF;
    const OURS: u32 = 0x8000;
    let result = (|| -> windows::core::Result<u32> {
        // DesktopShepherd' entries first.
        let mut count = 0u32;
        let subs: Vec<&Extra> = extras.iter().filter(|e| e.sub).collect();
        if !subs.is_empty() {
            let sub = CreatePopupMenu()?;
            for e in &subs {
                let _ = AppendMenuW(sub, MF_STRING, (OURS + e.id) as usize, &HSTRING::from(e.label.as_str()));
            }
            let _ = AppendMenuW(menu, MF_POPUP, sub.0 as usize, w!("Move to"));
            count += 1;
        }
        for e in extras.iter().filter(|e| !e.sub) {
            let _ = AppendMenuW(menu, MF_STRING, (OURS + e.id) as usize, &HSTRING::from(e.label.as_str()));
            count += 1;
        }
        if count > 0 {
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            count += 1;
        }
        cm.QueryContextMenu(menu, count, FIRST, LAST, CMF_NORMAL)?;
        let mut at = POINT::default();
        let _ = GetCursorPos(&mut at);
        OPEN.with(|o| *o.borrow_mut() = Some(cm.clone()));
        // The owner must be in front, or the menu won't close when clicking elsewhere.
        let _ = SetForegroundWindow(owner);
        let cmd = TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, at.x, at.y, owner, None).0 as u32;
        let _ = PostMessageW(owner, WM_NULL, WPARAM(0), LPARAM(0));
        OPEN.with(|o| *o.borrow_mut() = None);
        if cmd >= OURS {
            return Ok(cmd - OURS);
        }
        if cmd >= FIRST {
            // The full form (Unicode verb, the point where the menu was opened): some commands,
            // like Properties, fail with the short form.
            let verb = (cmd - FIRST) as usize;
            let info = CMINVOKECOMMANDINFOEX {
                cbSize: std::mem::size_of::<CMINVOKECOMMANDINFOEX>() as u32,
                fMask: 0x4000 /* CMIC_MASK_UNICODE */ | 0x2000_0000 /* CMIC_MASK_PTINVOKE */,
                hwnd: owner,
                lpVerb: PCSTR(verb as *const u8),
                lpVerbW: PCWSTR(verb as *const u16),
                nShow: SW_SHOWNORMAL.0,
                ptInvoke: at,
                ..Default::default()
            };
            cm.InvokeCommand(&info as *const CMINVOKECOMMANDINFOEX as *const CMINVOKECOMMANDINFO)?;
        }
        Ok(0)
    })();
    let _ = DestroyMenu(menu);
    result
}
