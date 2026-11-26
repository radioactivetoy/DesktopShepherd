//! Installing and uninstalling, built into the program itself (no separate installer tool).
//!
//! * `DesktopShepherd-Setup.exe` (any copy of the program whose file name contains "setup"):
//!   asks, then copies itself to `%LOCALAPPDATA%\Programs\DesktopShepherd\DesktopShepherd.exe`,
//!   adds a Start menu shortcut and an entry in Windows' "Installed apps" (per user, no admin
//!   rights needed), turns on start-with-Windows and starts the installed copy.
//! * `--uninstall` (what "Installed apps" runs): asks, stops the program (which gives the
//!   desktop icons back), removes the shortcut, the entries and the program folder. The saved
//!   boxes are kept unless the person says otherwise.

#![cfg(windows)]

use crate::i18n::tf;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, IDYES, MB_DEFBUTTON2, MB_ICONINFORMATION, MB_ICONQUESTION, MB_ICONWARNING, MB_OK, MB_YESNO, MESSAGEBOX_STYLE,
};

const APP: &str = "DesktopShepherd";
const UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\DesktopShepherd";

/// True when this copy of the program is the installer (its file name contains "setup").
pub fn is_setup_exe() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()))
        .map(|s| s.contains("setup"))
        .unwrap_or(false)
}

fn ask(text: &str, style: MESSAGEBOX_STYLE) -> bool {
    unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(APP), style) == IDYES }
}

fn tell(text: &str, style: MESSAGEBOX_STYLE) {
    unsafe {
        // A "quit" waiting in this thread's queue would close the message at once.
        {
            use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, MSG, PM_REMOVE, WM_QUIT};
            let mut m = MSG::default();
            while PeekMessageW(&mut m, None, WM_QUIT, WM_QUIT, PM_REMOVE).as_bool() {
                crate::storage::log_line("setup: dropped a pending quit message");
            }
        }
        let r = MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(APP), style | MB_OK);
        // (not after the data folder was deleted on uninstall: logging would create it again)
        if crate::storage::data_dir().exists() {
            crate::storage::log_line(&format!("setup message ({}): {}", r.0, text.lines().next().unwrap_or("")));
        }
    }
}

pub fn install_dir() -> PathBuf {
    dirs::data_local_dir().unwrap_or_else(std::env::temp_dir).join("Programs").join(APP)
}

fn shortcut_path() -> Option<PathBuf> {
    dirs::data_dir().map(|d| d.join("Microsoft\\Windows\\Start Menu\\Programs").join(format!("{APP}.lnk")))
}

/// Asks a running copy (this name or the old DeskBoxes one) to quit cleanly and waits up to 8 s
/// for it to be gone. Returns false if one is still running.
fn stop_running() -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenEventW, OpenMutexW, SetEvent, EVENT_MODIFY_STATE, SYNCHRONIZATION_SYNCHRONIZE};
    let events = [w!("Local\\DesktopShepherd.Quit"), w!("Local\\DeskBoxes.Quit")];
    let mutexes = [w!("Local\\DesktopShepherd.SingleInstance"), w!("Local\\DeskBoxes.SingleInstance")];
    unsafe {
        for e in events {
            if let Ok(h) = OpenEventW(EVENT_MODIFY_STATE, false, e) {
                let _ = SetEvent(h);
                let _ = CloseHandle(h);
            }
        }
        let start = Instant::now();
        loop {
            let running = mutexes.iter().any(|m| match OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, *m) {
                Ok(h) => {
                    let _ = CloseHandle(h);
                    true
                }
                Err(_) => false,
            });
            if !running {
                return true;
            }
            if start.elapsed() > Duration::from_secs(8) {
                return false;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }
}

/// A Start menu shortcut to `exe`.
fn make_shortcut(exe: &Path, lnk: &Path) -> windows::core::Result<()> {
    use windows::core::Interface;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(exe.as_os_str()))?;
        if let Some(dir) = exe.parent() {
            link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        }
        link.SetDescription(w!("Boxes for your desktop icons, plus widgets"))?;
        let file: IPersistFile = link.cast()?;
        file.Save(PCWSTR(HSTRING::from(lnk.as_os_str()).as_ptr()), true)?;
    }
    Ok(())
}

fn dir_size_kb(dir: &Path) -> u32 {
    std::fs::read_dir(dir).map(|rd| rd.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum::<u64>() / 1024).unwrap_or(0) as u32
}

/// Switches that mean "no windows, no questions" (for winget and scripts).
const SILENT_FLAGS: [&str; 5] = ["--silent", "/s", "-s", "/quiet", "--quiet"];

fn is_silent_flag(a: &str) -> bool {
    let a = a.to_ascii_lowercase();
    SILENT_FLAGS.contains(&a.as_str())
}

/// True when the command line (program name first) asks for a silent setup or uninstall.
pub fn is_silent(args: &[String]) -> bool {
    args.iter().skip(1).any(|a| is_silent_flag(a))
}

