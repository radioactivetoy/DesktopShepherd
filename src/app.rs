//! The running program: each box is its own small borderless window kept at the bottom of the
//! window stack (on the desktop, under every app), plus a settings window opened on demand.
//!
//! Performance rules followed here:
//! * nothing redraws unless the mouse/keyboard touches a box, a file changes, or a widget asks
//!   (a clock wakes once a minute);
//! * reading the desktop folder and loading icons happen on background threads;
//! * saving waits until changes settle, then writes once.

use crate::desktop;
use crate::i18n::{t, tf};
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
    /// Give every box the look of this one (Personalize > Apply this look to all boxes).
    ApplyLookToAll(u64),
    /// Open the Personalize window for this box.
    Personalize(u64),
    /// Open the settings window of this widget box on its "Widget settings" tab.
    WidgetSettings(u64),
    /// Bring back a deleted box (index into `deleted`).
    RestoreDeleted(usize),
    /// Save the layout to a file / load one (Settings > Layout).
    ExportLayout,
    ImportLayout,
    /// Give this box a keyboard shortcut (0 = none); no other box keeps the same one.
    SetBoxHotkey(u64, u8),
    /// Pick a folder (Windows' folder picker, on its own thread) and make a folder box for it.
    NewFolderBox,
    /// Right-click on an item: Windows' own menu (and the other selected items with it), with
    /// DesktopShepherd' entries on top, shown by a helper process.
    ItemMenu { from: u64, path: String },
    /// An icon renamed in place (F2): its path and the new name.
    RenameItem { path: String, name: String },
    Quit,
}

/// F2 on an icon: its name becomes editable right under the icon, like Explorer.
struct IconRename {
    path: String,
    box_id: u64,
    text: String,
    /// The whole name was selected once (typing replaces it).
    selected_once: bool,
    /// The box had the keyboard at least once (losing it afterwards keeps the new name).
    had_focus: bool,
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
    /// An icon being renamed in place.
    icon_rename: Option<IconRename>,
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
    /// Hide or show all boxes on the next pass (tray).
    toggle_hidden: bool,
    /// Where the next new box goes (and when that was decided).
    new_place: Option<(NewPlace, Instant)>,
    /// Where the open box menu was opened (screen points).
    menu_at: Pos2,
    /// Start drawing a new box on the next pass (menu, tray).
    start_draw: bool,
    /// When the mouse was last over each box (for fading).
    hover_at: HashMap<u64, Instant>,
    /// Pages: how many there are and which is shown (for the box menu), and a switch asked
    /// for from the tray.
    page_count: u32,
    current_page: u32,
    page_step: i32,
    /// Scroll this icon (lower-case path) into view in its box (type to jump).
    scroll_to: Option<String>,
    /// Each box's icons as last drawn (paths, in order) and how many fit in a row, for the arrow
    /// keys and Ctrl+A.
    grid_order: HashMap<u64, (Vec<String>, usize)>,
    /// Commands for box windows from elsewhere (menus, monitor changes): box id, command.
    box_cmds: Vec<(u64, ViewportCommand)>,
    /// Boxes whose next position update comes from a move or resize by the person.
    moved_by_person: HashSet<u64>,
}

pub enum TrayCmd {
    Open,
    NewBox,
    DrawBox,
    UndoDelete,
    Search,
    HideShow,
    NextPage,
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
    /// The box whose Personalize window is open, and whether to bring that window forward.
    personalize: Option<u64>,
    focus_personalize: bool,
    /// Which tab of that window: 0 = look, 1 = widget settings.
    personalize_tab: u8,
    /// Shared data changed (files, icons, layout, selection): every box draws on the next pass.
    /// Otherwise a DirectX box draws only for its own reasons (see `HostSystem::frame`).
    redraw_all: bool,
    /// Icon sizes (pixels) the boxes showed last pass (see `IconCache::keep_only_sizes`).
    icon_sizes: Vec<u32>,
    /// The quick search or the settings window was open last pass (they use extra icon sizes).
    extra_icons_open: bool,
    /// When unused icons were last dropped.
    icon_sweep_at: Instant,
    /// Fonts the boxes use, loaded into the drawing context (see `setup_style`).
    font_names: Vec<String>,
    /// When to put all boxes back just above the desktop after a display change.
    restack_at: Vec<Instant>,
    /// The quick-search window, while open.
    search: Option<SearchState>,
    /// A folder chosen in the folder picker (New > Folder box…), waiting to become a box.
    picked_folder: Arc<Mutex<Option<String>>>,
    /// The rename window (F2), while open: the item and the new name being typed.
    rename: Option<RenameState>,
    /// Files pasted with Ctrl+V that landed on the desktop, to put in the box worked in:
    /// (path, box, tab). Filled by the paste thread.
    pasted: Arc<Mutex<Vec<(String, u64, usize)>>>,
    /// What an open Windows menu is for: clicked path, all paths, "Move to" box ids by position.
    menu_ctx: Option<(String, Vec<String>, Vec<u64>)>,
    /// The quick-search shortcut registered with Windows (index into SEARCH_HOTKEYS) and
    /// whether Windows accepted it.
    hotkey_applied: usize,
    hotkey_ok: bool,
    /// The hide/show shortcut registered (index), whether Windows accepted it, and whether the
    /// boxes are hidden right now.
    hide_hotkey_applied: usize,
    hide_hotkey_ok: bool,
    boxes_hidden: bool,
    /// The monitor setup the boxes are placed for (see `desktop::setup_key`).
    current_setup: String,
    /// What the box windows were last shown or hidden for (page, hidden, each box's page).
    shown_for: Option<(u32, bool, Vec<(u64, u32)>)>,
    page_hotkeys_set: bool,
    /// Deleted boxes (for Undo) and when they were deleted.
    deleted: Vec<(Fence, Instant)>,
    /// A layout file chosen in "Import layout…", waiting to be loaded.
    picked_layout: Arc<Mutex<Option<String>>>,
    /// Box shortcuts registered now (numbers 1..9 = Ctrl+Alt+F2..F10).
    box_hotkeys_applied: Option<Vec<u8>>,
    /// A request left from before the start (desktop menu while not running) was looked for.
    pending_checked: bool,
    /// When this run started (the first update check waits a little).
    started_at: Instant,
    /// The accent setting last applied.
    accent_applied: Option<bool>,
    /// Adaptive text: per box, does it draw dark ink now; the rectangle (pixels) and wallpaper
    /// generation the decision was made for; the setting last applied.
    ink_dark: HashMap<u64, bool>,
    ink_rects: HashMap<u64, [i32; 4]>,
    ink_gen: u64,
    ink_applied: Option<bool>,
    /// OneDrive sync: the shared file's time we last wrote or loaded, and when it was checked.
    sync_seen: Option<std::time::SystemTime>,
    sync_check_at: Instant,
    /// The next RestoreBackup comes from the OneDrive copy: keep this PC's agenda links.
    restoring_sync: bool,
    /// A short message at the bottom of the main monitor (e.g. a refused rename), with its start time.
    note: Option<(String, Instant)>,
    /// The virtual desktop last seen (index).
    vdesk_seen: Option<usize>,
    /// The desktop menu entries were written for this run.
    desktop_menu_applied: Option<bool>,
    /// Type to jump: what was typed recently on the desktop and when (a pause starts over).
    ta_buf: String,
    ta_last: Option<Instant>,
}

