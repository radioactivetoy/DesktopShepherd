//! The running program: each box is its own small borderless window kept at the bottom of the
//! window stack (on the desktop, under every app), plus a settings window opened on demand.
//!
//! Performance rules followed here:
//! * nothing redraws unless the mouse/keyboard touches a box, a file changes, or a widget asks
//!   (a clock wakes once a minute);
//! * reading the desktop folder and loading icons happen on background threads;
//! * saving waits until changes settle, then writes once.

use crate::desktop;
use crate::fence_view::{self as fv, DeskState, ROLLED_HEIGHT};
use crate::ghost;
use crate::icons::IconCache;
use crate::model::{Category, DeskItem, Fence, Layout, SortKind, Tab, TabPos, TitlePos, WidgetConfig};
use crate::storage;
use crate::widgets;
use egui::{self, Align, Color32, CursorIcon, FontId, Pos2, Rect, ScrollArea, Sense, UiBuilder, Vec2, ViewportCommand};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SAVE_DELAY: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Boxes,
    Layout,
    Settings,
}

enum Act {
    /// Icons dropped at a screen point (points): move them to the box there, or out of their box.
    DropItems { from: u64, paths: Vec<String>, at: Pos2 },
    /// An item was opened from a box (counts for 'Sort by times opened').
    Opened(String),
    MoveItem { path: String, to: u64, tab: usize },
    MoveToNewFence { path: String },
    NewFence { auto: bool },
    NewWidget(&'static str),
    DeleteFence(u64),
    OpenManager,
    RestoreBackup(PathBuf),
    /// Replace the boxes with the ones from the original organizer's saved layout.
    ImportOriginal,
    /// Give every box the look of this one (Personalize > Apply this look to all boxes).
    ApplyLookToAll(u64),
    Quit,
}

#[derive(Default)]
struct UiState {
    selected: Option<String>,
    /// Selected icons (lower-case paths): click, Ctrl+click, or a selection rectangle.
    selection: HashSet<String>,
    /// Icons being dragged: source box and their paths.
    item_drag: Option<(u64, Vec<String>)>,
    /// Selection rectangle in progress: box id and where it started (window points).
    band: Option<(u64, Pos2)>,
    /// The icon picture flying with the mouse during an icon drag (window handle).
    drag_image: Option<ghost::DragImage>,
    /// Safety net: when an icon drag was first seen with the mouse button up.
    drag_up_since: Option<Instant>,
    /// (box id, tab index or None for the box title, text being typed)
    renaming: Option<(u64, Option<usize>, String)>,
    confirm_delete: Option<u64>,
    hover_open: HashSet<u64>,
    menu: Option<OpenMenu>,
    /// Boxes whose window corners Windows already rounds, with the radius asked for.
    rounded: HashMap<u64, i32>,
    /// Each box's window handle, once prepared (no-activate, sent behind all windows).
    hwnds: HashMap<u64, isize>,
    /// Boxes that currently have keyboard focus (sent back behind windows when they lose it).
    focused: HashSet<u64>,
    /// Frames left to redraw after a size change, so a grown box never shows a black area.
    burst: HashMap<u64, u8>,
    /// Last known size of each box window.
    sizes: HashMap<u64, Rect>,
    /// A move (kind 0) or resize (1 right, 2 bottom, 3 corner) in progress: box, kind,
    /// mouse position at the start, box rectangle at the start (screen points).
    drag: Option<(u64, u8, Pos2, Rect)>,
    /// Boxes the person just moved or resized: re-read which monitor they're on.
    rebind: HashSet<u64>,
    /// Boxes have Windows' glass (false = no-glass fallback: they paint a solid background).
    glass: bool,
    /// Snapping while moving/resizing (from the settings) and the gap between boxes (points).
    snap_on: bool,
    snap_gap: f32,
    /// Grid size in points (0 = grid snapping off).
    snap_grid: f32,
    /// Open the quick-search window on the next pass (box menu, tray, shortcut).
    open_search: bool,
    /// Commands for box windows from elsewhere (menus, monitor changes): box id, command.
    box_cmds: Vec<(u64, ViewportCommand)>,
    /// Boxes whose next position update comes from a move or resize by the person.
    moved_by_person: HashSet<u64>,
}

pub enum TrayCmd {
    Open,
    NewBox,
    Search,
    Quit,
}

pub struct DeskApp {
    layout: Layout,
    items: HashMap<String, DeskItem>,
    scan_rx: Receiver<HashMap<String, DeskItem>>,
    scan_wake: Sender<()>,
    extra_paths: Arc<Mutex<Vec<String>>>,
    icons: IconCache,
    spacing: (f32, f32),
    ui: UiState,
    actions: Vec<Act>,
    dirty_at: Option<Instant>,
    persist: bool,
    manager_open: bool,
    page: Page,
    /// Drawing the settings view off-screen for `--screenshot` (no windows, nothing saved).
    screenshot: bool,
    tray_cmds: Arc<Mutex<Vec<TrayCmd>>>,
    #[cfg(windows)]
    _tray: Option<tray_icon::TrayIcon>,
    icons_hidden: bool,
    backups: Vec<PathBuf>,
    /// Displays, ordered by role (main first), and a flag the watcher sets when they change.
    monitors: Vec<desktop::Monitor>,
    monitors_shared: Arc<Mutex<Vec<desktop::Monitor>>>,
    monitors_changed: Arc<std::sync::atomic::AtomicBool>,
    /// The DirectX/composition box windows (None only for `--screenshot`).
    #[cfg(windows)]
    hosts: Option<crate::host::HostSystem>,
    /// This pass draws every box (`redraw_all` was set): menu and settings draw too.
    panel_force: bool,
    /// Bring the settings window to the front on its next pass (tray "Open" while it's open).
    focus_settings: bool,
    /// Shared data changed (files, icons, layout, selection): every box draws on the next pass.
    /// Otherwise a DirectX box draws only for its own reasons (see `HostSystem::frame`).
    redraw_all: bool,
    /// Icon sizes (pixels) the boxes showed last pass (see `IconCache::keep_only_sizes`).
    icon_sizes: Vec<u32>,
    /// Fonts the boxes use, loaded into the drawing context (see `setup_style`).
    font_names: Vec<String>,
    /// When to put all boxes back just above the desktop after a display change.
    restack_at: Vec<Instant>,
    /// The quick-search window, while open.
    search: Option<SearchState>,
    /// The quick-search shortcut registered with Windows (index into SEARCH_HOTKEYS) and
    /// whether Windows accepted it.
    hotkey_applied: usize,
    hotkey_ok: bool,
}

/// Passes and box frames drawn (for DESKBOXES_DEBUG_DRAWS).
static DRAW_STATS: (std::sync::atomic::AtomicU64, std::sync::atomic::AtomicU64) =
    (std::sync::atomic::AtomicU64::new(0), std::sync::atomic::AtomicU64::new(0));
static DRAW_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static UPDATE_US: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A cheap fingerprint of the UI state that several boxes show (selection, renaming, an icon
/// drag): when it changes in one box, the others draw again too.
fn shared_ui_fingerprint(st: &UiState) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    // Order-independent sum over the selection set.
    let mut sel: u64 = 0;
    for p in &st.selection {
        let mut ph = std::collections::hash_map::DefaultHasher::new();
        p.hash(&mut ph);
        sel = sel.wrapping_add(ph.finish());
    }
    (sel, st.selection.len(), &st.selected, st.renaming.as_ref().map(|r| (r.0, r.1)), st.item_drag.is_some()).hash(&mut h);
    h.finish()
}

fn clean_paths(layout: &Layout) -> Vec<String> {
    layout.fences.iter().flat_map(|f| f.tabs.iter().flat_map(|t| t.items.iter().cloned())).collect()
}

impl DeskApp {
    /// `root`: the run loop's scheduling context (see runloop.rs); in `--screenshot` mode it
    /// also draws the settings view. Returns None when the boxes can't be shown at all.
    pub fn new(root: &egui::Context, screenshot: Option<PathBuf>) -> Option<Self> {
        let ctx = root.clone();
        let persist = screenshot.is_none();
        if !persist {
            setup_style(&ctx, &[]);
        }
        if persist {
            // Keep the boxes visible when Show desktop (Win+D) brings the desktop forward.
            desktop::watch_show_desktop();
        }

        let loaded = if persist { storage::load() } else { None };
        let first_run = loaded.is_none();
        let mut layout = loaded.unwrap_or_default();

        // The first desktop read happens right away (it is quick) so boxes start filled.
        let initial = desktop::scan(&clean_paths(&layout));
        if first_run {
            layout = default_layout(&initial);
        }
        sanitize(&mut layout);
        if persist {
            storage::backup_now(&layout);
            if first_run {
                let _ = storage::save(&layout);
            }
        }

        // Background desktop watcher: re-reads the folders every few seconds (or when woken)
        // and only reports when something actually changed.
        let extra_paths = Arc::new(Mutex::new(clean_paths(&layout)));
        let monitors_now = desktop::monitors();
        let monitors_shared = Arc::new(Mutex::new(monitors_now.clone()));
        let monitors_changed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (scan_tx, scan_rx) = channel();
        let (wake_tx, wake_rx) = channel::<()>();
        {
            let extra = extra_paths.clone();
            let ctx = ctx.clone();
            let mons = monitors_shared.clone();
            let mons_flag = monitors_changed.clone();
            let every = Duration::from_secs(layout.settings.rescan_secs.clamp(2, 60));
            let mut last: Vec<String> = {
                let mut k: Vec<String> = initial.iter().map(|(k, v)| format!("{k}|{}", v.name)).collect();
                k.sort();
                k
            };
            let _ = std::thread::Builder::new().name("desktop-watch".into()).spawn(move || loop {
                desktop::reapply_icon_hiding();
                // Displays added, removed or re-arranged: tell the app to re-place the boxes.
                let now = desktop::monitors();
                if let Ok(mut m) = mons.lock() {
                    if *m != now {
                        *m = now;
                        mons_flag.store(true, std::sync::atomic::Ordering::SeqCst);
                        ctx.request_repaint();
                    }
                }
                match wake_rx.recv_timeout(every) {
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => break,
                }
                let extras = extra.lock().map(|e| e.clone()).unwrap_or_default();
                let found = match std::panic::catch_unwind(|| desktop::scan(&extras)) {
                    Ok(f) => f,
                    Err(_) => continue,
                };
                let mut keys: Vec<String> = found.iter().map(|(k, v)| format!("{k}|{}", v.name)).collect();
                keys.sort();
                if keys != last {
                    last = keys;
                    if scan_tx.send(found).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            });
        }

        // Boxes drawn by Windows' composition system with DirectX (software DirectX if the
        // graphics card can't). Without it there is nothing to show: the supervisor retries later.
        #[cfg(windows)]
        let hosts = if persist {
            match crate::host::HostSystem::new(&ctx) {
                Ok(h) => {
                    setup_style(&h.ctx, &[]);
                    Some(h)
                }
                Err(e) => {
                    storage::log_line(&format!("DirectX boxes unavailable: {e:?}"));
                    return None;
                }
            }
        } else {
            None
        };
        let tray_cmds = Arc::new(Mutex::new(Vec::new()));
        #[cfg(windows)]
        let tray = if persist { make_tray(&ctx, tray_cmds.clone()) } else { None };

        if persist {
            desktop::set_low_priority(layout.settings.low_priority);
        }
        let mut app = Self {
            items: initial,
            scan_rx,
            scan_wake: wake_tx,
            extra_paths,
            icons: IconCache::new(&ctx),
            spacing: desktop::icon_spacing(),
            ui: UiState::default(),
            actions: Vec::new(),
            dirty_at: None,
            persist,
            manager_open: !persist || first_run || layout.fences.is_empty(),
            page: if persist { Page::Boxes } else { Page::Layout },
            screenshot: screenshot.is_some(),
            tray_cmds,
            #[cfg(windows)]
            _tray: tray,
            icons_hidden: false,
            backups: storage::list_backups(),
            monitors: monitors_now,
            monitors_shared,
            monitors_changed,
            #[cfg(windows)]
            hosts,
            panel_force: true,
            focus_settings: false,
            redraw_all: true,
            icon_sizes: Vec::new(),
            font_names: Vec::new(),
            restack_at: Vec::new(),
            search: None,
            hotkey_applied: usize::MAX,
            hotkey_ok: true,
            layout,
        };
        if app.persist {
            app.bind_unassigned();
            app.relayout_monitors();
        }
        app.apply_icon_hiding();
        Some(app)
    }

    /// Icons still loading (the screenshot waits for them).
    pub fn icons_busy(&self) -> bool {
        self.icons.busy()
    }

    /// Before the program ends (quit, Windows shutting down): save and give the icons back.
    pub fn shutdown(&mut self) {
        if self.dirty_at.is_some() {
            self.save_now();
        }
        if self.icons_hidden {
            desktop::set_desktop_icons_visible(true);
            self.icons_hidden = false;
        }
    }

    /// Moves a box window to where its saved rectangle says (after the app itself changed it:
    /// monitors, "bring back on screen", a restored layout). Windows the person moves update
    /// the rectangle the other way round, in `fence_window`.
    fn place_box(&mut self, id: u64) {
        if let Some(f) = self.layout.fences.iter().find(|f| f.id == id) {
            let h = if f.expanded { f.rect[3] } else { ROLLED_HEIGHT };
            self.ui.box_cmds.push((id, ViewportCommand::OuterPosition(Pos2::new(f.rect[0], f.rect[1]))));
            self.ui.box_cmds.push((id, ViewportCommand::InnerSize(Vec2::new(f.rect[2], h))));
        }
    }

    fn place_all_boxes(&mut self) {
        let ids: Vec<u64> = self.layout.fences.iter().map(|f| f.id).collect();
        for id in ids {
            self.place_box(id);
        }
    }

    fn mark_dirty(&mut self) {
        // The layout changed: boxes show it right away (saving waits until changes settle).
        self.redraw_all = true;
        if self.dirty_at.is_none() {
            self.dirty_at = Some(Instant::now());
        }
    }

    fn save_now(&mut self) {
        self.dirty_at = None;
        if !self.persist {
            return;
        }
        if let Ok(mut e) = self.extra_paths.lock() {
            *e = clean_paths(&self.layout);
        }
        if let Err(e) = storage::save(&self.layout) {
            storage::log_line(&format!("save failed: {e}"));
        }
    }

    fn apply_icon_hiding(&mut self) {
        if !self.persist {
            return;
        }
        let want_hidden = self.layout.settings.hide_desktop_icons;
        if want_hidden != self.icons_hidden {
            desktop::set_desktop_icons_visible(!want_hidden);
            self.icons_hidden = want_hidden;
        }
    }

    fn new_fence_rect(&self, size: [f32; 2]) -> [f32; 4] {
        let n = self.layout.fences.len() as f32;
        [120.0 + 30.0 * (n % 8.0), 120.0 + 30.0 * (n % 8.0), size[0], size[1]]
    }

    fn process_actions(&mut self) {
        let acts = std::mem::take(&mut self.actions);
        for a in acts {
            match a {
                Act::MoveItem { path, to, tab } => {
                    for f in &mut self.layout.fences {
                        if !f.auto_sort {
                            f.remove_item(&path);
                        }
                    }
                    if let Some(f) = self.layout.fences.iter_mut().find(|f| f.id == to) {
                        let t = tab.min(f.tabs.len().saturating_sub(1));
                        if f.tabs.is_empty() {
                            f.tabs.push(Tab { name: "Main".into(), items: vec![] });
                        }
                        f.tabs[t].items.push(path);
                    }
                    self.mark_dirty();
                }
                Act::MoveToNewFence { path } => {
                    for f in &mut self.layout.fences {
                        if !f.auto_sort {
                            f.remove_item(&path);
                        }
                    }
                    let id = self.layout.new_id();
                    let rect = self.new_fence_rect([420.0, 220.0]);
                    self.layout.fences.push(Fence {
                        id,
                        rect,
                        tabs: vec![Tab { name: "Main".into(), items: vec![path] }],
                        ..Fence::default()
                    });
                    self.mark_dirty();
                }
                Act::NewFence { auto } => {
                    let id = self.layout.new_id();
                    let rect = self.new_fence_rect([520.0, 280.0]);
                    self.layout.fences.push(Fence {
                        id,
                        rect,
                        auto_sort: auto,
                        name: if auto { "Desktop".into() } else { "New box".into() },
                        ..Fence::default()
                    });
                    self.mark_dirty();
                }
                Act::NewWidget(kind) => {
                    if let Some(k) = widgets::find(kind) {
                        let id = self.layout.new_id();
                        let rect = self.new_fence_rect(k.default_size);
                        self.layout.fences.push(Fence {
                            id,
                            rect,
                            name: k.label.into(),
                            title_pos: TitlePos::Top,
                            widget: Some(WidgetConfig { kind: k.id.into(), data: serde_json::json!({}) }),
                            ..Fence::default()
                        });
                        self.mark_dirty();
                    }
                }
                Act::DeleteFence(id) => {
                    self.layout.fences.retain(|f| f.id != id);
                    // Forget the window bookkeeping of the deleted box.
                    for m in [&mut self.ui.burst] {
                        m.remove(&id);
                    }
                    self.ui.sizes.remove(&id);
                    self.ui.hwnds.remove(&id);
                    self.ui.rounded.remove(&id);
                    self.ui.focused.remove(&id);
                    self.ui.hover_open.remove(&id);
                    self.mark_dirty();
                }
                Act::OpenManager => self.manager_open = true,
                Act::DropItems { from, paths, at } => {
                    let target = self
                        .layout
                        .fences
                        .iter()
                        .find(|f| {
                            let h = if f.expanded { f.rect[3] } else { ROLLED_HEIGHT };
                            Rect::from_min_size(Pos2::new(f.rect[0], f.rect[1]), Vec2::new(f.rect[2], h)).contains(at)
                        })
                        .map(|f| (f.id, f.auto_sort, f.widget.is_some(), f.sel_tab));
                    match target {
                        Some((id, _, _, _)) if id == from => {}
                        Some((_, _, true, _)) => {}
                        Some((id, false, false, tab)) => {
                            for p in paths {
                                self.actions.push(Act::MoveItem { path: p, to: id, tab });
                            }
                        }
                        // Onto the auto-sort box or the desktop: out of their box.
                        _ => {
                            if let Some(f) = self.layout.fences.iter_mut().find(|f| f.id == from && !f.auto_sort) {
                                for p in &paths {
                                    f.remove_item(p);
                                }
                                self.mark_dirty();
                            }
                        }
                    }
                }
                Act::Opened(p) => {
                    *self.layout.open_counts.entry(p.to_lowercase()).or_insert(0) += 1;
                    self.mark_dirty();
                }
                Act::RestoreBackup(p) => {
                    if let Some(mut l) = storage::load_backup(&p) {
                        storage::backup_now(&self.layout);
                        sanitize(&mut l);
                        self.layout = l;
                        self.place_all_boxes();
                        self.mark_dirty();
                        self.apply_icon_hiding();
                    }
                }
                Act::ImportOriginal => {
                    if let Some(mut l) = crate::import_original::boxes_from_original(&self.layout) {
                        storage::backup_now(&self.layout);
                        sanitize(&mut l);
                        self.layout = l;
                        self.place_all_boxes();
                        self.mark_dirty();
                        self.apply_icon_hiding();
                    }
                }
                Act::ApplyLookToAll(id) => {
                    if let Some(src) = self.layout.fences.iter().find(|f| f.id == id).cloned() {
                        for f in self.layout.fences.iter_mut().filter(|f| f.id != id) {
                            f.copy_look_from(&src);
                        }
                        self.mark_dirty();
                    }
                }
                Act::Quit => {
                    self.save_now();
                    // Ends the run loop cleanly (exit code 0: the supervisor stops too).
                    #[cfg(windows)]
                    crate::runloop::request_quit();
                }
            }
        }
    }
}

impl DeskApp {
    /// One pass of the app (called by the run loop; see runloop.rs).
    pub fn update(&mut self, ctx: &egui::Context) {
        let update_start = Instant::now();
        self.update_inner(ctx);
        UPDATE_US.fetch_add(update_start.elapsed().as_micros() as u64, std::sync::atomic::Ordering::Relaxed);
    }