/// True when the command line asks for the normal setup: nothing but silent switches (and
/// `--install`) after the program name (any other `--` option belongs to the program itself).
pub fn wants_setup(args: &[String]) -> bool {
    !args.iter().skip(1).any(|a| a.starts_with("--") && !is_silent_flag(a) && !a.eq_ignore_ascii_case("--install"))
}

/// `--install`: run the setup whatever the file is called. winget (and other tools) may save the
/// downloaded installer under another name, so "Setup" in the name can't be relied on.
pub fn wants_install(args: &[String]) -> bool {
    args.iter().skip(1).any(|a| a.eq_ignore_ascii_case("--install")) && wants_setup(args)
}

/// A message for the person; in silent mode only written to the log.
fn say(silent: bool, text: &str, style: MESSAGEBOX_STYLE) {
    if silent {
        crate::storage::log_line(&format!("setup (silent): {}", text.lines().next().unwrap_or("")));
    } else {
        tell(text, style);
    }
}

/// The installer (DesktopShepherd-Setup.exe). Returns the exit code (0 = fine).
pub fn run_setup(silent: bool) -> i32 {
    let dir = install_dir();
    let exe = dir.join(format!("{APP}.exe"));
    let again = exe.exists();
    let q = if again {
        tf("Update {}?\n\nThe installed copy in\n{}\nwill be replaced. Your boxes stay as they are.", &[&APP, &dir.display()])
    } else {
        tf("Install {}?\n\nIt goes to\n{}\n(just for you, no admin rights needed), gets a Start menu entry and starts with Windows.\n\nYou can remove it later from Settings > Apps > Installed apps.", &[&APP, &dir.display()])
    };
    if !silent && !ask(&q, MB_YESNO | MB_ICONQUESTION) {
        return 0;
    }
    if !stop_running() {
        say(silent, &tf("{} is still running and didn't close. Quit it from its tray icon, then run the setup again.", &[&APP]), MB_ICONWARNING);
        return 1;
    }
    let Ok(me) = std::env::current_exe() else { return 1 };
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::copy(&me, &exe).map(|_| ())) {
        say(silent, &tf("Could not copy the program to\n{}\n\n{}", &[&dir.display(), &e]), MB_ICONWARNING);
        return 1;
    }
    if let Some(lnk) = shortcut_path() {
        if let Err(e) = make_shortcut(&exe, &lnk) {
            crate::storage::log_line(&format!("setup: Start menu shortcut failed: {e}"));
        }
    }
    // The "Installed apps" entry.
    {
        use winreg::enums::HKEY_CURRENT_USER;
        if let Ok((key, _)) = winreg::RegKey::predef(HKEY_CURRENT_USER).create_subkey(UNINSTALL_KEY) {
            let q = |p: &Path| format!("\"{}\"", p.display());
            let _ = key.set_value("DisplayName", &APP);
            let _ = key.set_value("DisplayVersion", &env!("CARGO_PKG_VERSION"));
            let _ = key.set_value("Publisher", &APP);
            let _ = key.set_value("DisplayIcon", &format!("{},0", exe.display()));
            let _ = key.set_value("InstallLocation", &dir.display().to_string());
            let _ = key.set_value("UninstallString", &format!("{} --uninstall", q(&exe)));
            let _ = key.set_value("QuietUninstallString", &format!("{} --uninstall --silent", q(&exe)));
            let _ = key.set_value("NoModify", &1u32);
            let _ = key.set_value("NoRepair", &1u32);
            let _ = key.set_value("EstimatedSize", &dir_size_kb(&dir));
        }
    }
    // Start with Windows: remember it in the settings (the installed copy keeps the entry
    // pointing at itself each time it starts).
    // An existing layout is only switched to start with Windows. A new PC has none: the
    // installed copy is told (DESKTOPSHEPHERD_INSTALLED) and turns it on in its own first layout,
    // so the usual first-run boxes are made (never write an empty layout here).
    if let Some(mut layout) = crate::storage::load() {
        if !layout.settings.start_with_windows {
            layout.settings.start_with_windows = true;
            let _ = crate::storage::save(&layout);
        }
    }
    if silent {
        // No window and the program is not started: winget expects no UI.
        crate::storage::log_line("setup (silent): installed");
        return 0;
    }
    // Said before starting it: the starting program would close this message right away.
    let done = if again {
        tf("{} is updated. It starts when you press OK; its icon is in the tray (next to the clock).", &[&APP])
    } else {
        tf("{} is installed. It starts when you press OK; its icon is in the tray (next to the clock).", &[&APP])
    };
    tell(&done, MB_ICONINFORMATION);
    if let Err(e) = std::process::Command::new(&exe).current_dir(&dir).env("DESKTOPSHEPHERD_INSTALLED", "1").spawn() {
        tell(&tf("{} is installed but did not start: {}", &[&APP, &e]), MB_ICONWARNING);
    }
    0
}

