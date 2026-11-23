//! Update check: asks GitHub for the newest DesktopShepherd release (at start and once a day, if
//! turned on in Settings) and, when it is newer than this copy, offers it. "Update" downloads the
//! release's setup file and runs it (it asks, closes this copy and installs the new one). A
//! portable copy just opens the release page.
//!
//! Note: GitHub only answers for a public repository; for a private one the check reports that
//! the releases can't be reached.

use std::sync::Mutex;
use std::time::{Duration, Instant};

const REPO: &str = "radioactivetoy/DesktopShepherd";

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub setup_url: Option<String>,
    pub page: String,
}

#[derive(Default)]
struct State {
    checked_at: Option<Instant>,
    newer: Option<Release>,
    error: Option<String>,
    busy: bool,
    /// Downloading the setup file: Some(message) while busy or after a failure.
    install_note: Option<String>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// "1.2.10" > "1.2.9"; non-numbers count as 0.
pub fn newer_than(a: &str, b: &str) -> bool {
    version_key(a) > version_key(b)
}

/// A sortable form of a version like "1.2.3", "1.0.0-rc.2" or "v0.2.0-beta.5": the numbers, then
/// the pre-release stage (alpha < beta < rc < final) and its number. So 1.0.0-beta.9 < 1.0.0-rc.1
/// < 1.0.0 < 1.0.1-beta.1.
fn version_key(s: &str) -> (Vec<u64>, u8, u64) {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let s = s.split('+').next().unwrap_or("");
    let (core, pre) = match s.split_once('-') {
        Some((c, p)) => (c, Some(p.to_ascii_lowercase())),
        None => (s, None),
    };
    let mut nums: Vec<u64> = core.split('.').map(|p| p.parse().unwrap_or(0)).collect();
    while nums.len() < 3 {
        nums.push(0);
    }
    match pre {
        None => (nums, 3, 0),
        Some(p) => {
            let stage = if p.starts_with("rc") { 2 } else if p.starts_with("beta") { 1 } else { 0 };
            let n = p.split(|c: char| !c.is_ascii_digit()).filter(|x| !x.is_empty()).last().and_then(|x| x.parse().ok()).unwrap_or(0);
            (nums, stage, n)
        }
    }
}

#[cfg(test)]
mod version_tests {
    use super::newer_than;

    #[test]
    fn versions_compare_like_releases() {
        assert!(newer_than("1.0.0-rc.1", "0.2.0-beta.5"));
        assert!(newer_than("1.0.0", "1.0.0-rc.1"));
        assert!(!newer_than("1.0.0-rc.1", "1.0.0"));
        assert!(newer_than("1.0.0-rc.1", "1.0.0-beta.9"));
        assert!(newer_than("0.2.0-beta.10", "0.2.0-beta.9"));
        assert!(newer_than("v1.0.1", "1.0.0"));
        assert!(!newer_than("1.0.0", "1.0.0"));
        assert!(newer_than("1.1.0-beta.1", "1.0.9"));
    }
}

fn fetch_latest() -> Result<Release, String> {
    let txt = crate::net::get_text(&format!("https://api.github.com/repos/{REPO}/releases/latest")).map_err(|e| {
        if e.contains("404") {
            "no release published yet".to_string()
        } else {
            e
        }
    })?;
    let v: serde_json::Value = serde_json::from_str(&txt).map_err(|_| "GitHub answered strangely".to_string())?;
    let version = v["tag_name"].as_str().ok_or("no version in GitHub's answer")?.trim_start_matches(['v', 'V']).to_string();
    let setup_url = v["assets"].as_array().and_then(|a| {
        a.iter()
            .find(|x| x["name"].as_str().map(|n| n.to_lowercase().contains("setup") && n.to_lowercase().ends_with(".exe")).unwrap_or(false))
            .and_then(|x| x["browser_download_url"].as_str().map(str::to_string))
    });
    let page = v["html_url"].as_str().unwrap_or("").to_string();
    Ok(Release { version, setup_url, page })
}

/// Checks in the background if the last check is older than `every` (or `force`).
pub fn check(every: Duration, force: bool) {
    let Ok(mut g) = STATE.lock() else { return };
    let s = g.get_or_insert_with(State::default);
    if s.busy || (!force && s.checked_at.map(|t| t.elapsed() < every).unwrap_or(false)) {
        return;
    }
    s.busy = true;
    let _ = std::thread::Builder::new().name("update-check".into()).spawn(|| {
        let r = std::panic::catch_unwind(fetch_latest).unwrap_or_else(|_| Err("the update check failed".into()));
        if let Ok(mut g) = STATE.lock() {
            let s = g.get_or_insert_with(State::default);
            s.busy = false;
            s.checked_at = Some(Instant::now());
            match r {
                Ok(rel) => {
                    s.error = None;
                    s.newer = newer_than(&rel.version, env!("CARGO_PKG_VERSION")).then_some(rel);
                }
                Err(e) => {
                    crate::storage::log_line(&format!("update check: {e}"));
                    s.error = Some(e);
                }
            }
        }
        #[cfg(windows)]
        crate::runloop::wake_after(Duration::ZERO);
    });
}

/// (a newer release if there is one, the last error, checking now, the install note).
pub fn status() -> (Option<Release>, Option<String>, bool, Option<String>) {
    STATE
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|s| (s.newer.clone(), s.error.clone(), s.busy, s.install_note.clone())))
        .unwrap_or((None, None, false, None))
}