    fn update_inner(&mut self, ctx: &egui::Context) {
        if std::env::var_os("DESKBOXES_DEBUG_REPAINT").is_some() && ctx.cumulative_pass_nr() % 120 == 0 {
            storage::log_line(&format!("repaint causes: {:?}", ctx.repaint_causes()));
        }
        // ---- inputs from background threads and the tray icon
        let mut changed_files = false;
        while let Ok(found) = self.scan_rx.try_recv() {
            self.items = found;
            changed_files = true;
        }
        if changed_files {
            let keep: HashSet<String> = self.items.keys().cloned().collect();
            self.icons.forget_missing(&keep);
            self.redraw_all = true;
        }
        // The quick-search shortcut: (re)register it when the setting changes; a press toggles
        // the search window.
        #[cfg(windows)]
        if self.persist {
            let want = self.layout.settings.search_hotkey;
            if self.hotkey_applied != want {
                self.hotkey_ok = crate::runloop::set_search_hotkey(want);
                self.hotkey_applied = want;
            }
            if crate::runloop::take_search_request() {
                if self.search.is_some() {
                    self.search = None;
                } else {
                    self.ui.open_search = true;
                }
                self.redraw_all = true;
            }
        }
        // Icon sizes in use (pixels): when they change (View > Small/Medium/Large), drop the
        // icons of sizes no box shows any more.
        let s = desktop::system_scale();
        let sizes: Vec<u32> = {
            let mut v: Vec<u32> = self.layout.fences.iter().map(|f| ((f.icon_size as f32 * s).round() as u32).clamp(16, 256)).collect();
            v.sort_unstable();
            v.dedup();
            v
        };
        if sizes != self.icon_sizes {
            if !self.icon_sizes.is_empty() {
                self.icons.keep_only_sizes(&sizes.iter().copied().collect());
            }
            self.icon_sizes = sizes;
        }
        // Fonts chosen in boxes: load the ones in use (before any box draws this pass).
        let mut wanted: Vec<String> = self.layout.fences.iter().filter(|f| !f.font.is_empty()).map(|f| f.font.clone()).collect();
        wanted.sort();
        wanted.dedup();
        if wanted != self.font_names {
            #[cfg(windows)]
            let draw = self.hosts.as_ref().map(|h| h.ctx.clone()).unwrap_or_else(|| ctx.clone());
            #[cfg(not(windows))]
            let draw = ctx.clone();
            setup_style(&draw, &wanted);
            fv::set_loaded_fonts(&wanted);
            self.font_names = wanted;
            self.redraw_all = true;
        }
        #[cfg(windows)]
        let icons_arrived = match self.hosts.as_ref() {
            Some(h) => self.icons.poll(&h.ctx),
            None => self.icons.poll(ctx),
        };
        #[cfg(not(windows))]
        let icons_arrived = self.icons.poll(ctx);
        if icons_arrived {
            self.redraw_all = true;
        }
        let cmds: Vec<TrayCmd> = self.tray_cmds.lock().map(|mut v| std::mem::take(&mut *v)).unwrap_or_default();
        for c in cmds {
            match c {
                TrayCmd::Open => {
                    self.focus_settings = self.manager_open;
                    self.manager_open = true;
                }
                TrayCmd::NewBox => self.actions.push(Act::NewFence { auto: false }),
                TrayCmd::Search => self.ui.open_search = true,
                TrayCmd::Quit => self.actions.push(Act::Quit),
            }
        }


        if self.persist {
            // Displays changed: Windows told us at once (WM_DISPLAYCHANGE), or the watcher noticed.
            #[cfg(windows)]
            let told = crate::runloop::take_display_changed();
            #[cfg(not(windows))]
            let told = false;
            let noticed = self.monitors_changed.swap(false, std::sync::atomic::Ordering::SeqCst);
            if told || noticed {
                let now_monitors = if told { desktop::monitors() } else { self.monitors_shared.lock().map(|m| m.clone()).unwrap_or_default() };
                if !now_monitors.is_empty() {
                    if let Ok(mut m) = self.monitors_shared.lock() {
                        *m = now_monitors.clone();
                    }
                    self.monitors = now_monitors;
                }
                self.relayout_monitors();
                // When a monitor goes away Windows rearranges windows, and boxes can end up above
                // some apps: put them back just above the desktop now and again as things settle.
                let now = Instant::now();
                self.restack_at = vec![now, now + Duration::from_millis(1500), now + Duration::from_secs(4)];
            }
            if !self.restack_at.is_empty() {
                let now = Instant::now();
                if self.restack_at.iter().any(|t| *t <= now) {
                    self.restack_at.retain(|t| *t > now);
                    storage::log_line(&format!("display change: re-stacking {} boxes", self.ui.hwnds.len()));
                    for h in self.ui.hwnds.values() {
                        desktop::send_to_bottom(*h);
                    }
                }
                if let Some(next) = self.restack_at.iter().min() {
                    ctx.request_repaint_after(next.saturating_duration_since(now));
                }
            }
            self.bind_unassigned();
            let rebind: Vec<u64> = self.ui.rebind.drain().collect();
            if !rebind.is_empty() {
                for f in self.layout.fences.iter_mut().filter(|f| rebind.contains(&f.id)) {
                    bind_to_monitor(f, &self.monitors);
                }
                self.mark_dirty();
            }
        }

        // ---- the boxes (not drawn as real windows while taking a check picture)
        if self.persist {
            self.show_fences(ctx);
        }

        // ---- settings window
        if self.manager_open {
            self.show_manager(ctx);
        } else {
            #[cfg(windows)]
            if let Some(h) = self.hosts.as_mut() {
                h.close_panel(SETTINGS_PANEL);
            }
        }

        if !self.actions.is_empty() {
            self.redraw_all = true;
        }
        self.process_actions();
        // Something shared changed during this pass: draw every box once more, right away.
        if self.redraw_all && self.persist {
            ctx.request_repaint();
        }

        // ---- saving, once changes settle
        if let Some(t) = self.dirty_at {
            let left = SAVE_DELAY.saturating_sub(t.elapsed());
            if left.is_zero() {
                self.save_now();
            } else {
                ctx.request_repaint_after(left + Duration::from_millis(20));
            }
        }
    }
}

// ============================================================================================
// Boxes
// ============================================================================================

impl DeskApp {
    fn show_fences(&mut self, ctx: &egui::Context) {
        let claimed: HashSet<String> = self
            .layout
            .fences
            .iter()
            .filter(|f| !f.auto_sort && f.widget.is_none())
            .flat_map(|f| f.tabs.iter().flat_map(|t| t.items.iter().map(|p| p.to_lowercase())))
            .collect();
        let targets: Vec<(u64, String, Vec<String>)> = self
            .layout
            .fences
            .iter()
            .filter(|f| !f.auto_sort && f.widget.is_none())
            .map(|f| (f.id, f.name.clone(), f.tabs.iter().map(|t| t.name.clone()).collect()))
            .collect();
        let expand_on_hover = self.layout.settings.expand_on_hover;
        let mut dirty = false;
        let mut wake_scan = false;
        let desk = DeskState { items: &self.items, claimed: &claimed, spacing: self.spacing, rules: &self.layout.settings.categories, open_counts: &self.layout.open_counts };
        // DirectX boxes: Windows draws the glass, egui draws the contents with the GPU.
        #[cfg(windows)]
        if self.hosts.as_ref().map(|h| h.lost).unwrap_or(false) {
            // Graphics reset (sleep/resume, driver update): rebuild and carry on.
            let ok = self.hosts.as_mut().map(|h| h.recover(ctx)).unwrap_or(false);
            if ok {
                if let Some(h) = self.hosts.as_ref() {
                    setup_style(&h.ctx, &self.font_names);
                }
                self.icons.clear();
                self.ui.hwnds.clear();
                self.ui.rounded.clear();
            } else {
                ctx.request_repaint_after(Duration::from_secs(2));
            }
        }
        // Every box draws when shared data changed, and during an icon drag (the box under the
        // mouse shows where the icons will land).
        let force_all = std::mem::take(&mut self.redraw_all) || self.ui.item_drag.is_some();
        self.panel_force = force_all;
        let fingerprint = shared_ui_fingerprint(&self.ui);
        #[cfg(windows)]
        if let Some(hosts) = self.hosts.as_mut().filter(|h| !h.lost) {
            self.ui.glass = hosts.has_glass();
            self.ui.snap_on = self.layout.settings.snap;
            self.ui.snap_gap = self.layout.settings.snap_gap;
            self.ui.snap_grid = if self.layout.settings.snap_grid { self.layout.settings.grid_size.clamp(4.0, 200.0) } else { 0.0 };
            let ids: Vec<u64> = self.layout.fences.iter().map(|f| f.id).collect();
            hosts.retain(&ids);
            for fence in self.layout.fences.iter_mut() {
                let look = crate::host::Look {
                    tint: [fence.color[0], fence.color[1], fence.color[2], fence.alpha],
                    corner_points: fence.corner,
                    blur: fence.blur as f32 / 40.0,
                    no_activate: fence.widget.is_none(),
                };
                let h = if fence.expanded { fence.rect[3] } else { ROLLED_HEIGHT };
                let rect = [fence.rect[0], fence.rect[1], fence.rect[2], h];
                let id = fence.id;
                let ui_state = &mut self.ui;
                let icons = &mut self.icons;
                let actions = &mut self.actions;
                let t0 = Instant::now();
                let drew = hosts.frame(id, rect, &look, force_all, |ctx| {
                    let r = fence_window(ctx, fence, &desk, icons, ui_state, actions, expand_on_hover);
                    dirty |= r.0;
                    wake_scan |= r.1;
                });
                if drew {
                    DRAW_STATS.1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    DRAW_US.fetch_add(t0.elapsed().as_micros() as u64, std::sync::atomic::Ordering::Relaxed);
                }
            }
            // Wake up again when the earliest box timer is due (see HostSystem::next_due).
            if let Some(t) = hosts.next_due() {
                ctx.request_repaint_after(t.saturating_duration_since(Instant::now()).max(crate::host::MIN_PASS_GAP));
            }
        }
        if shared_ui_fingerprint(&self.ui) != fingerprint {
            self.redraw_all = true;
        }
        // DESKBOXES_DEBUG_DRAWS: log how many box frames each pass draws (measuring only).
        let passes = DRAW_STATS.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if passes % 300 == 0 && std::env::var_os("DESKBOXES_DEBUG_DRAWS").is_some() {
            let draws = DRAW_STATS.1.swap(0, std::sync::atomic::Ordering::Relaxed);
            let us = DRAW_US.swap(0, std::sync::atomic::Ordering::Relaxed);
            let upd = UPDATE_US.swap(0, std::sync::atomic::Ordering::Relaxed);
            storage::log_line(&format!("last 300 passes: {draws} box frames drawn in {} ms; update() total {} ms (force_all now: {force_all})", us / 1000, upd / 1000));
        }
        if self.ui.menu.is_some() {
            self.show_menu(ctx, &targets);
        } else {
            #[cfg(windows)]
            if let Some(h) = self.hosts.as_mut() {
                h.close_panel(MENU_PANEL);
            }
        }
        // Quick search (shortcut, tray, box menu).
        #[cfg(windows)]
        {
            if std::mem::take(&mut self.ui.open_search) && self.persist && self.search.is_none() {
                self.search = Some(SearchState::default());
            }
            if self.search.is_some() {
                self.show_search(ctx);
            } else if let Some(h) = self.hosts.as_mut() {
                h.close_panel(SEARCH_PANEL);
            }
        }
        // Commands for box windows from the menus (focus for renaming, roll up/down).
        for (id, cmd) in std::mem::take(&mut self.ui.box_cmds) {
            #[cfg(windows)]
            if let Some(h) = self.hosts.as_ref() {
                h.apply(id, &cmd);
                h.ctx.request_repaint();
            }
            #[cfg(not(windows))]
            let _ = (id, cmd);
        }
        if dirty {
            self.mark_dirty();
        }
        if wake_scan {
            if let Ok(mut e) = self.extra_paths.lock() {
                *e = clean_paths(&self.layout);
            }
            let _ = self.scan_wake.send(());
        }
    }
}

/// One box's window. Returns (layout changed, desktop should be re-read).
#[allow(clippy::too_many_arguments)]
fn fence_window(
    ctx: &egui::Context,
    fence: &mut Fence,
    desk: &DeskState,
    icons: &mut IconCache,
    st: &mut UiState,
    actions: &mut Vec<Act>,
    expand_on_hover: bool,
) -> (bool, bool) {
    let mut dirty = false;
    let mut wake = false;

    if std::env::var_os("DESKBOXES_DEBUG_REPAINT").is_some() && ctx.cumulative_pass_nr() % 120 == 0 {
        storage::log_line(&format!(
            "box {} causes: {:?} burst={:?} rect={:?}",
            fence.id,
            ctx.repaint_causes(),
            st.burst.get(&fence.id),
            ctx.input(|i| i.viewport().inner_rect)
        ));
    }
    if std::env::var_os("DESKBOXES_DEBUG_DRAG").is_some() && ctx.input(|i| i.pointer.any_pressed() || i.pointer.any_released()) {
        storage::log_line(&format!("box {} pointer pressed={} released={} at {:?} hovered_any={} dragged={:?}", fence.id, ctx.input(|i| i.pointer.any_pressed()), ctx.input(|i| i.pointer.any_released()), ctx.input(|i| i.pointer.interact_pos()), ctx.is_pointer_over_area(), ctx.dragged_id()));
    }
    // Hover-to-open for rolled-up boxes.
    let pointer_inside = ctx.input(|i| i.pointer.has_pointer());
    let any_down = ctx.input(|i| i.pointer.any_down());
    if !fence.expanded && expand_on_hover {
        let open = st.hover_open.contains(&fence.id);
        if pointer_inside && !open {
            st.hover_open.insert(fence.id);
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(fence.rect[2], fence.rect[3])));
            st.burst.insert(fence.id, 15);
        } else if !pointer_inside && open && !any_down {
            st.hover_open.remove(&fence.id);
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(fence.rect[2], ROLLED_HEIGHT)));
            st.burst.insert(fence.id, 15);
        }
    } else {
        st.hover_open.remove(&fence.id);
    }
    let showing_full = fence.expanded || st.hover_open.contains(&fence.id);

    // Native Windows 11 rounded corners matching the box's corner setting.
    let want = fence.corner.round() as i32;
    // Part of the desktop: clicking doesn't bring it forward; after typing in it, back down.
    if !st.hwnds.contains_key(&fence.id) {
        if let Some(h) = desktop::prepare_box(&format!("DeskBoxes box {}", fence.id), fence.widget.is_none()) {
            st.hwnds.insert(fence.id, h);
        }
    }
    let has_focus = ctx.input(|i| i.viewport().focused).unwrap_or(false);
    if has_focus {
        st.focused.insert(fence.id);
    } else if st.focused.remove(&fence.id) {
        if let Some(h) = st.hwnds.get(&fence.id) {
            desktop::send_to_bottom(*h);
        }
    }
    // After a size change, redraw a few frames so new areas are painted at once. (Moving needs
    // no redraw: Windows moves the composed box as it is.)
    if let Some(r) = ctx.input(|i| i.viewport().inner_rect) {
        let resized = st.sizes.get(&fence.id).map(|o| (o.size() - r.size()).length() > 0.5).unwrap_or(true);
        st.sizes.insert(fence.id, r);
        if resized {
            st.burst.insert(fence.id, 12);
        }
    }
    if let Some(b) = st.burst.get_mut(&fence.id) {
        if *b > 0 {
            *b -= 1;
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }
    // The box draws its own rounded corners (with the glass behind them), so Windows must not
    // re-round or outline the window: then the box looks exactly the same at rest as its
    // see-through copy does while being dragged.
    if st.rounded.get(&fence.id) != Some(&want) && desktop::round_window(&format!("DeskBoxes box {}", fence.id), 0.0) {
        st.rounded.insert(fence.id, want);
    }

    // Remember where the person moved or resized the box.
    if !any_down {
        if let Some(r) = ctx.input(|i| i.viewport().outer_rect) {
            let mut nr = fence.rect;
            nr[0] = r.min.x;
            nr[1] = r.min.y;
            nr[2] = r.width();
            if fence.expanded {
                nr[3] = r.height();
            }
            if nr.iter().zip(fence.rect.iter()).any(|(a, b)| (a - b).abs() > 0.5) && r.width() > 10.0 {
                fence.rect = nr;
                dirty = true;
                if st.moved_by_person.remove(&fence.id) {
                    st.rebind.insert(fence.id);
                }
            }
        }
    }

    // Files dragged in from Explorer join the current tab.
    let dropped: Vec<String> = ctx.input(|i| {
        i.raw.dropped_files.iter().filter_map(|f| f.path.as_ref().map(|p| p.to_string_lossy().to_string())).collect()
    });
    if !dropped.is_empty() && !fence.auto_sort && fence.widget.is_none() {
        for p in dropped {
            actions.push(Act::MoveItem { path: p, to: fence.id, tab: fence.sel_tab });
        }
        wake = true;
    }
    let hovering_files = ctx.input(|i| !i.raw.hovered_files.is_empty());

    let mut view = fence.clone();
    view.expanded = showing_full;

    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
        let rect = ui.max_rect();
        let painter = ui.painter().clone();
        let ppp = ctx.pixels_per_point();
        // Where the box is on screen right now (points).
        let current = st
            .hwnds
            .get(&fence.id)
            .and_then(|h| desktop::client_rect_points(*h, ppp))
            .or_else(|| ctx.input(|i| i.viewport().inner_rect))
            .unwrap_or_else(|| Rect::from_min_size(Pos2::new(fence.rect[0], fence.rect[1]), rect.size()));
        let screen = current;
        if st.glass {
            // Only the border: Windows draws the glass, tint and rounded corners.
            painter.rect_stroke(
                rect.shrink(0.5),
                egui::CornerRadius::same(fence.corner.round().clamp(0.0, 40.0) as u8),
                fv::border_stroke(fence, 1.0),
                egui::StrokeKind::Inside,
            );
        } else {
            // No glass available: a solid dark background with the box's tint and border.
            painter.rect_filled(rect, 0.0, Color32::from_rgb(30, 30, 34));
            fv::paint_background(&painter, &view, rect, 1.0);
        }
        if hovering_files && !fence.auto_sort {
            painter.rect_stroke(
                rect.shrink(2.0),
                egui::CornerRadius::same(6),
                egui::Stroke::new(2.0_f32, Color32::from_rgb(90, 170, 255)),
                egui::StrokeKind::Inside,
            );
        }

        // Safety net: an icon drag whose end was never reported (icon vanished mid-drag) is
        // cleared once the button has been up for half a second, so a normal drop (handled on
        // drag_stopped, within a frame or two) always goes first. Wall time, not pass numbers:
        // every box's frame is a pass of the shared context.
        if st.item_drag.is_some() || st.drag_image.is_some() {
            if ghost::primary_button_down() {
                st.drag_up_since = None;
            } else {
                let since = *st.drag_up_since.get_or_insert_with(Instant::now);
                if since.elapsed() > Duration::from_millis(500) {
                    st.item_drag = None;
                    if let Some(img) = st.drag_image.take() {
                        img.destroy();
                    }
                    st.drag_up_since = None;
                } else {
                    ctx.request_repaint_after(Duration::from_millis(550));
                }
            }
        } else {
            st.drag_up_since = None;
        }
        // Icons being dragged over this box from another one: show where they will land.
        if let Some((from, _)) = st.item_drag.as_ref() {
            let accepts = *from != fence.id && !fence.auto_sort && fence.widget.is_none();
            if accepts && desktop::cursor_points(ppp).map(|c| current.contains(c)).unwrap_or(false) {
                painter.rect_stroke(
                    rect.shrink(1.5),
                    egui::CornerRadius::same(fence.corner.round().clamp(0.0, 40.0) as u8),
                    egui::Stroke::new(2.5_f32, Color32::from_rgb(76, 194, 255)),
                    egui::StrokeKind::Inside,
                );
            }
        }
        // Background: right-click menu for the box.
        let bg = ui.interact(rect, ui.id().with("bg"), Sense::click());
        if bg.secondary_clicked() {
            open_menu(ctx, st, MenuTarget::Fence(fence.id));
        }
        if bg.clicked() {
            st.selected = None;
        }

        let tabs = if view.widget.is_some() { vec![(fence.name.clone(), vec![])] } else { fv::visible_tabs(&view, desk) };
        let tab_count = if view.widget.is_some() { 1 } else { tabs.len() };
        let a = fv::areas(&view, rect, tab_count, 1.0);

        // Title: drag to move, double-click to roll up / open.
        let title = ui.interact(a.title, ui.id().with("title"), Sense::click_and_drag());
        // Moving: a helper thread moves the DirectX box itself with the mouse (Windows moves
        // the composed box as it is; nothing is redrawn).
        if title.drag_started() && !fence.locked {
            if let Some(c) = desktop::cursor_points(ppp) {
                st.drag = Some((fence.id, 0, c, screen));
                #[cfg(windows)]
                if let Some(&bh) = st.hwnds.get(&fence.id) {
                    // Keep the exact spot the person grabbed (where the button went down, not where
                    // the mouse was once egui recognised the drag a few pixels later).
                    let local = ctx.input(|i| i.pointer.press_origin()).unwrap_or(c - current.min.to_vec2());
                    let grab = (local.x * ppp, local.y * ppp);
                    let snap = snap_targets(st, fence.id, 1.0);
                    crate::host::move_with_mouse(bh, (grab.0.round() as i32, grab.1.round() as i32), snap);
                }
            }
        }
        if title.drag_stopped() {
            st.drag = None;
            // Remember the monitor the person left it on (by role or fixed monitor, per the
            // settings) right away, and once more when its final position arrives.
            st.moved_by_person.insert(fence.id);
            st.rebind.insert(fence.id);
        }
        if title.double_clicked() {
            fence.expanded = !fence.expanded;
            st.hover_open.remove(&fence.id);
            let h = if fence.expanded { fence.rect[3] } else { ROLLED_HEIGHT };
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(fence.rect[2], h)));
            st.burst.insert(fence.id, 15);
            dirty = true;
        }
        if title.secondary_clicked() {
            open_menu(ctx, st, MenuTarget::Fence(fence.id));
        }
        fv::paint_title(&painter, &view, a.title, 1.0, title.hovered());

        // Renaming box or tab.
        if let Some((rid, tab, mut text)) = st.renaming.take() {
            if rid == fence.id {
                let edit_rect = Rect::from_min_size(rect.min + Vec2::new(30.0, 4.0), Vec2::new((rect.width() - 60.0).max(80.0), 22.0));
                let resp = ui.put(edit_rect, egui::TextEdit::singleline(&mut text).hint_text("Name"));
                resp.request_focus();
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
                if esc {
                    // dropped: keep old name
                } else if enter || resp.lost_focus() {
                    let t = text.trim().to_string();
                    if !t.is_empty() {
                        match tab {
                            None => fence.name = t,
                            Some(i) => {
                                if let Some(tab) = fence.tabs.get_mut(i) {
                                    tab.name = t;
                                }
                            }
                        }
                        dirty = true;
                    }
                } else {
                    st.renaming = Some((rid, tab, text));
                }
            } else {
                st.renaming = Some((rid, tab, text));
            }
        }

        if !showing_full {
            return;
        }

        // Tabs.
        let sel = fence.sel_tab.min(tabs.len().saturating_sub(1));
        if let Some(strip) = a.tabs {
            let names: Vec<String> = tabs.iter().map(|t| t.0.clone()).collect();
            let rects = fv::paint_tabs(&painter, &view, strip, &names, sel, 1.0);
            for (i, r) in rects.iter().enumerate() {
                let resp = ui.interact(*r, ui.id().with(("tab", i)), Sense::click());
                if resp.clicked() && fence.sel_tab != i {
                    fence.sel_tab = i;
                    dirty = true;
                }
                if resp.secondary_clicked() {
                    let t = if fence.auto_sort { MenuTarget::Fence(fence.id) } else { MenuTarget::Tab(fence.id, i) };
                    open_menu(ctx, st, t);
                }
            }
        }

        // Widget content.
        if let Some(wc) = fence.widget.as_mut() {
            let mut child = ui.new_child(UiBuilder::new().max_rect(a.content).layout(egui::Layout::top_down(Align::Min)));
            match widgets::find(&wc.kind) {
                Some(kind) => {
                    let out = (kind.make)().ui(&mut child, &mut wc.data);
                    dirty |= out.changed;
                    if let Some(d) = out.repaint_after {
                        ctx.request_repaint_after(d);
                    }
                }
                None => {
                    child.label(format!("Unknown widget \"{}\"", wc.kind));
                }
            }
        } else if let Some((_, items)) = tabs.get(sel) {
            // Icon grid.
            let cell = fv::cell_size(fence, desk.spacing);
            let style = fv::text_style(fence);
            let icon = fence.icon_size as f32;
            let manual = fence.sort == SortKind::Manual && !fence.auto_sort;
            let mut child = ui.new_child(UiBuilder::new().max_rect(a.content).layout(egui::Layout::top_down(Align::Min)));
            ScrollArea::vertical()
                .id_salt(("grid", fence.id, sel))
                .auto_shrink([false, false])
                .drag_to_scroll(false)
                .show(&mut child, |ui| {
                    let width = ui.available_width();
                    let cols = ((width / cell.x).floor() as usize).max(1);
                    let rows = items.len().div_ceil(cols);
                    // The whole grid area also takes drags on empty space: a selection rectangle.
                    let grid_h = (rows as f32 * cell.y).max(ui.available_height());
                    let (grid, band) = ui.allocate_exact_size(Vec2::new(width, grid_h), Sense::click_and_drag());
                    if band.drag_started() {
                        if let Some(p) = band.interact_pointer_pos() {
                            st.band = Some((fence.id, p));
                            if !ui.input(|i| i.modifiers.ctrl) {
                                st.selection.clear();
                            }
                        }
                    }
                    if band.clicked() {
                        st.selection.clear();
                        st.selected = None;
                    }
                    // Right-click on empty space: the box menu.
                    if band.secondary_clicked() {
                        open_menu(ctx, st, MenuTarget::Fence(fence.id));
                    }
                    let band_rect = match (st.band, band.dragged(), band.interact_pointer_pos()) {
                        (Some((id, start)), true, Some(now)) if id == fence.id => Some(Rect::from_two_pos(start, now)),
                        _ => None,
                    };
                    if band.drag_stopped() {
                        st.band = None;
                    }
                    let pad = ((width - cols as f32 * cell.x) / 2.0).max(0.0).min(cell.x * 0.5);
                    let mut reorder: Option<(String, String)> = None;
                    // (icon rectangles were only needed by the old see-through drag copy)
                    let clip = ui.clip_rect();
                    for (i, it) in items.iter().enumerate() {
                        let (c, r) = (i % cols, i / cols);
                        let cr = Rect::from_min_size(grid.min + Vec2::new(pad + c as f32 * cell.x, r as f32 * cell.y), cell);
                        if !cr.intersects(clip) {
                            continue; // off-screen: skip work
                        }
                        let resp = ui.interact(cr, ui.id().with(("item", &it.path)), Sense::click_and_drag());
                        let key = it.path.to_lowercase();
                        if let Some(b) = band_rect {
                            if b.intersects(cr.shrink(8.0)) {
                                st.selection.insert(key.clone());
                            }
                        }
                        let selected = st.selection.contains(&key);
                        let hl = if selected { 50 } else if resp.hovered() { 22 } else { 0 };
                        fv::paint_item(ui.painter(), icons, it, cr, icon, 1.0, hl, &style);
                        if resp.clicked() {
                            // Click: just this icon; Ctrl+click: add or remove it.
                            if ui.input(|i| i.modifiers.ctrl) {
                                if !st.selection.remove(&key) {
                                    st.selection.insert(key.clone());
                                }
                            } else {
                                st.selection.clear();
                                st.selection.insert(key.clone());
                            }
                            st.selected = Some(it.path.clone());
                        }
                        // Drag icons into another box (or onto the desktop to take them out).
                        if resp.drag_started() {
                            if !st.selection.contains(&key) {
                                st.selection.clear();
                                st.selection.insert(key.clone());
                            }
                            let paths: Vec<String> = items.iter().filter(|x| st.selection.contains(&x.path.to_lowercase())).map(|x| x.path.clone()).collect();
                            if std::env::var_os("DESKBOXES_DEBUG_DRAG").is_some() {
                                storage::log_line(&format!("icon drag start from box {}: {:?}", fence.id, paths));
                            }
                            st.item_drag = Some((fence.id, paths));
                            // A see-through picture of the icon flies with the mouse while dragging.
                            // Loaded on the picture's own thread: a slow shortcut (network drive)
                            // never freezes the boxes.
                            let ppp = ctx.pixels_per_point();
                            let px = (icon * ppp).round() as u32;
                            if let Some(old) = st.drag_image.take() {
                                old.destroy();
                            }
                            st.drag_image = Some(ghost::begin_icon_drag(it.path.clone(), px));
                        }
                        if resp.dragged() && st.item_drag.as_ref().map(|d| d.1.len() > 1 || st.drag_image.is_none()).unwrap_or(false) {
                            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                            if let Some(p) = resp.interact_pointer_pos() {
                                let n = st.item_drag.as_ref().map(|d| d.1.len()).unwrap_or(1);
                                let label = if n == 1 { it.name.clone() } else { format!("{n} items") };
                                let g = ui.painter().layout_no_wrap(label, egui::FontId::proportional(12.0), Color32::WHITE);
                                let r = Rect::from_min_size(p + Vec2::new(14.0, 14.0), g.size() + Vec2::new(12.0, 6.0));
                                let top = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, ui.id().with("dragbadge")));
                                top.rect_filled(r, egui::CornerRadius::same(4), Color32::from_rgba_unmultiplied(40, 40, 44, 230));
                                top.galley(r.min + Vec2::new(6.0, 3.0), g, Color32::WHITE);
                            }
                        }
                        if resp.drag_stopped() {
                            if let Some(img) = st.drag_image.take() {
                                img.destroy();
                            }
                            if let Some((from, paths)) = st.item_drag.take() {
                                if let Some(at) = desktop::cursor_points(desktop::system_scale()) {
                                    if std::env::var_os("DESKBOXES_DEBUG_DRAG").is_some() {
                                        storage::log_line(&format!("icon drop from box {from} at {at:?}"));
                                    }
                                    actions.push(Act::DropItems { from, paths, at });
                                }
                            }
                        }
                        if resp.double_clicked() {
                            desktop::open_item(&it.path);
                            actions.push(Act::Opened(it.path.clone()));
                        }
                        if manual {
                            // The payload is the item's path, so a changed list can't break it.
                            resp.dnd_set_drag_payload(it.path.clone());
                            if let Some(src) = resp.dnd_release_payload::<String>() {
                                reorder = Some(((*src).clone(), it.path.clone()));
                            }
                        }
                        if resp.secondary_clicked() {
                            st.selected = Some(it.path.clone());
                            open_menu(ctx, st, MenuTarget::Item(fence.id, it.path.clone()));
                        }
                        resp.on_hover_text_at_pointer(it.name.as_str());
                    }
                    if let Some(b) = band_rect {
                        ui.painter().rect(
                            b,
                            egui::CornerRadius::same(2),
                            Color32::from_rgba_unmultiplied(76, 194, 255, 40),
                            egui::Stroke::new(1.0_f32, Color32::from_rgb(76, 194, 255)),
                            egui::StrokeKind::Inside,
                        );
                    }
                    if let Some((moving, target)) = reorder {
                        // Look the items up by path in the shown list; if either is gone, do nothing.
                        let from = items.iter().position(|x| x.path.eq_ignore_ascii_case(&moving));
                        let to = items.iter().position(|x| x.path.eq_ignore_ascii_case(&target));
                        if let (Some(from), Some(to)) = (from, to) {
                            if from != to {
                                if let Some(tab) = fence.tabs.get_mut(sel) {
                                    tab.items.retain(|p| !p.eq_ignore_ascii_case(&moving));
                                    let pos = tab.items.iter().position(|p| p.eq_ignore_ascii_case(&target)).unwrap_or(tab.items.len());
                                    let pos = if from < to { (pos + 1).min(tab.items.len()) } else { pos };
                                    tab.items.insert(pos, moving);
                                    dirty = true;
                                }
                            }
                        }
                    }
                    if items.is_empty() {
                        let msg = if fence.auto_sort {
                            "Everything on the desktop is in a box."
                        } else {
                            "Drag files here, or right-click an icon › Move to."
                        };
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(msg).color(Color32::from_gray(150)).size(12.0));
                    }
                });
        }

        // Resize grips (right edge, bottom edge, corner). Resizing is done here too, a step at
        // a time with a redraw each step, so no unpainted (black) area ever shows.
        if !fence.locked && showing_full {
            let g = 6.0;
            let grips = [
                (Rect::from_min_max(Pos2::new(rect.max.x - 14.0, rect.max.y - 14.0), rect.max), 3u8, CursorIcon::ResizeSouthEast),
                (Rect::from_min_max(Pos2::new(rect.max.x - g, rect.min.y + 8.0), Pos2::new(rect.max.x, rect.max.y - 14.0)), 1u8, CursorIcon::ResizeHorizontal),
                (Rect::from_min_max(Pos2::new(rect.min.x + 8.0, rect.max.y - g), Pos2::new(rect.max.x - 14.0, rect.max.y)), 2u8, CursorIcon::ResizeVertical),
            ];
            for (i, (r, kind, cur)) in grips.into_iter().enumerate() {
                let resp = ui.interact(r, ui.id().with(("grip", i)), Sense::drag());
                if resp.hovered() || resp.dragged() {
                    ctx.set_cursor_icon(cur);
                }
                if resp.drag_started() {
                    if let Some(c) = desktop::cursor_points(ppp) {
                        st.drag = Some((fence.id, kind, c, screen));
                    }
                }
                if resp.dragged() {
                    if let (Some((id, k, grab, start)), Some(c)) = (st.drag, desktop::cursor_points(ppp)) {
                        if id == fence.id && k == kind {
                            let d = c - grab;
                            let mut w = if k & 1 != 0 { start.width() + d.x } else { start.width() };
                            let mut h = if k & 2 != 0 { start.height() + d.y } else { start.height() };
                            // The moving edges snap to other boxes and the screen edges (Alt: freely).
                            if !ui.input(|i| i.modifiers.alt) {
                                if let Some(s) = snap_targets(st, fence.id, ppp) {
                                    if k & 1 != 0 {
                                        w = s.snap_right(start.min.x + w, start.min.y, start.min.y + h) - start.min.x;
                                    }
                                    if k & 2 != 0 {
                                        h = s.snap_bottom(start.min.y + h, start.min.x, start.min.x + w) - start.min.y;
                                    }
                                }
                            }
                            ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(w.max(120.0), h.max(60.0))));
                            st.burst.insert(fence.id, 6);
                            ctx.request_repaint();
                        }
                    }
                }
                if resp.drag_stopped() {
                    st.drag = None;
                    st.moved_by_person.insert(fence.id);
                    st.rebind.insert(fence.id);
                }
            }
        }
    });

    (dirty, wake)
}