/// `--uninstall` (`--uninstall --silent`: no questions, the saved boxes are kept), run by
/// Windows' "Installed apps" or winget. Returns the exit code (0 = fine).
pub fn run_uninstall(silent: bool) -> i32 {
    if !silent && !ask(&tf("Remove {} from this computer?", &[&APP]), MB_YESNO | MB_ICONQUESTION) {
        return 0;
    }
    if !stop_running() {
        say(silent, &tf("{} is still running and didn't close. Quit it from its tray icon, then try again.", &[&APP]), MB_ICONWARNING);
        return 1;
    }
    // Make sure the normal desktop icons are back (the program does it when it quits, too).
    crate::desktop::force_show_desktop_icons();
    {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
        let hkcu = winreg::RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(run) = hkcu.open_subkey_with_flags("Software\\Microsoft\\Windows\\CurrentVersion\\Run", KEY_SET_VALUE) {
            let _ = run.delete_value(APP);
            let _ = run.delete_value("DeskBoxes");
        }
        let _ = hkcu.delete_subkey_all(UNINSTALL_KEY);
        let _ = hkcu.delete_subkey_all("Software\\Classes\\DesktopBackground\\Shell\\DesktopShepherd");
    }
    if let Some(lnk) = shortcut_path() {
        let _ = std::fs::remove_file(lnk);
    }
    let data = crate::storage::data_dir();
    // Silent: never deletes the person's data.
    let delete_data = !silent
        && ask(
            &tf("Also delete your saved boxes and settings?\n\n{}\n\nChoose No to keep them in case you install {} again.", &[&data.display(), &APP]),
            MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
        );
    if delete_data {
        let _ = std::fs::remove_dir_all(&data);
    } else if let Some(mut layout) = crate::storage::load() {
        // Kept for a later install: but nothing should start with Windows any more.
        layout.settings.start_with_windows = false;
        let _ = crate::storage::save(&layout);
    }
    // Said first: while this message is open the program is still running from the folder.
    if silent {
        crate::storage::log_line("setup (silent): removed");
    } else {
        tell(&tf("{} has been removed.", &[&APP]), MB_ICONINFORMATION);
    }
    // This program is running from the folder being removed: a short-lived, hidden command
    // window deletes it a moment after we exit.
    let dir = install_dir();
    let running_from_it = std::env::current_exe().ok().map(|p| p.starts_with(&dir)).unwrap_or(false);
    if running_from_it {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        // Windows' own cmd and ping by full path, run from System32 (never a program of the same
        // name from a folder others can write to).
        let sys32 = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows")).join("System32");
        let _ = std::process::Command::new(sys32.join("cmd.exe"))
            // Passed as typed: the usual quoting would garble the folder path for cmd.
            .raw_arg(format!("/C \"\"{}\" -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"\"", sys32.join("PING.EXE").display(), dir.display()))
            .current_dir(&sys32)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    } else {
        let _ = std::fs::remove_dir_all(&dir);
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn silent_switches() {
        for f in ["--silent", "/S", "/s", "-s", "-S", "/quiet", "/QUIET", "--quiet"] {
            assert!(is_silent(&v(&["setup.exe", f])), "{f}");
            assert!(wants_setup(&v(&["setup.exe", f])), "{f}");
        }
        assert!(is_silent(&v(&["x.exe", "--uninstall", "--silent"])));
    }

    #[test]
    fn not_silent() {
        assert!(!is_silent(&v(&["setup.exe"])));
        assert!(!is_silent(&v(&["--silent"]))); // that is the program name slot
        assert!(!is_silent(&v(&["setup.exe", "--uninstall"])));
        assert!(!is_silent(&v(&["setup.exe", "/silent"])));
    }

    #[test]
    fn install_switch_works_under_any_name() {
        // winget may save the installer as e.g. "<hash>.exe": --install still runs the setup.
        assert!(wants_install(&v(&["C:\\T\\WinGet\\3f6da844.exe", "--install", "--silent"])));
        assert!(wants_install(&v(&["x.exe", "--INSTALL"])));
        assert!(is_silent(&v(&["x.exe", "--install", "--silent"])));
        assert!(!is_silent(&v(&["x.exe", "--install"])));
        assert!(wants_setup(&v(&["setup.exe", "--install", "--silent"])));
        assert!(!wants_install(&v(&["x.exe"])));
        assert!(!wants_install(&v(&["x.exe", "--silent"])));
        assert!(!wants_install(&v(&["x.exe", "--install", "--worker"])));
    }

    #[test]
    fn setup_only_without_other_options() {
        assert!(wants_setup(&v(&["setup.exe"])));
        assert!(!wants_setup(&v(&["setup.exe", "--worker"])));
        assert!(!wants_setup(&v(&["setup.exe", "--silent", "--screenshot"])));
    }
}
