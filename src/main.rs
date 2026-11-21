#![windows_subsystem = "windows"]
//! DeskBoxes — a lightweight desktop icon organizer (boxes on the desktop) with widgets.

mod app;
mod comp_trial;
#[cfg(windows)]
mod d3d;
#[cfg(windows)]
mod host;
mod desktop;
mod fence_view;
mod ghost;
mod icons;
mod import_original;
mod model;
mod storage;
mod wallpaper;
mod widgets;

use eframe::egui;
use std::path::PathBuf;

/// Worker exit code: the window system could not start (the app never ran). Restart it later.
const EXIT_START_FAILED: i32 = 2;
/// Worker exit code: the window system could not start with a see-through main window.
/// Restart it at once without asking for see-through (`--opaque`).
const EXIT_RETRY_OPAQUE: i32 = 3;
/// Worker exit code: DirectX boxes are unavailable, and the classic boxes need a see-through
/// main window. Restart it at once with `--transparent`.
pub const EXIT_NEED_TRANSPARENT: i32 = 4;

/// How the main (invisible, 1x1) GL window is made. By default it is not see-through: that is
/// cheaper on every pass (measured: about 40 % less CPU while the mouse moves over a box) and
/// avoids the GL setups this PC's driver doesn't always offer. Only the classic boxes need it.
#[derive(Clone, Copy, PartialEq)]
enum RootMode {
    /// Not see-through; may ask for see-through if the classic boxes are needed.
    Default,
    /// See-through (`--transparent`), for the classic boxes.
    Transparent,
    /// Not see-through and never ask again (`--opaque`): see-through failed to start.
    OpaqueFinal,
}