const COLORS: &[(&str, [u8; 3], u8)] = &[
    ("Dark", [0, 0, 0], 170),
    ("Graphite", [40, 42, 48], 200),
    ("Night blue", [16, 32, 64], 190),
    ("Forest", [18, 52, 36], 190),
    ("Plum", [52, 24, 60], 190),
    ("Ember", [70, 26, 20], 190),
    ("Light", [235, 235, 240], 90),
    ("Glass (clear)", [0, 0, 0], 30),
];

// ---- Menus -----------------------------------------------------------------------------
// Right-click menus open in their own small always-on-top window at the mouse, so they are
// never cut off by a small box. Sub-menus open as pages inside it (with a Back row), Windows
// 11 rounds its corners, and it closes when it loses focus, on Escape, or after a choice.

#[derive(Clone, PartialEq, Debug)]
pub enum MenuTarget {
    Fence(u64),
    Tab(u64, usize),
    Item(u64, String),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MenuPage {
    Main,
    View,
    Sort,
    Look,
    New,
    MoveTo,
    MoveToTabs(u64),
}

pub struct OpenMenu {
    target: MenuTarget,
    page: MenuPage,
    /// Where the person clicked, in screen points.
    at: Pos2,
    size: Vec2,
    frames: u32,
    was_focused: bool,
    rounded: bool,
}

fn open_menu(ctx: &egui::Context, st: &mut UiState, target: MenuTarget) {
    let local = ctx.input(|i| i.pointer.interact_pos().or(i.pointer.latest_pos())).unwrap_or_default();
    let origin = ctx.input(|i| i.viewport().inner_rect.map(|r| r.min)).unwrap_or_default();
    st.confirm_delete = None;
    st.menu = Some(OpenMenu {
        target,
        page: MenuPage::Main,
        at: origin + local.to_vec2(),
        size: Vec2::new(270.0, 120.0),
        frames: 0,
        was_focused: false,
        rounded: false,
    });
}

/// A full-width menu row: icon, label, optional right-side mark (✓ or ›).
fn row(ui: &mut egui::Ui, icon_char: char, text: &str, mark: Option<char>, enabled: bool) -> egui::Response {
    let h = 34.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), if enabled { Sense::click() } else { Sense::hover() });
    let p = ui.painter();
    if enabled && resp.hovered() {
        p.rect_filled(rect, egui::CornerRadius::same(4), Color32::from_white_alpha(20));
    }
    let fg = if enabled { Color32::from_gray(240) } else { Color32::from_gray(120) };
    let font = FontId::proportional(14.0);
    p.text(Pos2::new(rect.min.x + 14.0, rect.center().y), egui::Align2::LEFT_CENTER, icon_char.to_string(), FontId::proportional(16.0), fg);
    p.text(Pos2::new(rect.min.x + 44.0, rect.center().y), egui::Align2::LEFT_CENTER, text, font.clone(), fg);
    if let Some(m) = mark {
        p.text(Pos2::new(rect.max.x - 12.0, rect.center().y), egui::Align2::RIGHT_CENTER, m.to_string(), FontId::proportional(12.0), fg);
    }
    if enabled {
        resp.on_hover_cursor(CursorIcon::Default)
    } else {
        resp
    }
}

