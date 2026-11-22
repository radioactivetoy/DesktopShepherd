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

/// The installer (DesktopShepherd-Setup.exe).
pub fn run_setup() {
    let dir = install_dir();
    let exe = dir.join(format!("{APP}.exe"));
    let again = exe.exists();
    let q = if again {
        format!("Update {APP}?\n\nThe installed copy in\n{}\nwill be replaced. Your boxes stay as they are.", dir.display())
    } else {
        format!("Install {APP}?\n\nIt goes to\n{}\n(just for you, no admin rights needed), gets a Start menu entry and starts with Windows.\n\nYou can remove it later from Settings > Apps > Installed apps.", dir.display())
    };
    if !ask(&q, MB_YESNO | MB_ICONQUESTION) {
        return;
    }
    if !stop_running() {
        tell(&format!("{APP} is still running and didn't close. Quit it from its tray icon, then run the setup again."), MB_ICONWARNING);
        return;
    }
    let Ok(me) = std::env::current_exe() else { return };
    if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::copy(&me, &exe).map(|_| ())) {
        tell(&format!("Could not copy the program to\n{}\n\n{e}", dir.display()), MB_ICONWARNING);
        return;
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
            let _ = key.set_value("DisplayIcon", &exe.display().to_string());
            let _ = key.set_value("InstallLocation", &dir.display().to_string());
            let _ = key.set_value("UninstallString", &format!("{} --uninstall", q(&exe)));
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
    // Said before starting it: the starting program would close this message right away.
    tell(&format!("{APP} is {}. It starts when you press OK; its icon is in the tray (next to the clock).", if again { "updated" } else { "installed" }), MB_ICONINFORMATION);
    if let Err(e) = std::process::Command::new(&exe).current_dir(&dir).env("DESKTOPSHEPHERD_INSTALLED", "1").spawn() {
        tell(&format!("{APP} is installed but did not start: {e}"), MB_ICONWARNING);
    }
}

/// `--uninstall`, run by Windows' "Installed apps".
pub fn run_uninstall() {
    if !ask(&format!("Remove {APP} from this computer?"), MB_YESNO | MB_ICONQUESTION) {
        return;
    }
    if !stop_running() {
        tell(&format!("{APP} is still running and didn't close. Quit it from its tray icon, then try again."), MB_ICONWARNING);
        return;
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
    let delete_data = ask(
        &format!("Also delete your saved boxes and settings?\n\n{}\n\nChoose No to keep them in case you install {APP} again.", data.display()),
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
    tell(&format!("{APP} has been removed."), MB_ICONINFORMATION);
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
}
