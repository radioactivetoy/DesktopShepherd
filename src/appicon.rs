//! The app icon as window icons, made at run time from the embedded PNG files (works with both
//! toolchains; the file icon of the .exe itself comes from a resource added by `build.rs`).

use std::sync::OnceLock;
use windows::Win32::UI::WindowsAndMessaging::{CreateIconFromResourceEx, HICON, LR_DEFAULTCOLOR};

static BIG: &[u8] = include_bytes!("../assets/icon/icon-32.png");
static SMALL: &[u8] = include_bytes!("../assets/icon/icon-16.png");

fn make(bytes: &[u8], size: i32) -> isize {
    // SAFETY: the bytes are a valid embedded PNG; Windows copies what it needs.
    unsafe {
        match CreateIconFromResourceEx(bytes, true, 0x0003_0000, size, size, LR_DEFAULTCOLOR) {
            Ok(h) => h.0 as isize,
            Err(_) => 0,
        }
    }
}

/// 32 px icon (title bar of bigger windows, Alt+Tab). Default (null) icon if it can't be made.
pub fn big() -> HICON {
    static C: OnceLock<isize> = OnceLock::new();
    HICON(*C.get_or_init(|| make(BIG, 32)) as *mut _)
}

/// 16 px icon (title bar).
pub fn small() -> HICON {
    static C: OnceLock<isize> = OnceLock::new();
    HICON(*C.get_or_init(|| make(SMALL, 16)) as *mut _)
}
