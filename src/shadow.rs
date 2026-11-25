//! Optional drop shadow for a box: a separate click-through layered window directly below the
//! box (class `DesktopShepherdShadow`). It only exists while `Fence.shadow` is on; with the
//! option off there is no window and no bitmap. The picture is drawn once (when the box size,
//! corner radius or display scale changes) and handed to Windows with `UpdateLayeredWindow`,
//! then the program's own copy is freed. Moving a box only moves the window.
//! The area under the box itself is fully transparent, so the glass is never darkened.

#[cfg(windows)]
mod imp {
    use std::sync::Mutex;
    use windows::core::w;
    use windows::Win32::Foundation::{COLORREF, HWND, POINT, RECT, SIZE};
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC, SelectObject, AC_SRC_ALPHA, AC_SRC_OVER, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, BLENDFUNCTION, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, GetWindowRect, RegisterClassExW, SetWindowPos, UpdateLayeredWindow,
        SWP_ASYNCWINDOWPOS, SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_SHOWWINDOW, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER, ULW_ALPHA, WNDCLASSEXW,
        WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
    };

    struct Entry {
        hwnd: isize,
        margin: i32,
        /// (box width, box height, corner radius bits, scale bits) of the picture on screen.
        drawn: Option<Key>,
        /// Box position the window was last placed for.
        at: (i32, i32),
        /// Shown or hidden as last told to Windows.
        shown: bool,
        /// The box is not shown (another page, all boxes hidden).
        app_hidden: bool,
        /// The box is faded out: no shadow either.
        faded: bool,
        /// The box size is still changing: shadow hidden until it has been stable for a moment.
        waiting: bool,
        /// (size key, when it was first seen) of the size being waited for.
        seen: Option<(Key, std::time::Instant)>,
        /// Size key whose picture could not be drawn (not retried until the size changes).
        failed: Option<Key>,
        /// Strength the picture is shown with (SourceConstantAlpha): 255 normally; with the box's
        /// hover lift on, a bit less at rest and 255 while the mouse is over the box.
        alpha: u8,
    }

    /// (box width, box height, corner radius bits, scale bits).
    type Key = (i32, i32, u32, u32);

    static SHADOWS: Mutex<Vec<(isize, Entry)>> = Mutex::new(Vec::new());
    /// Boxes that are faded out. Kept apart from SHADOWS: the fade can be set before the shadow exists.
    static FADED: Mutex<Vec<isize>> = Mutex::new(Vec::new());
    /// How long the box size must stay the same before the picture is redrawn.
    const SETTLE: std::time::Duration = std::time::Duration::from_millis(150);

    fn is_faded(b: isize) -> bool {
        FADED.lock().map(|f| f.contains(&b)).unwrap_or(false)
    }
    static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static CLASS: std::sync::Once = std::sync::Once::new();

    fn log_once(msg: &str) {
        if !LOGGED.swap(true, std::sync::atomic::Ordering::SeqCst) {
            crate::storage::log_line(&format!("shadow: {msg}"));
        }
    }

    unsafe extern "system" fn wndproc(h: HWND, m: u32, wp: windows::Win32::Foundation::WPARAM, lp: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
        DefWindowProcW(h, m, wp, lp)
    }

    fn margin_for(scale: f32) -> i32 {
        (24.0 * scale).round().max(1.0) as i32
    }

    fn create_window() -> Option<isize> {
        unsafe {
            let inst = GetModuleHandleW(None).ok()?;
            CLASS.call_once(|| {
                let c = WNDCLASSEXW {
                    cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                    lpfnWndProc: Some(wndproc),
                    hInstance: inst.into(),
                    lpszClassName: w!("DesktopShepherdShadow"),
                    ..Default::default()
                };
                RegisterClassExW(&c);
            });
            let hw = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("DesktopShepherdShadow"),
                w!("DesktopShepherd shadow"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                inst,
                None,
            )
            .ok()?;
            Some(hw.0 as isize)
        }
    }

    /// Soft box blur (3 passes, both directions) of a width x height alpha picture.
    fn blur(buf: &mut Vec<f32>, w: usize, h: usize, radius: usize) {
        if radius == 0 || w == 0 || h == 0 {
            return;
        }
        let mut tmp = vec![0.0f32; buf.len()];
        let norm = 1.0 / (2 * radius + 1) as f32;
        for _ in 0..3 {
            // horizontal: buf -> tmp
            for y in 0..h {
                let row = &buf[y * w..(y + 1) * w];
                let out = &mut tmp[y * w..(y + 1) * w];
                let mut sum: f32 = row.iter().take(radius.min(w - 1) + 1).sum();
                for x in 0..w {
                    out[x] = sum * norm;
                    let add = x + radius + 1;
                    if add < w {
                        sum += row[add];
                    }
                    if x >= radius {
                        sum -= row[x - radius];
                    }
                }
            }
            // vertical: tmp -> buf
            for v in buf.iter_mut() {
                *v = 0.0;
            }
            let mut col_sum = vec![0.0f32; w];
            for y in 0..radius.min(h - 1) + 1 {
                for x in 0..w {
                    col_sum[x] += tmp[y * w + x];
                }
            }
            for y in 0..h {
                for x in 0..w {
                    buf[y * w + x] = col_sum[x] * norm;
                }
                let add = y + radius + 1;
                if add < h {
                    for x in 0..w {
                        col_sum[x] += tmp[add * w + x];
                    }
                }
                if y >= radius {
                    for x in 0..w {
                        col_sum[x] -= tmp[(y - radius) * w + x];
                    }
                }
            }
        }
    }

    /// Coverage (0..1) of a rounded rectangle at a pixel centre.
    fn rr_cover(px: f32, py: f32, cx: f32, cy: f32, hx: f32, hy: f32, r: f32) -> f32 {
        let qx = (px - cx).abs() - (hx - r);
        let qy = (py - cy).abs() - (hy - r);
        // hypot only matters in the corner zone; elsewhere the distance is a simple difference.
        let outside = if qx > 0.0 && qy > 0.0 { qx.hypot(qy) } else { qx.max(0.0).max(qy.max(0.0)) };
        let d = outside + qx.max(qy).min(0.0) - r;
        (0.5 - d).clamp(0.0, 1.0)
    }

    /// One shadow layer: the box shape moved down by `dy`, blurred, scaled to `peak`.
    #[allow(clippy::too_many_arguments)]
    fn layer(bw: i32, bh: i32, m: i32, r: f32, dy: f32, blur_r: usize, peak: f32) -> Vec<f32> {
        let (w, h) = ((bw + 2 * m) as usize, (bh + 2 * m) as usize);
        let mut a = vec![0.0f32; w * h];
        let (cx, cy) = (m as f32 + bw as f32 / 2.0, m as f32 + bh as f32 / 2.0 + dy);
        for y in 0..h {
            for x in 0..w {
                a[y * w + x] = rr_cover(x as f32 + 0.5, y as f32 + 0.5, cx, cy, bw as f32 / 2.0, bh as f32 / 2.0, r);
            }
        }
        blur(&mut a, w, h, blur_r);
        for v in a.iter_mut() {
            *v *= peak;
        }
        a
    }

    /// Draws the shadow picture and gives it to the layered window. The bitmap and DCs are freed
    /// right away (Windows keeps its own copy).
    fn render(hwnd: isize, bw: i32, bh: i32, m: i32, corner_px: f32, scale: f32, at: (i32, i32), alpha: u8) -> bool {
        let (w, h) = (bw + 2 * m, bh + 2 * m);
        if w <= 0 || h <= 0 {
            return false;
        }
        let r = corner_px.min(bw.min(bh) as f32 / 2.0).max(0.0);
        // Contact shadow (tight, strong) + ambient shadow (wide, soft).
        let a1 = layer(bw, bh, m, r, 2.0 * scale, (3.0 * scale).round().max(1.0) as usize, 0.28);
        let a2 = layer(bw, bh, m, r, 8.0 * scale, (14.0 * scale).round().max(1.0) as usize, 0.22);
        unsafe {
            let screen = GetDC(None);
            let mem = CreateCompatibleDC(screen);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let bmp = CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, None, 0);
            let mut ok = false;
            if let (Ok(bmp), false) = (bmp, bits.is_null()) {
                let px = std::slice::from_raw_parts_mut(bits as *mut u32, (w * h) as usize);
                let (cx, cy) = (m as f32 + bw as f32 / 2.0, m as f32 + bh as f32 / 2.0);
                for y in 0..h as usize {
                    for x in 0..w as usize {
                        let i = y * w as usize + x;
                        // Fully transparent under the box itself.
                        let inside = rr_cover(x as f32 + 0.5, y as f32 + 0.5, cx, cy, bw as f32 / 2.0, bh as f32 / 2.0, r);
                        if inside >= 1.0 {
                            px[i] = 0;
                            continue;
                        }
                        let a = 1.0 - (1.0 - a1[i]) * (1.0 - a2[i]);
                        let a = (a * (1.0 - inside)).clamp(0.0, 1.0);
                        // Black: premultiplied colour is zero.
                        px[i] = ((a * 255.0 + 0.5) as u32) << 24;
                    }
                }
                let old = SelectObject(mem, bmp);
                let dst = POINT { x: at.0 - m, y: at.1 - m };
                let size = SIZE { cx: w, cy: h };
                let src = POINT { x: 0, y: 0 };
                let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: alpha, AlphaFormat: AC_SRC_ALPHA as u8 };
                ok = UpdateLayeredWindow(HWND(hwnd as *mut _), screen, Some(&dst), Some(&size), mem, Some(&src), COLORREF(0), Some(&blend), ULW_ALPHA).is_ok();
                SelectObject(mem, old);
                let _ = DeleteObject(HGDIOBJ(bmp.0));
            }
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            ok
        }
    }

    fn find(v: &mut Vec<(isize, Entry)>, b: isize) -> Option<&mut Entry> {
        v.iter_mut().find(|(k, _)| *k == b).map(|(_, e)| e)
    }

    fn apply_visibility(b: isize, e: &mut Entry) {
        let want = !e.app_hidden && !e.faded && !e.waiting;
        unsafe {
            let h = HWND(e.hwnd as *mut _);
            if want {
                // Directly below its box. The shadow is our own window: showing it and placing it is one
                // synchronous call (no second async step that could land in a different order).
                let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER;
                if e.shown {
                    let _ = SetWindowPos(h, HWND(b as *mut _), 0, 0, 0, 0, flags | SWP_ASYNCWINDOWPOS);
                } else {
                    let _ = SetWindowPos(h, HWND(b as *mut _), 0, 0, 0, 0, flags | SWP_SHOWWINDOW);
                }
            } else if e.shown {
                let _ = SetWindowPos(h, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_HIDEWINDOW);
            }
        }
        e.shown = want;
    }

    /// Brings the shadow of `b` in line with the box: creates / redraws / moves / removes it.
    /// UI thread only. `corner_pts` is the box's corner radius in points.
    pub fn sync(b: isize, on: bool, corner_pts: f32) {
        if !on {
            let Ok(mut all) = SHADOWS.lock() else { return };
            if let Some(i) = all.iter().position(|(k, _)| *k == b) {
                let (_, e) = all.remove(i);
                unsafe {
                    let _ = DestroyWindow(HWND(e.hwnd as *mut _));
                }
            }
            return;
        }
        unsafe {
            let bh = HWND(b as *mut _);
            let mut wr = RECT::default();
            if GetWindowRect(bh, &mut wr).is_err() {
                return;
            }
            let scale = (GetDpiForWindow(bh).max(96) as f32) / 96.0;
            let (bw, bht) = (wr.right - wr.left, wr.bottom - wr.top);
            let m = margin_for(scale);
            let corner_px = corner_pts * scale;
            let key: Key = (bw, bht, corner_px.to_bits(), scale.to_bits());
            // Step 1 (locked, quick): look the entry up, move it, or decide that a redraw is due.
            let hwnd = {
                let Ok(mut all) = SHADOWS.lock() else { return };
                if all.iter().all(|(k, _)| *k != b) {
                    let Some(hw) = create_window() else {
                        log_once("could not create the shadow window");
                        return;
                    };
                    all.push((
                        b,
                        Entry {
                            hwnd: hw,
                            margin: m,
                            drawn: None,
                            at: (i32::MIN, i32::MIN),
                            shown: false,
                            app_hidden: false,
                            faded: is_faded(b),
                            waiting: false,
                            seen: None,
                            failed: None,
                            alpha: 255,
                        },
                    ));
                }
                let Some(e) = find(&mut all, b) else { return };
                if e.drawn != Some(key) || e.margin != m {
                    if e.failed == Some(key) {
                        return; // already failed for this size: wait for another size
                    }
                    let now = std::time::Instant::now();
                    // The first picture is drawn at once; later ones wait until the size is stable.
                    let due = e.drawn.is_none()
                        || match e.seen {
                            Some((k, t)) if k == key => now.duration_since(t) >= SETTLE,
                            _ => {
                                e.seen = Some((key, now));
                                false
                            }
                        };
                    if !due {
                        if !e.waiting {
                            e.waiting = true;
                            apply_visibility(b, e);
                        }
                        crate::runloop::wake_after(SETTLE + std::time::Duration::from_millis(10));
                        return;
                    }
                    (e.hwnd, e.alpha)
                } else {
                    if e.at != (wr.left, wr.top) {
                        let _ = SetWindowPos(HWND(e.hwnd as *mut _), None, wr.left - m, wr.top - m, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
                        e.at = (wr.left, wr.top);
                    }
                    e.faded = is_faded(b);
                    let want = !e.app_hidden && !e.faded;
                    e.waiting = false;
                    if !e.shown && want || e.shown && !want {
                        apply_visibility(b, e);
                    }
                    return;
                }
            };
            // Step 2 (not locked): draw the picture and hand it to Windows.
            let (hwnd, alpha) = hwnd;
            let ok = render(hwnd, bw, bht, m, corner_px, scale, (wr.left, wr.top), alpha);
            // Step 3 (locked): note the result.
            let Ok(mut all) = SHADOWS.lock() else { return };
            let Some(e) = find(&mut all, b) else { return };
            e.margin = m;
            e.seen = None;
            if ok {
                e.drawn = Some(key);
                e.failed = None;
                e.at = (wr.left, wr.top);
                e.waiting = false;
                e.faded = is_faded(b);
                let want = !e.app_hidden && !e.faded;
                if !e.shown && want {
                    apply_visibility(b, e);
                }
            } else {
                log_once("could not draw the shadow");
                e.failed = Some(key);
            }
        }
    }

    /// Hover lift: with `on`, the shadow rests a bit lighter and becomes full strength while `hot`
    /// (the mouse is over the box). Only the blend strength changes: Windows keeps the picture, so
    /// nothing is redrawn. Cheap no-op when nothing changed.
    pub fn set_lift(b: isize, on: bool, hot: bool) {
        let want: u8 = if on && !hot { 170 } else { 255 };
        let Ok(mut all) = SHADOWS.lock() else { return };
        let Some(e) = find(&mut all, b) else { return };
        if e.alpha == want {
            return;
        }
        e.alpha = want;
        if e.drawn.is_none() {
            return; // the first picture will use the new strength
        }
        unsafe {
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: want, AlphaFormat: AC_SRC_ALPHA as u8 };
            // No new picture, position or size: only the blend changes.
            let _ = UpdateLayeredWindow(HWND(e.hwnd as *mut _), None, None, None, None, None, COLORREF(0), Some(&blend), ULW_ALPHA);
        }
    }

    /// Called from the box-move helper thread: the box is now at (x, y); never blocks.
    pub fn follow(b: isize, x: i32, y: i32) {
        let Ok(mut all) = SHADOWS.lock() else { return };
        if let Some(e) = find(&mut all, b) {
            unsafe {
                let _ = SetWindowPos(
                    HWND(e.hwnd as *mut _),
                    None,
                    x - e.margin,
                    y - e.margin,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS,
                );
            }
            e.at = (x, y);
        }
    }

    /// The box was put back in the stack: its shadow goes directly below it.
    pub fn restack(b: isize) {
        let Ok(mut all) = SHADOWS.lock() else { return };
        if let Some(e) = find(&mut all, b) {
            if e.shown {
                unsafe {
                    let _ = SetWindowPos(HWND(e.hwnd as *mut _), HWND(b as *mut _), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_ASYNCWINDOWPOS);
                }
            }
        }
    }

    /// The box is shown or hidden by the app (page, hide all).
    pub fn set_visible(b: isize, show: bool) {
        let Ok(mut all) = SHADOWS.lock() else { return };
        if let Some(e) = find(&mut all, b) {
            e.app_hidden = !show;
            apply_visibility(b, e);
        }
    }

    /// The box is faded out below half: hide its shadow.
    pub fn set_faded(b: isize, faded: bool) {
        // Always remembered, even while the box has no shadow (yet): sync reads it when it creates one.
        if let Ok(mut f) = FADED.lock() {
            f.retain(|k| *k != b);
            if faded {
                f.push(b);
            }
        }
        let Ok(mut all) = SHADOWS.lock() else { return };
        if let Some(e) = find(&mut all, b) {
            if e.faded != faded {
                e.faded = faded;
                apply_visibility(b, e);
            }
        }
    }

    pub fn destroy(b: isize) {
        if let Ok(mut f) = FADED.lock() {
            f.retain(|k| *k != b);
        }
        sync(b, false, 0.0);
    }

    pub fn window_of(b: isize) -> Option<isize> {
        let mut all = SHADOWS.lock().ok()?;
        find(&mut all, b).map(|e| e.hwnd)
    }
}

#[cfg(windows)]
pub use imp::{destroy, follow, restack, set_faded, set_lift, set_visible, sync, window_of};

#[cfg(not(windows))]
mod stub {
    pub fn sync(_b: isize, _on: bool, _corner_pts: f32) {}
    pub fn set_lift(_b: isize, _on: bool, _hot: bool) {}
    pub fn follow(_b: isize, _x: i32, _y: i32) {}
    pub fn restack(_b: isize) {}
    pub fn set_visible(_b: isize, _show: bool) {}
    pub fn set_faded(_b: isize, _faded: bool) {}
    pub fn destroy(_b: isize) {}
    pub fn window_of(_b: isize) -> Option<isize> {
        None
    }
}
#[cfg(not(windows))]
pub use stub::*;
