//! The picture of an icon that flies with the mouse while icons are dragged between boxes: a
//! small per-pixel transparent ("layered") window, always on top and click-through, moved by its
//! own helper thread. It removes itself when the mouse button is released.

#[cfg(windows)]
mod imp {
    use windows::core::w;
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, POINT, SIZE, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, AC_SRC_ALPHA,
        AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SetWindowPos, ShowWindow, UpdateLayeredWindow,
        SWP_NOACTIVATE, SWP_NOSIZE, SW_SHOWNOACTIVATE, ULW_ALPHA, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
        WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
    };

    unsafe extern "system" fn ghost_proc(h: HWND, m: u32, wp: WPARAM, lp: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
        DefWindowProcW(h, m, wp, lp)
    }

    fn class_ready() -> bool {
        static READY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *READY.get_or_init(|| unsafe {
            let Ok(inst) = GetModuleHandleW(None) else { return false };
            let wc = WNDCLASSW { lpfnWndProc: Some(ghost_proc), hInstance: inst.into(), lpszClassName: w!("DeskBoxesGhost"), ..Default::default() };
            RegisterClassW(&wc) != 0
        })
    }

    fn bitmap_info(w: usize, h: usize) -> BITMAPINFO {
        BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Creates the see-through window from premultiplied BGRA pixels at (x, y) screen pixels.
    fn create(w: usize, h: usize, bgra: &[u8], x: i32, y: i32) -> Option<isize> {
        if !class_ready() || bgra.len() != w * h * 4 {
            return None;
        }
        unsafe {
            let inst = GetModuleHandleW(None).ok()?;
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
                w!("DeskBoxesGhost"),
                w!("DeskBoxes dragged icon"),
                WS_POPUP,
                x,
                y,
                w as i32,
                h as i32,
                None,
                None,
                inst,
                None,
            )
            .ok()?;
            if !paint(hwnd, w, h, bgra, (x, y)) {
                let _ = DestroyWindow(hwnd);
                return None;
            }
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            Some(hwnd.0 as isize)
        }
    }

    unsafe fn paint(hwnd: HWND, w: usize, h: usize, bgra: &[u8], at: (i32, i32)) -> bool {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(screen);
        let mut info = bitmap_info(w, h);
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut ok = false;
        if let Ok(bmp) = CreateDIBSection(dc, &mut info, DIB_RGB_COLORS, &mut bits, None, 0) {
            if !bits.is_null() {
                std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());
                let old = SelectObject(dc, HGDIOBJ(bmp.0));
                let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
                let pos = POINT { x: at.0, y: at.1 };
                let size = SIZE { cx: w as i32, cy: h as i32 };
                let src = POINT { x: 0, y: 0 };
                ok = UpdateLayeredWindow(hwnd, screen, Some(&pos), Some(&size), dc, Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA).is_ok();
                SelectObject(dc, old);
            }
            let _ = DeleteObject(HGDIOBJ(bmp.0));
        }
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        ok
    }

    /// Shows the picture under the mouse and moves it from its own helper thread (about every
    /// millisecond) until the mouse button is released. grab = mouse position minus the
    /// picture's top-left (pixels). Returns the picture's window handle.
    pub fn begin_icon_drag(w: usize, h: usize, bgra: Vec<u8>, grab: (i32, i32)) -> Option<isize> {
        let (tx, rx) = std::sync::mpsc::channel::<Option<isize>>();
        std::thread::Builder::new()
            .name("icon-drag".into())
            .spawn(move || unsafe {
                use windows::Win32::UI::WindowsAndMessaging::{
                    DispatchMessageW, GetCursorPos, IsWindow, PeekMessageW, TranslateMessage, HWND_TOPMOST, MSG, PM_REMOVE,
                    SWP_NOMOVE, SWP_NOZORDER,
                };
                let mut p = POINT::default();
                let _ = GetCursorPos(&mut p);
                let Some(hwnd) = create(w, h, &bgra, p.x - grab.0, p.y - grab.1) else {
                    let _ = tx.send(None);
                    return;
                };
                let hw = HWND(hwnd as *mut _);
                let _ = SetWindowPos(hw, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOMOVE);
                let _ = tx.send(Some(hwnd));
                let mut last = (p.x, p.y);
                loop {
                    let mut msg = MSG::default();
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    if !IsWindow(hw).as_bool() {
                        break;
                    }
                    // Button released: remove the picture ourselves, so it can never get stuck
                    // if the app misses the end of the drag.
                    if !super::primary_button_down() {
                        let _ = DestroyWindow(hw);
                        break;
                    }
                    let mut c = POINT::default();
                    if GetCursorPos(&mut c).is_ok() && (c.x, c.y) != last {
                        let _ = SetWindowPos(hw, None, c.x - grab.0, c.y - grab.1, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
                        last = (c.x, c.y);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            })
            .ok()?;
        rx.recv_timeout(std::time::Duration::from_secs(2)).ok().flatten()
    }

    /// Removes the picture (it lives on its own thread: ask it to close).
    pub fn destroy(hwnd: isize) {
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
            let _ = PostMessageW(HWND(hwnd as *mut _), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn begin_icon_drag(_: usize, _: usize, _: Vec<u8>, _: (i32, i32)) -> Option<isize> {
        None
    }
    pub fn destroy(_: isize) {}
}

pub use imp::{begin_icon_drag, destroy};

/// True while the user's main mouse button is held. Honors "swap primary and secondary
/// buttons": then the physical right button is the main one.
#[cfg(windows)]
pub(crate) fn primary_button_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_SWAPBUTTON};
    unsafe {
        let vk = if GetSystemMetrics(SM_SWAPBUTTON) != 0 { VK_RBUTTON } else { VK_LBUTTON };
        (GetAsyncKeyState(vk.0 as i32) as u16 & 0x8000) != 0
    }
}

#[cfg(not(windows))]
pub(crate) fn primary_button_down() -> bool {
    false
}