fn sub(ui: &mut egui::Ui, icon_char: char, text: &str) -> bool {
    row(ui, icon_char, text, Some(icon::CHEVRON), true).clicked()
}

fn check(ui: &mut egui::Ui, on: bool, text: &str) -> bool {
    row(ui, if on { icon::CHECK } else { ' ' }, text, None, true).clicked()
}

fn back(ui: &mut egui::Ui, page: &mut MenuPage, title: &str) {
    if row(ui, icon::BACK, title, None, true).clicked() {
        *page = MenuPage::Main;
    }
    sep(ui);
}

fn sep(ui: &mut egui::Ui) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 9.0), Sense::hover());
    ui.painter().hline(r.x_range().shrink(4.0), r.center().y, egui::Stroke::new(1.0_f32, Color32::from_white_alpha(22)));
}

/// A row that opens an inline colour picker (no pop-up: it would be cut off by the menu window).
/// The row starts with a square of the current colour. Returns true when the colour changed.
fn color_row(ui: &mut egui::Ui, id: &str, label: &str, color: &mut Color32, alpha: bool) -> bool {
    let mut changed = false;
    let mut job = egui::text::LayoutJob::default();
    job.append("\u{25A0}  ", 0.0, egui::TextFormat { font_id: FontId::proportional(16.0), color: color.to_opaque(), ..Default::default() });
    job.append(label, 0.0, egui::TextFormat { font_id: FontId::proportional(14.0), color: Color32::from_gray(225), ..Default::default() });
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.vertical(|ui| {
            egui::CollapsingHeader::new(job).id_salt(id).show(ui, |ui| {
                ui.spacing_mut().slider_width = 190.0;
                let mode = if alpha { egui::widgets::color_picker::Alpha::OnlyBlend } else { egui::widgets::color_picker::Alpha::Opaque };
                changed = egui::widgets::color_picker::color_picker_color32(ui, color, mode);
            });
        });
    });
    changed
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        ui.label(egui::RichText::new(text).size(12.0).color(Color32::from_gray(160)));
    });
}

