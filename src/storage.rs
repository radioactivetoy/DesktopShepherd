//! Saving and loading the layout safely: atomic writes (a crash mid-save never leaves a broken
//! file), dated backups (like the original's layout backups), and fallback to the newest good
//! backup when the main file is damaged.

use crate::model::Layout;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

const MAX_BACKUPS: usize = 10;

pub fn data_dir() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(std::env::temp_dir);
    base.join("DeskBoxes")
}

fn layout_path() -> PathBuf {
    data_dir().join("layout.json")
}

fn backup_dir() -> PathBuf {
    data_dir().join("backups")
}

pub fn log_line(msg: &str) {
    let dir = data_dir();
    let _ = fs::create_dir_all(&dir);
    let path = dir.join("log.txt");
    // Keep the log small; the full old log is kept as log.old.txt.
    if fs::metadata(&path).map(|m| m.len() > 256 * 1024).unwrap_or(false) {
        let old = dir.join("log.old.txt");
        // On Windows rename fails if the target exists, so remove it first.
        let _ = fs::remove_file(&old);
        let _ = fs::rename(&path, &old);
    }
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), msg);
    }
}

fn parse(text: &str) -> Option<Layout> {
    serde_json::from_str::<Layout>(text).ok()
}

/// Loads the layout; `None` means first run (nothing saved yet and no backup).
pub fn load() -> Option<Layout> {
    if let Ok(text) = fs::read_to_string(layout_path()) {
        if let Some(l) = parse(&text) {
            return Some(l);
        }
        log_line("layout.json was damaged; trying backups");
    }
    for b in list_backups().into_iter().rev() {
        if let Ok(text) = fs::read_to_string(&b) {
            if let Some(l) = parse(&text) {
                log_line(&format!("restored layout from {}", b.display()));
                return Some(l);
            }
        }
    }
    None
}

pub fn list_backups() -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(backup_dir())
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

pub fn load_backup(path: &PathBuf) -> Option<Layout> {
    fs::read_to_string(path).ok().and_then(|t| parse(&t))
}

/// Writes the layout atomically: temp file, flush to disk, then replace.
pub fn save(layout: &Layout) -> std::io::Result<()> {
    let dir = data_dir();
    fs::create_dir_all(&dir)?;
    let text = serde_json::to_string_pretty(layout).map_err(std::io::Error::other)?;
    let tmp = dir.join("layout.json.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, layout_path())?;
    Ok(())
}

/// Keeps a dated copy of the current layout (once per start, only if it changed since the last
/// backup) and trims old ones.
pub fn backup_now(layout: &Layout) {
    let Ok(text) = serde_json::to_string_pretty(layout) else { return };
    let dir = backup_dir();
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let backups = list_backups();
    if let Some(last) = backups.last() {
        if fs::read_to_string(last).map(|t| t == text).unwrap_or(false) {
            return;
        }
    }
    let name = format!("layout-{}.json", chrono::Local::now().format("%Y%m%d%H%M%S"));
    let _ = fs::write(dir.join(name), text);
    let backups = list_backups();
    if backups.len() > MAX_BACKUPS {
        for old in &backups[..backups.len() - MAX_BACKUPS] {
            let _ = fs::remove_file(old);
        }
    }
}
