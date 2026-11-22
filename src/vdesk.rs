//! Windows' virtual desktops (Win+Ctrl+D, Win+Ctrl+←/→): which one is shown, so each can show its
//! own page of boxes ("Link pages to virtual desktops" in Settings).
//!
//! Windows keeps the list of desktops and the current one in the registry; a helper thread waits
//! for Windows to say that key changed (no polling), then the app is woken. Box windows are moved
//! to the desktop being shown with the documented IVirtualDesktopManager (our own windows only).

#![cfg(windows)]

use std::sync::atomic::{AtomicI32, AtomicBool, Ordering};

const KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\VirtualDesktops";

/// Index of the desktop shown (0 = first), -1 = unknown.
static CURRENT: AtomicI32 = AtomicI32::new(-1);
static STARTED: AtomicBool = AtomicBool::new(false);

/// (index of the current desktop, its id), read from the registry.
fn read() -> Option<(usize, windows::core::GUID)> {
    use winreg::enums::HKEY_CURRENT_USER;
    let key = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey(KEY).ok()?;
    let ids = key.get_raw_value("VirtualDesktopIDs").ok()?.bytes;
    let cur = key.get_raw_value("CurrentVirtualDesktop").ok()?.bytes;
    if cur.len() != 16 {
        return None;
    }
    let idx = ids.chunks_exact(16).position(|c| c == cur.as_slice()).unwrap_or(0);
    let g = windows::core::GUID::from_values(
        u32::from_le_bytes([cur[0], cur[1], cur[2], cur[3]]),
        u16::from_le_bytes([cur[4], cur[5]]),
        u16::from_le_bytes([cur[6], cur[7]]),
        [cur[8], cur[9], cur[10], cur[11], cur[12], cur[13], cur[14], cur[15]],
    );
    Some((idx, g))
}

/// The desktop shown now (0 = first), if Windows says.
pub fn current() -> Option<usize> {
    if !STARTED.swap(true, Ordering::SeqCst) {
        if let Some((i, _)) = read() {
            CURRENT.store(i as i32, Ordering::SeqCst);
        }
        watch();
    }
    let c = CURRENT.load(Ordering::SeqCst);
    (c >= 0).then_some(c as usize)
}

/// Waits (asleep) for the registry key to change and updates `CURRENT`.
fn watch() {
    let _ = std::thread::Builder::new().name("virtual-desktops".into()).spawn(|| unsafe {
        use windows::core::HSTRING;
        use windows::Win32::Foundation::BOOL;
        use windows::Win32::System::Registry::{RegCloseKey, RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, REG_NOTIFY_CHANGE_LAST_SET};
        let mut key = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(KEY), 0, KEY_NOTIFY, &mut key).is_err() {
            return;
        }
        loop {
            // Synchronous: returns when a value in the key changes.
            if RegNotifyChangeKeyValue(key, BOOL(0), REG_NOTIFY_CHANGE_LAST_SET, None, BOOL(0)).is_err() {
                break;
            }
            if let Some((i, _)) = read() {
                if CURRENT.swap(i as i32, Ordering::SeqCst) != i as i32 {
                    crate::runloop::wake_after(std::time::Duration::ZERO);
                }
            }
        }
        let _ = RegCloseKey(key);
    });
}

/// Moves our box windows onto the desktop shown now, so they are there whichever desktop it is.
pub fn bring_windows_here(hwnds: &[isize]) {
    let Some((_, id)) = read() else { return };
    unsafe {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
        use windows::Win32::UI::Shell::{IVirtualDesktopManager, VirtualDesktopManager};
        let Ok(m) = CoCreateInstance::<_, IVirtualDesktopManager>(&VirtualDesktopManager, None, CLSCTX_ALL) else { return };
        for h in hwnds {
            let hwnd = HWND(*h as *mut _);
            if m.IsWindowOnCurrentVirtualDesktop(hwnd).map(|b| !b.as_bool()).unwrap_or(false) {
                let _ = m.MoveWindowToDesktop(hwnd, &id);
            }
        }
    }
}
