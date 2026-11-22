//! Programs from the Start menu, for quick search ("Search also finds programs" in Settings).
//!
//! The Start menu folders (for everyone and for this user) are read on a helper thread the first
//! time search needs them and again when they are 10 minutes old; search never waits for it.
//! Shortcuts that only uninstall or open help pages are left out.

use crate::model::DeskItem;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

static APPS: Mutex<Option<(Instant, std::sync::Arc<Vec<DeskItem>>)>> = Mutex::new(None);
static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn start_menu_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = std::env::var_os("ProgramData") {
        v.push(PathBuf::from(p).join("Microsoft\\Windows\\Start Menu\\Programs"));
    }
    if let Some(p) = dirs::data_dir() {
        v.push(p.join("Microsoft\\Windows\\Start Menu\\Programs"));
    }
    v
}

fn skip(name: &str) -> bool {
    let n = name.to_lowercase();
    ["uninstall", "desinstal", "readme", "read me", "help", "website", "license", "release notes", "manual"].iter().any(|w| n.contains(w))
}

fn walk(dir: &Path, depth: usize, out: &mut Vec<DeskItem>) {
    if depth > 4 || out.len() > 3000 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(&p, depth + 1, out);
            continue;
        }
        let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
        if ext != "lnk" && ext != "url" && ext != "appref-ms" {
            continue;
        }
        if let Some(mut it) = crate::desktop::item_from_path(&p) {
            if skip(&it.name) {
                continue;
            }
            it.on_desktop = false;
            out.push(it);
        }
    }
}

fn read_all() -> Vec<DeskItem> {
    let mut v = Vec::new();
    for d in start_menu_dirs() {
        walk(&d, 0, &mut v);
    }
    // The same program for everyone and for this user: keep one.
    let mut seen = std::collections::HashSet::new();
    v.retain(|it| seen.insert(it.name.to_lowercase()));
    v
}

/// The Start menu programs known now (empty until the first read finishes); starts a read on a
/// helper thread when there is none yet or it is 10 minutes old.
pub fn list() -> std::sync::Arc<Vec<DeskItem>> {
    let current = APPS.lock().ok().and_then(|g| g.clone());
    let stale = current.as_ref().map(|(t, _)| t.elapsed() >= Duration::from_secs(600)).unwrap_or(true);
    if stale && !BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
        let spawned = std::thread::Builder::new().name("start-menu".into()).spawn(|| {
            let v = std::panic::catch_unwind(read_all).unwrap_or_default();
            if let Ok(mut g) = APPS.lock() {
                *g = Some((Instant::now(), std::sync::Arc::new(v)));
            }
            BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
            #[cfg(windows)]
            crate::runloop::wake_after(Duration::ZERO);
        });
        if spawned.is_err() {
            BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    current.map(|(_, v)| v).unwrap_or_default()
}

/// Desktop items plus (if `with_apps`) Start menu programs whose name isn't on the desktop already.
pub fn search_pool<'a>(desk: impl Iterator<Item = &'a DeskItem>, apps: &'a [DeskItem], with_apps: bool) -> Vec<&'a DeskItem> {
    let mut v: Vec<&DeskItem> = desk.collect();
    if with_apps {
        let names: std::collections::HashSet<String> = v.iter().map(|i| i.name.to_lowercase()).collect();
        v.extend(apps.iter().filter(|a| !names.contains(&a.name.to_lowercase())));
    }
    v
}

/// The setting "Search also finds programs" (the app keeps it up to date each pass).
static ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_enabled(on: bool) {
    ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}
