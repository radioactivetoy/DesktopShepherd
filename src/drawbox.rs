//! "Draw a new box": a dimmed, see-through layer over every monitor; the person drags a
//! rectangle where the new box (or widget) should go. Esc, a right-click or switching away
//! cancels. It lives only while drawing: a plain window of ours (nothing in Explorer, no
//! hooks), drawn with GDI, on the app's thread.

#![cfg(windows)]

use std::sync::Mutex;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, CreateFontW, CreateSolidBrush, DeleteObject, DrawTextW, EndPaint, FillRect, FrameRect, InvalidateRect, SelectObject, SetBkMode,
    SetTextColor, DT_CENTER, DT_SINGLELINE, DT_VCENTER, HBRUSH, HFONT, PAINTSTRUCT, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows::Win32::UI::WindowsAndMessaging::*;

/// The overlay while it is open, the drag (start, now) in screen pixels, and the result.
static WINDOW: Mutex<isize> = Mutex::new(0);
static DRAG: Mutex<Option<(POINT, POINT)>> = Mutex::new(None);
/// Some(Some(rect)) = drawn (left, top, right, bottom in pixels), Some(None) = cancelled.
static RESULT: Mutex<Option<Option<[i32; 4]>>> = Mutex::new(None);

/// The finished drawing, once: Some(Some(rect px)) or Some(None) if it was cancelled.
pub fn take_result() -> Option<Option<[i32; 4]>> {
    RESULT.lock().ok().and_then(|mut r| r.take())
}

pub fn is_open() -> bool {
    WINDOW.lock().map(|w| *w != 0).unwrap_or(false)
}

fn finish(hwnd: HWND, rect: Option<[i32; 4]>) {
    if let Ok(mut r) = RESULT.lock() {
        *r = Some(rect);
    }
    if let Ok(mut w) = WINDOW.lock() {
        *w = 0;
    }
    if let Ok(mut d) = DRAG.lock() {
        *d = None;
    }
    unsafe {
        let _ = ReleaseCapture();
        let _ = DestroyWindow(hwnd);
    }
    crate::runloop::wake_after(std::time::Duration::ZERO);
}

fn screen_point(hwnd: HWND, lp: LPARAM) -> POINT {
    let mut p = POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
    unsafe {
        let _ = windows::Win32::Graphics::Gdi::ClientToScreen(hwnd, &mut p);
    }
    p
}

fn sel_rect(a: POINT, b: POINT) -> RECT {
    RECT { left: a.x.min(b.x), top: a.y.min(b.y), right: a.x.max(b.x), bottom: a.y.max(b.y) }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_SETCURSOR => {
            if let Ok(c) = LoadCursorW(None, IDC_CROSS) {
                SetCursor(c);
            }
            LRESULT(1)
        }
        WM_LBUTTONDOWN => {
            let p = screen_point(hwnd, lp);
            if let Ok(mut d) = DRAG.lock() {
                *d = Some((p, p));
            }
            SetCapture(hwnd);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            let p = screen_point(hwnd, lp);
            if let Ok(mut d) = DRAG.lock() {
                if let Some((_, now)) = d.as_mut() {
                    *now = p;
                    let _ = InvalidateRect(hwnd, None, false);
                }
            }
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let p = screen_point(hwnd, lp);
            let drag = DRAG.lock().ok().and_then(|d| *d);
            if let Some((a, _)) = drag {
                let r = sel_rect(a, p);
                finish(hwnd, Some([r.left, r.top, r.right, r.bottom]));
            }
            LRESULT(0)
        }
        WM_RBUTTONDOWN => {
            finish(hwnd, None);
            LRESULT(0)
        }
        WM_KEYDOWN if wp.0 == VK_ESCAPE.0 as usize => {
            finish(hwnd, None);
            LRESULT(0)
        }
        WM_ACTIVATE if (wp.0 & 0xFFFF) == 0 => {
            // Another window came forward (Alt+Tab): stop drawing.
            if is_open() {
                finish(hwnd, None);
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let dark = CreateSolidBrush(COLORREF(0x0018_1410));
            FillRect(dc, &client, dark);
            let _ = DeleteObject(dark);
            // The hint, centred near the top of the main monitor.
            let main = RECT { left: -client_origin().0, top: -client_origin().1, right: -client_origin().0 + GetSystemMetrics(SM_CXSCREEN), bottom: -client_origin().1 + 140 };
            let font: HFONT = CreateFontW(34, 0, 0, 0, 400, 0, 0, 0, 0, 0, 0, 5, 0, w!("Segoe UI"));
            let old = SelectObject(dc, font);
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(0x00FF_FFFF));
            let mut hint: Vec<u16> = crate::i18n::t("Drag to draw the new box  ·  Esc to cancel").encode_utf16().collect();
            let mut hr = main;
            DrawTextW(dc, &mut hint, &mut hr, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
            SelectObject(dc, old);
            let _ = DeleteObject(font);
            if let Some((a, b)) = DRAG.lock().ok().and_then(|d| *d) {
                let (ox, oy) = client_origin();
                let mut r = sel_rect(a, b);
                r.left -= ox;
                r.right -= ox;
                r.top -= oy;
                r.bottom -= oy;
                let fill = CreateSolidBrush(COLORREF(0x00E6_8C46)); // light blue (BGR)
                FillRect(dc, &r, fill);
                let _ = DeleteObject(fill);
                let edge = CreateSolidBrush(COLORREF(0x00FF_FFFF));
                FrameRect(dc, &r, edge);
                let _ = DeleteObject(edge);
            }
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Top-left of the overlay = top-left of the whole desktop (all monitors), in pixels.
fn client_origin() -> (i32, i32) {
    unsafe { (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN)) }
}

/// Opens the overlay (does nothing if it is already open). Call from the app's thread.
pub fn start() {
    if is_open() {
        return;
    }
    unsafe {
        let Ok(inst) = GetModuleHandleW(None) else { return };
        static REGISTERED: std::sync::Once = std::sync::Once::new();
        REGISTERED.call_once(|| {
            let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: inst.into(), lpszClassName: w!("DesktopShepherdDraw"), hbrBackground: HBRUSH::default(), ..Default::default() };
            RegisterClassW(&wc);
        });
        let (x, y) = client_origin();
        let (cw, ch) = (GetSystemMetrics(SM_CXVIRTUALSCREEN), GetSystemMetrics(SM_CYVIRTUALSCREEN));
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
            w!("DesktopShepherdDraw"),
            w!("Draw a new box"),
            WS_POPUP,
            x,
            y,
            cw.max(1),
            ch.max(1),
            None,
            None,
            inst,
            None,
        ) else {
            return;
        };
        // See-through: the desktop shows dimly below.
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 150, LWA_ALPHA);
        if let Ok(mut w) = WINDOW.lock() {
            *w = hwnd.0 as isize;
        }
        if let Ok(mut r) = RESULT.lock() {
            *r = None;
        }
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = PCWSTR::null();
    }
}