fn set_note(n: Option<String>) {
    if let Ok(mut g) = STATE.lock() {
        g.get_or_insert_with(State::default).install_note = n;
    }
    #[cfg(windows)]
    crate::runloop::wake_after(Duration::ZERO);
}

fn is_portable() -> bool {
    std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.join("portable.txt").exists())).unwrap_or(false)
}

/// Downloads the new setup file and runs it (or opens the release page for a portable copy).
pub fn install(rel: Release) {
    let _ = std::thread::Builder::new().name("update-install".into()).spawn(move || {
        let open_page = |url: &str| {
            #[cfg(windows)]
            unsafe {
                use windows::core::{w, HSTRING, PCWSTR};
                use windows::Win32::UI::Shell::ShellExecuteW;
                use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
                let _ = ShellExecuteW(None, w!("open"), &HSTRING::from(url), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
            }
            let _ = url;
        };
        let Some(url) = rel.setup_url.clone().filter(|_| !is_portable()) else {
            open_page(&rel.page);
            return;
        };
        set_note(Some(crate::i18n::t("Downloading the update…").to_string()));
        match crate::net::get_bytes(&url, 128 * 1024 * 1024) {
            Ok(bytes) if bytes.len() > 100_000 && bytes.starts_with(b"MZ") => {
                let path = std::env::temp_dir().join(format!("DesktopShepherd-Setup-{}.exe", rel.version));
                if let Err(e) = std::fs::write(&path, &bytes) {
                    set_note(Some(format!("{}: {e}", crate::i18n::t("Could not save the update"))));
                    return;
                }
                set_note(None);
                // The setup asks, closes this copy and installs the new version.
                if let Err(e) = std::process::Command::new(&path).spawn() {
                    set_note(Some(format!("{}: {e}", crate::i18n::t("Could not start the update"))));
                }
            }
            Ok(_) => set_note(Some(crate::i18n::t("The downloaded file is not a program; opening the release page instead.").to_string())),
            Err(e) => {
                set_note(Some(format!("{}: {e}", crate::i18n::t("Could not download the update"))));
                open_page(&rel.page);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::newer_than;

    #[test]
    fn versions() {
        assert!(newer_than("0.2.0", "0.1.9"));
        assert!(newer_than("v1.10.0", "1.9.3"));
        assert!(!newer_than("0.1.0", "0.1.0"));
        assert!(!newer_than("0.1", "0.1.0"));
    }
}

/// A check has finished at least once in this run.
pub fn checked() -> bool {
    STATE.lock().ok().and_then(|g| g.as_ref().map(|s| s.checked_at.is_some())).unwrap_or(false)
}
