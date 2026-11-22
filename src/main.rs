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
mod search;
mod snap;
#[cfg(windows)]
mod runloop;
mod storage;
mod widgets;

use std::path::PathBuf;

/// Worker exit code: the app could not start (no graphics device, no window). The supervisor
/// tries again later.
pub const EXIT_START_FAILED: i32 = 2;

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
    loop {
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("--worker");
        if boot && first {
            cmd.arg("--boot");
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

    #[cfg(windows)]
    let code = runloop::run(screenshot);
    #[cfg(not(windows))]
    let code = {
        let _ = screenshot;
        EXIT_START_FAILED
    };
    desktop::restore_desktop_icons();
    if code != 0 {
        // Never report a failed start as a clean quit, or the supervisor would stop for good.
        std::process::exit(code);
    }
}