/// The box menu. Returns (layout changed, close the menu).
#[allow(clippy::too_many_arguments)]
fn fence_menu(ui: &mut egui::Ui, fence: &mut Fence, st: &mut UiState, page: &mut MenuPage, actions: &mut Vec<Act>, ctx_for_box: &egui::Context) -> (bool, bool) {
    let mut dirty = false;
    let mut close = false;
    match *page {
        MenuPage::Main => {
            if row(ui, icon::RENAME, "Rename box", None, true).clicked() {
                st.renaming = Some((fence.id, None, fence.name.clone()));
                st.box_cmds.push((fence.id, ViewportCommand::Focus));
                close = true;
            }
            let (ic_roll, label) = if fence.expanded { (icon::UP, "Roll up") } else { (icon::DOWN, "Unroll") };
            if row(ui, ic_roll, label, None, true).clicked() {
                fence.expanded = !fence.expanded;
                st.hover_open.remove(&fence.id);
                let h = if fence.expanded { fence.rect[3] } else { ROLLED_HEIGHT };
                st.box_cmds.push((fence.id, ViewportCommand::InnerSize(Vec2::new(fence.rect[2], h))));
                st.burst.insert(fence.id, 15);
                ctx_for_box.request_repaint();
                dirty = true;
                close = true;
            }
            if row(ui, icon::LOCK, "Lock position and size", fence.locked.then_some(icon::CHECK), true).clicked() {
                fence.locked = !fence.locked;
                dirty = true;
                close = true;
            }
            sep(ui);
            if fence.widget.is_none() {
                if sub(ui, icon::VIEW, "View") {
                    *page = MenuPage::View;
                }
                if sub(ui, icon::SORT, "Sort by") {
                    *page = MenuPage::Sort;
                }
                if !fence.auto_sort && row(ui, icon::TAB, "New tab", None, true).clicked() {
                    let n = fence.tabs.len() + 1;
                    fence.tabs.push(Tab { name: format!("Tab {n}"), items: vec![] });
                    fence.sel_tab = fence.tabs.len() - 1;
                    dirty = true;
                    close = true;
                }
                if row(ui, icon::AUTO, "Collect other desktop items", fence.auto_sort.then_some(icon::CHECK), true)
                    .on_hover_text("Shows every desktop item no other box holds, sorted into tabs by kind")
                    .clicked()
                {
                    fence.auto_sort = !fence.auto_sort;
                    dirty = true;
                    close = true;
                }
            }
            if sub(ui, icon::COLOR, "Personalize") {
                *page = MenuPage::Look;
            }
            sep(ui);
            if sub(ui, icon::ADD, "New") {
                *page = MenuPage::New;
            }
            if row(ui, icon::SEARCH, "Search desktop", None, true).clicked() {
                st.open_search = true;
                close = true;
            }
            if row(ui, icon::SETTINGS, "DeskBoxes settings", None, true).clicked() {
                actions.push(Act::OpenManager);
                close = true;
            }
            sep(ui);
            if st.confirm_delete == Some(fence.id) {
                let r = row(ui, icon::DELETE, "Click again to delete (files stay)", None, true);
                if r.clicked() {
                    actions.push(Act::DeleteFence(fence.id));
                    st.confirm_delete = None;
                    close = true;
                }
            } else if row(ui, icon::DELETE, "Delete box", None, true).clicked() {
                st.confirm_delete = Some(fence.id);
            }
        }
        MenuPage::View => {
            back(ui, page, "View");
            for (s, label) in [(32, "Small icons"), (48, "Medium icons"), (96, "Large icons")] {
                if check(ui, fence.icon_size == s, label) {
                    fence.icon_size = s;
                    dirty = true;
                    close = true;
                }
            }
        }
        MenuPage::Sort => {
            back(ui, page, "Sort by");
            for (k, label) in [(SortKind::Manual, "As I arrange them"), (SortKind::Name, "Name"), (SortKind::Size, "Size"), (SortKind::Type, "Item type"), (SortKind::Modified, "Date modified"), (SortKind::OpenCount, "Times opened")] {
                if check(ui, fence.sort == k, label) {
                    fence.sort = k;
                    dirty = true;
                    close = true;
                }
            }
        }
        MenuPage::Look => {
            back(ui, page, "Personalize");
            // Taller than a menu should be: it scrolls.
            ScrollArea::vertical().max_height(540.0).auto_shrink([false, true]).show(ui, |ui| {
                heading(ui, "Colour");
                // One row of swatches; the clear-glass one is drawn as see-through (a checkerboard).
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    ui.add_space(12.0);
                    for (name, rgb, alpha) in COLORS {
                        let (r, resp) = ui.allocate_exact_size(Vec2::new(28.0, 30.0), Sense::click());
                        let p = ui.painter();
                        let c = r.center();
                        let selected = fence.color == *rgb && fence.alpha == *alpha;
                        if *alpha < 60 {
                            // Clear glass: checkerboard inside the circle.
                            p.circle_filled(c, 10.0, Color32::from_gray(200));
                            for (dx, dy) in [(-4.0, -4.0), (4.0, 4.0)] {
                                p.rect_filled(Rect::from_center_size(c + Vec2::new(dx, dy), Vec2::splat(7.0)), 0.0, Color32::from_gray(120));
                            }
                        } else {
                            p.circle_filled(c, 10.0, Color32::from_rgb(rgb[0], rgb[1], rgb[2]));
                        }
                        p.circle_stroke(c, 10.0, egui::Stroke::new(1.0_f32, Color32::from_white_alpha(70)));
                        if selected {
                            p.circle_stroke(c, 13.0, egui::Stroke::new(2.0_f32, Color32::from_rgb(76, 194, 255)));
                        } else if resp.hovered() {
                            p.circle_stroke(c, 13.0, egui::Stroke::new(1.0_f32, Color32::from_white_alpha(90)));
                        }
                        if resp.on_hover_text(*name).clicked() {
                            fence.color = *rgb;
                            fence.alpha = *alpha;
                            dirty = true;
                        }
                    }
                });
                // Any colour (with its opacity), picked by hand.
                let mut tint = Color32::from_rgba_unmultiplied(fence.color[0], fence.color[1], fence.color[2], fence.alpha);
                if color_row(ui, "box-colour", "Any colour…", &mut tint, true) {
                    let [r, g, b, a] = tint.to_srgba_unmultiplied();
                    fence.color = [r, g, b];
                    fence.alpha = a;
                    dirty = true;
                }
                sep(ui);
                // Aligned rows: name, slider, value.
                let slider_row = |ui: &mut egui::Ui, label: &str, value: String, slider: &mut dyn FnMut(&mut egui::Ui) -> bool| -> bool {
                    let mut changed = false;
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        let (lr, _) = ui.allocate_exact_size(Vec2::new(70.0, 30.0), Sense::hover());
                        ui.painter().text(Pos2::new(lr.min.x, lr.center().y), egui::Align2::LEFT_CENTER, label, FontId::proportional(14.0), Color32::from_gray(225));
                        ui.spacing_mut().slider_width = 120.0;
                        changed = slider(ui);
                        ui.add_sized([36.0, 30.0], egui::Label::new(egui::RichText::new(value).size(13.0).color(Color32::from_gray(180))).halign(Align::Max));
                    });
                    changed
                };
                let alpha_pct = format!("{}%", (fence.alpha as f32 / 2.55).round());
                dirty |= slider_row(ui, "Opacity", alpha_pct, &mut |ui| ui.add(egui::Slider::new(&mut fence.alpha, 0..=255).show_value(false)).changed());
                let corner_txt = format!("{}", fence.corner.round());
                dirty |= slider_row(ui, "Corners", corner_txt, &mut |ui| ui.add(egui::Slider::new(&mut fence.corner, 0.0..=16.0).step_by(1.0).show_value(false)).changed());
                let blur_txt = format!("{}", fence.blur);
                dirty |= slider_row(ui, "Blur", blur_txt, &mut |ui| ui.add(egui::Slider::new(&mut fence.blur, 0..=40).show_value(false)).changed());
                sep(ui);
                heading(ui, "Border");
                let bw = format!("{}", fence.border_width);
                dirty |= slider_row(ui, "Thickness", bw, &mut |ui| ui.add(egui::Slider::new(&mut fence.border_width, 0.0..=6.0).step_by(0.5).show_value(false)).changed());
                let [r, g, b, a] = fence.border_color;
                let mut border = Color32::from_rgba_unmultiplied(r, g, b, a);
                if color_row(ui, "border-colour", "Border colour…", &mut border, true) {
                    fence.border_color = border.to_srgba_unmultiplied();
                    dirty = true;
                }
                sep(ui);
                heading(ui, "Text");
                let [r, g, b] = fence.text_color;
                let mut text = Color32::from_rgb(r, g, b);
                if color_row(ui, "text-colour", "Text colour…", &mut text, false) {
                    let [r, g, b, _] = text.to_srgba_unmultiplied();
                    fence.text_color = [r, g, b];
                    dirty = true;
                }
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    dirty |= ui.checkbox(&mut fence.text_shadow, "Shadow behind the text").changed();
                });
                let ts = format!("{}", fence.title_size.round());
                dirty |= slider_row(ui, "Title size", ts, &mut |ui| ui.add(egui::Slider::new(&mut fence.title_size, 8.0..=32.0).step_by(1.0).show_value(false)).changed());
                let cs = format!("{}", fence.caption_size.round());
                dirty |= slider_row(ui, "Text size", cs, &mut |ui| ui.add(egui::Slider::new(&mut fence.caption_size, 7.0..=24.0).step_by(1.0).show_value(false)).changed());
                // Fonts: only those installed on this PC are offered.
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    ui.label(egui::RichText::new("Font").size(14.0).color(Color32::from_gray(225)));
                });
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(14.0);
                    ui.spacing_mut().item_spacing = Vec2::new(4.0, 4.0);
                    for (name, file) in fv::FONTS {
                        if !std::path::Path::new("C:/Windows/Fonts").join(file).exists() {
                            continue;
                        }
                        let is_default = *name == "Segoe UI";
                        let on = if is_default { fence.font.is_empty() } else { fence.font == *name };
                        if ui.selectable_label(on, egui::RichText::new(*name).size(12.5)).clicked() && !on {
                            fence.font = if is_default { String::new() } else { name.to_string() };
                            dirty = true;
                        }
                    }
                });
                if fence.widget.is_none() {
                    sep(ui);
                    heading(ui, "Icons");
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        dirty |= ui.checkbox(&mut fence.show_captions, "Show names under the icons").changed();
                    });
                    let is = format!("{}", fence.icon_size);
                    dirty |= slider_row(ui, "Size", is, &mut |ui| ui.add(egui::Slider::new(&mut fence.icon_size, 16..=128).step_by(4.0).show_value(false)).changed());
                    let sp = if fence.icon_spacing <= 0.0 { "auto".to_string() } else { format!("{}", fence.icon_spacing.round()) };
                    dirty |= slider_row(ui, "Spacing", sp, &mut |ui| ui.add(egui::Slider::new(&mut fence.icon_spacing, 0.0..=200.0).step_by(2.0).show_value(false)).changed());
                }
                sep(ui);
                // Two-way switches for where the title and the tabs go.
                let switch = |ui: &mut egui::Ui, label: &str, first: &str, second: &str, is_first: bool| -> Option<bool> {
                    let mut pick = None;
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        let (lr, _) = ui.allocate_exact_size(Vec2::new(70.0, 30.0), Sense::hover());
                        ui.painter().text(Pos2::new(lr.min.x, lr.center().y), egui::Align2::LEFT_CENTER, label, FontId::proportional(14.0), Color32::from_gray(225));
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for (i, text) in [first, second].into_iter().enumerate() {
                            let on = (i == 0) == is_first;
                            let fill = if on { Color32::from_rgb(76, 194, 255).linear_multiply(0.35) } else { Color32::from_white_alpha(10) };
                            let b = egui::Button::new(egui::RichText::new(text).size(13.0).color(if on { Color32::WHITE } else { Color32::from_gray(200) }))
                                .fill(fill)
                                .corner_radius(egui::CornerRadius::same(4))
                                .min_size(Vec2::new(78.0, 26.0));
                            if ui.add(b).clicked() {
                                pick = Some(i == 0);
                            }
                        }
                    });
                    pick
                };
                if let Some(left) = switch(ui, "Title", "Left side", "Top", fence.title_pos == TitlePos::Left) {
                    fence.title_pos = if left { TitlePos::Left } else { TitlePos::Top };
                    dirty = true;
                }
                if let Some(right) = switch(ui, "Tabs", "Right side", "Top", fence.tab_pos == TabPos::Right) {
                    fence.tab_pos = if right { TabPos::Right } else { TabPos::Top };
                    dirty = true;
                }
                sep(ui);
                if row(ui, icon::COLOR, "Apply this look to all boxes", None, true).clicked() {
                    actions.push(Act::ApplyLookToAll(fence.id));
                    close = true;
                }
                ui.add_space(4.0);
            });
        }
        MenuPage::New => {
            back(ui, page, "New");
            if row(ui, icon::VIEW, "Box", None, true).clicked() {
                actions.push(Act::NewFence { auto: false });
                close = true;
            }
            sep(ui);
            for k in widgets::REGISTRY {
                if row(ui, k.icon, &format!("{} widget", k.label), None, true).clicked() {
                    actions.push(Act::NewWidget(k.id));
                    close = true;
                }
            }
        }
        MenuPage::MoveTo | MenuPage::MoveToTabs(_) => *page = MenuPage::Main,
    }
    (dirty, close)
}

fn tab_menu(ui: &mut egui::Ui, fence: &mut Fence, tab: usize, st: &mut UiState, _ctx_for_box: &egui::Context) -> (bool, bool) {
    let mut dirty = false;
    let mut close = false;
    if row(ui, icon::RENAME, "Rename tab", None, true).clicked() {
        if let Some(t) = fence.tabs.get(tab) {
            st.renaming = Some((fence.id, Some(tab), t.name.clone()));
            st.box_cmds.push((fence.id, ViewportCommand::Focus));
        }
        close = true;
    }
    if row(ui, icon::TAB, "New tab", None, true).clicked() {
        let n = fence.tabs.len() + 1;
        fence.tabs.push(Tab { name: format!("Tab {n}"), items: vec![] });
        fence.sel_tab = fence.tabs.len() - 1;
        dirty = true;
        close = true;
    }
    let can_delete = fence.tabs.len() > 1;
    if row(ui, icon::DELETE, "Delete tab", None, can_delete).on_hover_text("Its items go back to the desktop").clicked() && can_delete {
        if tab < fence.tabs.len() {
            fence.tabs.remove(tab);
        }
        fence.sel_tab = 0;
        dirty = true;
        close = true;
    }
    (dirty, close)
}

