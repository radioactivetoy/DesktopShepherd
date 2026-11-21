//! Truly see-through boxes while dragging. When the person grabs a box, a snapshot of it is
//! turned into a per-pixel transparent "layered" window (the same kind of window the original
//! app uses): glass areas become partly transparent, empty corners fully transparent, icons and
//! text stay solid. Windows itself moves and blends that window over the real desktop, so the
//! background never jitters, however fast the drag. The real box is made invisible meanwhile
//! (it keeps the mouse) and lands where the drag ends; then the snapshot window is removed.

#[cfg(windows)]
mod imp {
    use windows::core::w;
    use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, AC_SRC_ALPHA,
        AC_SRC_OVER, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows::Win32::Storage::Xps::{PrintWindow, PRINT_WINDOW_FLAGS};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect, GetWindowLongPtrW, RegisterClassW,
        SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos, ShowWindow, UpdateLayeredWindow, GWL_EXSTYLE,
        HWND_BOTTOM, LWA_ALPHA, SWP_NOACTIVATE, SWP_NOSIZE, SW_SHOWNOACTIVATE, ULW_ALPHA, WNDCLASSW, WS_EX_LAYERED,
        WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
    };

    /// Where Windows' own move ended: copy window, left, top (screen pixels).
    static DONE: std::sync::Mutex<Option<(isize, i32, i32)>> = std::sync::Mutex::new(None);

    unsafe extern "system" fn ghost_proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        const WM_EXITSIZEMOVE: u32 = 0x0232;
        if m == WM_EXITSIZEMOVE {
            let mut r = RECT::default();
            if windows::Win32::UI::WindowsAndMessaging::GetWindowRect(h, &mut r).is_ok() {
                if let Ok(mut d) = DONE.lock() {
                    *d = Some((h.0 as isize, r.left, r.top));
                }
                if let Some(w) = super::WAKE.get() {
                    w();
                }
            }
        }
        DefWindowProcW(h, m, wp, lp)
    }

    /// Shows the see-through copy and moves it with the mouse from its own small thread, about
    /// every millisecond and independent of the app's drawing, like Windows dragging a window.
    /// grab = mouse position minus the copy's top-left (pixels). Ends when the left button is
    /// released: the final top-left is stored for 	ake_done and the app is woken. Returns the
    /// copy's window handle.
    /// Pictures waiting to be shown by a copy's helper (copy window -> sender).
    static UPDATES: std::sync::Mutex<Vec<(isize, std::sync::mpsc::Sender<(usize, usize, Vec<u8>)>)>> = std::sync::Mutex::new(Vec::new());

    /// `bake`: for a frosted box, the desktop picture, its top-left (pixels) and the blur
    /// strength; the helper then draws the frosted glass itself for every position.
    pub fn begin_drag(w: usize, h: usize, bgra: Vec<u8>, x: i32, y: i32, grab: (i32, i32), bake: Option<(std::sync::Arc<eframe::egui::ColorImage>, (i32, i32), u32)>) -> Option<isize> {
        let (tx, rx) = std::sync::mpsc::channel::<Option<isize>>();
        let (utx, urx) = std::sync::mpsc::channel::<(usize, usize, Vec<u8>)>();
        std::thread::Builder::new()
            .name("box-drag".into())
            .spawn(move || unsafe {
                use windows::Win32::UI::WindowsAndMessaging::{
                    DispatchMessageW, GetCursorPos, IsWindow, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
                };
                let Some(hwnd) = create(w, h, &bgra, x, y) else {
                    let _ = tx.send(None);
                    return;
                };
                // Frosted boxes: the copy's glass is drawn here from the desktop picture, so it
                // matches the resting box exactly (Windows' own blur would cover the whole
                // rectangle, square corners included).
                let mut base = bgra.clone();
                let frost = |base: &[u8], px: i32, py: i32| -> Option<Vec<u8>> {
                    let (pic, o, radius) = bake.as_ref()?;
                    let glass = crate::wallpaper::crop_scaled(pic, eframe::egui::Pos2::new(o.0 as f32, o.1 as f32), [px, py, w as i32, h as i32], *radius);
                    Some(super::compose_over(base, &glass))
                };
                if let Some(img) = frost(&base, x, y) {
                    let _ = paint(HWND(hwnd as *mut _), w, h, &img, Some((x, y)));
                }
                if let Ok(mut u) = UPDATES.lock() {
                    u.push((hwnd, utx));
                }
                let _ = tx.send(Some(hwnd));
                let hw = HWND(hwnd as *mut _);
                let (mut lx, mut ly) = (x, y);
                let mut dragging = true;
                loop {
                    let mut msg = MSG::default();
                    while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    if !IsWindow(hw).as_bool() {
                        break; // the app removed the copy
                    }
                    // A better picture from the app (exact transparency): show it here, on the
                    // thread that owns the copy.
                    while let Ok((uw, uh, px)) = urx.try_recv() {
                        if uw == w && uh == h {
                            base = px;
                            match frost(&base, lx, ly) {
                                Some(img) => {
                                    let _ = paint(hw, w, h, &img, None);
                                }
                                None => {
                                    let _ = paint(hw, w, h, &base, None);
                                }
                            }
                        }
                    }
                    if dragging {
                        let mut p = POINT::default();
                        if GetCursorPos(&mut p).is_ok() {
                            let (nx, ny) = (p.x - grab.0, p.y - grab.1);
                            if nx != lx || ny != ly {
                                // Frosted: new glass for the new spot, moved and shown in one step.
                                match frost(&base, nx, ny) {
                                    Some(img) => {
                                        let _ = paint(hw, w, h, &img, Some((nx, ny)));
                                    }
                                    None => move_to(hwnd, nx, ny),
                                }
                                lx = nx;
                                ly = ny;
                            }
                        }
                        if !super::primary_button_down() {
                            dragging = false;
                            if std::env::var_os("DESKBOXES_DEBUG_DRAG").is_some() {
                                crate::storage::log_line(&format!("drag helper: button up at {lx},{ly} (start {x},{y}, grab {:?})", grab));
                            }
                            if let Ok(mut d) = DONE.lock() {
                                *d = Some((hwnd, lx, ly));
                            }
                            if let Some(w) = super::WAKE.get() {
                                w();
                            }
                        }
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                if let Ok(mut u) = UPDATES.lock() {
                    u.retain(|(h2, _)| *h2 != hwnd);
                }
            })
            .ok()?;
        rx.recv_timeout(std::time::Duration::from_secs(2)).ok().flatten()
    }
    /// The finished Windows move, once: copy window and its final top-left (pixels).
    pub fn take_done() -> Option<(isize, i32, i32)> {
        DONE.lock().ok().and_then(|mut d| d.take())
    }

    /// Tells a box window the mouse button went up (Windows' move swallowed the real one).
    #[allow(dead_code)]
    pub fn post_button_up(hwnd: isize) {
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_LBUTTONUP};
            let _ = PostMessageW(HWND(hwnd as *mut _), WM_LBUTTONUP, WPARAM(0), LPARAM(0));
        }
    }

    fn class_ready() -> bool {
        static READY: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *READY.get_or_init(|| unsafe {
            let Ok(inst) = GetModuleHandleW(None) else { return false };
            let wc = WNDCLASSW {
                lpfnWndProc: Some(ghost_proc),
                hInstance: inst.into(),
                lpszClassName: w!("DeskBoxesGhost"),
                ..Default::default()
            };
            RegisterClassW(&wc) != 0
        })
    }

    /// The box window's current picture (even if partly covered): width, height, BGRA rows.
    pub fn capture(hwnd: isize) -> Option<(usize, usize, Vec<u8>)> {
        unsafe {
            let h = HWND(hwnd as *mut _);
            let mut r = RECT::default();
            GetClientRect(h, &mut r).ok()?;
            let (w, ht) = ((r.right - r.left).max(1) as usize, (r.bottom - r.top).max(1) as usize);
            if w > 8000 || ht > 8000 {
                return None;
            }
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(screen);
            let mut info = bitmap_info(w, ht);
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(dc, &mut info, DIB_RGB_COLORS, &mut bits, None, 0).ok();
            let mut out = None;
            if let Some(bmp) = bmp {
                let old = SelectObject(dc, HGDIOBJ(bmp.0));
                // PW_CLIENTONLY | PW_RENDERFULLCONTENT: the drawing area, rendered even if covered.
                if PrintWindow(h, dc, PRINT_WINDOW_FLAGS(1 | 2)).as_bool() && !bits.is_null() {
                    out = Some((w, ht, std::slice::from_raw_parts(bits as *const u8, w * ht * 4).to_vec()));
                }
                SelectObject(dc, old);
                let _ = DeleteObject(HGDIOBJ(bmp.0));
            }
            let _ = DeleteDC(dc);
            ReleaseDC(None, screen);
            out
        }
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
    pub fn create(w: usize, h: usize, bgra: &[u8], x: i32, y: i32) -> Option<isize> {
        if !class_ready() || bgra.len() != w * h * 4 {
            return None;
        }
        unsafe {
            let inst = GetModuleHandleW(None).ok()?;
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
                w!("DeskBoxesGhost"),
                w!("DeskBoxes moving box"),
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
            if !paint(hwnd, w, h, bgra, Some((x, y))) {
                let _ = DestroyWindow(hwnd);
                return None;
            }
            let _ = SetWindowPos(hwnd, HWND_BOTTOM, 0, 0, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            Some(hwnd.0 as isize)
        }
    }

    /// Replaces the picture of an existing see-through window (keeps its position).
    pub fn update(hwnd: isize, w: usize, h: usize, bgra: &[u8]) {
        if bgra.len() != w * h * 4 {
            return;
        }
        // Hand it to the copy's own helper thread when there is one.
        if let Ok(u) = UPDATES.lock() {
            if let Some((_, s)) = u.iter().find(|(h2, _)| *h2 == hwnd) {
                let _ = s.send((w, h, bgra.to_vec()));
                return;
            }
        }
        unsafe {
            paint(HWND(hwnd as *mut _), w, h, bgra, None);
        }
    }

    unsafe fn paint(hwnd: HWND, w: usize, h: usize, bgra: &[u8], at: Option<(i32, i32)>) -> bool {
        let screen = GetDC(None);
        let dc = CreateCompatibleDC(screen);
        let mut info = bitmap_info(w, h);
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut ok = false;
        if let Ok(bmp) = CreateDIBSection(dc, &mut info, DIB_RGB_COLORS, &mut bits, None, 0) {
            if !bits.is_null() {
                std::ptr::copy_nonoverlapping(bgra.as_ptr(), bits as *mut u8, bgra.len());
                let old = SelectObject(dc, HGDIOBJ(bmp.0));
                let blend = BLENDFUNCTION {
                    BlendOp: AC_SRC_OVER as u8,
                    BlendFlags: 0,
                    SourceConstantAlpha: 255,
                    AlphaFormat: AC_SRC_ALPHA as u8,
                };
                let pos = at.map(|(x, y)| POINT { x, y });
                let size = SIZE { cx: w as i32, cy: h as i32 };
                let src = POINT { x: 0, y: 0 };
                ok = UpdateLayeredWindow(
                    hwnd,
                    screen,
                    pos.as_ref().map(|p| p as *const POINT),
                    Some(&size),
                    dc,
                    Some(&src),
                    COLORREF(0),
                    Some(&blend),
                    ULW_ALPHA,
                )
                .is_ok();
                SelectObject(dc, old);
            }
            let _ = DeleteObject(HGDIOBJ(bmp.0));
        }
        let _ = DeleteDC(dc);
        ReleaseDC(None, screen);
        ok
    }
    /// Asks Windows to blur whatever is behind the copy (frosted glass), like the original's
    /// blur effect. Uses the window-composition setting Windows offers for this.
    #[allow(dead_code)]
    pub fn enable_blur_behind(hwnd: isize) {
        #[repr(C)]
        struct AccentPolicy {
            state: u32,
            flags: u32,
            color: u32,
            animation: u32,
        }
        #[repr(C)]
        struct CompositionData {
            attribute: u32,
            data: *mut core::ffi::c_void,
            size: usize,
        }
        unsafe {
            use windows::core::s;
            use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
            let Ok(user32) = GetModuleHandleW(w!("user32.dll")) else { return };
            let Some(f) = GetProcAddress(user32, s!("SetWindowCompositionAttribute")) else { return };
            let set: unsafe extern "system" fn(HWND, *mut CompositionData) -> i32 = std::mem::transmute(f);
            let mut accent = AccentPolicy { state: 3 /* blur behind */, flags: 0, color: 0, animation: 0 };
            let mut data = CompositionData {
                attribute: 19, /* accent policy */
                data: &mut accent as *mut _ as *mut _,
                size: std::mem::size_of::<AccentPolicy>(),
            };
            set(HWND(hwnd as *mut _), &mut data);
        }
    }

    /// A picture of the dragged icon that flies with the mouse, above everything, until the
    /// app removes it when the drag ends.
    pub fn begin_icon_drag(w: usize, h: usize, bgra: Vec<u8>, grab: (i32, i32)) -> Option<isize> {
        let (tx, rx) = std::sync::mpsc::channel::<Option<isize>>();
        std::thread::Builder::new()
            .name("icon-drag".into())
            .spawn(move || unsafe {
                use windows::Win32::UI::WindowsAndMessaging::{
                    DestroyWindow, DispatchMessageW, GetCursorPos, IsWindow, PeekMessageW, TranslateMessage, HWND_TOPMOST,
                    MSG, PM_REMOVE, SWP_NOMOVE, SWP_NOZORDER,
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

    pub fn move_to(hwnd: isize, x: i32, y: i32) {
        unsafe {
            let _ = SetWindowPos(HWND(hwnd as *mut _), HWND_BOTTOM, x, y, 0, 0, SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    pub fn destroy(hwnd: isize) {
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
            // The copy lives on its own thread: ask it to close (DefWindowProc destroys it).
            let _ = PostMessageW(HWND(hwnd as *mut _), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }

    /// Makes a box window invisible (Some(0)) while it keeps receiving the mouse, or normal (None).
    pub fn set_alpha(hwnd: isize, alpha: Option<u8>) {
        unsafe {
            let h = HWND(hwnd as *mut _);
            let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
            match alpha {
                Some(a) => {
                    SetWindowLongPtrW(h, GWL_EXSTYLE, ex | WS_EX_LAYERED.0 as isize);
                    let _ = SetLayeredWindowAttributes(h, COLORREF(0), a, LWA_ALPHA);
                }
                None => {
                    // Fully visible again, but the window stays in see-through mode: switching
                    // modes back would show an empty frame for a moment (a flash).
                    if ex & WS_EX_LAYERED.0 as isize != 0 {
                        let _ = SetLayeredWindowAttributes(h, COLORREF(0), 255, LWA_ALPHA);
                    }
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn capture(_: isize) -> Option<(usize, usize, Vec<u8>)> {
        None
    }
    pub fn create(_: usize, _: usize, _: &[u8], _: i32, _: i32) -> Option<isize> {
        None
    }
    pub fn update(_: isize, _: usize, _: usize, _: &[u8]) {}
    pub fn begin_drag(_: usize, _: usize, _: Vec<u8>, _: i32, _: i32, _: (i32, i32), _: Option<(std::sync::Arc<eframe::egui::ColorImage>, (i32, i32), u32)>) -> Option<isize> {
        None
    }
    pub fn take_done() -> Option<(isize, i32, i32)> {
        None
    }
    pub fn post_button_up(_: isize) {}
    pub fn move_to(_: isize, _: i32, _: i32) {}
    pub fn begin_icon_drag(_: usize, _: usize, _: Vec<u8>, _: (i32, i32)) -> Option<isize> {
        None
    }
    pub fn destroy(_: isize) {}
    pub fn set_alpha(_: isize, _: Option<u8>) {}
}

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

/// Wakes the app (asks for a redraw) when Windows finishes moving the copy.
pub static WAKE: std::sync::OnceLock<Box<dyn Fn() + Send + Sync>> = std::sync::OnceLock::new();

pub use imp::{begin_drag, begin_icon_drag, capture, destroy, set_alpha, take_done, update};

/// Turns a snapshot of a box (drawn as wallpaper + tint + content) into premultiplied BGRA with
/// real transparency, for the first moments of a drag (until the exact copy is ready). Each
/// pixel is compared with "tint over wallpaper" and with plain wallpaper, using the closest
/// match among the 3x3 wallpaper pixels around it (so fine wallpaper detail never leaves
/// specks); only pixels clearly unlike both (icons, text) stay solid. Expected colours are
/// computed once per pixel and compared with whole numbers, so this takes a few milliseconds.
pub fn to_see_through(
    w: usize,
    h: usize,
    bgra: &[u8],
    tint_pm: [u8; 3],
    alpha: u8,
    wall: impl Fn(usize, usize) -> [u8; 3],
) -> Vec<u8> {
    // The tint exactly as the drawing engine stores it (premultiplied the engine's own way),
    // so the glass of the copy equals the resting box from the first instant.
    let a = alpha as i32;
    let pm: [i32; 3] = [tint_pm[0] as i32, tint_pm[1] as i32, tint_pm[2] as i32];
    // Wallpaper colour behind every pixel, looked up once.
    let mut wl = vec![[0i16; 3]; w * h];
    for y in 0..h {
        for x in 0..w {
            let v = wall(x, y);
            wl[y * w + x] = [v[0] as i16, v[1] as i16, v[2] as i16];
        }
    }
    // "Tint over wallpaper" colour behind every pixel (how the engine blends: tint + rest x wallpaper).
    let mut ex = vec![[0i16; 3]; w * h];
    for i in 0..w * h {
        for k in 0..3 {
            ex[i][k] = (pm[k] + (wl[i][k] as i32 * (255 - a) + 127) / 255) as i16;
        }
    }
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        let y0 = y.saturating_sub(1);
        let y1 = (y + 2).min(h);
        for x in 0..w {
            let i = (y * w + x) * 4;
            let c = [bgra[i + 2] as i16, bgra[i + 1] as i16, bgra[i] as i16];
            let (mut d_tint, mut d_wall) = (i32::MAX, i32::MAX);
            let x0 = x.saturating_sub(1);
            let x1 = (x + 2).min(w);
            for ny in y0..y1 {
                let row = ny * w;
                for nx in x0..x1 {
                    let (e, v) = (ex[row + nx], wl[row + nx]);
                    let dt = (c[0] - e[0]).abs() as i32 + (c[1] - e[1]).abs() as i32 + (c[2] - e[2]).abs() as i32;
                    let dw = (c[0] - v[0]).abs() as i32 + (c[1] - v[1]).abs() as i32 + (c[2] - v[2]).abs() as i32;
                    if dt < d_tint {
                        d_tint = dt;
                    }
                    if dw < d_wall {
                        d_wall = dw;
                    }
                }
            }
            let (r, g, b, al) = if d_wall < d_tint && d_wall <= 36 {
                (0u8, 0u8, 0u8, 0u8)
            } else if d_tint <= 60 {
                (pm[0] as u8, pm[1] as u8, pm[2] as u8, alpha)
            } else {
                (c[0] as u8, c[1] as u8, c[2] as u8, 255)
            };
            out[i] = b;
            out[i + 1] = g;
            out[i + 2] = r;
            out[i + 3] = al;
        }
    }
    out
}
/// Exact see-through pixels from two pictures of the same box, one drawn on black and one on
/// white: alpha = 255 - (white - black), colour = the black picture (already premultiplied).
pub fn from_black_white(w: usize, h: usize, black: &[u8], white: &[u8]) -> Option<Vec<u8>> {
    if black.len() != w * h * 4 || white.len() != black.len() {
        return None;
    }
    let mut out = vec![0u8; black.len()];
    for i in (0..black.len()).step_by(4) {
        let d = (0..3).map(|k| white[i + k] as i32 - black[i + k] as i32).sum::<i32>() / 3;
        let a = (255 - d).clamp(0, 255) as u8;
        out[i] = black[i].min(a);
        out[i + 1] = black[i + 1].min(a);
        out[i + 2] = black[i + 2].min(a);
        out[i + 3] = a;
    }
    Some(out)
}
/// Puts the box's see-through content (premultiplied BGRA) over its frosted glass: the result
/// is solid inside the box and clear where the content is clear (outside the rounded corners).
pub fn compose_over(base: &[u8], glass: &eframe::egui::ColorImage) -> Vec<u8> {
    let mut out = vec![0u8; base.len()];
    for (i, g) in glass.pixels.iter().enumerate() {
        let p = i * 4;
        if p + 3 >= base.len() {
            break;
        }
        let a = base[p + 3] as u32;
        if a == 0 {
            continue; // outside the box shape: stays clear
        }
        let keep = 255 - a;
        out[p] = (base[p] as u32 + (g.b() as u32 * keep + 127) / 255).min(255) as u8;
        out[p + 1] = (base[p + 1] as u32 + (g.g() as u32 * keep + 127) / 255).min(255) as u8;
        out[p + 2] = (base[p + 2] as u32 + (g.r() as u32 * keep + 127) / 255).min(255) as u8;
        out[p + 3] = 255;
    }
    out
}