/// Passes and box frames drawn (for DESKTOPSHEPHERD_DEBUG_DRAWS).
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
    let mut v: Vec<String> = layout.fences.iter().flat_map(|f| f.tabs.iter().flat_map(|t| t.items.iter().cloned())).collect();
    // Folder boxes: the scanner lists these folders too.
    v.extend(layout.fences.iter().filter_map(|f| f.folder.as_ref()).map(|d| format!("dir:{d}")));
    v
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
            crate::i18n::set_lang(layout.settings.language); // the first boxes get their names in this language
            layout = default_layout(&initial);
            // Just installed by the setup: start with Windows (the setup asked for it).
            if std::env::var_os("DESKTOPSHEPHERD_INSTALLED").is_some() {
                layout.settings.start_with_windows = true;
            }
        }
        sanitize(&mut layout);
        // The language, before any text is made (tray menu, windows).
        crate::i18n::set_lang(layout.settings.language);
        refresh_accent(layout.settings.follow_accent);
        crate::theme::refresh();
        crate::theme::set_choice(layout.settings.theme);
        if persist {
            storage::backup_now(&layout);
            if first_run {
                let _ = storage::save(&layout);
            }
        }

        // Background desktop reader: re-reads the folders when the folder watcher (below) or the app
        // wakes it, and only reports when something actually changed.
        let extra_paths = Arc::new(Mutex::new(clean_paths(&layout)));
        let monitors_now = desktop::monitors();
        let monitors_shared = Arc::new(Mutex::new(monitors_now.clone()));
        let monitors_changed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (scan_tx, scan_rx) = channel();
        let (wake_tx, wake_rx) = channel::<()>();
        {
            let extra = extra_paths.clone();
            let ctx = ctx.clone();
            let mut last: Vec<String> = {
                let mut k: Vec<String> = initial.iter().map(|(k, v)| format!("{k}|{}", v.name)).collect();
                k.sort();
                k
            };
            let _ = std::thread::Builder::new().name("desktop-scan".into()).spawn(move || loop {
                // Woken by the folder watcher (a file was added, removed, renamed or changed) or by
                // the app; once a minute anyway as a safety net. (Monitors: WM_DISPLAYCHANGE.)
                match wake_rx.recv_timeout(Duration::from_secs(60)) {
                    Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => break,
                }
                while wake_rx.try_recv().is_ok() {} // several changes at once: one read
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
        // Folder watcher: Windows tells us when something in the desktop folders (or a folder
        // box's folder) changes; the thread sleeps otherwise. It also keeps Windows' own desktop
        // icons hidden (Explorer can show them again) with a cheap check every 4 s.
        {
            let extra = extra_paths.clone();
            let wake = wake_tx.clone();
            desktop::watch_folders(
                move || {
                    let mut dirs = desktop::desktop_dirs();
                    if let Ok(e) = extra.lock() {
                        dirs.extend(e.iter().filter_map(|p| p.strip_prefix("dir:")).map(std::path::PathBuf::from));
                    }
                    dirs
                },
                move || wake.send(()).is_ok(),
            );
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
            personalize: None,
            focus_personalize: false,
            personalize_tab: 0,
            redraw_all: true,
            icon_sizes: Vec::new(),
            extra_icons_open: false,
            icon_sweep_at: Instant::now(),
            font_names: Vec::new(),
            restack_at: Vec::new(),
            search: None,
            menu_ctx: None,
            rename: None,
            picked_folder: Arc::new(Mutex::new(None)),
            pasted: Arc::new(Mutex::new(Vec::new())),
            hotkey_applied: usize::MAX,
            hotkey_ok: true,
            hide_hotkey_applied: usize::MAX,
            hide_hotkey_ok: true,
            boxes_hidden: false,
            current_setup: String::new(),
            shown_for: None,
            page_hotkeys_set: false,
            deleted: Vec::new(),
            picked_layout: Arc::new(Mutex::new(None)),
            box_hotkeys_applied: None,
            pending_checked: false,
            started_at: Instant::now(),
            accent_applied: None,
            ink_dark: HashMap::new(),
            ink_rects: HashMap::new(),
            ink_gen: 0,
            ink_applied: None,
            sync_seen: None,
            sync_check_at: Instant::now() - Duration::from_secs(60),
            restoring_sync: false,
            note: None,
            vdesk_seen: None,
            desktop_menu_applied: None,
            ta_buf: String::new(),
            ta_last: None,
            layout,
        };
        if app.persist {
            app.bind_unassigned();
            app.switch_setup_or_relayout();
            // Keep the start-with-Windows entry pointing at this copy of the program (after a
            // rename, a move or an update), off a helper thread so the start isn't slowed.
            if app.layout.settings.start_with_windows {
                let _ = std::thread::Builder::new().name("autostart".into()).spawn(|| desktop::set_start_with_windows(true));
            }
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
        if self.layout.settings.per_setup && !self.current_setup.is_empty() {
            let map = self.layout.fences.iter().map(|f| (f.id, f.rect)).collect();
            self.layout.setup_positions.insert(self.current_setup.clone(), map);
        }
        if let Ok(mut e) = self.extra_paths.lock() {
            *e = clean_paths(&self.layout);
        }
        if let Err(e) = storage::save(&self.layout) {
            storage::log_line(&format!("save failed: {e}"));
        }
        if self.layout.settings.sync_onedrive {
            self.sync_seen = storage::write_sync(&self.layout);
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

    /// Where a new box goes: the rectangle just drawn, or next to where the person right-clicked
    /// (box menu / desktop menu), else a free spot near the top-left of the main monitor.
    fn new_fence_rect(&mut self, size: [f32; 2]) -> [f32; 4] {
        let place = self.ui.new_place.take().filter(|(_, t)| t.elapsed() < Duration::from_secs(120)).map(|(p, _)| p);
        match place {
            Some(NewPlace::Rect(r)) if r.width() >= 120.0 && r.height() >= 60.0 => [r.min.x, r.min.y, r.width(), r.height()],
            Some(NewPlace::Rect(r)) => self.rect_at(r.min, size),
            Some(NewPlace::At(p)) => self.rect_at(p, size),
            None => {
                let n = self.layout.fences.len() as f32;
                [120.0 + 30.0 * (n % 8.0), 120.0 + 30.0 * (n % 8.0), size[0], size[1]]
            }
        }
    }

    /// A box of `size` with its corner at `p`, kept inside that monitor's work area.
    fn rect_at(&self, p: Pos2, size: [f32; 2]) -> [f32; 4] {
        let work = desktop::work_area_at(p, desktop::system_scale());
        let w = size[0].min(work.width());
        let h = size[1].min(work.height());
        let x = p.x.clamp(work.min.x, (work.max.x - w).max(work.min.x));
        let y = p.y.clamp(work.min.y, (work.max.y - h).max(work.min.y));
        [x, y, w, h]
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
                        name: if auto { t("Desktop").into() } else { t("New box").into() },
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
                            name: t(k.label).into(),
                            title_pos: TitlePos::Top,
                            widget: Some(WidgetConfig { kind: k.id.into(), data: serde_json::json!({}) }),
                            ..Fence::default()
                        });
                        self.mark_dirty();
                    }
                }
                Act::DeleteFence(id) => {
                    // Kept for "Undo" (the last 10, until the program closes).
                    if let Some(f) = self.layout.fences.iter().find(|f| f.id == id) {
                        self.deleted.push((f.clone(), Instant::now()));
                        if self.deleted.len() > 10 {
                            self.deleted.remove(0);
                        }
                    }
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
                Act::RestoreDeleted(i) => {
                    if i < self.deleted.len() {
                        let (f, _) = self.deleted.remove(i);
                        // Show it: switch to its page, show hidden boxes.
                        if f.page != self.layout.current_page {
                            self.layout.current_page = f.page;
                        }
                        if self.boxes_hidden {
                            self.set_boxes_hidden(false);
                        }
                        let id = f.id;
                        if self.layout.fences.iter().any(|o| o.id == id) {
                            let mut f = f;
                            f.id = self.layout.new_id();
                            self.layout.fences.push(f);
                        } else {
                            self.layout.fences.push(f);
                        }
                        if let Ok(mut e) = self.extra_paths.lock() {
                            *e = clean_paths(&self.layout);
                        }
                        let _ = self.scan_wake.send(());
                        self.mark_dirty();
                        self.redraw_all = true;
                    }
                }
                Act::ExportLayout => {
                    #[cfg(windows)]
                    {
                        let text = serde_json::to_string_pretty(&self.layout).unwrap_or_default();
                        let _ = std::thread::Builder::new().name("export-layout".into()).spawn(move || {
                            if let Some(p) = desktop::pick_file(true, t("Export layout"), "DesktopShepherd layout.json") {
                                if let Err(e) = std::fs::write(&p, text) {
                                    storage::log_line(&format!("export failed: {e}"));
                                }
                            }
                        });
                    }
                }
                Act::ImportLayout => {
                    #[cfg(windows)]
                    {
                        let picked = self.picked_layout.clone();
                        let _ = std::thread::Builder::new().name("import-layout".into()).spawn(move || {
                            if let Some(p) = desktop::pick_file(false, t("Import layout"), "") {
                                if let Ok(mut s) = picked.lock() {
                                    *s = Some(p);
                                }
                                crate::runloop::wake_after(Duration::ZERO);
                            }
                        });
                    }
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
                        .map(|f| (f.id, f.auto_sort, f.widget.is_some() || f.folder.is_some(), f.sel_tab));
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
                Act::RenameItem { path, name } => {
                    self.finish_rename(&path, &name);
                    // The keyboard goes back to the desktop (F2, Ctrl+C... work again on the icons).
                    desktop::focus_desktop();
                }
                Act::Opened(p) => {
                    *self.layout.open_counts.entry(p.to_lowercase()).or_insert(0) += 1;
                    self.mark_dirty();
                }
                Act::RestoreBackup(p) => {
                    let from_sync = std::mem::take(&mut self.restoring_sync);
                    if let Some(mut l) = storage::load_backup(&p) {
                        storage::backup_now(&self.layout);
                        if from_sync {
                            l.keep_agenda_urls_from(&self.layout);
                        }
                        sanitize(&mut l);
                        self.layout = l;
                        self.place_all_boxes();
                        self.mark_dirty();
                        self.apply_icon_hiding();
                        // The restored settings take effect, and the list shows the backup just made.
                        #[cfg(windows)]
                        {
                            desktop::set_start_with_windows(self.layout.settings.start_with_windows);
                            desktop::set_low_priority(self.layout.settings.low_priority);
                        }
                        self.backups = storage::list_backups();
                    }
                }
                Act::ItemMenu { from, path } => {
                    let key = path.to_lowercase();
                    let mut paths = vec![path.clone()];
                    if self.ui.selection.contains(&key) {
                        for k in self.ui.selection.iter().filter(|k| **k != key) {
                            if let Some(it) = self.items.get(k) {
                                paths.push(it.path.clone());
                            }
                        }
                    }
                    let from_auto = self.layout.fences.iter().find(|f| f.id == from).map(|f| f.auto_sort).unwrap_or(false);
                    let targets: Vec<(u64, String)> = self
                        .layout
                        .fences
                        .iter()
                        .filter(|f| !f.auto_sort && f.widget.is_none() && f.folder.is_none() && (f.id != from || from_auto))
                        .map(|f| (f.id, f.name.clone()))
                        .collect();
                    #[cfg(windows)]
                    {
                        use crate::shellmenu::Extra;
                        let mut extras: Vec<Extra> = targets.iter().enumerate().map(|(i, (_, n))| Extra { id: 100 + i as u32, label: n.clone(), sub: true }).collect();
                        extras.push(Extra { id: 99, label: t("New box").into(), sub: true });
                        if !from_auto {
                            extras.push(Extra { id: 1, label: t("Remove from this box").into(), sub: false });
                        }
                        extras.push(Extra { id: 3, label: t("Open file location").into(), sub: false });
                        if crate::shellmenu::spawn(&paths, &extras) {
                            self.menu_ctx = Some((path.clone(), paths, targets.iter().map(|t| t.0).collect()));
                        } else {
                            // The helper couldn't start: DesktopShepherd' own menu instead.
                            let at = desktop::cursor_points(desktop::system_scale()).unwrap_or_default();
                            self.ui.menu = Some(OpenMenu { target: MenuTarget::Item(from, path.clone()), page: MenuPage::Main, at, size: Vec2::new(270.0, 120.0), frames: 0, was_focused: false, rounded: false });
                        }
                    }
                }
                Act::NewFolderBox => {
                    #[cfg(windows)]
                    {
                        let wake = self.scan_wake.clone();
                        let picked = self.picked_folder.clone();
                        let _ = std::thread::Builder::new().name("folder-picker".into()).spawn(move || {
                            if let Some(dir) = desktop::pick_folder() {
                                if let Ok(mut p) = picked.lock() {
                                    *p = Some(dir);
                                }
                                let _ = wake.send(());
                                crate::runloop::wake_after(Duration::ZERO);
                            }
                        });
                    }
                }
                Act::Personalize(id) => {
                    self.personalize = Some(id);
                    self.personalize_tab = 0;
                    self.focus_personalize = true;
                }
                Act::SetBoxHotkey(id, n) => {
                    for f in &mut self.layout.fences {
                        if f.id == id {
                            f.hotkey = n;
                        } else if n != 0 && f.hotkey == n {
                            f.hotkey = 0;
                        }
                    }
                    self.mark_dirty();
                }
                Act::WidgetSettings(id) => {
                    self.personalize = Some(id);
                    self.personalize_tab = 1;
                    self.focus_personalize = true;
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
        if std::env::var_os("DESKTOPSHEPHERD_DEBUG_REPAINT").is_some() && ctx.cumulative_pass_nr() % 120 == 0 {
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
        // Typing on the desktop: while the desktop itself has the focus (and Windows' icons are
        // hidden, so Explorer's own type-to-select isn't needed), borrow the letter keys.
        #[cfg(windows)]
        if self.persist {
            let mode = self.layout.settings.desktop_typing;
            let want = self.layout.settings.hide_desktop_icons && desktop::foreground_is_desktop();
            crate::runloop::set_typing_keys(want, mode != 0);
            if desktop::foreground_changed_recently() {
                // The focus may still be settling: look again shortly.
                crate::runloop::wake_after(Duration::from_millis(150));
            }
            for (mods, vk) in crate::runloop::take_typed() {
                let ctrl = mods & 0x0002 != 0;
                match (ctrl, vk) {
                    (false, 0x0D) => {
                        // Enter opens what is selected.
                        if let Some(p) = self.ui.selected.clone() {
                            desktop::open_item(&p);
                            self.actions.push(Act::Opened(p));
                        }
                    }
                    (false, 0x1B) => {
                        self.ui.selection.clear();
                        self.ui.selected = None;
                        self.redraw_all = true;
                    }
                    (false, 0x25..=0x28) => self.arrow_key(vk),
                    (false, 0x2E) => self.delete_selected(),
                    (false, 0x71) => self.start_rename(),
                    (true, 0x41) => self.select_all_in_box(),
                    (true, 0x43) | (true, 0x58) => {
                        let paths = self.selected_paths();
                        crate::fileops::set_clipboard_files(crate::runloop::main_hwnd(), &paths, vk == 0x58);
                    }
                    (true, 0x56) => self.paste_files(),
                    (false, _) => {
                        if let Some(c) = char::from_u32(vk).map(|c| c.to_ascii_lowercase()).filter(|c| c.is_ascii_alphanumeric()) {
                            if mode == 2 {
                                // Letters typed before the search window has the keyboard still count.
                                match self.search.as_mut() {
                                    Some(s) => s.query.push(c),
                                    None => self.search = Some(SearchState { query: c.to_string(), ..Default::default() }),
                                }
                            } else if mode == 1 {
                                self.type_ahead(c);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // The desktop's right-click menu entries, whenever the setting changes (and once at start,
        // so they point at this copy of the program).
        #[cfg(windows)]
        if self.persist && self.desktop_menu_applied != Some(self.layout.settings.desktop_menu) {
            let on = self.layout.settings.desktop_menu;
            self.desktop_menu_applied = Some(on);
            let _ = std::thread::Builder::new().name("desktop-menu".into()).spawn(move || desktop::set_desktop_menu(on));
        }
        // New versions on GitHub (Settings > Updates): at start (after 20 s) and once a day.
        if self.persist && self.layout.settings.check_updates && self.layout.settings.updates_asked {
            if self.started_at.elapsed() >= Duration::from_secs(20) {
                crate::update::check(Duration::from_secs(24 * 3600), false);
            } else {
                crate::runloop::wake_after(Duration::from_secs(21) - self.started_at.elapsed().min(Duration::from_secs(20)));
            }
        }
        // OneDrive sync: every 20 s, load the shared layout if another PC saved a newer one.
        if self.persist && self.layout.settings.sync_onedrive && self.sync_check_at.elapsed() >= Duration::from_secs(20) {
            self.sync_check_at = Instant::now();
            let t = storage::sync_modified();
            match (t, self.sync_seen) {
                (Some(t), Some(seen)) if t > seen => {
                    self.sync_seen = Some(t);
                    if let Some(p) = storage::sync_path() {
                        storage::log_line("sync: loading the layout saved on another PC");
                        self.restoring_sync = true;
                        self.actions.push(Act::RestoreBackup(p));
                    }
                }
                (Some(t), None) => {
                    // Just turned on (or first check): this PC's layout goes to OneDrive.
                    let _ = t;
                    self.sync_seen = storage::write_sync(&self.layout);
                }
                (None, _) => self.sync_seen = storage::write_sync(&self.layout),
                _ => {}
            }
            crate::runloop::wake_after(Duration::from_secs(21));
        }
        crate::apps::set_enabled(self.layout.settings.search_apps);
        if let Some(p) = self.picked_layout.lock().ok().and_then(|mut p| p.take()) {
            self.actions.push(Act::RestoreBackup(std::path::PathBuf::from(p)));
        }
        // New boxes from Ctrl+Alt+B, the menus or the desktop's right-click menu.
        #[cfg(windows)]
        if self.persist {
            if crate::runloop::take_draw_request() || std::mem::take(&mut self.ui.start_draw) {
                crate::drawbox::start();
            }
            let mut requests = crate::runloop::take_new_requests();
            if !self.pending_checked {
                self.pending_checked = true;
                requests.extend(desktop::take_pending_new_request());
            }
            for req in requests {
                let mut parts = req.split_whitespace();
                let what = parts.next().unwrap_or("").to_string();
                let x: f32 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(200.0);
                let y: f32 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(200.0);
                let s = desktop::system_scale();
                let at = Pos2::new(x / s, y / s);
                if what == "draw" {
                    crate::drawbox::start();
                    continue;
                }
                self.ui.new_place = Some((NewPlace::At(at), Instant::now()));
                match what.as_str() {
                    "box" => self.actions.push(Act::NewFence { auto: false }),
                    "folder" => self.actions.push(Act::NewFolderBox),
                    w => {
                        if let Some(k) = w.strip_prefix("widget:").and_then(widgets::find) {
                            self.actions.push(Act::NewWidget(k.id));
                        }
                    }
                }
                // Make sure it can be seen: the page shown, boxes not hidden.
                if self.boxes_hidden {
                    self.set_boxes_hidden(false);
                }
            }
            if let Some(drawn) = crate::drawbox::take_result() {
                if let Some([l, t, r, b]) = drawn {
                    let s = desktop::system_scale();
                    let rect = Rect::from_min_max(Pos2::new(l as f32 / s, t as f32 / s), Pos2::new(r as f32 / s, b as f32 / s));
                    self.ui.new_place = Some((NewPlace::Rect(rect), Instant::now()));
                    // Ask what to create, at the corner where the drag ended.
                    self.ui.confirm_delete = None;
                    self.ui.menu = Some(OpenMenu {
                        target: MenuTarget::Create,
                        page: MenuPage::Main,
                        at: rect.max,
                        // Its real height up front, so near the bottom edge it opens upward.
                        size: Vec2::new(270.0, 34.0 * (2 + widgets::REGISTRY.len()) as f32 + 24.0),
                        frames: 0,
                        was_focused: false,
                        rounded: false,
                    });
                    self.redraw_all = true;
                }
            }
        }
        // Box shortcuts (Ctrl+Alt+F2..F10): bring the box into view, or roll it up / down.
        #[cfg(windows)]
        if self.persist {
            let mut nums: Vec<u8> = self.layout.fences.iter().map(|f| f.hotkey).filter(|n| (1..=9).contains(n)).collect();
            nums.sort_unstable();
            nums.dedup();
            if self.box_hotkeys_applied.as_ref() != Some(&nums) {
                crate::runloop::set_box_hotkeys(&nums);
                self.box_hotkeys_applied = Some(nums);
            }
            for n in crate::runloop::take_box_keys() {
                let Some(i) = self.layout.fences.iter().position(|f| f.hotkey == n) else { continue };
                let (page, id) = (self.layout.fences[i].page, self.layout.fences[i].id);
                let mut shown = true;
                if self.boxes_hidden {
                    self.set_boxes_hidden(false);
                    shown = false;
                }
                if page != self.layout.current_page {
                    self.layout.current_page = page;
                    self.mark_dirty();
                    shown = false;
                }
                let f = &mut self.layout.fences[i];
                if shown || !f.expanded {
                    // Already in view: roll it up or down (a rolled-up box always opens).
                    f.expanded = !f.expanded;
                    self.ui.hover_open.remove(&id);
                    let h = if f.expanded { f.rect[3] } else { ROLLED_HEIGHT };
                    self.ui.box_cmds.push((id, ViewportCommand::InnerSize(Vec2::new(f.rect[2], h))));
                    self.ui.burst.insert(id, 15);
                    self.mark_dirty();
                }
                self.redraw_all = true;
            }
        }
        // Virtual desktops: boxes come along to the desktop shown, and (if linked) each desktop
        // shows its own page.
        #[cfg(windows)]
        if self.persist {
            if let Some(vd) = crate::vdesk::current() {
                if self.vdesk_seen != Some(vd) {
                    let first = self.vdesk_seen.is_none();
                    self.vdesk_seen = Some(vd);
                    let hw: Vec<isize> = self.ui.hwnds.values().copied().collect();
                    let hw: Vec<isize> = hw.iter().flat_map(|h| [Some(*h), crate::shadow::window_of(*h)]).flatten().collect();
                    crate::vdesk::bring_windows_here(&hw);
                    if self.layout.settings.link_vdesks && !first && self.layout.current_page != vd as u32 {
                        self.layout.current_page = vd as u32;
                        self.mark_dirty();
                    }
                }
            }
        }
        // Pages: Ctrl+Alt+PageDown / PageUp, or the tray; then show only the current page's boxes.
        #[cfg(windows)]
        if self.persist {
            if !self.page_hotkeys_set {
                crate::runloop::set_page_hotkeys();
                self.page_hotkeys_set = true;
            }
            let step = crate::runloop::take_page_step() + std::mem::take(&mut self.ui.page_step);
            let count = self.layout.fences.iter().map(|f| f.page + 1).max().unwrap_or(1).max(self.layout.current_page + 1);
            if step != 0 && count > 1 {
                let cur = self.layout.current_page as i64;
                self.layout.current_page = (cur + step as i64).rem_euclid(count as i64) as u32;
                self.mark_dirty();
            }
            self.ui.page_count = count;
            self.ui.current_page = self.layout.current_page;
            let want = (self.layout.current_page, self.boxes_hidden, self.layout.fences.iter().map(|f| (f.id, f.page)).collect::<Vec<_>>());
            if self.shown_for.as_ref() != Some(&want) {
                for f in &self.layout.fences {
                    let show = !self.boxes_hidden && f.page == self.layout.current_page;
                    if let Some(h) = self.ui.hwnds.get(&f.id) {
                        desktop::show_box_window(*h, show);
                    }
                    if !show {
                        if let Some(hosts) = self.hosts.as_mut() {
                            hosts.release_hidden(f.id);
                        }
                    }
                }
                self.shown_for = Some(want);
                self.redraw_all = true;
                // Nothing stays selected in a box you can no longer see (Delete would remove it).
                self.ui.selection.clear();
                self.ui.selected = None;
                self.ui.icon_rename = None; // its box may not be shown any more
            }
        }
        // Hide/show all boxes (shortcut or tray).
        #[cfg(windows)]
        if self.persist {
            let want = self.layout.settings.hide_hotkey;
            if self.hide_hotkey_applied != want {
                self.hide_hotkey_ok = crate::runloop::set_hide_hotkey(want);
                self.hide_hotkey_applied = want;
            }
            if crate::runloop::take_hide_request() || std::mem::take(&mut self.ui.toggle_hidden) {
                self.set_boxes_hidden(!self.boxes_hidden);
            }
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
        // The quick-search list (22 pt) and the settings window's layout preview fetch icons in
        // other sizes: drop those when the window closes (search boxes keep their 20 pt ones).
        let extra_open = self.search.is_some() || self.manager_open;
        let closed_extra = self.extra_icons_open && !extra_open;
        self.extra_icons_open = extra_open;
        if sizes != self.icon_sizes || closed_extra {
            if !self.icon_sizes.is_empty() || closed_extra {
                let mut keep: HashSet<u32> = sizes.iter().copied().collect();
                if self.layout.fences.iter().any(|f| f.widget.as_ref().map(|w| w.kind == "search").unwrap_or(false)) {
                    keep.insert(((20.0 * s).round() as u32).clamp(16, 256));
                }
                self.icons.keep_only_sizes(&keep);
            }
            self.icon_sizes = sizes;
        }
        // Windows' accent colour changed, or the setting: style everything again.
        #[cfg(windows)]
        let colour_changed = crate::runloop::take_colors_changed();
        #[cfg(not(windows))]
        let colour_changed = false;
        if (colour_changed || self.accent_applied != Some(self.layout.settings.follow_accent)) && refresh_accent(self.layout.settings.follow_accent) {
            self.font_names = vec!["\u{0}restyle".into()]; // forces the restyle below
        }
        self.accent_applied = Some(self.layout.settings.follow_accent);
        // Adaptive text: the wallpaper reader runs only while the setting is on.
        #[cfg(windows)]
        {
            let on = self.layout.settings.auto_ink;
            if self.ink_applied != Some(on) {
                crate::backdrop::set_enabled(on);
                if !on {
                    self.ink_dark.clear();
                    self.ink_rects.clear();
                }
                self.redraw_all = true;
                self.ink_applied = Some(on);
            } else if on && colour_changed {
                crate::backdrop::poke();
            }
        }
        // Light or dark menus and settings: Windows changed, or the setting did.
        let theme_changed = (colour_changed && crate::theme::refresh()) | crate::theme::set_choice(self.layout.settings.theme);
        if theme_changed {
            self.panel_force = true;
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
        // Once a minute: drop icons unused for 5 minutes that no box on screen can show: items only
        // in boxes on other pages / hidden, or gone from the desktop. (A box on screen that wasn't
        // redrawn still shows its icons: those stay.)
        if self.icon_sweep_at.elapsed() >= Duration::from_secs(60) {
            self.icon_sweep_at = Instant::now();
            let (page, hidden) = (self.layout.current_page, self.boxes_hidden);
            let shown = |f: &&Fence| !hidden && f.page == page;
            let in_hidden: HashSet<String> = self.layout.fences.iter().filter(|f| !shown(f)).flat_map(|f| f.tabs.iter().flat_map(|t| t.items.iter().map(|p| p.to_lowercase()))).collect();
            let in_shown: HashSet<String> = self.layout.fences.iter().filter(shown).flat_map(|f| f.tabs.iter().flat_map(|t| t.items.iter().map(|p| p.to_lowercase()))).collect();
            let items = &self.items;
            self.icons.drop_unused(Duration::from_secs(300), |p| {
                let k = p.to_lowercase();
                !items.contains_key(&k) || (in_hidden.contains(&k) && !in_shown.contains(&k))
            });
        }
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
                TrayCmd::HideShow => self.ui.toggle_hidden = true,
                TrayCmd::NextPage => self.ui.page_step += 1,
                TrayCmd::DrawBox => self.ui.start_draw = true,
                TrayCmd::UndoDelete => {
                    if !self.deleted.is_empty() {
                        let i = self.deleted.len() - 1;
                        self.actions.push(Act::RestoreDeleted(i));
                    }
                }
                TrayCmd::Quit => self.actions.push(Act::Quit),
            }
        }


        if self.persist {
            // Displays changed: Windows told us at once (WM_DISPLAYCHANGE), or the watcher noticed.
            #[cfg(windows)]
            if desktop::take_restack_request() {
                let now = Instant::now();
                self.restack_at.extend([now + Duration::from_millis(700), now + Duration::from_millis(2500)]);
            }
            #[cfg(windows)]
            if crate::runloop::take_explorer_restarted() {
                // Explorer restarted: a new desktop window. Put the boxes back just above it as it
                // settles (icon hiding is re-applied by the regular check).
                storage::log_line("Explorer restarted");
                let now = Instant::now();
                self.restack_at = vec![now + Duration::from_millis(800), now + Duration::from_secs(3), now + Duration::from_secs(8)];
            }
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
                self.switch_setup_or_relayout();
                // When a monitor goes away Windows rearranges windows, and boxes can end up above
                // some apps: put them back just above the desktop now and again as things settle.
                let now = Instant::now();
                self.restack_at = vec![now, now + Duration::from_millis(1500), now + Duration::from_secs(4)];
            }
            if !self.restack_at.is_empty() {
                let now = Instant::now();
                if self.restack_at.iter().any(|t| *t <= now) {
                    self.restack_at.retain(|t| *t > now);
                    storage::log_line(&format!("re-stacking {} boxes", self.ui.hwnds.len()));
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

        // ---- Personalize window
        if self.personalize.is_some() {
            self.show_personalize();
        } else {
            #[cfg(windows)]
            if let Some(h) = self.hosts.as_mut() {
                h.close_panel(PERSONALIZE_PANEL);
            }
        }

        // A DesktopShepherd entry chosen in Windows' menu (helper process). Windows' own commands may
        // also have renamed, deleted or added files: read the desktop again either way.
        #[cfg(windows)]
        for (id, clicked) in crate::shellmenu::take_done() {
            if let Some((p, paths, targets)) = self.menu_ctx.clone().filter(|c| c.0 == clicked) {
                match id {
                    1 => {
                        for q in &paths {
                            self.actions.push(Act::MoveItem { path: q.clone(), to: 0, tab: 0 });
                        }
                    }
                    3 => desktop::show_in_folder(&p),
                    99 => {
                        for q in &paths {
                            self.actions.push(Act::MoveToNewFence { path: q.clone() });
                        }
                    }
                    n if n >= 100 => {
                        if let Some(&to) = targets.get((n - 100) as usize) {
                            let tab = self.layout.fences.iter().find(|f| f.id == to).map(|f| f.sel_tab).unwrap_or(0);
                            for q in &paths {
                                self.actions.push(Act::MoveItem { path: q.clone(), to, tab });
                            }
                        }
                    }
                    _ => {}
                }
            }
            let _ = self.scan_wake.send(());
        }
        // A folder was picked: make its folder box.
        let picked = self.picked_folder.lock().ok().and_then(|mut p| p.take());
        if let Some(dir) = picked {
            let id = self.layout.new_id();
            let rect = self.new_fence_rect([520.0, 300.0]);
            let name = std::path::Path::new(&dir).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| dir.clone());
            self.layout.fences.push(Fence { id, rect, name, sort: SortKind::Modified, folder: Some(dir), ..Fence::default() });
            self.mark_dirty();
            if let Ok(mut e) = self.extra_paths.lock() {
                *e = clean_paths(&self.layout);
            }
            let _ = self.scan_wake.send(());
        }
        // Files pasted with Ctrl+V: put them in the box they were pasted into.
        let pasted: Vec<(String, u64, usize)> = self.pasted.lock().map(|mut p| std::mem::take(&mut *p)).unwrap_or_default();
        for (path, to, tab) in pasted {
            self.actions.push(Act::MoveItem { path, to, tab });
        }
        let ids_before: HashSet<u64> = self.layout.fences.iter().map(|f| f.id).collect();
        if !self.actions.is_empty() {
            self.redraw_all = true;
        }
        self.process_actions();
        let cur_page = self.layout.current_page;
        for f in self.layout.fences.iter_mut().filter(|f| !ids_before.contains(&f.id)) {
            f.page = cur_page;
        }
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
            let ink_gen_now = crate::backdrop::generation();
            for fence in self.layout.fences.iter_mut() {
                if self.boxes_hidden || fence.page != self.layout.current_page {
                    continue; // on another page, or all hidden: nothing to draw
                }
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
                // Adaptive text: dark ink when the wallpaper behind this box is bright (with
                // hysteresis; looked at again only when the wallpaper, the box or the setting changed).
                let mut ink_force = false;
                let mut dark = false;
                if self.layout.settings.auto_ink && hosts.has_glass() {
                    let sc = desktop::system_scale();
                    let px = [(rect[0] * sc).round() as i32, (rect[1] * sc).round() as i32, ((rect[0] + rect[2]) * sc).round() as i32, ((rect[1] + rect[3]) * sc).round() as i32];
                    let was = self.ink_dark.get(&id).copied();
                    if was.is_none() || ink_gen_now != self.ink_gen || self.ink_rects.get(&id) != Some(&px) {
                        let prev = was.unwrap_or(false);
                        let now = match crate::backdrop::brightness(px) {
                            Some(b) => {
                                if prev {
                                    b > 0.50
                                } else {
                                    b > 0.60
                                }
                            }
                            None => false,
                        };
                        if std::env::var_os("DESKTOPSHEPHERD_DEBUG_INK").is_some() {
                            storage::log_line(&format!("ink: box {id} {px:?} brightness {:?} dark {now}", crate::backdrop::brightness(px)));
                        }
                        self.ink_rects.insert(id, px);
                        self.ink_dark.insert(id, now);
                        ink_force = was.is_some() && was != Some(now);
                    }
                    dark = self.ink_dark.get(&id).copied().unwrap_or(false);
                }
                crate::ink::set(dark);
                let drew = hosts.frame(id, rect, &look, force_all || ink_force, |ctx| {
                    let r = fence_window(ctx, fence, &desk, icons, ui_state, actions, expand_on_hover);
                    dirty |= r.0;
                    wake_scan |= r.1;
                });
                // Part of the desktop (clicks don't bring it forward) and square Windows corners (the box
                // draws its own): set up once, by the handle the host created.
                // Fading while the mouse is away (Settings > Desktop): the compositor animates it.
                {
                    let s = &self.layout.settings;
                    let menu_here = matches!(self.ui.menu.as_ref().map(|m| &m.target), Some(MenuTarget::Fence(i)) if *i == id);
                    let target = if !s.fade_idle || menu_here {
                        1.0
                    } else {
                        let delay = Duration::from_secs(s.fade_delay.clamp(1, 60) as u64);
                        match self.ui.hover_at.get(&id) {
                            Some(t) if t.elapsed() < delay => {
                                // Come back when it is time to fade.
                                crate::runloop::wake_after(delay - t.elapsed() + Duration::from_millis(30));
                                1.0
                            }
                            _ => s.fade_opacity.clamp(5, 100) as f32 / 100.0,
                        }
                    };
                    hosts.set_box_opacity(id, target);
                }
                if let Some(hw) = hosts.hwnd(id) {
                    if !self.ui.hwnds.contains_key(&id) {
                        if let Some(hw) = desktop::prepare_box(hw, fence.widget.is_none()) {
                            self.ui.hwnds.insert(id, hw);
                        }
                    }
                    if self.ui.rounded.get(&id) != Some(&0) && desktop::round_window(hw, 0.0) {
                        self.ui.rounded.insert(id, 0);
                    }
                    // Optional drop shadow: its own window below the box (only exists while on).
                    crate::shadow::sync(hw, fence.shadow, fence.corner);
                }
                if drew {
                    DRAW_STATS.1.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    DRAW_US.fetch_add(t0.elapsed().as_micros() as u64, std::sync::atomic::Ordering::Relaxed);
                }
            }
            crate::ink::set(false);
            self.ink_gen = ink_gen_now;
            // Wake up again when the earliest box timer is due (see HostSystem::next_due).
            if let Some(t) = hosts.next_due() {
                ctx.request_repaint_after(t.saturating_duration_since(Instant::now()).max(crate::host::MIN_PASS_GAP));
            }
        }
        if shared_ui_fingerprint(&self.ui) != fingerprint {
            self.redraw_all = true;
        }
        // DESKTOPSHEPHERD_DEBUG_DRAWS: log how many box frames each pass draws (measuring only).
        let passes = DRAW_STATS.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if passes % 300 == 0 && std::env::var_os("DESKTOPSHEPHERD_DEBUG_DRAWS").is_some() {
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
            if self.rename.is_some() {
                self.show_rename(ctx);
            } else if let Some(h) = self.hosts.as_mut() {
                h.close_panel(RENAME_PANEL);
            }
            // "Version x is available · Update · Later", once per version.
            let offer = crate::update::status().0.filter(|r| r.version != self.layout.settings.skipped_version && self.layout.settings.check_updates && self.layout.settings.updates_asked);
            // The one-time question (nothing is checked until the person answered).
            if self.persist && !self.layout.settings.updates_asked && self.started_at.elapsed() >= Duration::from_secs(10) {
                self.show_updates_ask_toast(ctx);
            } else if self.persist && !self.layout.settings.updates_asked {
                crate::runloop::wake_after(Duration::from_secs(11) - self.started_at.elapsed().min(Duration::from_secs(10)));
            } else if let Some(h) = self.hosts.as_mut() {
                h.close_panel(UPDATES_ASK_PANEL);
            }
            // A short message (a refused rename…) for 6 seconds.
            if self.note.as_ref().map(|(_, t)| t.elapsed() < Duration::from_secs(6)).unwrap_or(false) {
                self.show_note_toast(ctx);
            } else {
                self.note = None;
                if let Some(h) = self.hosts.as_mut() {
                    h.close_panel(NOTE_PANEL);
                }
            }
            if let Some(rel) = offer {
                self.show_update_toast(ctx, rel);
            } else if let Some(h) = self.hosts.as_mut() {
                h.close_panel(UPDATE_PANEL);
            }
            // "Box deleted · Undo" for 8 seconds after a delete.
            if self.deleted.last().map(|(_, t)| t.elapsed() < Duration::from_secs(8)).unwrap_or(false) {
                self.show_undo_toast(ctx);
            } else if let Some(h) = self.hosts.as_mut() {
                h.close_panel(UNDO_PANEL);
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

    if std::env::var_os("DESKTOPSHEPHERD_DEBUG_REPAINT").is_some() && ctx.cumulative_pass_nr() % 120 == 0 {
        storage::log_line(&format!(
            "box {} causes: {:?} burst={:?} rect={:?}",
            fence.id,
            ctx.repaint_causes(),
            st.burst.get(&fence.id),
            ctx.input(|i| i.viewport().inner_rect)
        ));
    }
    if std::env::var_os("DESKTOPSHEPHERD_DEBUG_DRAG").is_some() && ctx.input(|i| i.pointer.any_pressed() || i.pointer.any_released()) {
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
    if ctx.input(|i| i.pointer.has_pointer()) {
        st.hover_at.insert(fence.id, Instant::now());
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
    // (done by handle in show_fences, right after the box's frame)
    let _ = want;

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
    if !dropped.is_empty() && !fence.auto_sort && fence.widget.is_none() && fence.folder.is_none() {
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
            // Only the border: Windows draws the glass, tint and rounded corners. Drawn on the very
            // edge, so no glass shows outside it.
            painter.rect_stroke(
                rect,
                egui::CornerRadius::same(fence.corner.round().clamp(0.0, 40.0) as u8),
                fv::border_stroke(fence, 1.0),
                egui::StrokeKind::Inside,
            );
            if fence.raised {
                fv::paint_raised(&painter, rect.shrink(0.5), fence.corner.clamp(0.0, 40.0), 1.0);
            }
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
            let accepts = *from != fence.id && !fence.auto_sort && fence.widget.is_none() && fence.folder.is_none();
            if accepts && desktop::cursor_points(ppp).map(|c| current.contains(c)).unwrap_or(false) {
                painter.rect_stroke(
                    rect.shrink(1.5),
                    egui::CornerRadius::same(fence.corner.round().clamp(0.0, 40.0) as u8),
                    egui::Stroke::new(2.5_f32, accent()),
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
                let resp = ui.put(edit_rect, egui::TextEdit::singleline(&mut text).hint_text(t("Name")));
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
            if crate::ink::is_dark() {
                // Bright wallpaper: plain text in widgets turns dark too.
                let v = child.visuals_mut();
                v.override_text_color = Some(crate::ink::text());
                // Entry fields and small controls turn light, so the dark text reads on them.
                v.extreme_bg_color = Color32::from_white_alpha(150);
                v.widgets.inactive.weak_bg_fill = Color32::from_white_alpha(110);
                v.widgets.inactive.bg_fill = Color32::from_white_alpha(110);
                v.widgets.hovered.weak_bg_fill = Color32::from_white_alpha(160);
                v.widgets.active.weak_bg_fill = Color32::from_white_alpha(190);
            }
            match widgets::find(&wc.kind) {
                Some(_) if wc.kind == "search" => search_widget(&mut child, fence.id, desk, icons, actions),
                Some(kind) => {
                    let out = (kind.make)().ui(&mut child, &mut wc.data);
                    dirty |= out.changed;
                    if let Some(d) = out.repaint_after {
                        ctx.request_repaint_after(d);
                    }
                }
                None => {
                    child.label(tf("Unknown widget \"{}\"", &[&wc.kind]));
                }
            }
        } else if let Some((_, items)) = tabs.get(sel) {
            // Icon grid.
            let cell = fv::cell_size(fence, desk.spacing);
            let style = fv::text_style(fence);
            let icon = fence.icon_size as f32;
            let manual = fence.sort == SortKind::Manual && !fence.auto_sort;
            // "Pages with dots": one screenful of icons at a time; the wheel or the dots flip.
            let paged = fence.overflow == 1;
            let mut content = a.content;
            let mut page_n = 0usize;
            let mut page_h = 0.0f32;
            let mut pages = 1usize;
            if paged {
                let cols = ((content.width() / cell.x).floor() as usize).max(1);
                let rows_fit = (((content.height() - 16.0) / cell.y).floor() as usize).max(1);
                let per_page = rows_fit * cols;
                pages = items.len().div_ceil(per_page).max(1);
                page_h = rows_fit as f32 * cell.y;
                if pages > 1 {
                    content.max.y = content.min.y + page_h;
                }
                let pid = egui::Id::new(("box-page", fence.id, sel));
                page_n = ctx.data(|d| d.get_temp::<usize>(pid)).unwrap_or(0);
                // An icon asked for (search, arrow keys, typing): go to its page.
                if let Some(i) = st.scroll_to.as_deref().and_then(|k| items.iter().position(|it| it.path.eq_ignore_ascii_case(k))) {
                    page_n = i / per_page;
                    st.scroll_to = None;
                }
                if pages > 1 && ui.rect_contains_pointer(a.content) {
                    let dy = ui.input(|i| i.raw_scroll_delta.y);
                    if dy != 0.0 {
                        let mut acc = ctx.data(|d| d.get_temp::<f32>(pid.with("acc"))).unwrap_or(0.0) + dy;
                        if acc <= -30.0 {
                            page_n += 1;
                            acc = 0.0;
                        } else if acc >= 30.0 {
                            page_n = page_n.saturating_sub(1);
                            acc = 0.0;
                        }
                        ctx.data_mut(|d| d.insert_temp(pid.with("acc"), acc));
                    }
                }
                page_n = page_n.min(pages - 1);
                if pages > 1 {
                    // The dots, centred under the icons; click one to go to that page.
                    let gap = 12.0;
                    let y = a.content.max.y - 8.0;
                    let x0 = a.content.center().x - (pages - 1) as f32 * gap / 2.0;
                    for p in 0..pages {
                        let c = Pos2::new(x0 + p as f32 * gap, y);
                        let r = Rect::from_center_size(c, Vec2::new(gap, 16.0));
                        let resp = ui.interact(r, ui.id().with(("dot", fence.id, p)), Sense::click());
                        if resp.clicked() {
                            page_n = p;
                        }
                        let alpha = if p == page_n { 235 } else if resp.hovered() { 160 } else { 90 };
                        ui.painter().circle_filled(c, if p == page_n { 3.5 } else { 3.0 }, if crate::ink::is_dark() { Color32::from_black_alpha(alpha) } else { Color32::from_white_alpha(alpha) });
                    }
                }
                ctx.data_mut(|d| d.insert_temp(pid, page_n));
            }
            let mut child = ui.new_child(UiBuilder::new().max_rect(content).layout(egui::Layout::top_down(Align::Min)));
            let mut area = ScrollArea::vertical()
                .id_salt(("grid", fence.id, sel))
                .auto_shrink([false, false])
                .drag_to_scroll(false);
            if paged {
                area = area
                    .enable_scrolling(false)
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .vertical_scroll_offset(page_n as f32 * page_h);
            }
            area
                .show(&mut child, |ui| {
                    let width = ui.available_width();
                    let cols = ((width / cell.x).floor() as usize).max(1);
                    let rows = items.len().div_ceil(cols);
                    // The whole grid area also takes drags on empty space: a selection rectangle.
                    let grid_h = (rows as f32 * cell.y).max(ui.available_height()).max(pages as f32 * page_h);
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
                    st.grid_order.insert(fence.id, (items.iter().map(|i| i.path.clone()).collect(), cols));
                    let mut reorder: Option<(String, String)> = None;
                    // (icon rectangles were only needed by the old see-through drag copy)
                    let clip = ui.clip_rect();
                    for (i, it) in items.iter().enumerate() {
                        let (c, r) = (i % cols, i / cols);
                        let cr = Rect::from_min_size(grid.min + Vec2::new(pad + c as f32 * cell.x, r as f32 * cell.y), cell);
                        if st.scroll_to.as_deref().map(|k| it.path.eq_ignore_ascii_case(k)).unwrap_or(false) {
                            ui.scroll_to_rect(cr, Some(Align::Center));
                            st.scroll_to = None;
                        }
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
                        let renaming_this = st.icon_rename.as_ref().map(|r| r.box_id == fence.id && r.path.eq_ignore_ascii_case(&it.path)).unwrap_or(false);
                        if renaming_this {
                            let plain = fv::TextStyle { captions: false, ..style.clone() };
                            fv::paint_item(ui.painter(), icons, it, cr, icon, 1.0, hl, &plain);
                            rename_in_place(ui, st, actions, cr, icon, &style);
                        } else {
                            fv::paint_item(ui.painter(), icons, it, cr, icon, 1.0, hl, &style);
                        }
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
                            if std::env::var_os("DESKTOPSHEPHERD_DEBUG_DRAG").is_some() {
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
                                let label = if n == 1 { it.name.clone() } else { tf("{} items", &[&n]) };
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
                                    if std::env::var_os("DESKTOPSHEPHERD_DEBUG_DRAG").is_some() {
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
                            if !st.selection.contains(&key) {
                                st.selection.clear();
                                st.selection.insert(key.clone());
                            }
                            // Windows' own menu with DesktopShepherd' entries on top (falls back to ours).
                            actions.push(Act::ItemMenu { from: fence.id, path: it.path.clone() });
                        }
                        resp.on_hover_text_at_pointer(it.name.as_str());
                    }
                    if let Some(b) = band_rect {
                        ui.painter().rect(
                            b,
                            egui::CornerRadius::same(2),
                            Color32::from_rgba_unmultiplied(76, 194, 255, 40),
                            egui::Stroke::new(1.0_f32, accent()),
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
                            t("Everything on the desktop is in a box.")
                        } else {
                            t("Drag files here, or right-click an icon › Move to.")
                        };
                        ui.add_space(8.0);
                        ui.label(egui::RichText::new(msg).color(crate::ink::faint()).size(12.0));
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
    /// After drawing a rectangle: what to create there.
    Create,
}

/// Where a new box should go (see `DeskApp::new_fence_rect`).
#[derive(Clone, Copy, Debug)]
pub enum NewPlace {
    At(Pos2),
    Rect(Rect),
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MenuPage {
    Main,
    View,
    Sort,
    New,
    MoveTo,
    MoveToTabs(u64),
    Pages,
    Shortcut,
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
        p.rect_filled(rect, egui::CornerRadius::same(4), crate::theme::hover());
    }
    let fg = if enabled { crate::theme::text() } else { crate::theme::disabled() };
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
    ui.painter().hline(r.x_range().shrink(4.0), r.center().y, egui::Stroke::new(1.0_f32, crate::theme::line()));
}

/// A row that opens an inline colour picker (no pop-up: it would be cut off by the menu window).
/// The row starts with a square of the current colour. Returns true when the colour changed.
fn color_row(ui: &mut egui::Ui, id: &str, label: &str, color: &mut Color32, alpha: bool) -> bool {
    let mut changed = false;
    let mut job = egui::text::LayoutJob::default();
    job.append("\u{25A0}  ", 0.0, egui::TextFormat { font_id: FontId::proportional(16.0), color: color.to_opaque(), ..Default::default() });
    job.append(label, 0.0, egui::TextFormat { font_id: FontId::proportional(14.0), color: crate::theme::text(), ..Default::default() });
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
        ui.label(egui::RichText::new(text).size(12.0).color(crate::theme::faint()));
    });
}


/// One aligned row of the Personalize window: name, slider, value.
fn slider_row(ui: &mut egui::Ui, label: &str, value: String, slider: &mut dyn FnMut(&mut egui::Ui) -> bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_space(14.0);
        let (lr, _) = ui.allocate_exact_size(Vec2::new(70.0, 30.0), Sense::hover());
        ui.painter().text(Pos2::new(lr.min.x, lr.center().y), egui::Align2::LEFT_CENTER, label, FontId::proportional(14.0), crate::theme::text());
        ui.spacing_mut().slider_width = 130.0;
        changed = slider(ui);
        ui.add_sized([40.0, 30.0], egui::Label::new(egui::RichText::new(value).size(13.0).color(crate::theme::soft())).halign(Align::Max));
    });
    changed
}

/// Personalize window, left column: colour, glass and border. Returns true if the look changed.
fn look_left(ui: &mut egui::Ui, fence: &mut Fence) -> bool {
    let mut dirty = false;
                heading(ui, t("Colour"));
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
                        p.circle_stroke(c, 10.0, egui::Stroke::new(1.0_f32, crate::theme::text().linear_multiply(0.3)));
                        if selected {
                            p.circle_stroke(c, 13.0, egui::Stroke::new(2.0_f32, accent()));
                        } else if resp.hovered() {
                            p.circle_stroke(c, 13.0, egui::Stroke::new(1.0_f32, crate::theme::text().linear_multiply(0.4)));
                        }
                        if resp.on_hover_text(t(*name)).clicked() {
                            fence.color = *rgb;
                            fence.alpha = *alpha;
                            dirty = true;
                        }
                    }
                });
                // Any colour (with its opacity), picked by hand.
                let mut tint = Color32::from_rgba_unmultiplied(fence.color[0], fence.color[1], fence.color[2], fence.alpha);
                if color_row(ui, "box-colour", t("Any colour…"), &mut tint, true) {
                    let [r, g, b, a] = tint.to_srgba_unmultiplied();
                    fence.color = [r, g, b];
                    fence.alpha = a;
                    dirty = true;
                }
                sep(ui);
                let alpha_pct = format!("{}%", (fence.alpha as f32 / 2.55).round());
                dirty |= slider_row(ui, t("Opacity"), alpha_pct, &mut |ui| ui.add(egui::Slider::new(&mut fence.alpha, 0..=255).show_value(false)).changed());
                let corner_txt = format!("{}", fence.corner.round());
                dirty |= slider_row(ui, t("Corners"), corner_txt, &mut |ui| ui.add(egui::Slider::new(&mut fence.corner, 0.0..=16.0).step_by(1.0).show_value(false)).changed());
                let blur_txt = format!("{}", fence.blur);
                dirty |= slider_row(ui, t("Blur"), blur_txt, &mut |ui| ui.add(egui::Slider::new(&mut fence.blur, 0..=40).show_value(false)).changed());
                sep(ui);
                heading(ui, t("Border"));
                let bw = format!("{}", fence.border_width);
                dirty |= slider_row(ui, t("Thickness"), bw, &mut |ui| ui.add(egui::Slider::new(&mut fence.border_width, 0.0..=6.0).step_by(0.5).show_value(false)).changed());
                let [r, g, b, a] = fence.border_color;
                let mut border = Color32::from_rgba_unmultiplied(r, g, b, a);
                if color_row(ui, "border-colour", t("Border colour…"), &mut border, true) {
                    fence.border_color = border.to_srgba_unmultiplied();
                    dirty = true;
                }
    dirty
}

/// Personalize window, right column: text, icons, title and tab placement, apply to all.
fn look_right(ui: &mut egui::Ui, fence: &mut Fence, actions: &mut Vec<Act>) -> bool {
    let mut dirty = false;
                heading(ui, t("Text"));
                let [r, g, b] = fence.text_color;
                let mut text = Color32::from_rgb(r, g, b);
                if color_row(ui, "text-colour", t("Text colour…"), &mut text, false) {
                    let [r, g, b, _] = text.to_srgba_unmultiplied();
                    fence.text_color = [r, g, b];
                    dirty = true;
                }
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    dirty |= ui.checkbox(&mut fence.text_shadow, t("Shadow behind the text")).changed();
                });
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    dirty |= ui.checkbox(&mut fence.shadow, t("Drop shadow")).changed();
                    dirty |= ui.checkbox(&mut fence.raised, t("Raised look")).changed();
                });
                let ts = format!("{}", fence.title_size.round());
                dirty |= slider_row(ui, t("Title size"), ts, &mut |ui| ui.add(egui::Slider::new(&mut fence.title_size, 8.0..=32.0).step_by(1.0).show_value(false)).changed());
                let cs = format!("{}", fence.caption_size.round());
                dirty |= slider_row(ui, t("Text size"), cs, &mut |ui| ui.add(egui::Slider::new(&mut fence.caption_size, 7.0..=24.0).step_by(1.0).show_value(false)).changed());
                // Font: a drop-down list of the fonts installed on this PC.
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    let (lr, _) = ui.allocate_exact_size(Vec2::new(70.0, 30.0), Sense::hover());
                    ui.painter().text(Pos2::new(lr.min.x, lr.center().y), egui::Align2::LEFT_CENTER, t("Font"), FontId::proportional(14.0), crate::theme::text());
                    let shown = if fence.font.is_empty() { "Segoe UI".to_string() } else { fence.font.clone() };
                    egui::ComboBox::from_id_salt("font-list").selected_text(shown.clone()).width(190.0).height(320.0).show_ui(ui, |ui| {
                        for name in fv::installed_fonts() {
                            if ui.selectable_label(shown == *name, *name).clicked() && shown != *name {
                                fence.font = if *name == "Segoe UI" { String::new() } else { name.to_string() };
                                dirty = true;
                            }
                        }
                    });
                });
                if fence.widget.is_none() {
                    sep(ui);
                    heading(ui, t("Icons"));
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        dirty |= ui.checkbox(&mut fence.show_captions, t("Show names under the icons")).changed();
                    });
                    let is = format!("{}", fence.icon_size);
                    dirty |= slider_row(ui, t("Size"), is, &mut |ui| ui.add(egui::Slider::new(&mut fence.icon_size, 16..=128).step_by(4.0).show_value(false)).changed());
                    let sp = if fence.icon_spacing <= 0.0 { t("auto").to_string() } else { format!("{}", fence.icon_spacing.round()) };
                    dirty |= slider_row(ui, t("Spacing"), sp, &mut |ui| ui.add(egui::Slider::new(&mut fence.icon_spacing, 0.0..=200.0).step_by(2.0).show_value(false)).changed());
                }
                sep(ui);
                // Two-way switches for where the title and the tabs go.
                let switch = |ui: &mut egui::Ui, label: &str, first: &str, second: &str, is_first: bool| -> Option<bool> {
                    let mut pick = None;
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        let (lr, _) = ui.allocate_exact_size(Vec2::new(70.0, 30.0), Sense::hover());
                        ui.painter().text(Pos2::new(lr.min.x, lr.center().y), egui::Align2::LEFT_CENTER, label, FontId::proportional(14.0), crate::theme::text());
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for (i, text) in [first, second].into_iter().enumerate() {
                            let on = (i == 0) == is_first;
                            let fill = if on { accent().linear_multiply(0.35) } else { crate::theme::hover() };
                            let b = egui::Button::new(egui::RichText::new(text).size(13.0).color(if on { crate::theme::strong() } else { crate::theme::soft() }))
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
                // Title: off, left side, top or bottom.
                ui.horizontal(|ui| {
                    ui.add_space(14.0);
                    let (lr, _) = ui.allocate_exact_size(Vec2::new(70.0, 30.0), Sense::hover());
                    ui.painter().text(Pos2::new(lr.min.x, lr.center().y), egui::Align2::LEFT_CENTER, t("Title"), FontId::proportional(14.0), crate::theme::text());
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (pos, text) in [(TitlePos::Hidden, t("Off")), (TitlePos::Left, t("Left")), (TitlePos::Top, t("Top")), (TitlePos::Bottom, t("Bottom"))] {
                        let on = fence.title_pos == pos;
                        let fill = if on { accent().linear_multiply(0.35) } else { crate::theme::hover() };
                        let b = egui::Button::new(egui::RichText::new(text).size(13.0).color(if on { crate::theme::strong() } else { crate::theme::soft() }))
                            .fill(fill)
                            .corner_radius(egui::CornerRadius::same(4))
                            .min_size(Vec2::new(54.0, 26.0));
                        if ui.add(b).clicked() && !on {
                            fence.title_pos = pos;
                            dirty = true;
                        }
                    }
                });
                if let Some(right) = switch(ui, t("Tabs"), t("Right side"), t("Top"), fence.tab_pos == TabPos::Right) {
                    fence.tab_pos = if right { TabPos::Right } else { TabPos::Top };
                    dirty = true;
                }
                sep(ui);
                if row(ui, icon::COLOR, t("Apply this look to all boxes"), None, true).clicked() {
                    actions.push(Act::ApplyLookToAll(fence.id));
                }
                ui.add_space(4.0);    dirty
}

/// The name field under an icon being renamed (F2): Enter or a click elsewhere keeps the new name,
/// Esc cancels. The whole name is selected at first, like Explorer.
fn rename_in_place(ui: &mut egui::Ui, st: &mut UiState, actions: &mut Vec<Act>, cell: Rect, icon: f32, style: &fv::TextStyle) {
    let Some(mut r) = st.icon_rename.take() else { return };
    let font = FontId::new(style.size, style.family.clone());
    let top = cell.min.y + 6.0 + icon + 2.0;
    let rect = Rect::from_min_max(Pos2::new(cell.min.x + 1.0, top), Pos2::new(cell.max.x - 1.0, top + style.size * 1.45 + 6.0));
    ui.painter().rect_filled(rect, egui::CornerRadius::same(2), Color32::from_rgb(32, 32, 35));
    ui.painter().rect_stroke(rect, egui::CornerRadius::same(2), egui::Stroke::new(1.0_f32, Color32::from_rgb(0, 120, 215)), egui::StrokeKind::Inside);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(3.0, 2.0))));
    let out = egui::TextEdit::singleline(&mut r.text)
        .font(font)
        .frame(false)
        .horizontal_align(Align::Center)
        .text_color(Color32::WHITE)
        .desired_width(rect.width() - 6.0)
        .show(&mut child);
    let te = out.response.clone();
    if !r.selected_once {
        let mut state = out.state;
        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(r.text.chars().count()));
        state.cursor.set_char_range(Some(all));
        state.store(ui.ctx(), te.id);
        r.selected_once = true;
    }
    let window_focused = ui.input(|i| i.viewport().focused.unwrap_or(false));
    if window_focused {
        r.had_focus = true;
    }
    let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
    let clicked_elsewhere = ui.input(|i| i.pointer.any_pressed()) && !ui.rect_contains_pointer(rect);
    if esc {
        desktop::focus_desktop();
    } else if enter || clicked_elsewhere || (r.had_focus && !window_focused) {
        actions.push(Act::RenameItem { path: r.path, name: r.text });
    } else {
        if !te.has_focus() {
            te.request_focus();
        }
        st.icon_rename = Some(r);
    }
}

/// The box menu. Returns (layout changed, close the menu).
#[allow(clippy::too_many_arguments)]
fn fence_menu(ui: &mut egui::Ui, fence: &mut Fence, st: &mut UiState, page: &mut MenuPage, actions: &mut Vec<Act>, ctx_for_box: &egui::Context) -> (bool, bool) {
    let mut dirty = false;
    let mut close = false;
    match *page {
        MenuPage::Main => {
            if row(ui, icon::RENAME, t("Rename box"), None, true).clicked() {
                st.renaming = Some((fence.id, None, fence.name.clone()));
                st.box_cmds.push((fence.id, ViewportCommand::Focus));
                close = true;
            }
            let (ic_roll, label) = if fence.expanded { (icon::UP, t("Roll up")) } else { (icon::DOWN, t("Unroll")) };
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
            if row(ui, icon::LOCK, t("Lock position and size"), fence.locked.then_some(icon::CHECK), true).clicked() {
                fence.locked = !fence.locked;
                dirty = true;
                close = true;
            }
            sep(ui);
            if fence.widget.is_none() {
                if sub(ui, icon::VIEW, t("View")) {
                    *page = MenuPage::View;
                }
                if sub(ui, icon::SORT, t("Sort by")) {
                    *page = MenuPage::Sort;
                }
                if !fence.auto_sort && row(ui, icon::TAB, t("New tab"), None, true).clicked() {
                    let n = fence.tabs.len() + 1;
                    fence.tabs.push(Tab { name: tf("Tab {}", &[&n]), items: vec![] });
                    fence.sel_tab = fence.tabs.len() - 1;
                    dirty = true;
                    close = true;
                }
                if row(ui, icon::AUTO, t("Collect other desktop items"), fence.auto_sort.then_some(icon::CHECK), true)
                    .on_hover_text(t("Shows every desktop item no other box holds, sorted into tabs by kind"))
                    .clicked()
                {
                    fence.auto_sort = !fence.auto_sort;
                    dirty = true;
                    close = true;
                }
            }
            if row(ui, icon::COLOR, t("Personalize…"), None, true).clicked() {
                actions.push(Act::Personalize(fence.id));
                close = true;
            }
            let has_settings = fence.widget.as_ref().and_then(|w| widgets::find(&w.kind)).map(|k| (k.make)().has_settings()).unwrap_or(false);
            if has_settings && row(ui, icon::SETTINGS, t("Widget settings…"), None, true).clicked() {
                actions.push(Act::WidgetSettings(fence.id));
                close = true;
            }
            sep(ui);
            if sub(ui, icon::ADD, t("New")) {
                *page = MenuPage::New;
            }
            if sub(ui, icon::TAB, t("Move to page")) {
                *page = MenuPage::Pages;
            }
            if sub(ui, icon::CHEVRON, t("Keyboard shortcut")) {
                *page = MenuPage::Shortcut;
            }
            if row(ui, icon::SEARCH, t("Search desktop"), None, true).clicked() {
                st.open_search = true;
                close = true;
            }
            if row(ui, icon::SETTINGS, t("DesktopShepherd settings"), None, true).clicked() {
                actions.push(Act::OpenManager);
                close = true;
            }
            sep(ui);
            if st.confirm_delete == Some(fence.id) {
                let r = row(ui, icon::DELETE, t("Click again to delete (files stay)"), None, true);
                if r.clicked() {
                    actions.push(Act::DeleteFence(fence.id));
                    st.confirm_delete = None;
                    close = true;
                }
            } else if row(ui, icon::DELETE, t("Delete box"), None, true).clicked() {
                st.confirm_delete = Some(fence.id);
            }
        }
        MenuPage::View => {
            back(ui, page, t("View"));
            for (s, label) in [(32, t("Small icons")), (48, t("Medium icons")), (96, t("Large icons"))] {
                if check(ui, fence.icon_size == s, label) {
                    fence.icon_size = s;
                    dirty = true;
                    close = true;
                }
            }
            sep(ui);
            for (v, label) in [(0u8, t("Scroll when icons don't fit")), (1, t("Pages with dots when icons don't fit"))] {
                if check(ui, fence.overflow == v, label) {
                    fence.overflow = v;
                    dirty = true;
                    close = true;
                }
            }
        }
        MenuPage::Sort => {
            back(ui, page, t("Sort by"));
            for (k, label) in [(SortKind::Manual, t("As I arrange them")), (SortKind::Name, t("Name")), (SortKind::Size, t("Size")), (SortKind::Type, t("Item type")), (SortKind::Modified, t("Date modified")), (SortKind::OpenCount, t("Times opened"))] {
                if check(ui, fence.sort == k, label) {
                    fence.sort = k;
                    dirty = true;
                    close = true;
                }
            }
        }
        MenuPage::Shortcut => {
            back(ui, page, t("Keyboard shortcut"));
            for n in 0..=9u8 {
                let label = if n == 0 { t("None").to_string() } else { format!("Ctrl+Alt+F{}", n + 1) };
                if check(ui, fence.hotkey == n, &label) {
                    actions.push(Act::SetBoxHotkey(fence.id, n));
                    close = true;
                }
            }
            ui.label(egui::RichText::new(t("Brings the box into view; pressed again, rolls it up or down.")).size(11.5).color(crate::theme::faint()));
        }
        MenuPage::Pages => {
            back(ui, page, t("Move to page"));
            for p in 0..st.page_count {
                let label = if p == st.current_page { tf("Page {} (shown now)", &[&(p + 1)]) } else { tf("Page {}", &[&(p + 1)]) };
                if check(ui, fence.page == p, &label) {
                    fence.page = p;
                    dirty = true;
                    close = true;
                }
            }
            sep(ui);
            if row(ui, icon::ADD, t("New page"), None, true).clicked() {
                fence.page = st.page_count;
                dirty = true;
                close = true;
            }
            ui.label(egui::RichText::new(t("Switch pages with Ctrl+Alt+PageDown / PageUp or the tray icon.")).size(11.5).color(crate::theme::faint()));
        }
        MenuPage::New => {
            back(ui, page, t("New"));
            let before = actions.len();
            if row(ui, icon::ADD, t("Draw a new box…"), None, true).on_hover_text(t("Drag a rectangle anywhere on the screen (Ctrl+Alt+B)")).clicked() {
                st.start_draw = true;
                close = true;
            }
            sep(ui);
            if row(ui, icon::VIEW, t("Box"), None, true).clicked() {
                actions.push(Act::NewFence { auto: false });
                close = true;
            }
            if row(ui, icon::FOLDER, t("Folder box…"), None, true).on_hover_text(t("A box that shows what is in a folder, live")).clicked() {
                actions.push(Act::NewFolderBox);
                close = true;
            }
            sep(ui);
            for k in widgets::REGISTRY {
                if row(ui, k.icon, &tf("{} widget", &[&t(k.label)]), None, true).clicked() {
                    actions.push(Act::NewWidget(k.id));
                    close = true;
                }
            }
            // Created next to where the menu was opened.
            if actions.len() > before {
                st.new_place = Some((NewPlace::At(st.menu_at), Instant::now()));
            }
        }
        MenuPage::MoveTo | MenuPage::MoveToTabs(_) => *page = MenuPage::Main,
    }
    (dirty, close)
}

/// The menu after drawing a rectangle: what to create in it. Returns true to close the menu.
fn create_menu(ui: &mut egui::Ui, actions: &mut Vec<Act>) -> bool {
    let mut close = false;
    if row(ui, icon::VIEW, t("Box"), None, true).clicked() {
        actions.push(Act::NewFence { auto: false });
        close = true;
    }
    if row(ui, icon::FOLDER, t("Folder box…"), None, true).clicked() {
        actions.push(Act::NewFolderBox);
        close = true;
    }
    sep(ui);
    for k in widgets::REGISTRY {
        if row(ui, k.icon, t(k.label), None, true).clicked() {
            actions.push(Act::NewWidget(k.id));
            close = true;
        }
    }
    close
}

fn tab_menu(ui: &mut egui::Ui, fence: &mut Fence, tab: usize, st: &mut UiState, _ctx_for_box: &egui::Context) -> (bool, bool) {
    let mut dirty = false;
    let mut close = false;
    if row(ui, icon::RENAME, t("Rename tab"), None, true).clicked() {
        if let Some(t) = fence.tabs.get(tab) {
            st.renaming = Some((fence.id, Some(tab), t.name.clone()));
            st.box_cmds.push((fence.id, ViewportCommand::Focus));
        }
        close = true;
    }
    if row(ui, icon::TAB, t("New tab"), None, true).clicked() {
        let n = fence.tabs.len() + 1;
        fence.tabs.push(Tab { name: tf("Tab {}", &[&n]), items: vec![] });
        fence.sel_tab = fence.tabs.len() - 1;
        dirty = true;
        close = true;
    }
    let can_delete = fence.tabs.len() > 1;
    if row(ui, icon::DELETE, t("Delete tab"), None, can_delete).on_hover_text(t("Its items go back to the desktop")).clicked() && can_delete {
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
            back(ui, page, t("Move to"));
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
            if row(ui, icon::ADD, t("New box"), None, true).clicked() {
                actions.push(Act::MoveToNewFence { path: it.path.clone() });
                close = true;
            }
        }
        MenuPage::MoveToTabs(id) => {
            if row(ui, icon::BACK, t("Move to"), None, true).clicked() {
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
            let r = row(ui, icon::OPEN, t("Open"), None, true);
            if r.clicked() {
                desktop::open_item(&it.path);
                actions.push(Act::Opened(it.path.clone()));
                close = true;
            }
            if row(ui, icon::FOLDER, t("Open file location"), None, true).clicked() {
                desktop::show_in_folder(&it.path);
                close = true;
            }
            sep(ui);
            if sub(ui, icon::MOVE, t("Move to")) {
                *page = MenuPage::MoveTo;
            }
            if !from_auto && row(ui, icon::REMOVE, t("Remove from this box"), None, true).clicked() {
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
            hosts.panel(MENU_PANEL, crate::host::PanelKind::Menu, "DesktopShepherd menu", rect_px, true, force, |pctx| {
                self.menu_body(pctx, &mut m, targets, &root_ctx, &mut close, &mut dirty, &mut measured);
            });
            if !m.rounded && m.frames >= 2 {
                m.rounded = hosts.panel_hwnd(MENU_PANEL).map(|h| desktop::round_window(h, 8.0)).unwrap_or(false) || m.frames > 30;
            }
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
            match ctx.input(|i| i.viewport().focused) {
                Some(true) => m.was_focused = true,
                Some(false) if m.was_focused => *close = true,
                _ => {}
            }
            if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.viewport().close_requested()) {
                *close = true;
            }
            let frame = egui::Frame::new()
                .fill(crate::theme::menu())
                .stroke(egui::Stroke::new(1.0_f32, crate::theme::line()))
                .inner_margin(egui::Margin::same(5));
            egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.set_width(m.size.x - 12.0);
                let top = ui.cursor().min.y;
                self.ui.menu_at = m.at;
                match m.target.clone() {
                    MenuTarget::Create => {
                        *close |= create_menu(ui, &mut self.actions);
                    }
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
            hosts.panel(SETTINGS_PANEL, crate::host::PanelKind::Settings, "DesktopShepherd", rect_px, false, force, |pctx| {
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
            egui::TopBottomPanel::top("mgr-top").frame(egui::Frame::new().fill(crate::theme::bar()).inner_margin(10.0)).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("DesktopShepherd").size(20.0).strong().color(crate::theme::strong()));
                    ui.add_space(20.0);
                    ui.selectable_value(&mut self.page, Page::Boxes, t("Boxes"));
                    ui.selectable_value(&mut self.page, Page::Layout, t("Layout"));
                    ui.selectable_value(&mut self.page, Page::Settings, t("Settings"));
                });
            });
            egui::CentralPanel::default().frame(egui::Frame::new().fill(crate::theme::deep()).inner_margin(12.0)).show(ctx, |ui| {
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
        // A short row: the three kinds of box, and every widget behind one "Add widget" list (it
        // grows with new widgets without running off the window).
        ui.horizontal_wrapped(|ui| {
            if ui.button(t("➕ New box")).clicked() {
                self.actions.push(Act::NewFence { auto: false });
            }
            if ui.button(t("➕ New auto-sort box")).on_hover_text(t("Collects every desktop item no other box holds, sorted into tabs by kind")).clicked() {
                self.actions.push(Act::NewFence { auto: true });
            }
            if ui.button(t("➕ New folder box…")).on_hover_text(t("A box that shows what is in a folder, live")).clicked() {
                self.actions.push(Act::NewFolderBox);
            }
            ui.separator();
            ui.menu_button(tf("➕ Add widget  {}", &[&icon::DOWN]), |ui| {
                ui.set_min_width(230.0);
                for k in widgets::REGISTRY {
                    let text = egui::RichText::new(format!("{}    {}", k.icon, t(k.label))).size(14.0);
                    if ui.add(egui::Button::new(text).frame(false).min_size(Vec2::new(220.0, 28.0))).clicked() {
                        self.actions.push(Act::NewWidget(k.id));
                        ui.close_menu();
                    }
                }
            });
        });
        if !self.deleted.is_empty() {
            ui.add_space(4.0);
            let mut restore = None;
            ui.horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(t("Recently deleted:")).color(crate::theme::soft()));
                for (i, (f, _)) in self.deleted.iter().enumerate().rev() {
                    if ui.button(format!("↶ {}", f.name)).on_hover_text(t("Bring this box back")).clicked() {
                        restore = Some(i);
                    }
                }
            });
            if let Some(i) = restore {
                self.actions.push(Act::RestoreDeleted(i));
            }
        }
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
            egui::RichText::new(tf(
                "{} items on the desktop, {} not in a box yet. Tip: right-click a box for its options; double-click its title to roll it up.",
                &[&self.items.values().filter(|v| v.on_desktop).count(), &unsorted],
            ))
            .color(crate::theme::soft()),
        );
        ui.add_space(8.0);
        let mut dirty = false;
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Grid::new("boxes").num_columns(5).spacing([16.0, 8.0]).striped(true).show(ui, |ui| {
                ui.strong(t("Name"));
                ui.strong(t("Kind"));
                ui.strong(t("Contents"));
                ui.strong(t("State"));
                ui.strong("");
                ui.end_row();
                for f in self.layout.fences.iter_mut() {
                    // A fixed size: in a grid, a text field otherwise keeps the column's first
                    // (tiny) width forever.
                    dirty |= ui.add_sized([200.0, 26.0], egui::TextEdit::singleline(&mut f.name)).changed();
                    let kind = match &f.widget {
                        Some(w) => t(widgets::find(&w.kind).map(|k| k.label).unwrap_or("Widget")).to_string(),
                        None if f.auto_sort => t("Auto-sort box").into(),
                        None => t("Box").into(),
                    };
                    ui.label(kind);
                    if f.widget.is_some() {
                        ui.label("—");
                    } else if f.auto_sort {
                        ui.label(tf("{} items", &[&unsorted]));
                    } else {
                        let n: usize = f.tabs.iter().map(|t| t.items.len()).sum();
                        ui.label(tf("{} items in {} tab(s)", &[&n, &f.tabs.len()]));
                    }
                    ui.horizontal(|ui| {
                        if ui.selectable_label(!f.expanded, t("Rolled up")).clicked() {
                            f.expanded = !f.expanded;
                            let h = if f.expanded { f.rect[3] } else { ROLLED_HEIGHT };
                            self.ui.box_cmds.push((f.id, ViewportCommand::InnerSize(Vec2::new(f.rect[2], h))));
                            dirty = true;
                        }
                        dirty |= ui.checkbox(&mut f.locked, t("Locked")).changed();
                    });
                    if self.ui.confirm_delete == Some(f.id) {
                        if ui.button(egui::RichText::new(t("Confirm delete")).color(crate::theme::warn())).clicked() {
                            self.actions.push(Act::DeleteFence(f.id));
                            self.ui.confirm_delete = None;
                        }
                    } else if ui.button(t("Delete")).clicked() {
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
        // Dots switch between "all monitors" (0) and each monitor on its own (1, 2, ...).
        let monitors = if self.monitors.is_empty() { desktop::monitors() } else { self.monitors.clone() };
        let fixed = self.layout.settings.monitor_mode == crate::model::MonitorMode::Fixed;
        let role_name = |i: usize| match i {
            0 => t("Main monitor").to_string(),
            1 => t("Second monitor").to_string(),
            2 => t("Third monitor").to_string(),
            n => tf("Monitor {}", &[&(n + 1)]),
        };
        let label_of = |i: usize, m: &desktop::Monitor| -> String {
            let dev = m.name.trim_start_matches("\\\\.\\").to_string();
            let size = format!("{}×{}", m.rect.width().round(), m.rect.height().round());
            if fixed {
                // Fixed mode: boxes stay on this physical display, whatever its role.
                tf("{} (fixed){}  ·  {}", &[&dev, &(if m.primary { t(", main now") } else { "" }), &size])
            } else {
                format!("{}  ·  {dev}  ·  {size}", role_name(i))
            }
        };
        let vid = ui.make_persistent_id("layout-view");
        let mut view: usize = ui.ctx().data(|d| d.get_temp(vid)).unwrap_or(0);
        if view > monitors.len() || monitors.len() < 2 {
            view = 0;
        }
        ui.horizontal(|ui| {
            let title = if view == 0 {
                if monitors.len() > 1 { tf("All {} monitors", &[&monitors.len()]) } else { t("Your desktop, as the boxes sit on it").to_string() }
            } else {
                label_of(view - 1, &monitors[view - 1])
            };
            ui.label(egui::RichText::new(title).color(crate::theme::soft()).size(15.0));
            ui.add_space(12.0);
            let [sw, sh] = desktop::primary_screen_points();
            if self.persist && ui.button(t("Export layout…")).on_hover_text(t("Save all boxes and settings to a file (it includes your agenda links)")).clicked() {
                self.actions.push(Act::ExportLayout);
            }
            if self.persist && ui.button(t("Import layout…")).on_hover_text(t("Replace the boxes and settings with a saved file (the current ones are backed up first)")).clicked() {
                self.actions.push(Act::ImportLayout);
            }
            if self.persist && ui.button(t("Bring every box back on screen")).clicked() {
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
        if monitors.len() > 1 {
            ui.label(
                egui::RichText::new(if fixed {
                    t("Boxes stay on their display (Settings > Monitors: fixed).")
                } else {
                    t("Boxes follow the monitor's role: main, second… (Settings > Monitors: by role).")
                })
                .size(12.0)
                .color(crate::theme::faint()),
            );
        }
        ui.add_space(6.0);
        // The area shown: one monitor, or all of them together.
        let screen = if view == 0 {
            monitors.iter().map(|m| m.rect).reduce(|a, b| a.union(b)).unwrap_or(Rect::from_min_size(Pos2::ZERO, Vec2::new(1920.0, 1080.0)))
        } else {
            monitors[view - 1].rect
        };
        let dots_h = if monitors.len() > 1 { 22.0 } else { 0.0 };
        let avail = ui.available_rect_before_wrap();
        let room = Vec2::new(avail.width(), (avail.height() - dots_h).max(50.0));
        let scale = (room.x / screen.width()).min(room.y / screen.height()).min(1.0);
        let area = Rect::from_min_size(avail.min, screen.size() * scale);
        ui.allocate_rect(area, Sense::hover());
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, Color32::from_rgb(12, 14, 18));
        let to_area = |r: Rect| Rect::from_min_size(area.min + (r.min - screen.min) * scale, r.size() * scale);
        // Each monitor: its own background and name in the corner.
        for (i, m) in monitors.iter().enumerate() {
            let r = to_area(m.rect);
            painter.rect_filled(r.shrink(1.0), 4.0, Color32::from_rgb(20, 24, 32));
            painter.rect_stroke(r.shrink(1.0), 4.0, egui::Stroke::new(1.0_f32, Color32::from_white_alpha(if view == i + 1 { 0 } else { 40 })), egui::StrokeKind::Inside);
        }
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
            let br = Rect::from_min_size(Pos2::new(f.rect[0], f.rect[1]), Vec2::new(f.rect[2], h));
            if !screen.intersects(br) {
                continue; // on another monitor
            }
            // The settings window is drawn by the same egui context as the boxes, so the boxes'
            // icon cache serves the preview.
            fv::paint_static(&painter, &mut self.icons, f, &desk, to_area(br), scale);
        }
        // A monitor with no box: say so rather than show an empty area.
        let on_screen = self.layout.fences.iter().any(|f| screen.intersects(Rect::from_min_size(Pos2::new(f.rect[0], f.rect[1]), Vec2::new(f.rect[2], f.rect[3]))));
        if view > 0 && !on_screen {
            painter.text(area.center(), egui::Align2::CENTER_CENTER, t("No boxes on this monitor yet"), FontId::proportional(15.0), Color32::from_gray(150));
        }
        // Monitor names on top of the boxes, in each monitor's bottom-left corner.
        if view == 0 && monitors.len() > 1 {
            for (i, m) in monitors.iter().enumerate() {
                let r = to_area(m.rect);
                let text = if fixed { m.name.trim_start_matches("\\\\.\\").to_string() } else { role_name(i) };
                let g = painter.layout_no_wrap(text, FontId::proportional(13.0), Color32::WHITE);
                let size = g.size() + Vec2::new(12.0, 6.0);
                let tag = Rect::from_min_size(Pos2::new(r.min.x + 8.0, r.max.y - 8.0 - size.y), size);
                painter.rect_filled(tag, 4.0, Color32::from_rgba_unmultiplied(70, 140, 230, 230));
                painter.galley(tag.min + Vec2::new(6.0, 3.0), g, Color32::WHITE);
            }
        }
        // The dots: all monitors, then each one.
        if monitors.len() > 1 {
            let n = monitors.len() + 1;
            let gap = 16.0;
            let y = area.max.y + dots_h / 2.0 + 2.0;
            let cx = avail.center().x.min(area.center().x.max(avail.min.x + gap * n as f32));
            for i in 0..n {
                let c = Pos2::new(cx + (i as f32 - (n as f32 - 1.0) / 2.0) * gap, y);
                let r = Rect::from_center_size(c, Vec2::new(gap, 18.0));
                let tip = if i == 0 { t("All monitors").to_string() } else { label_of(i - 1, &monitors[i - 1]) };
                let resp = ui.interact(r, ui.id().with(("layout-dot", i)), Sense::click()).on_hover_text(tip);
                if resp.clicked() {
                    view = i;
                }
                let alpha = if i == view { 235 } else if resp.hovered() { 160 } else { 90 };
                ui.painter().circle_filled(c, if i == view { 4.0 } else { 3.5 }, crate::theme::text().linear_multiply(alpha as f32 / 255.0));
            }
        }
        ui.ctx().data_mut(|d| d.insert_temp(vid, view));
    }

    fn page_settings(&mut self, ui: &mut egui::Ui) {
        let s_setups: Vec<String> = { let mut k: Vec<String> = self.layout.setup_positions.keys().cloned().collect(); k.sort(); k };
        let mut forget_setup: Option<String> = None;
        let s = &mut self.layout.settings;
        let mut dirty = false;
        let mut icons_changed = false;
        ui.horizontal(|ui| {
            ui.label(crate::i18n::t("Language"));
            for (v, name) in crate::i18n::CHOICES {
                let label = if *v == 0 { crate::i18n::t(name) } else { name };
                if ui.selectable_label(s.language == *v, label).clicked() && s.language != *v {
                    s.language = *v;
                    crate::i18n::set_lang(*v);
                    dirty = true;
                }
            }
        });
        ui.add_space(6.0);
        ui.heading(t("Desktop"));
        dirty |= ui.checkbox(&mut s.desktop_menu, t("Add \"DesktopShepherd\" to the desktop's right-click menu (new box or widget where you click)")).changed();
        ui.label(egui::RichText::new(t("On Windows 11 it is under \"Show more options\". Or press Ctrl+Alt+B anywhere and drag a rectangle.")).color(crate::theme::faint()).size(12.0));
        if ui.checkbox(&mut s.hide_desktop_icons, t("Hide Windows' own desktop icons (the boxes show them)")).changed() {
            dirty = true;
            icons_changed = true;
        }
        ui.label(egui::RichText::new(t("They always come back when DesktopShepherd closes, even after a crash.")).color(crate::theme::faint()).size(12.0));
        dirty |= ui.checkbox(&mut s.expand_on_hover, t("Open rolled-up boxes while the mouse is over them")).changed();
        ui.horizontal(|ui| {
            dirty |= ui.checkbox(&mut s.fade_idle, t("Fade boxes while the mouse is away")).changed();
            if s.fade_idle {
                ui.label(t("to"));
                dirty |= ui.add(egui::Slider::new(&mut s.fade_opacity, 5..=90).suffix(" %")).changed();
                ui.label(t("after"));
                dirty |= ui.add(egui::DragValue::new(&mut s.fade_delay).range(1..=60).suffix(" s")).changed();
            }
        });
        ui.horizontal(|ui| {
            dirty |= ui.checkbox(&mut s.snap, t("Snap boxes to each other and to the screen edges")).changed();
            ui.label(t("gap"));
            dirty |= ui.add(egui::DragValue::new(&mut s.snap_gap).range(0.0..=40.0).speed(0.5).suffix(" pt")).changed();
        });
        ui.horizontal(|ui| {
            dirty |= ui.checkbox(&mut s.snap_grid, t("Also snap to a grid")).changed();
            ui.label(t("size"));
            dirty |= ui.add(egui::DragValue::new(&mut s.grid_size).range(4.0..=200.0).speed(1.0).suffix(" pt")).changed();
        });
        ui.label(egui::RichText::new(t("Hold Alt while moving or resizing a box to place it freely.")).color(crate::theme::faint()).size(12.0));
        dirty |= ui.checkbox(&mut s.follow_accent, t("Use Windows' accent colour for highlights")).changed();
        dirty |= ui.checkbox(&mut s.auto_ink, t("Darker text on bright wallpapers")).changed();
        ui.horizontal_wrapped(|ui| {
            ui.label(t("Menus and settings:"));
            for (i, label) in [t("Like Windows"), t("Dark"), t("Light")].into_iter().enumerate() {
                if ui.selectable_label(s.theme as usize == i, label).clicked() && s.theme as usize != i {
                    s.theme = i as u8;
                    dirty = true;
                }
            }
        });
        dirty |= ui.checkbox(&mut s.search_apps, t("Quick search also finds programs (from the Start menu)")).changed();
        #[cfg(windows)]
        ui.horizontal_wrapped(|ui| {
            ui.label(t("Quick search shortcut:"));
            for (i, (label, _, _)) in crate::runloop::SEARCH_HOTKEYS.iter().enumerate() {
                if ui.selectable_label(s.search_hotkey == i, t(*label)).clicked() && s.search_hotkey != i {
                    s.search_hotkey = i;
                    dirty = true;
                }
            }
        });
        #[cfg(windows)]
        if !self.hotkey_ok {
            ui.label(egui::RichText::new(t("That shortcut is already used by another program: pick another one.")).color(crate::theme::warn()).size(12.0));
        }
        #[cfg(windows)]
        ui.horizontal_wrapped(|ui| {
            ui.label(t("Hide / show all boxes:"));
            for (i, (label, _, _)) in crate::runloop::SEARCH_HOTKEYS.iter().enumerate() {
                if ui.selectable_label(s.hide_hotkey == i, t(*label)).clicked() && s.hide_hotkey != i {
                    s.hide_hotkey = i;
                    dirty = true;
                }
            }
        });
        #[cfg(windows)]
        if !self.hide_hotkey_ok {
            ui.label(egui::RichText::new(t("That shortcut is already used by another program: pick another one.")).color(crate::theme::warn()).size(12.0));
        }
        #[cfg(windows)]
        ui.horizontal_wrapped(|ui| {
            ui.label(t("Typing on the desktop:"));
            for (i, label) in [t("Does nothing"), t("Jumps to the icon that starts with it"), t("Starts a quick search")].iter().enumerate() {
                if ui.selectable_label(s.desktop_typing as usize == i, *label).clicked() && s.desktop_typing as usize != i {
                    s.desktop_typing = i as u8;
                    dirty = true;
                }
            }
        });
        ui.label(egui::RichText::new(t("Click an empty part of the desktop, then type: the same letter again goes to the next match, Enter opens it.")).color(crate::theme::faint()).size(12.0));
        ui.label(egui::RichText::new(t("Frosted glass: set each box's blur in its menu (Personalize > Blur).")).color(crate::theme::faint()).size(12.0));
        ui.add_space(10.0);
        ui.heading(t("Monitors"));
        dirty |= ui.checkbox(&mut s.link_vdesks, t("Each Windows virtual desktop shows its own page of boxes (desktop 1 = page 1, desktop 2 = page 2…)")).changed();
        if ui.checkbox(&mut s.per_setup, t("Remember box positions for each monitor setup (e.g. laptop alone / docked)")).changed() {
            dirty = true;
        }
        if s.per_setup {
            let mut forget = None;
            for key in s_setups.iter() {
                ui.horizontal(|ui| {
                    let here = *key == self.current_setup;
                    ui.label(egui::RichText::new(format!("{}{key}", if here { "● " } else { "   " })).size(12.0).color(crate::theme::soft()));
                    if !here && ui.small_button(t("Forget")).clicked() {
                        forget = Some(key.clone());
                    }
                });
            }
            if let Some(k) = forget {
                forget_setup = Some(k);
            }
        }
        let before = s.monitor_mode;
        ui.add_space(4.0);
        ui.label(egui::RichText::new(t("Bind each box to:")).strong());
        ui.radio_value(
            &mut s.monitor_mode,
            crate::model::MonitorMode::Role,
            t("The monitor's role (main, second…): when the roles change, the boxes move with them"),
        );
        ui.radio_value(
            &mut s.monitor_mode,
            crate::model::MonitorMode::Fixed,
            t("The monitor itself: boxes always stay on the monitor where they were created or moved"),
        );
        ui.label(egui::RichText::new(t("Moving a box to another monitor binds it there. Settings > Layout shows each monitor.")).color(crate::theme::faint()).size(12.0));
        let monitors_changed = s.monitor_mode != before;
        dirty |= monitors_changed;
        for (i, m) in self.monitors.iter().enumerate() {
            let role = match i {
                0 => t("Main monitor").to_string(),
                1 => t("Second monitor").to_string(),
                n => tf("Monitor {}", &[&(n + 1)]),
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
                egui::RichText::new(tf(
                    "{}: {} × {} ({}) — {} box(es)",
                    &[&role, &m.rect.width().round(), &m.rect.height().round(), &m.name.trim_start_matches("\\\\.\\"), &count],
                ))
                .color(crate::theme::faint())
                .size(12.0),
            );
        }
        ui.add_space(10.0);
        ui.heading(t("Auto-sort tabs"));
        ui.label(
            egui::RichText::new(t("Each tab collects items by file type, name pattern (* and ?) and/or how recently they changed; the first matching rule wins. Folders and everything else get their own tabs."))
                .color(crate::theme::faint())
                .size(12.0),
        );
        let mut remove = None;
        egui::Grid::new("cats").num_columns(5).spacing([10.0, 6.0]).show(ui, |ui| {
            for (i, r) in s.categories.iter_mut().enumerate() {
                dirty |= ui.add_sized([120.0, 26.0], egui::TextEdit::singleline(&mut r.name)).changed();
                let mut exts = r.exts.join(", ");
                if ui.add_sized([300.0, 26.0], egui::TextEdit::singleline(&mut exts).hint_text(t("types: zip, rar, 7z"))).changed() {
                    r.exts = exts
                        .split([',', ' ', ';'])
                        .map(|e| e.trim().trim_start_matches('.').to_lowercase())
                        .filter(|e| !e.is_empty())
                        .collect();
                    dirty = true;
                }
                dirty |= ui.add_sized([150.0, 26.0], egui::TextEdit::singleline(&mut r.pattern).hint_text(t("name: *invoice*"))).changed();
                ui.horizontal(|ui| {
                    ui.label(t("changed in last"));
                    dirty |= ui.add(egui::DragValue::new(&mut r.days).range(0..=3650).suffix(t(" days"))).on_hover_text(t("0 = any age")).changed();
                });
                if ui.small_button(t("Remove")).clicked() {
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
            if ui.button(t("Add tab rule")).clicked() {
                s.categories.push(crate::model::CategoryRule { name: t("New tab").into(), ..Default::default() });
                dirty = true;
            }
            if ui.button(t("Reset to defaults")).clicked() {
                s.categories = crate::model::default_categories();
                dirty = true;
            }
        });
        ui.add_space(10.0);
        ui.heading(t("Computer"));
        if ui.checkbox(&mut s.start_with_windows, t("Start DesktopShepherd when Windows starts")).changed() {
            if self.persist {
                desktop::set_start_with_windows(s.start_with_windows);
            }
            dirty = true;
        }
        if ui.checkbox(&mut s.low_priority, t("Run at low priority (never slows other programs)")).changed() {
            if self.persist {
                desktop::set_low_priority(s.low_priority);
            }
            dirty = true;
        }
        ui.label(egui::RichText::new(t("New, renamed and removed desktop files show up as soon as Windows reports them.")).color(crate::theme::faint()).size(12.0));
        ui.add_space(10.0);
        ui.heading(t("Sync"));
        match storage::sync_path() {
            Some(p) => {
                if ui.checkbox(&mut s.sync_onedrive, t("Keep my boxes and settings in OneDrive (shared between my PCs)")).changed() {
                    dirty = true;
                    if !s.sync_onedrive {
                        self.sync_seen = None;
                    }
                }
                ui.label(egui::RichText::new(tf("File: {}. Changes from another PC are loaded within half a minute; your layout is backed up first.", &[&p.display()])).size(12.0).color(crate::theme::faint()));
            }
            None => {
                ui.label(egui::RichText::new(t("OneDrive isn't set up on this PC.")).size(12.0).color(crate::theme::faint()));
            }
        }
        ui.add_space(10.0);
        ui.heading(t("Updates"));
        {
            let (newer, err, busy, note) = crate::update::status();
            ui.horizontal(|ui| {
                ui.label(tf("This is version {}.", &[&env!("CARGO_PKG_VERSION")]));
                if busy {
                    ui.label(egui::RichText::new(t("Checking…")).color(crate::theme::faint()));
                } else if ui.button(t("Check now")).clicked() {
                    crate::update::check(Duration::ZERO, true);
                }
            });
            dirty |= ui.checkbox(&mut s.check_updates, t("Look for new versions automatically (once a day)")).changed();
            if let Some(rel) = newer {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(tf("Version {} is available.", &[&rel.version])).strong().color(if crate::theme::is_light() { Color32::from_rgb(20, 120, 40) } else { Color32::from_rgb(120, 210, 130) }));
                    if ui.button(t("Update now")).clicked() {
                        crate::update::install(rel.clone());
                    }
                });
            } else if let Some(e) = err {
                ui.label(egui::RichText::new(e).size(12.0).color(crate::theme::warn()));
            } else if !busy && crate::update::checked() {
                ui.label(egui::RichText::new(t("You have the newest version.")).size(12.0).color(crate::theme::faint()));
            }
            if let Some(n) = note {
                ui.label(egui::RichText::new(n).size(12.0).color(crate::theme::soft()));
            }
        }
        ui.add_space(10.0);
        ui.heading(t("Saved layouts"));
        ui.label(egui::RichText::new(t("A copy of your layout is kept each time DesktopShepherd starts (the last 10).")).color(crate::theme::faint()).size(12.0));
        let mut restore = None;
        for b in self.backups.iter().rev().take(10) {
            ui.horizontal(|ui| {
                ui.label(b.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
                if self.persist && ui.small_button(t("Restore")).clicked() {
                    restore = Some(b.clone());
                }
            });
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button(t("Open data folder")).clicked() {
                desktop::open_folder(&storage::data_dir());
            }
            if self.persist && ui.button(t("Quit DesktopShepherd")).clicked() {
                self.actions.push(Act::Quit);
            }
        });
        ui.add_space(6.0);
        ui.label(egui::RichText::new(tf("{} desktop items", &[&self.items.len()])).color(crate::theme::disabled()).size(11.0));
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
        if let Some(k) = forget_setup {
            self.layout.setup_positions.remove(&k);
            self.mark_dirty();
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
        name: t("Apps").into(),
        rect: [m, m, apps_w, top_h],
        tabs: vec![Tab { name: "Apps".into(), items: apps.iter().map(|i| i.path.clone()).collect() }],
        ..Fence::default()
    });
    let id = layout.new_id();
    layout.fences.push(Fence {
        id,
        name: t("Desktop").into(),
        rect: [m * 2.0 + apps_w, m, sw - apps_w - m * 3.0, top_h],
        auto_sort: true,
        ..Fence::default()
    });
    let id = layout.new_id();
    layout.fences.push(Fence {
        id,
        name: t("Clock").into(),
        rect: [m, m * 2.0 + top_h, 260.0, 130.0],
        title_pos: TitlePos::Top,
        widget: Some(WidgetConfig { kind: "clock".into(), data: serde_json::json!({}) }),
        ..Fence::default()
    });
    let id = layout.new_id();
    layout.fences.push(Fence {
        id,
        name: t("Note").into(),
        rect: [m * 2.0 + 260.0, m * 2.0 + top_h, 320.0, 200.0],
        title_pos: TitlePos::Top,
        widget: Some(WidgetConfig {
            kind: "note".into(),
            data: serde_json::json!({
                "text": t("Welcome to DesktopShepherd!\n• Drag a box by its title, resize from its edges.\n• Double-click a title to roll the box up.\n• Right-click a box or an icon for options.")
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
    // Version 3: the person asked that the normal desktop icons disappear while DesktopShepherd runs.
    if layout.version < 3 {
        layout.settings.hide_desktop_icons = true;
        layout.version = 3;
    }
    // The screen area in pixels; box positions are in points (pixels / display scale).
    let vs = {
        let px = desktop::virtual_screen();
        let s = desktop::system_scale().max(0.5);
        [px[0] / s, px[1] / s, px[2] / s, px[3] / s]
    };
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
        // A widget's saved data is always an object (a hand-edited file could hold anything else,
        // and widgets add fields to it).
        if let Some(w) = f.widget.as_mut() {
            if !w.data.is_object() {
                w.data = serde_json::json!({});
            }
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
/// The highlight colour: Windows' accent colour (Settings > Desktop "Use Windows' accent colour"),
/// or Windows 11's default light blue.
static ACCENT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x4C_C2_FF);

pub fn accent() -> Color32 {
    let v = ACCENT.load(std::sync::atomic::Ordering::Relaxed);
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// Reads Windows' accent colour (if `follow`), made light enough to read on the dark boxes.
/// Returns true if it changed.
fn refresh_accent(follow: bool) -> bool {
    let mut rgb: u32 = 0x4C_C2_FF;
    #[cfg(windows)]
    if follow {
        use winreg::enums::HKEY_CURRENT_USER;
        if let Ok(k) = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey("Software\\Microsoft\\Windows\\DWM") {
            if let Ok(v) = k.get_value::<u32, _>("AccentColor") {
                // Stored as 0xAABBGGRR.
                let (r, g, b) = ((v & 0xFF) as f32, ((v >> 8) & 0xFF) as f32, ((v >> 16) & 0xFF) as f32);
                // Too dark to read on dark glass: mix with white until it is light enough.
                let lum = 0.299 * r + 0.587 * g + 0.114 * b;
                let t = if lum < 140.0 { ((140.0 - lum) / (255.0 - lum).max(1.0)).clamp(0.0, 0.8) } else { 0.0 };
                let mix = |c: f32| (c + (255.0 - c) * t).round() as u32;
                rgb = (mix(r) << 16) | (mix(g) << 8) | mix(b);
            }
        }
    }
    let _ = follow;
    ACCENT.swap(rgb, std::sync::atomic::Ordering::Relaxed) != rgb
}

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
        if let Ok(bytes) = std::fs::read(fv::fonts_dir().join(file)) {
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
        let accent = accent(); // Windows' accent colour (or Windows 11's default blue)
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

    // The app icon; if it can't be decoded, a small drawn one (four rounded tiles).
    let embedded = image::load_from_memory(include_bytes!("../assets/icon/icon-32.png")).ok().map(|i| i.to_rgba8()).and_then(|b| {
        let (w, h) = (b.width(), b.height());
        tray_icon::Icon::from_rgba(b.into_raw(), w, h).ok()
    });
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
    let icon = match embedded {
        Some(i) => i,
        None => tray_icon::Icon::from_rgba(rgba, n as u32, n as u32).ok()?,
    };
    let menu = Menu::new();
    let open = MenuItem::new(t("Open DesktopShepherd"), true, None);
    let new_box = MenuItem::new(t("New box"), true, None);
    let search = MenuItem::new(t("Search desktop"), true, None);
    let hide = MenuItem::new(t("Hide / show boxes"), true, None);
    let next_page = MenuItem::new(t("Next page of boxes"), true, None);
    let draw_box = MenuItem::new(t("Draw a new box…  (Ctrl+Alt+B)"), true, None);
    let undo_delete = MenuItem::new(t("Bring back the last deleted box"), true, None);
    let quit = MenuItem::new(t("Quit"), true, None);
    let _ = menu.append_items(&[&open, &search, &hide, &next_page, &new_box, &draw_box, &undo_delete, &PredefinedMenuItem::separator(), &quit]);
    let (open_id, new_id, quit_id, search_id, hide_id, page_id, draw_id, undo_id) = (open.id().clone(), new_box.id().clone(), quit.id().clone(), search.id().clone(), hide.id().clone(), next_page.id().clone(), draw_box.id().clone(), undo_delete.id().clone());
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
            } else if e.id == hide_id {
                Some(TrayCmd::HideShow)
            } else if e.id == page_id {
                Some(TrayCmd::NextPage)
            } else if e.id == draw_id {
                Some(TrayCmd::DrawBox)
            } else if e.id == undo_id {
                Some(TrayCmd::UndoDelete)
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
        .with_tooltip("DesktopShepherd")
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
        // Folder boxes: the items of their folder.
        let folders: Vec<(String, String)> = self
            .layout
            .fences
            .iter()
            .filter_map(|f| f.folder.as_ref().map(|d| (d.trim_end_matches(['\\', '/']).to_lowercase(), f.name.clone())))
            .collect();
        if !folders.is_empty() {
            for key in self.items.keys() {
                let parent = std::path::Path::new(key).parent().map(|p| p.to_string_lossy().to_lowercase()).unwrap_or_default();
                if let Some((_, name)) = folders.iter().find(|(d, _)| *d == parent) {
                    m.entry(key.clone()).or_insert_with(|| name.clone());
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
        // Desktop items, and (if turned on) the Start menu's programs.
        let apps = crate::apps::list();
        let pool = crate::apps::search_pool(self.items.values(), &apps, self.layout.settings.search_apps);
        let results: Vec<DeskItem> = crate::search::rank(pool, &s.query, 12).into_iter().cloned().collect();
        let homes = self.item_homes();
        let app_paths: HashSet<String> = apps.iter().map(|a| a.path.to_lowercase()).collect();
        let other = self.layout.fences.iter().find(|f| f.auto_sort).map(|f| f.name.clone()).unwrap_or_else(|| t("Desktop").into());
        if s.sel >= results.len() {
            s.sel = results.len().saturating_sub(1);
        }
        let mut close = false;
        let mut open: Option<(String, bool)> = None;
        let mut measured = s.height;
        let force = self.panel_force;
        if let Some(mut hosts) = self.hosts.take() {
            let icons = &mut self.icons;
            hosts.panel(SEARCH_PANEL, crate::host::PanelKind::Menu, "DesktopShepherd search", rect_px, true, force, |pctx| {
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
                    .fill(crate::theme::card())
                    .stroke(egui::Stroke::new(1.0_f32, crate::theme::line()))
                    .inner_margin(egui::Margin::same(10));
                egui::CentralPanel::default().frame(frame).show(pctx, |ui| {
                    let top = ui.cursor().min.y;
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(icon::SEARCH.to_string()).size(18.0).color(crate::theme::soft()));
                        let te = ui.add(
                            egui::TextEdit::singleline(&mut s.query)
                                .hint_text(t("Search the desktop…"))
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
                        ui.label(egui::RichText::new(t("Type part of a name. Enter opens, Ctrl+Enter opens its folder, Esc closes.")).size(12.5).color(crate::theme::faint()));
                    } else if results.is_empty() {
                        ui.label(egui::RichText::new(t("Nothing on the desktop matches.")).size(13.0).color(crate::theme::faint()));
                    }
                    for (i, it) in results.iter().enumerate() {
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 34.0), Sense::click());
                        if resp.hovered() && ui.input(|inp| inp.pointer.delta() != Vec2::ZERO) {
                            s.sel = i;
                        }
                        let p = ui.painter();
                        if i == s.sel {
                            p.rect_filled(rect, egui::CornerRadius::same(5), accent().linear_multiply(0.28));
                        }
                        let ir = Rect::from_min_size(Pos2::new(rect.min.x + 6.0, rect.center().y - 11.0), Vec2::splat(22.0));
                        match icons.get(&it.path, (22.0 * ppp).round() as u32) {
                            Some(t) => {
                                p.image(t, ir, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                            }
                            None => {
                                p.rect_filled(ir.shrink(3.0), egui::CornerRadius::same(3), crate::theme::disabled());
                            }
                        }
                        let key = it.path.to_lowercase();
                        let home = homes.get(&key).cloned().unwrap_or_else(|| if app_paths.contains(&key) { t("Program").to_string() } else { other.clone() });
                        p.text(Pos2::new(rect.min.x + 38.0, rect.center().y), egui::Align2::LEFT_CENTER, &it.name, FontId::proportional(14.5), crate::theme::text());
                        p.text(Pos2::new(rect.max.x - 8.0, rect.center().y), egui::Align2::RIGHT_CENTER, home, FontId::proportional(12.0), crate::theme::faint());
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

impl DeskApp {
    /// Type to jump (like the Windows desktop): a letter selects the next icon whose name starts
    /// with it; the same letter again goes on to the next one; letters typed quickly (within a
    /// second) narrow it down ("st" → "Steam"). Looks through the open boxes, top to bottom and
    /// left to right, every tab, and switches to the tab and scrolls the icon into view.
    fn type_ahead(&mut self, c: char) {
        let now = Instant::now();
        if self.ta_last.map(|t| now - t > Duration::from_millis(1000)).unwrap_or(true) {
            self.ta_buf.clear();
        }
        self.ta_buf.push(c);
        self.ta_last = Some(now);
        // Every icon in the open boxes, in reading order: (box id, tab, path, name).
        let claimed: HashSet<String> = self
            .layout
            .fences
            .iter()
            .filter(|f| !f.auto_sort && f.widget.is_none())
            .flat_map(|f| f.tabs.iter().flat_map(|t| t.items.iter().map(|p| p.to_lowercase())))
            .collect();
        let desk = DeskState { items: &self.items, claimed: &claimed, spacing: self.spacing, rules: &self.layout.settings.categories, open_counts: &self.layout.open_counts };
        // Only boxes you can see: open, on the page shown, not all hidden.
        let (page, hidden) = (self.layout.current_page, self.boxes_hidden);
        let mut boxes: Vec<&Fence> = self.layout.fences.iter().filter(|f| !hidden && f.page == page && f.expanded && f.widget.is_none()).collect();
        boxes.sort_by(|a, b| (a.rect[1] as i64, a.rect[0] as i64).cmp(&(b.rect[1] as i64, b.rect[0] as i64)));
        let mut order: Vec<(u64, usize, String, String)> = Vec::new();
        for f in boxes {
            for (ti, (_, items)) in fv::visible_tabs(f, &desk).into_iter().enumerate() {
                for it in items {
                    order.push((f.id, ti, it.path, it.name.to_lowercase()));
                }
            }
        }
        if order.is_empty() {
            return;
        }
        let cur = self.ui.selected.as_ref().and_then(|p| order.iter().position(|o| o.2.eq_ignore_ascii_case(p)));
        // "s", "ss", "sss": next item starting with s. Otherwise: the typed text, from the current one.
        let same = self.ta_buf.chars().all(|x| x == c);
        let (prefix, start) = if same { (c.to_string(), cur.map(|i| i + 1).unwrap_or(0)) } else { (self.ta_buf.clone(), cur.unwrap_or(0)) };
        let n = order.len();
        let Some(i) = (0..n).map(|k| (start + k) % n).find(|&i| order[i].3.starts_with(&prefix)) else { return };
        let (id, tab, path, _) = order[i].clone();
        if let Some(f) = self.layout.fences.iter_mut().find(|f| f.id == id) {
            if f.sel_tab != tab {
                f.sel_tab = tab;
                self.mark_dirty();
            }
        }
        let key = path.to_lowercase();
        self.ui.selection.clear();
        self.ui.selection.insert(key.clone());
        self.ui.selected = Some(path);
        self.ui.scroll_to = Some(key);
        self.redraw_all = true;
    }
}

/// The rename window's state (F2).
#[derive(Default)]
struct RenameState {
    /// The item being renamed.
    path: String,
    /// The new name as typed (without a hidden extension).
    text: String,
    was_focused: bool,
    /// The whole name was selected once (typing replaces it, like Explorer).
    selected_once: bool,
}

#[cfg(windows)]
const RENAME_PANEL: u64 = u64::MAX - 4;

impl DeskApp {
    /// Paths of the selected icons (the clicked one first).
    fn selected_paths(&self) -> Vec<String> {
        let mut v: Vec<String> = Vec::new();
        if let Some(p) = &self.ui.selected {
            v.push(p.clone());
        }
        for k in &self.ui.selection {
            if let Some(it) = self.items.get(k) {
                if !v.iter().any(|p| p.eq_ignore_ascii_case(&it.path)) {
                    v.push(it.path.clone());
                }
            }
        }
        v
    }

    /// The box holding the selected icon (as last drawn), with its icons and row length.
    fn active_box(&self) -> Option<(u64, Vec<String>, usize)> {
        let sel = self.ui.selected.as_ref()?;
        self.ui
            .grid_order
            .iter()
            .find(|(_, (paths, _))| paths.iter().any(|p| p.eq_ignore_ascii_case(sel)))
            .map(|(id, (paths, cols))| (*id, paths.clone(), *cols))
    }

    fn select_one(&mut self, path: String) {
        let key = path.to_lowercase();
        self.ui.selection.clear();
        self.ui.selection.insert(key.clone());
        self.ui.selected = Some(path);
        self.ui.scroll_to = Some(key);
        self.redraw_all = true;
    }

    /// Arrow keys: move the selection between icons of the same box (left/right one step,
    /// up/down one row).
    fn arrow_key(&mut self, vk: u32) {
        let Some((_, paths, cols)) = self.active_box() else { return };
        let Some(sel) = self.ui.selected.clone() else { return };
        let Some(i) = paths.iter().position(|p| p.eq_ignore_ascii_case(&sel)) else { return };
        let cols = cols.max(1) as isize;
        let step = match vk {
            0x25 => -1,
            0x27 => 1,
            0x26 => -cols,
            _ => cols,
        };
        let j = i as isize + step;
        if j >= 0 && (j as usize) < paths.len() {
            self.select_one(paths[j as usize].clone());
        }
    }

    /// Ctrl+A: every icon in the box of the selected one.
    fn select_all_in_box(&mut self) {
        let Some((_, paths, _)) = self.active_box() else { return };
        self.ui.selection = paths.iter().map(|p| p.to_lowercase()).collect();
        self.redraw_all = true;
    }

    /// Delete: the selected files go to the Recycle Bin (Windows' own operation, can be undone).
    fn delete_selected(&mut self) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        #[cfg(windows)]
        {
            let wake = self.scan_wake.clone();
            crate::fileops::recycle(paths, move || {
                let _ = wake.send(());
            });
        }
        self.ui.selection.clear();
        self.ui.selected = None;
        self.redraw_all = true;
    }

    /// Ctrl+V: files from the clipboard are copied (or moved, if cut) to the desktop and put in
    /// the box being worked in.
    fn paste_files(&mut self) {
        let Some(dest) = desktop::desktop_dirs().into_iter().next() else { return };
        // The box under the mouse (like pasting in an Explorer window); with the mouse elsewhere,
        // the box of the selected icon.
        let under_mouse = desktop::cursor_points(desktop::system_scale()).and_then(|c| {
            let page = self.layout.current_page;
            self.layout
                .fences
                .iter()
                .filter(|f| !self.boxes_hidden && f.page == page)
                .find(|f| {
                    let h = if f.expanded { f.rect[3] } else { ROLLED_HEIGHT };
                    egui::Rect::from_min_size(Pos2::new(f.rect[0], f.rect[1]), Vec2::new(f.rect[2], h)).contains(c)
                })
                .map(|f| f.id)
        });
        let target = under_mouse.or_else(|| self.active_box().map(|b| b.0)).filter(|id| self.layout.fences.iter().any(|f| f.id == *id && !f.auto_sort && f.widget.is_none() && f.folder.is_none()));
        let tab = target.and_then(|id| self.layout.fences.iter().find(|f| f.id == id)).map(|f| f.sel_tab).unwrap_or(0);
        #[cfg(windows)]
        {
            let wake = self.scan_wake.clone();
            let landed = self.pasted.clone();
            crate::fileops::paste_into(crate::runloop::main_hwnd(), dest.to_string_lossy().to_string(), move |paths| {
                if let (Some(to), Ok(mut l)) = (target, landed.lock()) {
                    l.extend(paths.into_iter().map(|p| (p, to, tab)));
                }
                let _ = wake.send(());
                crate::runloop::wake_after(Duration::ZERO);
            });
        }
        #[cfg(not(windows))]
        let _ = (dest, target, tab);
    }

    /// F2: a small window to rename the selected item.
    fn start_rename(&mut self) {
        let Some(p) = self.ui.selected.clone() else { return };
        let Some(it) = self.items.get(&p.to_lowercase()) else { return };
        if let Some((box_id, _, _)) = self.active_box() {
            self.ui.icon_rename = Some(IconRename { path: it.path.clone(), box_id, text: it.name.clone(), selected_once: false, had_focus: false });
            self.ui.box_cmds.push((box_id, ViewportCommand::Focus));
            self.redraw_all = true;
            return;
        }
        self.rename = Some(RenameState { path: it.path.clone(), text: it.name.clone(), was_focused: false, selected_once: false });
    }

    /// Renames the file, keeping a hidden extension, and updates every box that holds it.
    fn finish_rename(&mut self, path: &str, new_name: &str) {
        // Windows drops dots and spaces at the end of a name: do it here so the boxes keep the
        // real name; device names (CON, NUL, COM1…) can't be file names.
        let new_name = new_name.trim().trim_end_matches(['.', ' ']);
        let reserved = {
            let base = new_name.split('.').next().unwrap_or("").trim().to_ascii_uppercase();
            matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || ((base.starts_with("COM") || base.starts_with("LPT")) && base.len() == 4 && base[3..].chars().all(|c| c.is_ascii_digit() && c != '0'))
        };
        let Some(it) = self.items.get(&path.to_lowercase()).cloned() else { return };
        if new_name == it.name {
            return; // nothing changes
        }
        if new_name.is_empty() || reserved || new_name.contains(['\\', '/', ':', '*', '?', '"', '<', '>', '|']) {
            self.note = Some((t("Couldn't rename: the name has characters Windows doesn't allow.").to_string(), Instant::now()));
            self.redraw_all = true;
            crate::runloop::wake_after(Duration::ZERO);
            return;
        }
        let old = std::path::Path::new(&it.path);
        let Some(dir) = old.parent() else { return };
        // The extension comes from the real path (the item's `ext` is lower-cased).
        let real_ext = old.extension().map(|e| e.to_string_lossy().to_string()).unwrap_or_default();
        let file = if it.is_dir || real_ext.is_empty() { new_name.to_string() } else { format!("{new_name}.{real_ext}") };
        let new_path = dir.join(file);
        // Only the letter case changes ("foo" -> "Foo"): the "existing" file is this one.
        let same_file = new_path.to_string_lossy().to_lowercase() == it.path.to_lowercase();
        if same_file && new_path.to_string_lossy() == it.path.as_str() {
            return; // nothing changes
        }
        if new_path.exists() && !same_file {
            storage::log_line(&format!("rename: {} already exists", new_path.display()));
            self.note = Some((t("Couldn't rename: a file with that name already exists.").to_string(), Instant::now()));
            self.redraw_all = true;
            crate::runloop::wake_after(Duration::ZERO);
            return;
        }
        if let Err(e) = std::fs::rename(old, &new_path) {
            storage::log_line(&format!("rename failed: {e}"));
            self.note = Some((t("Couldn't rename: Windows refused (the file may be open).").to_string(), Instant::now()));
            self.redraw_all = true;
            crate::runloop::wake_after(Duration::ZERO);
            return;
        }
        let new_s = new_path.to_string_lossy().to_string();
        for f in &mut self.layout.fences {
            for t in &mut f.tabs {
                for p in &mut t.items {
                    if p.eq_ignore_ascii_case(&it.path) {
                        *p = new_s.clone();
                    }
                }
            }
        }
        if let Some(c) = self.layout.open_counts.remove(&it.path.to_lowercase()) {
            self.layout.open_counts.insert(new_s.to_lowercase(), c);
        }
        self.select_one(new_s);
        self.mark_dirty();
        let _ = self.scan_wake.send(());
    }

    #[cfg(windows)]
    fn show_rename(&mut self, ctx: &egui::Context) {
        let Some(mut r) = self.rename.take() else { return };
        let scale = desktop::system_scale();
        let [sw, sh] = desktop::primary_screen_points();
        let (w, h) = (440.0_f32.min(sw - 40.0), 96.0);
        let (x, y) = (((sw - w) / 2.0).max(0.0), (sh * 0.2).max(0.0));
        let rect_px = [(x * scale) as i32, (y * scale) as i32, (w * scale) as i32, (h * scale) as i32];
        let mut close = false;
        let mut done: Option<String> = None;
        let force = self.panel_force;
        if let Some(mut hosts) = self.hosts.take() {
            hosts.panel(RENAME_PANEL, crate::host::PanelKind::Menu, "DesktopShepherd rename", rect_px, true, force, |pctx| {
                match pctx.input(|i| i.viewport().focused) {
                    Some(true) => r.was_focused = true,
                    Some(false) if r.was_focused => close = true,
                    _ => {}
                }
                let (esc, enter) = pctx.input(|i| (i.key_pressed(egui::Key::Escape) || i.viewport().close_requested(), i.key_pressed(egui::Key::Enter)));
                if esc {
                    close = true;
                }
                if enter {
                    done = Some(r.text.clone());
                }
                let frame = egui::Frame::new().fill(crate::theme::card()).stroke(egui::Stroke::new(1.0_f32, crate::theme::line())).inner_margin(egui::Margin::same(12));
                egui::CentralPanel::default().frame(frame).show(pctx, |ui| {
                    ui.label(egui::RichText::new(t("Rename")).size(13.0).color(crate::theme::soft()));
                    let out = egui::TextEdit::singleline(&mut r.text).font(FontId::proportional(16.0)).desired_width(f32::INFINITY).show(ui);
                    let te = out.response.clone();
                    if !r.selected_once {
                        // Select the whole name, so typing replaces it.
                        let mut state = out.state;
                        let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(r.text.chars().count()));
                        state.cursor.set_char_range(Some(all));
                        state.store(ui.ctx(), te.id);
                        r.selected_once = true;
                    }
                    if !te.has_focus() && !close && done.is_none() {
                        te.request_focus();
                    }
                    ui.label(egui::RichText::new(t("Enter renames, Esc cancels.")).size(11.5).color(crate::theme::faint()));
                });
            });
            if close || done.is_some() {
                hosts.close_panel(RENAME_PANEL);
            }
            self.hosts = Some(hosts);
        }
        let _ = ctx;
        if let Some(name) = done {
            self.finish_rename(&r.path.clone(), &name);
        } else if !close {
            self.rename = Some(r);
        }
    }
}

impl DeskApp {
    /// Hides every box (a clean desktop) or shows them again, just above the desktop.
    fn set_boxes_hidden(&mut self, hidden: bool) {
        // The windows are shown or hidden by the page/visibility step on this pass.
        self.boxes_hidden = hidden;
        self.redraw_all = true;
    }
}

impl DeskApp {
    /// After monitors changed (and at start): with "remember per monitor setup" on, put every
    /// box back where it was the last time this setup was used; otherwise (or for a new setup)
    /// re-place boxes by monitor role / fixed monitor as before.
    fn switch_setup_or_relayout(&mut self) {
        let key = desktop::setup_key(&self.monitors);
        let per_setup = self.layout.settings.per_setup;
        if per_setup && !self.current_setup.is_empty() && key != self.current_setup {
            // Keep the positions of the setup we are leaving.
            let map = self.layout.fences.iter().map(|f| (f.id, f.rect)).collect();
            self.layout.setup_positions.insert(self.current_setup.clone(), map);
        }
        let saved = if per_setup { self.layout.setup_positions.get(&key).cloned() } else { None };
        self.current_setup = key;
        match saved {
            Some(map) => {
                let mut moved = false;
                for f in &mut self.layout.fences {
                    if let Some(r) = map.get(&f.id) {
                        if f.rect != *r {
                            f.rect = *r;
                            moved = true;
                        }
                    }
                }
                if moved {
                    let ids: Vec<u64> = self.layout.fences.iter().map(|f| f.id).collect();
                    for id in ids {
                        self.place_box(id);
                    }
                    self.ui.rebind.extend(self.layout.fences.iter().map(|f| f.id));
                    self.mark_dirty();
                }
            }
            None => self.relayout_monitors(),
        }
    }
}

/// The search box widget: type, and the matching desktop items show underneath; click one or
/// press Enter to open it (arrow keys pick, Esc clears). Uses the same ranking as quick search.
fn search_widget(ui: &mut egui::Ui, id: u64, desk: &DeskState, icons: &mut IconCache, actions: &mut Vec<Act>) {
    let qid = egui::Id::new(("search-widget", id));
    let mut q: String = ui.ctx().data(|d| d.get_temp(qid)).unwrap_or_default();
    let mut sel: usize = ui.ctx().data(|d| d.get_temp(qid.with("sel"))).unwrap_or(0);
    let r = ui.add(
        egui::TextEdit::singleline(&mut q)
            .hint_text(tf("{}  Search the desktop", &[&icon::SEARCH]))
            .desired_width(f32::INFINITY)
            .margin(egui::Margin::symmetric(8, 6)),
    );
    if r.changed() {
        sel = 0;
    }
    let row_h = 28.0;
    let fits = ((ui.available_height() - 6.0) / row_h).floor().max(1.0) as usize;
    let apps = crate::apps::list();
    let results: Vec<&DeskItem> = if q.trim().is_empty() { Vec::new() } else { crate::search::rank(crate::apps::search_pool(desk.items.values(), &apps, crate::apps::enabled()), &q, fits) };
    let (up, down, enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::ArrowUp), i.key_pressed(egui::Key::ArrowDown), i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
    let mut open: Option<String> = None;
    if r.has_focus() || r.lost_focus() {
        if down && !results.is_empty() {
            sel = (sel + 1).min(results.len() - 1);
        }
        if up {
            sel = sel.saturating_sub(1);
        }
        if esc {
            q.clear();
        }
    }
    if r.lost_focus() && enter {
        open = results.get(sel).map(|it| it.path.clone());
        r.request_focus();
    }
    ui.add_space(6.0);
    if !q.trim().is_empty() && results.is_empty() {
        ui.label(egui::RichText::new(t("Nothing found")).size(13.0).color(crate::ink::faint()));
    }
    let px = (20.0 * ui.ctx().pixels_per_point()).round() as u32;
    for (i, it) in results.iter().enumerate() {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click());
        if resp.hovered() {
            sel = i;
        }
        if i == sel {
            ui.painter().rect_filled(rect, 4.0, Color32::from_rgba_unmultiplied(60, 130, 230, 150));
        }
        let ir = Rect::from_center_size(Pos2::new(rect.min.x + 16.0, rect.center().y), Vec2::splat(20.0));
        match icons.get(&it.path, px) {
            Some(tex) => {
                ui.painter().image(tex, ir, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                ui.painter().rect_filled(ir.shrink(3.0), 3.0, Color32::from_gray(110));
            }
        }
        let text = ui.painter().layout(it.name.clone(), FontId::proportional(13.5), if i == sel { Color32::WHITE } else { crate::ink::text() }, rect.width() - 40.0);
        ui.painter().with_clip_rect(rect).galley(Pos2::new(rect.min.x + 34.0, rect.center().y - text.size().y / 2.0), text, Color32::WHITE);
        if resp.clicked() {
            open = Some(it.path.clone());
        }
    }
    if let Some(p) = open {
        desktop::open_item(&p);
        actions.push(Act::Opened(p));
        q.clear();
        sel = 0;
    }
    ui.ctx().data_mut(|d| {
        d.insert_temp(qid, q);
        d.insert_temp(qid.with("sel"), sel);
    });
}

const PERSONALIZE_PANEL: u64 = u64::MAX - 5;

impl DeskApp {
    /// The Personalize window of one box: two columns, changes show on the box at once.
    fn show_personalize(&mut self) {
        let Some(id) = self.personalize else { return };
        if !self.layout.fences.iter().any(|f| f.id == id) {
            self.personalize = None; // the box was deleted
            return;
        }
        #[cfg(windows)]
        if let Some(mut hosts) = self.hosts.take() {
            let s = desktop::system_scale();
            let [sw, sh] = desktop::primary_screen_points();
            let (w, h) = (820.0_f32.min(sw - 40.0), 640.0_f32.min(sh - 80.0));
            let (x, y) = (((sw - w) / 2.0).max(0.0), ((sh - h) / 2.0).max(0.0));
            let rect_px = [(x * s) as i32, (y * s) as i32, (w * s) as i32, (h * s) as i32];
            let force = self.panel_force;
            let mut close = false;
            let mut dirty = false;
            let fences = &mut self.layout.fences;
            let actions = &mut self.actions;
            let mut tab = self.personalize_tab;
            let mut resize = false;
            hosts.panel(PERSONALIZE_PANEL, crate::host::PanelKind::Settings, "Personalize", rect_px, false, force, |pctx| {
                if pctx.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)) {
                    close = true;
                }
                let Some(fence) = fences.iter_mut().find(|f| f.id == id) else { return };
                let widget_kind = fence.widget.as_ref().and_then(|w| widgets::find(&w.kind));
                egui::TopBottomPanel::top("look-top").frame(egui::Frame::new().fill(crate::theme::bar()).inner_margin(10.0)).show(pctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(t("Personalize")).size(20.0).strong().color(crate::theme::strong()));
                        ui.label(egui::RichText::new(format!("  {}", fence.name)).size(15.0).color(crate::theme::soft()));
                        if widget_kind.map(|k| (k.make)().has_settings()).unwrap_or(false) {
                            ui.add_space(24.0);
                            ui.selectable_value(&mut tab, 0, egui::RichText::new(t("Look")).size(14.0));
                            ui.selectable_value(&mut tab, 1, egui::RichText::new(t("Widget settings")).size(14.0));
                        } else {
                            tab = 0;
                        }
                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            if ui.button(t("Done")).clicked() {
                                close = true;
                            }
                        });
                    });
                });
                egui::CentralPanel::default().frame(egui::Frame::new().fill(crate::theme::bg()).inner_margin(10.0)).show(pctx, |ui| {
                    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        match (tab, widget_kind, fence.widget.as_mut()) {
                            (1, Some(kind), Some(wc)) => {
                                ui.spacing_mut().item_spacing.y = 8.0;
                                dirty |= (kind.make)().settings(ui, &mut wc.data);
                                // A widget can ask for a box size that fits a look it offers.
                                if let Some(size) = wc.data.as_object_mut().and_then(|o| o.remove("resize_to")) {
                                    if let (Some(w), Some(h)) = (size[0].as_f64(), size[1].as_f64()) {
                                        fence.rect[2] = w as f32;
                                        fence.rect[3] = h as f32;
                                        resize = true;
                                    }
                                }
                            }
                            _ => {
                                ui.columns(2, |cols| {
                                    dirty |= look_left(&mut cols[0], fence);
                                    dirty |= look_right(&mut cols[1], fence, actions);
                                });
                            }
                        }
                    });
                });
            });
            self.personalize_tab = tab;
            if std::mem::take(&mut self.focus_personalize) {
                hosts.apply(PERSONALIZE_PANEL, &ViewportCommand::Focus);
            }
            if close {
                hosts.close_panel(PERSONALIZE_PANEL);
                self.personalize = None;
            }
            self.hosts = Some(hosts);
            if resize {
                self.place_box(id);
            }
            if dirty {
                self.mark_dirty();
                self.redraw_all = true;
            }
        }
    }
}

#[cfg(windows)]
const UNDO_PANEL: u64 = u64::MAX - 6;

impl DeskApp {
    /// A small note at the bottom of the main monitor after a box was deleted, with "Undo".
    #[cfg(windows)]
    fn show_undo_toast(&mut self, ctx: &egui::Context) {
        let Some((f, at)) = self.deleted.last() else { return };
        let name = f.name.clone();
        let left = Duration::from_secs(8).saturating_sub(at.elapsed());
        let scale = desktop::system_scale();
        let work = desktop::work_area_at(Pos2::new(10.0, 10.0), scale);
        let (w, h) = (380.0_f32, 52.0_f32);
        let (x, y) = (work.center().x - w / 2.0, work.max.y - h - 24.0);
        let rect_px = [(x * scale) as i32, (y * scale) as i32, (w * scale) as i32, (h * scale) as i32];
        let force = self.panel_force;
        let mut undo = false;
        if let Some(mut hosts) = self.hosts.take() {
            hosts.panel(UNDO_PANEL, crate::host::PanelKind::Menu, "DesktopShepherd undo", rect_px, true, force, |pctx| {
                let frame = egui::Frame::new().fill(crate::theme::menu()).stroke(egui::Stroke::new(1.0_f32, crate::theme::line())).inner_margin(egui::Margin::symmetric(14, 10));
                egui::CentralPanel::default().frame(frame).show(pctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::Label::new(egui::RichText::new(tf("“{}” deleted", &[&name])).size(14.0).color(crate::theme::strong())).truncate());
                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            if ui.button(egui::RichText::new(t("Undo")).size(14.0)).clicked() {
                                undo = true;
                            }
                        });
                    });
                });
            });
            self.hosts = Some(hosts);
        }
        if undo {
            let i = self.deleted.len() - 1;
            self.actions.push(Act::RestoreDeleted(i));
        }
        // Close it on time.
        ctx.request_repaint_after(left + Duration::from_millis(50));
        crate::runloop::wake_after(left + Duration::from_millis(50));
    }
}

#[cfg(windows)]
const UPDATE_PANEL: u64 = u64::MAX - 7;

impl DeskApp {
    /// A small note at the bottom right of the main monitor: a new version is available.
    #[cfg(windows)]
    fn show_update_toast(&mut self, _ctx: &egui::Context, rel: crate::update::Release) {
        let scale = desktop::system_scale();
        let work = desktop::work_area_at(Pos2::new(10.0, 10.0), scale);
        let (w, h) = (420.0_f32, 56.0_f32);
        let (x, y) = (work.max.x - w - 24.0, work.max.y - h - 24.0);
        let rect_px = [(x * scale) as i32, (y * scale) as i32, (w * scale) as i32, (h * scale) as i32];
        let force = self.panel_force;
        let (mut update, mut later) = (false, false);
        if let Some(mut hosts) = self.hosts.take() {
            hosts.panel(UPDATE_PANEL, crate::host::PanelKind::Menu, "DesktopShepherd update", rect_px, true, force, |pctx| {
                let frame = egui::Frame::new().fill(crate::theme::menu()).stroke(egui::Stroke::new(1.0_f32, crate::theme::line())).inner_margin(egui::Margin::symmetric(14, 10));
                egui::CentralPanel::default().frame(frame).show(pctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(tf("DesktopShepherd {} is available", &[&rel.version])).size(14.0).color(crate::theme::strong()));
                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            if ui.button(t("Later")).clicked() {
                                later = true;
                            }
                            if ui.button(egui::RichText::new(t("Update")).strong()).clicked() {
                                update = true;
                            }
                        });
                    });
                });
            });
            self.hosts = Some(hosts);
        }
        if update || later {
            // Either way the note goes; "Update" starts the download and the setup.
            self.layout.settings.skipped_version = rel.version.clone();
            self.mark_dirty();
            if update {
                crate::update::install(rel);
            }
        }
    }
}