/// The icon menu. Returns close.
fn item_menu(ui: &mut egui::Ui, from: u64, from_auto: bool, it: &DeskItem, targets: &[(u64, String, Vec<String>)], page: &mut MenuPage, actions: &mut Vec<Act>) -> bool {
    let mut close = false;
    match *page {
        MenuPage::MoveTo => {
            back(ui, page, "Move to");
            for (id, name, tabs) in targets {
                if tabs.len() > 1 {
                    if sub(ui, icon::VIEW, name) {
                        *page = MenuPage::MoveToTabs(*id);
                    }
                } else if row(ui, icon::VIEW, name, None, *id != from || from_auto).clicked() {
                    actions.push(Act::MoveItem { path: it.path.clone(), to: *id, tab: 0 });
                    close = true;
                }
            }
            sep(ui);
            if row(ui, icon::ADD, "New box", None, true).clicked() {
                actions.push(Act::MoveToNewFence { path: it.path.clone() });
                close = true;
            }
        }
        MenuPage::MoveToTabs(id) => {
            if row(ui, icon::BACK, "Move to", None, true).clicked() {
                *page = MenuPage::MoveTo;
            }
            sep(ui);
            if let Some((_, _, tabs)) = targets.iter().find(|t| t.0 == id) {
                for (ti, tn) in tabs.iter().enumerate() {
                    if row(ui, icon::TAB, tn, None, true).clicked() {
                        actions.push(Act::MoveItem { path: it.path.clone(), to: id, tab: ti });
                        close = true;
                    }
                }
            }
        }
        _ => {
            let r = row(ui, icon::OPEN, "Open", None, true);
            if r.clicked() {
                desktop::open_item(&it.path);
                actions.push(Act::Opened(it.path.clone()));
                close = true;
            }
            if row(ui, icon::FOLDER, "Open file location", None, true).clicked() {
                desktop::show_in_folder(&it.path);
                close = true;
            }
            sep(ui);
            if sub(ui, icon::MOVE, "Move to") {
                *page = MenuPage::MoveTo;
            }
            if !from_auto && row(ui, icon::REMOVE, "Remove from this box", None, true).clicked() {
                // Not deleted: it goes back to the desktop / auto-sort box.
                actions.push(Act::MoveItem { path: it.path.clone(), to: 0, tab: 0 });
                close = true;
            }
        }
    }
    close
}

impl DeskApp {
    fn show_menu(&mut self, ctx: &egui::Context, targets: &[(u64, String, Vec<String>)]) {
        let Some(mut m) = self.ui.menu.take() else { return };
        // Keep the menu inside the screen: open upward/leftward near the edges. (Menu points
        // are box points: pixels / the display scale.)
        let work = desktop::work_area_at(m.at, desktop::system_scale());
        let mut pos = m.at + Vec2::new(2.0, 2.0);
        if pos.x + m.size.x > work.max.x {
            pos.x = (m.at.x - m.size.x).max(work.min.x);
        }
        if pos.y + m.size.y > work.max.y {
            pos.y = (m.at.y - m.size.y).max(work.min.y);
        }
        let mut close = false;
        let mut dirty = false;
        let mut measured = m.size;
        let root_ctx = ctx.clone();
        // The menu is a DirectX window drawn like the boxes.
        #[cfg(windows)]
        if let Some(mut hosts) = self.hosts.take() {
            let s = desktop::system_scale();
            let rect_px = [(pos.x * s).round() as i32, (pos.y * s).round() as i32, (m.size.x * s).round() as i32, (m.size.y * s).round() as i32];
            let force = self.panel_force;
            hosts.panel(MENU_PANEL, crate::host::PanelKind::Menu, "DeskBoxes menu", rect_px, true, force, |pctx| {
                self.menu_body(pctx, &mut m, targets, &root_ctx, &mut close, &mut dirty, &mut measured);
            });
            if close {
                hosts.close_panel(MENU_PANEL);
            }
            self.hosts = Some(hosts);
        }
        if (measured.y - m.size.y).abs() > 1.0 {
            m.size = measured;
            ctx.request_repaint();
        }
        if dirty {
            self.mark_dirty();
        }
        if !close {
            self.ui.menu = Some(m);
        }
    }

    /// The menu's contents (same for the DirectX and the GL window).
    #[allow(clippy::too_many_arguments)]
    fn menu_body(
        &mut self,
        ctx: &egui::Context,
        m: &mut OpenMenu,
        targets: &[(u64, String, Vec<String>)],
        root_ctx: &egui::Context,
        close: &mut bool,
        dirty: &mut bool,
        measured: &mut Vec2,
    ) {
        {
            m.frames += 1;
            if m.frames == 1 {
                ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
            if !m.rounded && m.frames >= 2 {
                m.rounded = desktop::round_window("DeskBoxes menu", 8.0) || m.frames > 30;
            }
            match ctx.input(|i| i.viewport().focused) {
                Some(true) => m.was_focused = true,
                Some(false) if m.was_focused => *close = true,
                _ => {}
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.viewport().close_requested()) {
                *close = true;
            }
            let frame = egui::Frame::new()
                .fill(Color32::from_rgb(44, 44, 46))
                .stroke(egui::Stroke::new(1.0_f32, Color32::from_white_alpha(26)))
                .inner_margin(egui::Margin::same(5));
            egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.set_width(m.size.x - 12.0);
                let top = ui.cursor().min.y;
                match m.target.clone() {
                    MenuTarget::Fence(id) => {
                        let Layout { fences, .. } = &mut self.layout;
                        if let Some(f) = fences.iter_mut().find(|f| f.id == id) {
                            let (d, c) = fence_menu(ui, f, &mut self.ui, &mut m.page, &mut self.actions, root_ctx);
                            *dirty |= d;
                            *close |= c;
                        } else {
                            *close = true;
                        }
                    }
                    MenuTarget::Tab(id, t) => {
                        if let Some(f) = self.layout.fences.iter_mut().find(|f| f.id == id) {
                            let (d, c) = tab_menu(ui, f, t, &mut self.ui, root_ctx);
                            *dirty |= d;
                            *close |= c;
                        } else {
                            *close = true;
                        }
                    }
                    MenuTarget::Item(id, path) => {
                        let from_auto = self.layout.fences.iter().find(|f| f.id == id).map(|f| f.auto_sort).unwrap_or(false);
                        if let Some(it) = self.items.get(&path.to_lowercase()).cloned() {
                            *close |= item_menu(ui, id, from_auto, &it, targets, &mut m.page, &mut self.actions);
                        } else {
                            *close = true;
                        }
                    }
                }
                let h = ui.cursor().min.y - top + 12.0;
                *measured = Vec2::new(m.size.x, h.max(40.0));
            });
        }
    }
}

/// Panel ids for the menu and the settings window (box ids are small numbers).
#[cfg(windows)]
const MENU_PANEL: u64 = u64::MAX - 1;
#[cfg(windows)]
const SETTINGS_PANEL: u64 = u64::MAX - 2;

// ============================================================================================
// Settings window
// ============================================================================================

impl DeskApp {
    fn show_manager(&mut self, ctx: &egui::Context) {
        if self.screenshot {
            // Check picture: the run loop draws the root context off-screen (see runloop.rs).
            self.manager_contents(ctx);
            return;
        }
        let mut close = false;
        // The settings window is a DirectX window drawn like the boxes.
        #[cfg(windows)]
        if let Some(mut hosts) = self.hosts.take() {
            let s = desktop::system_scale();
            let [sw, sh] = desktop::primary_screen_points();
            let (w, h) = (1100.0_f32.min(sw - 40.0), 680.0_f32.min(sh - 80.0));
            let (x, y) = (((sw - w) / 2.0).max(0.0), ((sh - h) / 2.0).max(0.0));
            let rect_px = [(x * s) as i32, (y * s) as i32, (w * s) as i32, (h * s) as i32];
            let force = self.panel_force;
            hosts.panel(SETTINGS_PANEL, crate::host::PanelKind::Settings, "DeskBoxes", rect_px, false, force, |pctx| {
                if pctx.input(|i| i.viewport().close_requested()) {
                    close = true;
                }
                self.manager_contents(pctx);
            });
            if std::mem::take(&mut self.focus_settings) {
                hosts.apply(SETTINGS_PANEL, &ViewportCommand::Focus);
            }
            if close {
                hosts.close_panel(SETTINGS_PANEL);
                self.manager_open = false;
            }
            self.hosts = Some(hosts);
        }
    }

