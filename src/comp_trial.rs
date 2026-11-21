//! Trial (run with `--composition-trial`): can Windows' composition system draw a box that is
//! truly see-through, frosted by Windows itself (on the graphics card), with rounded corners,
//! and move smoothly as a normal window? Opens one test box for a few seconds, then exits.
//! Nothing here touches the person's boxes or settings.

#[cfg(windows)]
pub fn run(seconds: u64) -> windows::core::Result<()> {
    use windows::core::{w, Interface};
    use windows::Foundation::Numerics::Vector2;
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_USE_HOSTBACKDROPBRUSH};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
    use windows::Win32::System::WinRT::{
        CreateDispatcherQueueController, DispatcherQueueOptions, RoInitialize, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT,
        RO_INIT_SINGLETHREADED,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, PeekMessageW, RegisterClassW, TranslateMessage, HTCAPTION, MSG,
        PM_REMOVE, WM_NCHITTEST, WNDCLASSW, WS_EX_NOREDIRECTIONBITMAP, WS_EX_TOOLWINDOW, WS_POPUP, WS_VISIBLE,
    };
    use windows::UI::Color;
    use windows::UI::Composition::Compositor;

    unsafe extern "system" fn proc(h: HWND, m: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
        if m == WM_NCHITTEST {
            return LRESULT(HTCAPTION as isize); // drag anywhere: Windows moves it natively
        }
        DefWindowProcW(h, m, wp, lp)
    }

    unsafe {
        let _ = RoInitialize(RO_INIT_SINGLETHREADED);
        let _controller = CreateDispatcherQueueController(DispatcherQueueOptions {
            dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
            threadType: DQTYPE_THREAD_CURRENT,
            apartmentType: DQTAT_COM_NONE,
        })?;
        let inst = GetModuleHandleW(None)?;
        let wc = WNDCLASSW { lpfnWndProc: Some(proc), hInstance: inst.into(), lpszClassName: w!("DeskBoxesCompTrial"), ..Default::default() };
        RegisterClassW(&wc);
        let (x, y, wd, ht) = (300, 200, 520, 340);
        let hwnd = CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_TOOLWINDOW,
            w!("DeskBoxesCompTrial"),
            w!("DeskBoxes composition trial"),
            WS_POPUP | WS_VISIBLE,
            x,
            y,
            wd,
            ht,
            None,
            None,
            inst,
            None,
        )?;
        // Ask for Windows' own frosted backdrop (what Windows 11 uses for acrylic).
        let on = BOOL(1);
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_HOSTBACKDROPBRUSH, &on as *const _ as *const _, 4);

        let compositor = Compositor::new()?;
        let interop: ICompositorDesktopInterop = compositor.cast()?;
        let target = interop.CreateDesktopWindowTarget(hwnd, false)?;
        let size = Vector2 { X: wd as f32, Y: ht as f32 };

        let container = compositor.CreateContainerVisual()?;
        container.SetSize(size)?;
        // Layer 1: the frosted desktop behind the window, done by Windows on the GPU.
        let backdrop = compositor.CreateSpriteVisual()?;
        backdrop.SetBrush(&compositor.CreateHostBackdropBrush()?)?;
        backdrop.SetSize(size)?;
        // Layer 2: the box's tint (semi-transparent blue).
        let tint = compositor.CreateSpriteVisual()?;
        tint.SetBrush(&compositor.CreateColorBrushWithColor(Color { A: 110, R: 16, G: 32, B: 64 })?)?;
        tint.SetSize(size)?;
        let kids = container.Children()?;
        kids.InsertAtTop(&backdrop)?;
        kids.InsertAtTop(&tint)?;
        // Rounded corners: everything is clipped to a rounded rectangle.
        let geo = compositor.CreateRoundedRectangleGeometry()?;
        geo.SetSize(size)?;
        geo.SetCornerRadius(Vector2 { X: 8.0, Y: 8.0 })?;
        container.SetClip(&compositor.CreateGeometricClipWithGeometry(&geo)?)?;
        target.SetRoot(&container)?;

        let end = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
        let mut msg = MSG::default();
        while std::time::Instant::now() < end {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let _ = hwnd;
        Ok(())
    }
}

#[cfg(not(windows))]
pub fn run(_seconds: u64) -> Result<(), ()> {
    Ok(())
}