#[cfg(windows)]
const UPDATES_ASK_PANEL: u64 = u64::MAX - 8;
#[cfg(windows)]
const NOTE_PANEL: u64 = u64::MAX - 9;

impl DeskApp {
    /// Asked once, before anything goes online: look for new versions once a day?
    #[cfg(windows)]
    fn show_updates_ask_toast(&mut self, _ctx: &egui::Context) {
        let scale = desktop::system_scale();
        let work = desktop::work_area_at(Pos2::new(10.0, 10.0), scale);
        let (w, h) = (440.0_f32, 100.0_f32);
        let (x, y) = (work.max.x - w - 24.0, work.max.y - h - 24.0 - 70.0);
        let rect_px = [(x * scale) as i32, (y * scale) as i32, (w * scale) as i32, (h * scale) as i32];
        let force = self.panel_force;
        let mut answer: Option<bool> = None;
        if let Some(mut hosts) = self.hosts.take() {
            hosts.panel(UPDATES_ASK_PANEL, crate::host::PanelKind::Menu, "DesktopShepherd updates question", rect_px, true, force, |pctx| {
                let frame = egui::Frame::new().fill(crate::theme::menu()).stroke(egui::Stroke::new(1.0_f32, crate::theme::line())).inner_margin(egui::Margin::symmetric(14, 10));
                egui::CentralPanel::default().frame(frame).show(pctx, |ui| {
                    ui.label(egui::RichText::new(t("Check once a day for new versions of DesktopShepherd? (Only asks GitHub for the latest version number.)")).size(14.0).color(crate::theme::strong()));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(egui::RichText::new(t("Yes")).strong()).clicked() {
                            answer = Some(true);
                        }
                        if ui.button(t("No")).clicked() {
                            answer = Some(false);
                        }
                    });
                });
            });
            self.hosts = Some(hosts);
        }
        if let Some(yes) = answer {
            self.layout.settings.check_updates = yes;
            self.layout.settings.updates_asked = true;
            self.mark_dirty();
            self.redraw_all = true;
            crate::runloop::wake_after(Duration::ZERO);
        }
    }

    /// A short message at the bottom of the main monitor.
    #[cfg(windows)]
    fn show_note_toast(&mut self, ctx: &egui::Context) {
        let Some((text, at)) = self.note.clone() else { return };
        let left = Duration::from_secs(6).saturating_sub(at.elapsed());
        let scale = desktop::system_scale();
        let work = desktop::work_area_at(Pos2::new(10.0, 10.0), scale);
        let (w, h) = (460.0_f32, 48.0_f32);
        let (x, y) = (work.center().x - w / 2.0, work.max.y - h - 24.0 - 70.0);
        let rect_px = [(x * scale) as i32, (y * scale) as i32, (w * scale) as i32, (h * scale) as i32];
        let force = self.panel_force;
        if let Some(mut hosts) = self.hosts.take() {
            hosts.panel(NOTE_PANEL, crate::host::PanelKind::Menu, "DesktopShepherd note", rect_px, true, force, |pctx| {
                let frame = egui::Frame::new().fill(crate::theme::menu()).stroke(egui::Stroke::new(1.0_f32, crate::theme::line())).inner_margin(egui::Margin::symmetric(14, 10));
                egui::CentralPanel::default().frame(frame).show(pctx, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(&text).size(14.0).color(crate::theme::strong())).truncate());
                });
            });
            self.hosts = Some(hosts);
        }
        ctx.request_repaint_after(left + Duration::from_millis(50));
        crate::runloop::wake_after(left + Duration::from_millis(50));
    }
}