    fn manager_contents(&mut self, ctx: &egui::Context) {
        {
            egui::TopBottomPanel::top("mgr-top").frame(egui::Frame::new().fill(Color32::from_rgb(24, 25, 28)).inner_margin(10.0)).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("DeskBoxes").size(20.0).strong().color(Color32::WHITE));
                    ui.add_space(20.0);
                    ui.selectable_value(&mut self.page, Page::Boxes, "Boxes");
                    ui.selectable_value(&mut self.page, Page::Layout, "Layout");
                    ui.selectable_value(&mut self.page, Page::Settings, "Settings");
                });
            });
            egui::CentralPanel::default().frame(egui::Frame::new().fill(Color32::from_rgb(16, 17, 19)).inner_margin(12.0)).show(ctx, |ui| {
                match self.page {
                    Page::Boxes => self.page_boxes(ui),
                    Page::Layout => self.page_layout(ui),
                    Page::Settings => {
                        // Longer than the window: scroll.
                        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.page_settings(ui));
                    }
                }
            });
        }
    }

    fn page_boxes(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("➕ New box").clicked() {
                self.actions.push(Act::NewFence { auto: false });
            }
            if ui.button("➕ New auto-sort box").on_hover_text("Collects every desktop item no other box holds, sorted into tabs by kind").clicked() {
                self.actions.push(Act::NewFence { auto: true });
            }
            ui.separator();
            ui.label("Widgets:");
            for k in widgets::REGISTRY {
                if ui.button(format!("➕ {}", k.label)).clicked() {
                    self.actions.push(Act::NewWidget(k.id));
                }
            }
        });
        ui.add_space(8.0);
        let claimed: HashSet<String> = self
            .layout
            .fences
            .iter()
            .filter(|f| !f.auto_sort)
            .flat_map(|f| f.tabs.iter().flat_map(|t| t.items.iter().map(|p| p.to_lowercase())))
            .collect();
        let unsorted = self.items.iter().filter(|(k, v)| v.on_desktop && !claimed.contains(*k)).count();
        ui.label(
            egui::RichText::new(format!(
                "{} items on the desktop, {} not in a box yet. Tip: right-click a box for its options; double-click its title to roll it up.",
                self.items.values().filter(|v| v.on_desktop).count(),
                unsorted
            ))
            .color(Color32::from_gray(170)),
        );
        ui.add_space(8.0);
        let mut dirty = false;
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("boxes").num_columns(5).spacing([16.0, 8.0]).striped(true).show(ui, |ui| {
                ui.strong("Name");
                ui.strong("Kind");
                ui.strong("Contents");
                ui.strong("State");
                ui.strong("");
                ui.end_row();
                for f in self.layout.fences.iter_mut() {
                    // A fixed size: in a grid, a text field otherwise keeps the column's first
                    // (tiny) width forever.
                    dirty |= ui.add_sized([200.0, 26.0], egui::TextEdit::singleline(&mut f.name)).changed();
                    let kind = match &f.widget {
                        Some(w) => widgets::find(&w.kind).map(|k| k.label).unwrap_or("Widget").to_string(),
                        None if f.auto_sort => "Auto-sort box".into(),
                        None => "Box".into(),
                    };
                    ui.label(kind);
                    if f.widget.is_some() {
                        ui.label("—");
                    } else if f.auto_sort {
                        ui.label(format!("{unsorted} items"));
                    } else {
                        let n: usize = f.tabs.iter().map(|t| t.items.len()).sum();
                        ui.label(format!("{n} items in {} tab(s)", f.tabs.len()));
                    }
                    ui.horizontal(|ui| {
                        if ui.selectable_label(!f.expanded, "Rolled up").clicked() {
                            f.expanded = !f.expanded;
                            let h = if f.expanded { f.rect[3] } else { ROLLED_HEIGHT };
                            self.ui.box_cmds.push((f.id, ViewportCommand::InnerSize(Vec2::new(f.rect[2], h))));
                            dirty = true;
                        }
                        dirty |= ui.checkbox(&mut f.locked, "Locked").changed();
                    });
                    if self.ui.confirm_delete == Some(f.id) {
                        if ui.button(egui::RichText::new("Confirm delete").color(Color32::from_rgb(255, 120, 110))).clicked() {
                            self.actions.push(Act::DeleteFence(f.id));
                            self.ui.confirm_delete = None;
                        }
                    } else if ui.button("Delete").clicked() {
                        self.ui.confirm_delete = Some(f.id);
                    }
                    ui.end_row();
                }
            });
        });
        if dirty {
            self.mark_dirty();
        }
    }

    fn page_layout(&mut self, ui: &mut egui::Ui) {
        // Show the main monitor, widened to include any box sitting on another monitor.
        let [sw, sh] = desktop::primary_screen_points();
        let mut screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(sw, sh));
        for f in &self.layout.fences {
            screen = screen.union(Rect::from_min_size(Pos2::new(f.rect[0], f.rect[1]), Vec2::new(f.rect[2], f.rect[3])));
        }
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Your desktop, as the boxes sit on it").color(Color32::from_gray(180)));
            if self.persist && ui.button("Bring every box back on screen").clicked() {
                let main = Rect::from_min_size(Pos2::ZERO, Vec2::new(sw, sh));
                for f in &mut self.layout.fences {
                    f.rect[2] = f.rect[2].min(main.width());
                    f.rect[3] = f.rect[3].min(main.height());
                    f.rect[0] = f.rect[0].clamp(main.min.x, (main.max.x - f.rect[2]).max(main.min.x));
                    f.rect[1] = f.rect[1].clamp(main.min.y, (main.max.y - f.rect[3]).max(main.min.y));
                }
                self.place_all_boxes();
                self.mark_dirty();
            }
        });
        ui.add_space(6.0);
        let avail = ui.available_rect_before_wrap();
        let scale = (avail.width() / screen.width()).min(avail.height() / screen.height()).min(1.0);
        let area = Rect::from_min_size(avail.min, screen.size() * scale);
        ui.allocate_rect(area, Sense::hover());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, Color32::from_rgb(20, 24, 32));
        let claimed: HashSet<String> = self
            .layout
            .fences
            .iter()
            .filter(|f| !f.auto_sort && f.widget.is_none())
            .flat_map(|f| f.tabs.iter().flat_map(|t| t.items.iter().map(|p| p.to_lowercase())))
            .collect();
        let desk = DeskState { items: &self.items, claimed: &claimed, spacing: self.spacing, rules: &self.layout.settings.categories, open_counts: &self.layout.open_counts };
        for f in &self.layout.fences {
            let h = if f.expanded { f.rect[3] } else { ROLLED_HEIGHT };
            let r = Rect::from_min_size(
                area.min + (Pos2::new(f.rect[0], f.rect[1]) - screen.min) * scale,
                Vec2::new(f.rect[2], h) * scale,
            );
            // The settings window is drawn by the same egui context as the boxes (DirectX) or as
            // the classic boxes (GL), so the boxes' icon cache serves the preview either way.
            fv::paint_static(&painter, &mut self.icons, f, &desk, r, scale);
        }
    }

    fn page_settings(&mut self, ui: &mut egui::Ui) {
        let s = &mut self.layout.settings;
        let mut dirty = false;
        let mut icons_changed = false;
        ui.heading("Desktop");
        if ui.checkbox(&mut s.hide_desktop_icons, "Hide Windows' own desktop icons (the boxes show them)").changed() {
            dirty = true;
            icons_changed = true;
        }
        ui.label(egui::RichText::new("They always come back when DeskBoxes closes, even after a crash.").color(Color32::from_gray(150)).size(12.0));
        dirty |= ui.checkbox(&mut s.expand_on_hover, "Open rolled-up boxes while the mouse is over them").changed();
        ui.horizontal(|ui| {
            dirty |= ui.checkbox(&mut s.snap, "Snap boxes to each other and to the screen edges").changed();
            ui.label("gap");
            dirty |= ui.add(egui::DragValue::new(&mut s.snap_gap).range(0.0..=40.0).speed(0.5).suffix(" pt")).changed();
        });
        ui.horizontal(|ui| {
            dirty |= ui.checkbox(&mut s.snap_grid, "Also snap to a grid").changed();
            ui.label("size");
            dirty |= ui.add(egui::DragValue::new(&mut s.grid_size).range(4.0..=200.0).speed(1.0).suffix(" pt")).changed();
        });
        ui.label(egui::RichText::new("Hold Alt while moving or resizing a box to place it freely.").color(Color32::from_gray(150)).size(12.0));
        #[cfg(windows)]
        ui.horizontal_wrapped(|ui| {
            ui.label("Quick search shortcut:");
            for (i, (label, _, _)) in crate::runloop::SEARCH_HOTKEYS.iter().enumerate() {
                if ui.selectable_label(s.search_hotkey == i, *label).clicked() && s.search_hotkey != i {
                    s.search_hotkey = i;
                    dirty = true;
                }
            }
        });
        #[cfg(windows)]
        if !self.hotkey_ok {
            ui.label(egui::RichText::new("That shortcut is already used by another program: pick another one.").color(Color32::from_rgb(255, 160, 120)).size(12.0));
        }
        ui.label(egui::RichText::new("Frosted glass: set each box's blur in its menu (Personalize > Blur).").color(Color32::from_gray(150)).size(12.0));
        ui.add_space(10.0);
        ui.heading("Monitors");
        let before = s.monitor_mode;
        ui.radio_value(
            &mut s.monitor_mode,
            crate::model::MonitorMode::Role,
            "Boxes follow the monitor's role: if you change the main monitor, boxes move with it",
        );
        ui.radio_value(
            &mut s.monitor_mode,
            crate::model::MonitorMode::Fixed,
            "Boxes stay on the same physical monitor, whatever its role",
        );
        let monitors_changed = s.monitor_mode != before;
        dirty |= monitors_changed;
        for (i, m) in self.monitors.iter().enumerate() {
            let role = match i {
                0 => "Main monitor".to_string(),
                1 => "Second monitor".to_string(),
                n => format!("Monitor {}", n + 1),
            };
            let count = self
                .layout
                .fences
                .iter()
                .filter(|f| match s.monitor_mode {
                    crate::model::MonitorMode::Role => f.monitor_role == i,
                    crate::model::MonitorMode::Fixed => f.monitor_name == m.name,
                })
                .count();
            ui.label(
                egui::RichText::new(format!(
                    "{role}: {} × {} ({}) — {count} box(es)",
                    m.rect.width().round(),
                    m.rect.height().round(),
                    m.name.trim_start_matches("\\\\.\\")
                ))
                .color(Color32::from_gray(160))
                .size(12.0),
            );
        }
        ui.add_space(10.0);
        ui.heading("Auto-sort tabs");
        ui.label(
            egui::RichText::new("Each tab collects these file types (folders and everything else get their own tabs).")
                .color(Color32::from_gray(150))
                .size(12.0),
        );
        let mut remove = None;
        egui::Grid::new("cats").num_columns(3).spacing([10.0, 6.0]).show(ui, |ui| {
            for (i, r) in s.categories.iter_mut().enumerate() {
                dirty |= ui.add_sized([120.0, 26.0], egui::TextEdit::singleline(&mut r.name)).changed();
                let mut exts = r.exts.join(", ");
                if ui.add_sized([420.0, 26.0], egui::TextEdit::singleline(&mut exts).hint_text("zip, rar, 7z")).changed() {
                    r.exts = exts
                        .split([',', ' ', ';'])
                        .map(|e| e.trim().trim_start_matches('.').to_lowercase())
                        .filter(|e| !e.is_empty())
                        .collect();
                    dirty = true;
                }
                if ui.small_button("Remove").clicked() {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some(i) = remove {
            s.categories.remove(i);
            dirty = true;
        }
        ui.horizontal(|ui| {
            if ui.button("Add tab rule").clicked() {
                s.categories.push(crate::model::CategoryRule { name: "New tab".into(), exts: vec![] });
                dirty = true;
            }
            if ui.button("Reset to defaults").clicked() {
                s.categories = crate::model::default_categories();
                dirty = true;
            }
        });
        ui.add_space(10.0);
        ui.heading("Computer");
        if ui.checkbox(&mut s.start_with_windows, "Start DeskBoxes when Windows starts").changed() {
            if self.persist {
                desktop::set_start_with_windows(s.start_with_windows);
            }
            dirty = true;
        }
        if ui.checkbox(&mut s.low_priority, "Run at low priority (never slows other programs)").changed() {
            if self.persist {
                desktop::set_low_priority(s.low_priority);
            }
            dirty = true;
        }
        ui.horizontal(|ui| {
            ui.label("Check the desktop for new files every");
            dirty |= ui.add(egui::DragValue::new(&mut s.rescan_secs).range(2..=60).suffix(" s")).changed();
            ui.label(egui::RichText::new("(applies after restart)").color(Color32::from_gray(150)));
        });
        ui.add_space(10.0);
        ui.heading("Saved layouts");
        ui.label(egui::RichText::new("A copy of your layout is kept each time DeskBoxes starts (the last 10).").color(Color32::from_gray(150)).size(12.0));
        let mut restore = None;
        for b in self.backups.iter().rev().take(10) {
            ui.horizontal(|ui| {
                ui.label(b.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
                if self.persist && ui.small_button("Restore").clicked() {
                    restore = Some(b.clone());
                }
            });
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Open data folder").clicked() {
                desktop::open_folder(&storage::data_dir());
            }
            if self.persist && crate::import_original::original_available()
                && ui.button("Import boxes from the original app").on_hover_text("Replaces your boxes with the ones the original organizer saved (your current layout is kept as a backup above).").clicked()
            {
                self.actions.push(Act::ImportOriginal);
            }
            if self.persist && ui.button("Quit DeskBoxes").clicked() {
                self.actions.push(Act::Quit);
            }
        });
        ui.add_space(6.0);
        ui.label(egui::RichText::new(format!("{} desktop items", self.items.len())).color(Color32::from_gray(110)).size(11.0));
        if let Some(p) = restore {
            self.actions.push(Act::RestoreBackup(p));
        }
        if dirty {
            self.mark_dirty();
        }
        if icons_changed {
            self.apply_icon_hiding();
        }
        if monitors_changed && self.persist {
            self.relayout_monitors();
        }
    }
}

// ============================================================================================
// Helpers
// ============================================================================================

/// First run: one box for shortcuts/apps, one auto-sort box for everything else, and a clock.
fn default_layout(items: &HashMap<String, DeskItem>) -> Layout {
    let mut layout = Layout { version: 1, next_id: 1, ..Default::default() };
    let mut apps: Vec<&DeskItem> = items.values().filter(|i| i.on_desktop && i.category == Category::Apps).collect();
    apps.sort_by_key(|i| i.name.to_lowercase());
    // Proportions follow the original's saved layout picture: wide boxes across the top,
    // a margin of about 2% around them.
    let [sw, sh] = desktop::primary_screen_points();
    let m = (sw * 0.022).round();
    let top_h = (sh * 0.36).round();
    let apps_w = (sw * 0.62).round();
    let id = layout.new_id();
    layout.fences.push(Fence {
        id,
        name: "Apps".into(),
        rect: [m, m, apps_w, top_h],
        tabs: vec![Tab { name: "Apps".into(), items: apps.iter().map(|i| i.path.clone()).collect() }],
        ..Fence::default()
    });
    let id = layout.new_id();
    layout.fences.push(Fence {
        id,
        name: "Desktop".into(),
        rect: [m * 2.0 + apps_w, m, sw - apps_w - m * 3.0, top_h],
        auto_sort: true,
        ..Fence::default()
    });
    let id = layout.new_id();
    layout.fences.push(Fence {
        id,
        name: "Clock".into(),
        rect: [m, m * 2.0 + top_h, 260.0, 130.0],
        title_pos: TitlePos::Top,
        widget: Some(WidgetConfig { kind: "clock".into(), data: serde_json::json!({}) }),
        ..Fence::default()
    });
    let id = layout.new_id();
    layout.fences.push(Fence {
        id,
        name: "Note".into(),
        rect: [m * 2.0 + 260.0, m * 2.0 + top_h, 320.0, 200.0],
        title_pos: TitlePos::Top,
        widget: Some(WidgetConfig {
            kind: "note".into(),
            data: serde_json::json!({
                "text": "Welcome to DeskBoxes!\n• Drag a box by its title, resize from its edges.\n• Double-click a title to roll the box up.\n• Right-click a box or an icon for options."
            }),
        }),
        ..Fence::default()
    });
    layout
}

/// Records which monitor a box sits on (the one holding its centre, else the nearest) and its
/// offset from that monitor's corner.
fn bind_to_monitor(f: &mut Fence, monitors: &[desktop::Monitor]) {
    if monitors.is_empty() {
        return;
    }
    let r = Rect::from_min_size(Pos2::new(f.rect[0], f.rect[1]), Vec2::new(f.rect[2], f.rect[3].max(ROLLED_HEIGHT)));
    let c = r.center();
    let idx = monitors
        .iter()
        .position(|m| m.rect.contains(c))
        .unwrap_or_else(|| {
            monitors
                .iter()
                .enumerate()
                .min_by(|a, b| {
                    let da = a.1.rect.distance_sq_to_pos(c);
                    let db = b.1.rect.distance_sq_to_pos(c);
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })
                .map(|(i, _)| i)
                .unwrap_or(0)
        });
    let m = &monitors[idx];
    f.monitor_role = idx;
    f.monitor_name = m.name.clone();
    f.monitor_offset = [f.rect[0] - m.rect.min.x, f.rect[1] - m.rect.min.y];
}

impl DeskApp {
    fn bind_unassigned(&mut self) {
        let mut changed = false;
        for f in self.layout.fences.iter_mut().filter(|f| f.monitor_name.is_empty()) {
            bind_to_monitor(f, &self.monitors);
            changed = true;
        }
        if changed {
            self.mark_dirty();
        }
    }

    /// Puts every box on its monitor: by role (main, second, ...) or on the same physical
    /// display, depending on the setting. A box whose display is gone waits on the main one
    /// and returns when the display comes back (its assignment is kept).
    fn relayout_monitors(&mut self) {
        if self.monitors.is_empty() {
            return;
        }
        let mode = self.layout.settings.monitor_mode;
        let mut changed = false;
        for f in &mut self.layout.fences {
            let target = match mode {
                crate::model::MonitorMode::Role => self.monitors.get(f.monitor_role).or(self.monitors.first()),
                crate::model::MonitorMode::Fixed => self
                    .monitors
                    .iter()
                    .find(|m| m.name == f.monitor_name)
                    .or(self.monitors.first()),
            };
            let Some(m) = target else { continue };
            let w = f.rect[2].min(m.rect.width());
            let h = (if f.expanded { f.rect[3] } else { ROLLED_HEIGHT }).min(m.rect.height());
            let x = (m.rect.min.x + f.monitor_offset[0]).clamp(m.rect.min.x, (m.rect.max.x - w).max(m.rect.min.x));
            let y = (m.rect.min.y + f.monitor_offset[1]).clamp(m.rect.min.y, (m.rect.max.y - h).max(m.rect.min.y));
            if (x - f.rect[0]).abs() > 0.5 || (y - f.rect[1]).abs() > 0.5 || (w - f.rect[2]).abs() > 0.5 {
                f.rect[0] = x;
                f.rect[1] = y;
                f.rect[2] = w;
                // Move the box window there.
                self.ui.box_cmds.push((f.id, ViewportCommand::OuterPosition(Pos2::new(x, y))));
                self.ui.box_cmds.push((f.id, ViewportCommand::InnerSize(Vec2::new(w, h))));
                changed = true;
            }
        }
        if changed {
            self.mark_dirty();
        }
    }
}

/// Repairs anything odd in a loaded layout so drawing can never fail on it.
fn sanitize(layout: &mut Layout) {
    // Version 2: boxes still on the first build's guessed look (opacity 170, 6 px corners)
    // move to the defaults read from the original (opacity 50, see model.rs).
    if layout.version < 2 {
        for f in &mut layout.fences {
            if f.alpha == 170 && (f.corner - 6.0).abs() < 0.01 && f.color == [0, 0, 0] {
                f.alpha = Fence::default().alpha;
                f.corner = Fence::default().corner;
            }
            if f.icon_size == 64 {
                f.icon_size = 48;
            }
        }
        layout.version = 2;
    }
    // Version 3: the person asked that the normal desktop icons disappear while DeskBoxes runs.
    if layout.version < 3 {
        layout.settings.hide_desktop_icons = true;
        layout.version = 3;
    }
    let vs = desktop::virtual_screen();
    let mut seen = HashSet::new();
    for f in &mut layout.fences {
        if f.id == 0 || !seen.insert(f.id) {
            f.id = layout.next_id.max(1) + 1;
            layout.next_id = f.id;
            seen.insert(f.id);
        }
        layout.next_id = layout.next_id.max(f.id);
        if f.tabs.is_empty() {
            f.tabs.push(Tab { name: "Main".into(), items: vec![] });
        }
        f.sel_tab = f.sel_tab.min(f.tabs.len() - 1);
        f.rect[2] = if f.rect[2].is_finite() { f.rect[2].clamp(120.0, 4000.0) } else { 400.0 };
        f.rect[3] = if f.rect[3].is_finite() { f.rect[3].clamp(ROLLED_HEIGHT, 3000.0) } else { 260.0 };
        // A box far outside every monitor (e.g. a screen was unplugged) comes back.
        let off = !f.rect[0].is_finite()
            || !f.rect[1].is_finite()
            || f.rect[0] + f.rect[2] < vs[0] - 50.0
            || f.rect[1] + 20.0 < vs[1] - 50.0
            || f.rect[0] > vs[0] + vs[2]
            || f.rect[1] > vs[1] + vs[3];
        if off {
            f.rect[0] = 60.0;
            f.rect[1] = 60.0;
        }
        f.icon_size = f.icon_size.clamp(16, 256);
        f.corner = f.corner.clamp(0.0, 40.0);
        let fix = |v: f32, lo: f32, hi: f32, def: f32| if v.is_finite() { v.clamp(lo, hi) } else { def };
        f.border_width = fix(f.border_width, 0.0, 6.0, 1.0);
        f.title_size = fix(f.title_size, 8.0, 32.0, 13.0);
        f.caption_size = fix(f.caption_size, 7.0, 24.0, 12.0);
        f.icon_spacing = fix(f.icon_spacing, 0.0, 300.0, 0.0);
        if !f.font.is_empty() && !fv::FONTS.iter().any(|(n, _)| *n == f.font) {
            f.font.clear();
        }
        for t in &mut f.tabs {
            let mut seen_items = HashSet::new();
            t.items.retain(|p| !p.is_empty() && seen_items.insert(p.to_lowercase()));
        }
    }
}

/// Fonts and the Windows 11 dark look for a drawing context. `extra_fonts`: the fonts boxes use
/// (names from `fence_view::FONTS`), each loaded from the Windows Fonts folder as its own family.
fn setup_style(ctx: &egui::Context, extra_fonts: &[String]) {
    let mut fonts = egui::FontDefinitions::default();
    // Use Windows' own UI font when present, for the same look as desktop captions.
    if let Ok(bytes) = std::fs::read("C:\\Windows\\Fonts\\segoeui.ttf") {
        fonts.font_data.insert("segoe".into(), Arc::new(egui::FontData::from_owned(bytes)));
        if let Some(f) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
            f.insert(0, "segoe".into());
        }
    }
    // Windows 11's line icons (Segoe Fluent Icons; Windows 10's MDL2 font as a fallback),
    // used as a fallback font so menu labels can carry the same icons as system menus.
    for file in ["C:\\Windows\\Fonts\\SegoeIcons.ttf", "C:\\Windows\\Fonts\\segmdl2.ttf"] {
        if let Ok(bytes) = std::fs::read(file) {
            fonts.font_data.insert("win-icons".into(), Arc::new(egui::FontData::from_owned(bytes)));
            if let Some(f) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                f.push("win-icons".into());
            }
            break;
        }
    }
    // The fonts chosen in boxes (Personalize > Font): each falls back to the default fonts for
    // characters it lacks (icons, other scripts).
    let fallback: Vec<String> = fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();
    for name in extra_fonts {
        let Some((_, file)) = fv::FONTS.iter().find(|(n, _)| *n == name.as_str()) else { continue };
        if let Ok(bytes) = std::fs::read(std::path::Path::new("C:/Windows/Fonts").join(file)) {
            fonts.font_data.insert(name.clone(), Arc::new(egui::FontData::from_owned(bytes)));
            let mut chain = vec![name.clone()];
            chain.extend(fallback.iter().cloned());
            fonts.families.insert(egui::FontFamily::Name(name.as_str().into()), chain);
        }
    }
    ctx.set_fonts(fonts);
    ctx.set_visuals(egui::Visuals::dark());
    ctx.style_mut(|s| {
        use egui::{CornerRadius, Margin, Shadow, Stroke};
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(14.0));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(14.0));
        s.text_styles.insert(egui::TextStyle::Heading, FontId::proportional(20.0));
        s.interaction.tooltip_delay = 0.8;
        // Windows 11 dark look: rounded menus with a soft shadow, roomy rows, quiet hover.
        s.spacing.button_padding = Vec2::new(10.0, 7.0);
        s.spacing.item_spacing = Vec2::new(8.0, 4.0);
        s.spacing.menu_margin = Margin::same(5);
        s.spacing.menu_width = 250.0;
        s.spacing.interact_size.y = 30.0;
        let v = &mut s.visuals;
        let accent = Color32::from_rgb(76, 194, 255); // Windows 11 dark-mode accent blue
        v.window_fill = Color32::from_rgb(44, 44, 46);
        v.panel_fill = Color32::from_rgb(32, 32, 32);
        v.extreme_bg_color = Color32::from_rgb(28, 28, 28);
        v.window_corner_radius = CornerRadius::same(8);
        v.menu_corner_radius = CornerRadius::same(8);
        v.window_stroke = Stroke::new(1.0_f32, Color32::from_white_alpha(26));
        v.popup_shadow = Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(120) };
        v.window_shadow = Shadow { offset: [0, 10], blur: 32, spread: 0, color: Color32::from_black_alpha(110) };
        v.selection.bg_fill = accent.linear_multiply(0.45);
        v.selection.stroke = Stroke::new(1.0_f32, Color32::WHITE);
        v.hyperlink_color = accent;
        v.slider_trailing_fill = true;
        for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open, &mut v.widgets.noninteractive] {
            w.corner_radius = CornerRadius::same(4);
        }
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_white_alpha(18)); // separators
        v.widgets.inactive.weak_bg_fill = Color32::from_rgb(55, 55, 57);
        v.widgets.inactive.bg_fill = Color32::from_rgb(55, 55, 57);
        v.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_white_alpha(14));
        v.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, Color32::from_gray(235));
        v.widgets.hovered.weak_bg_fill = Color32::from_white_alpha(20);
        v.widgets.hovered.bg_fill = Color32::from_white_alpha(20);
        v.widgets.hovered.bg_stroke = Stroke::NONE;
        v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::WHITE);
        v.widgets.hovered.expansion = 0.0;
        v.widgets.active.weak_bg_fill = Color32::from_white_alpha(12);
        v.widgets.active.bg_fill = Color32::from_white_alpha(12);
        v.widgets.active.expansion = 0.0;
        v.widgets.open.weak_bg_fill = Color32::from_white_alpha(20);
        v.widgets.open.bg_fill = Color32::from_white_alpha(20);
    });
}