/// The worker may exit with EXIT_NEED_TRANSPARENT to get a see-through main window.
pub static MAY_ASK_TRANSPARENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The safety net, like the original's launcher (0x140012300 in the original launcher, which checks
/// every 2000 ms and restarts the organizer): runs the real app as a child process and starts
/// it again if it ever stops without the person quitting. Only a clean quit (exit code 0) ends
/// both. Failures in a row wait longer and longer (2 s up to 5 min), so a broken state can never
/// loop fast, yet the app comes back by itself once the cause (e.g. a graphics driver) is gone.
fn supervise(boot: bool) {
    use std::time::{Duration, Instant};
    const WAITS: [u64; 7] = [2, 5, 15, 30, 60, 120, 300];
    let Ok(exe) = std::env::current_exe() else { return };
    let mut failures_in_a_row = 0usize;
    let mut first = true;
    let mut mode = RootMode::Default;
    loop {
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("--worker");
        if boot && first {
            cmd.arg("--boot");
        }
        match mode {
            RootMode::Default => {}
            RootMode::Transparent => {
                cmd.arg("--transparent");
            }
            RootMode::OpaqueFinal => {
                cmd.arg("--opaque");
            }
        }
        first = false;
        let started = Instant::now();
        let status = cmd.status();
        // However the app stopped (quit, crash, or force-closed), give the desktop icons back
        // right away; a restarted app hides them again itself.
        if storage::load().map(|l| l.settings.hide_desktop_icons).unwrap_or(false) {
            desktop::force_show_desktop_icons();
        }
        match status {
            Ok(s) if s.success() => return,
            Ok(s) if s.code() == Some(EXIT_NEED_TRANSPARENT) && mode == RootMode::Default => {
                storage::log_line("DirectX boxes unavailable; restarting with a see-through main window for the classic boxes");
                mode = RootMode::Transparent;
                continue;
            }
            Ok(s) if s.code() == Some(EXIT_RETRY_OPAQUE) && mode == RootMode::Transparent => {
                storage::log_line("app could not start with a see-through main window; retrying without");
                mode = RootMode::OpaqueFinal;
                continue;
            }
            Ok(s) => storage::log_line(&format!("app stopped unexpectedly ({s}); restarting")),
            Err(e) => storage::log_line(&format!("could not start the app: {e}")),
        }
        // A run of a minute or more counts as healthy: the next failure starts the waits over.
        if started.elapsed() >= Duration::from_secs(60) {
            failures_in_a_row = 0;
        }
        let wait = WAITS[failures_in_a_row.min(WAITS.len() - 1)];
        failures_in_a_row += 1;
        if failures_in_a_row >= 3 {
            storage::log_line(&format!("{failures_in_a_row} failures in a row; next try in {wait} s"));
        }
        std::thread::sleep(Duration::from_secs(wait));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let screenshot = args
        .iter()
        .position(|a| a == "--screenshot")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);

    // Hidden trial of Windows' composition system (see comp_trial.rs).
    if args.iter().any(|a| a == "--composition-trial") {
        if let Err(e) = comp_trial::run(6) {
            storage::log_line(&format!("composition trial failed: {e:?}"));
        }
        return;
    }
    // Diagnostic: write the original organizer's decrypted layout to a file.
    if let Some(out) = args.iter().position(|a| a == "--dump-original").and_then(|i| args.get(i + 1)) {
        if let Some(v) = import_original::read_cache("desk.cache") {
            let _ = std::fs::write(out, serde_json::to_string_pretty(&v).unwrap_or_default());
        }
        return;
    }
    // Replace the saved boxes with the original organizer's (run while the app is closed);
    // the current layout is backed up first.
    if args.iter().any(|a| a == "--import-original") {
        let current = storage::load().unwrap_or_default();
        match import_original::boxes_from_original(&current) {
            Some(l) => {
                storage::backup_now(&current);
                if let Err(e) = storage::save(&l) {
                    storage::log_line(&format!("import failed: {e}"));
                }
            }
            None => storage::log_line("import: the original app's layout could not be read"),
        }
        return;
    }
    let worker = args.iter().any(|a| a == "--worker");
    let boot = args.iter().any(|a| a == "--boot");
    let transparent = args.iter().any(|a| a == "--transparent");
    let opaque_final = args.iter().any(|a| a == "--opaque");
    MAY_ASK_TRANSPARENT.store(worker && !transparent && !opaque_final, std::sync::atomic::Ordering::SeqCst);

    if screenshot.is_none() && !worker {
        // Only one copy runs at a time (a check picture may run beside it: it saves nothing).
        if !desktop::claim_single_instance() {
            return;
        }
        supervise(boot);
        return;
    }

    // Wait for Explorer's desktop like the original (0x1802b00a0): 120 × 50 ms normally,
    // 600 × 50 ms when started with Windows.
    if screenshot.is_none() {
        desktop::wait_for_desktop(if boot { 600 } else { 120 });
    }

    // If anything ever goes wrong: log it and give the desktop icons back before leaving.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        storage::log_line(&format!("crash: {info}"));
        desktop::restore_desktop_icons();
        default_hook(info);
    }));

    // The main window is a 1x1 invisible, click-through window; the boxes and the settings
    // window are its children. It keeps the program alive without showing anything.
    let mut viewport = if screenshot.is_some() {
        // Check picture: a normal-sized window showing the layout, opened without focus and
        // behind other windows so it never gets in the person's way.
        egui::ViewportBuilder::default()
            .with_title("DeskBoxes")
            .with_inner_size([1100.0, 680.0])
            .with_active(false)
            .with_window_level(egui::WindowLevel::AlwaysOnBottom)
    } else {
        egui::ViewportBuilder::default()
            .with_title("DeskBoxes")
            .with_inner_size([1.0, 1.0])
            .with_position([0.0, 0.0])
            .with_decorations(false)
            // Not see-through unless the classic boxes need it (see RootMode); the 1x1 window
            // sits invisible below the desktop either way.
            .with_transparent(transparent)
            .with_taskbar(false)
            .with_resizable(false)
            .with_mouse_passthrough(true)
            .with_active(false)
            .with_window_level(egui::WindowLevel::AlwaysOnBottom)
    };
    viewport.app_id = Some("DeskBoxes".into());

    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        // Off: the main GL window is invisible (1x1) and DirectX boxes present on their own;
        // waiting for the screen refresh in GL made the driver spin on every pass.
        vsync: false,
        // No colour noise: it would skew the exact see-through copy used while dragging.
        dithering: false,
        persist_window: false,
        ..Default::default()
    };

    let result = eframe::run_native(
        "DeskBoxes",
        options,
        Box::new(move |cc| Ok(Box::new(app::DeskApp::new(cc, screenshot)))),
    );
    if let Err(e) = result {
        storage::log_line(&format!("could not start: {e}"));
        desktop::restore_desktop_icons();
        // Never report a failed start as a clean quit, or the supervisor would stop for good.
        let msg = e.to_string();
        let code = if transparent && msg.contains("glutin config") { EXIT_RETRY_OPAQUE } else { EXIT_START_FAILED };
        std::process::exit(code);
    }
    desktop::restore_desktop_icons();
}