/// A menu label with a Windows 11 line icon in front, like system menus.
#[allow(dead_code)]
fn ic(icon: char, text: &str) -> String {
    format!("{icon}    {text}")
}

/// Icons from Segoe Fluent Icons / Segoe MDL2 Assets.
mod icon {
    pub const RENAME: char = '\u{E8AC}';
    pub const UP: char = '\u{E70E}';
    pub const DOWN: char = '\u{E70D}';
    pub const ADD: char = '\u{E710}';
    pub const SORT: char = '\u{E8CB}';
    pub const VIEW: char = '\u{E80A}';
    pub const COLOR: char = '\u{E790}';
    pub const SETTINGS: char = '\u{E713}';
    pub const DELETE: char = '\u{E74D}';
    pub const OPEN: char = '\u{E8E5}';
    pub const FOLDER: char = '\u{E838}';
    pub const MOVE: char = '\u{E8DE}';
    pub const REMOVE: char = '\u{E711}';
    pub const TAB: char = '\u{E8A9}';
    pub const AUTO: char = '\u{E8B7}';
    pub const LOCK: char = '\u{E72E}';
    pub const CHEVRON: char = '\u{E76C}';
    pub const CHECK: char = '\u{E73E}';
    pub const BACK: char = '\u{E72B}';
    pub const SEARCH: char = '\u{E721}';
}

#[cfg(windows)]
fn make_tray(ctx: &egui::Context, cmds: Arc<Mutex<Vec<TrayCmd>>>) -> Option<tray_icon::TrayIcon> {
    use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

    // A small drawn icon: four rounded tiles.
    let n = 32usize;
    let mut rgba = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (cx, cy) = (x % 16, y % 16);
            let inside = (2..=13).contains(&cx) && (2..=13).contains(&cy);
            if inside {
                let i = (y * n + x) * 4;
                let c: [u8; 3] = if (x < 16) ^ (y < 16) { [90, 170, 255] } else { [235, 235, 240] };
                rgba[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
    }
    let icon = tray_icon::Icon::from_rgba(rgba, n as u32, n as u32).ok()?;
    let menu = Menu::new();
    let open = MenuItem::new("Open DeskBoxes", true, None);
    let new_box = MenuItem::new("New box", true, None);
    let search = MenuItem::new("Search desktop", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let _ = menu.append_items(&[&open, &search, &new_box, &PredefinedMenuItem::separator(), &quit]);
    let (open_id, new_id, quit_id, search_id) = (open.id().clone(), new_box.id().clone(), quit.id().clone(), search.id().clone());
    {
        let cmds = cmds.clone();
        let ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let cmd = if e.id == open_id {
                Some(TrayCmd::Open)
            } else if e.id == new_id {
                Some(TrayCmd::NewBox)
            } else if e.id == search_id {
                Some(TrayCmd::Search)
            } else if e.id == quit_id {
                Some(TrayCmd::Quit)
            } else {
                None
            };
            if let (Some(c), Ok(mut v)) = (cmd, cmds.lock()) {
                v.push(c);
                ctx.request_repaint();
            }
        }));
    }
    {
        let ctx = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                if let Ok(mut v) = cmds.lock() {
                    v.push(TrayCmd::Open);
                    ctx.request_repaint();
                }
            }
        }));
    }
    TrayIconBuilder::new()
        .with_tooltip("DeskBoxes")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()
        .ok()
}

/// What a box being moved or resized can snap to: the other boxes and every monitor's usable
/// area, converted to the caller's unit (`px_per_unit` = 1 for pixels, the display scale for
/// points). None when snapping is off.
fn snap_targets(st: &UiState, own: u64, px_per_unit: f32) -> Option<crate::snap::Snap> {
    if !st.snap_on {
        return None;
    }
    let k = px_per_unit.max(0.1);
    let to = |r: [i32; 4]| crate::snap::R::new(r[0] as f32 / k, r[1] as f32 / k, r[2] as f32 / k, r[3] as f32 / k);
    let boxes = st.hwnds.iter().filter(|(id, _)| **id != own).filter_map(|(_, h)| desktop::window_rect_px(*h)).map(to).collect();
    let screens = desktop::work_areas_px().into_iter().map(to).collect();
    let s = desktop::system_scale();
    Some(crate::snap::Snap { boxes, screens, dist: 10.0 * s / k, gap: st.snap_gap.max(0.0) * s / k, grid: st.snap_grid * s / k })
}

/// The quick-search window's state while it is open.
#[derive(Default)]
struct SearchState {
    query: String,
    /// Highlighted result (arrow keys / mouse).
    sel: usize,
    /// The window had the keyboard focus once (losing it then closes the window).
    was_focused: bool,
    /// Measured height of the contents, in points.
    height: f32,
}

#[cfg(windows)]
const SEARCH_PANEL: u64 = u64::MAX - 3;

impl DeskApp {
    /// The box an item is shown in, by lower-case path (items in no box: the auto-sort box).
    fn item_homes(&self) -> HashMap<String, String> {
        let mut m = HashMap::new();
        for f in self.layout.fences.iter().filter(|f| !f.auto_sort && f.widget.is_none()) {
            for t in &f.tabs {
                for p in &t.items {
                    m.entry(p.to_lowercase()).or_insert_with(|| f.name.clone());
                }
            }
        }
        m
    }

    /// Quick search: a box at the top of the main monitor; typing filters every desktop item.
    /// Enter opens the highlighted one (and selects it in its box), Ctrl+Enter opens its
    /// folder, arrow keys move, Esc or clicking elsewhere closes.
    #[cfg(windows)]
    fn show_search(&mut self, ctx: &egui::Context) {
        let Some(mut s) = self.search.take() else { return };
        if s.height < 60.0 {
            s.height = 120.0;
        }
        let scale = desktop::system_scale();
        let [sw, sh] = desktop::primary_screen_points();
        let w = 600.0_f32.min(sw - 40.0);
        let (x, y) = (((sw - w) / 2.0).max(0.0), (sh * 0.16).max(0.0));
        let rect_px = [(x * scale) as i32, (y * scale) as i32, (w * scale) as i32, (s.height * scale) as i32];
        let results: Vec<DeskItem> = crate::search::rank(self.items.values(), &s.query, 12).into_iter().cloned().collect();
        let homes = self.item_homes();
        let other = self.layout.fences.iter().find(|f| f.auto_sort).map(|f| f.name.clone()).unwrap_or_else(|| "Desktop".into());
        if s.sel >= results.len() {
            s.sel = results.len().saturating_sub(1);
        }
        let mut close = false;
        let mut open: Option<(String, bool)> = None;
        let mut measured = s.height;
        let force = self.panel_force;
        if let Some(mut hosts) = self.hosts.take() {
            let icons = &mut self.icons;
            hosts.panel(SEARCH_PANEL, crate::host::PanelKind::Menu, "DeskBoxes search", rect_px, true, force, |pctx| {
                match pctx.input(|i| i.viewport().focused) {
                    Some(true) => s.was_focused = true,
                    Some(false) if s.was_focused => close = true,
                    _ => {}
                }
                // Keys first (the text field would swallow some of them).
                let (esc, up, down, enter, ctrl) = pctx.input(|i| {
                    (
                        i.key_pressed(egui::Key::Escape) || i.viewport().close_requested(),
                        i.key_pressed(egui::Key::ArrowUp),
                        i.key_pressed(egui::Key::ArrowDown),
                        i.key_pressed(egui::Key::Enter),
                        i.modifiers.ctrl,
                    )
                });
                if esc {
                    close = true;
                }
                if down && !results.is_empty() {
                    s.sel = (s.sel + 1).min(results.len() - 1);
                }
                if up {
                    s.sel = s.sel.saturating_sub(1);
                }
                if enter {
                    if let Some(it) = results.get(s.sel) {
                        open = Some((it.path.clone(), ctrl));
                    }
                }
                let ppp = pctx.pixels_per_point();
                let frame = egui::Frame::new()
                    .fill(Color32::from_rgb(40, 40, 43))
                    .stroke(egui::Stroke::new(1.0_f32, Color32::from_white_alpha(30)))
                    .inner_margin(egui::Margin::same(10));
                egui::CentralPanel::default().frame(frame).show(pctx, |ui| {
                    let top = ui.cursor().min.y;
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(icon::SEARCH.to_string()).size(18.0).color(Color32::from_gray(190)));
                        let te = ui.add(
                            egui::TextEdit::singleline(&mut s.query)
                                .hint_text("Search the desktop…")
                                .font(FontId::proportional(18.0))
                                .frame(false)
                                .desired_width(f32::INFINITY),
                        );
                        if !te.has_focus() && !close && open.is_none() {
                            te.request_focus();
                        }
                        if te.changed() {
                            s.sel = 0;
                        }
                    });
                    ui.add_space(6.0);
                    if s.query.trim().is_empty() {
                        ui.label(egui::RichText::new("Type part of a name. Enter opens, Ctrl+Enter opens its folder, Esc closes.").size(12.5).color(Color32::from_gray(150)));
                    } else if results.is_empty() {
                        ui.label(egui::RichText::new("Nothing on the desktop matches.").size(13.0).color(Color32::from_gray(160)));
                    }
                    for (i, it) in results.iter().enumerate() {
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
                        if resp.hovered() && ui.input(|inp| inp.pointer.delta() != Vec2::ZERO) {
                            s.sel = i;
                        }
                        let p = ui.painter();
                        if i == s.sel {
                            p.rect_filled(rect, egui::CornerRadius::same(5), Color32::from_rgb(76, 194, 255).linear_multiply(0.28));
                        }
                        let ir = Rect::from_min_size(Pos2::new(rect.min.x + 6.0, rect.center().y - 11.0), Vec2::splat(22.0));
                        match icons.get(&it.path, (22.0 * ppp).round() as u32) {
                            Some(t) => {
                                p.image(t, ir, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                            }
                            None => {
                                p.rect_filled(ir.shrink(3.0), egui::CornerRadius::same(3), Color32::from_gray(110));
                            }
                        }
                        let home = homes.get(&it.path.to_lowercase()).cloned().unwrap_or_else(|| other.clone());
                        p.text(Pos2::new(rect.min.x + 38.0, rect.center().y), egui::Align2::LEFT_CENTER, &it.name, FontId::proportional(14.5), Color32::from_gray(240));
                        p.text(Pos2::new(rect.max.x - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, home, FontId::proportional(12.0), Color32::from_gray(150));
                        if resp.clicked() {
                            open = Some((it.path.clone(), ctrl));
                        }
                    }
                    measured = ui.cursor().min.y - top + 22.0;
                });
            });
            if close || open.is_some() {
                hosts.close_panel(SEARCH_PANEL);
            }
            self.hosts = Some(hosts);
        }
        if let Some((path, folder)) = open {
            if folder {
                desktop::show_in_folder(&path);
            } else {
                desktop::open_item(&path);
                self.actions.push(Act::Opened(path.clone()));
            }
            // Show it in its box too.
            self.ui.selection.clear();
            self.ui.selection.insert(path.to_lowercase());
            self.ui.selected = Some(path);
            self.redraw_all = true;
            close = true;
        }
        let measured = measured.clamp(80.0, 540.0);
        if (measured - s.height).abs() > 1.0 {
            s.height = measured;
            ctx.request_repaint();
        }
        if !close {
            self.search = Some(s);
        }
    }
}
