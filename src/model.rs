//! The saved layout: boxes (fences), their tabs and items, and app-wide settings.
//! Field set follows the original's per-box record (see MECHANICS.md).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TitlePos {
    #[default]
    Left,
    Top,
    Bottom,
    /// No title shown: a thin strip at the top still moves the box (and rolls it up).
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TabPos {
    #[default]
    Right,
    Top,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SortKind {
    /// The order the person arranged by hand.
    #[default]
    Manual,
    Name,
    /// Item size (original: STRID_DESK_SORT_SIZE, menu 0x180223170).
    Size,
    Type,
    /// Date modified (STRID_DESK_SORT_TIME).
    Modified,
    /// How often it was opened from a box (STRID_DESK_SORT_OPEN_COUNT).
    OpenCount,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Tab {
    pub name: String,
    /// Full paths of the files/folders/shortcuts shown in this tab.
    #[serde(default)]
    pub items: Vec<String>,
}

/// A widget living in a box instead of icons. `kind` names an entry of the widget registry,
/// `data` is that widget's own saved state (free-form, so new widgets need no format change).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WidgetConfig {
    pub kind: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Fence {
    pub id: u64,
    pub name: String,
    /// Outer rectangle in screen points: x, y, width, height.
    pub rect: [f32; 4],
    /// false = rolled up to its title bar.
    pub expanded: bool,
    pub locked: bool,
    /// Catches every desktop item no other box holds, split into tabs by kind.
    pub auto_sort: bool,
    pub title_pos: TitlePos,
    pub tab_pos: TabPos,
    pub tabs: Vec<Tab>,
    pub sel_tab: usize,
    pub sort: SortKind,
    /// Background colour (RGB) and its opacity 0..255.
    pub color: [u8; 3],
    pub alpha: u8,
    pub border_color: [u8; 4],
    pub corner: f32,
    pub icon_size: u32,
    pub widget: Option<WidgetConfig>,
    /// Which monitor the box belongs to: its role (0 = main, 1 = second, ...; like the
    /// original's per-box `primary` field), its device name, and its offset from that
    /// monitor's top-left corner. Empty name = not assigned yet.
    /// Frosted glass for this box: how strongly the desktop behind it is blurred (0 = clear),
    /// like the original's per-box effectDegree.
    pub blur: u8,
    pub monitor_role: usize,
    pub monitor_name: String,
    pub monitor_offset: [f32; 2],
    /// Border thickness in points (0 = no border).
    pub border_width: f32,
    /// Colour of the title, tabs and icon captions.
    pub text_color: [u8; 3],
    /// A dark shadow behind captions (easier to read on light glass).
    pub text_shadow: bool,
    /// Font name from `fence_view::FONTS` (empty = Segoe UI, the default).
    pub font: String,
    /// Text sizes in points: the title, and the tabs and icon captions.
    pub title_size: f32,
    pub caption_size: f32,
    /// Show the names under the icons (false = icons only).
    pub show_captions: bool,
    /// Horizontal distance between icons in points (0 = Windows' own desktop spacing).
    pub icon_spacing: f32,
    /// A folder box: shows the live contents of this folder instead of its own tabs.
    pub folder: Option<String>,
    /// The page this box is on (0 = the first). Only the current page's boxes are shown.
    pub page: u32,
    /// When the icons don't fit: 0 = scroll up/down, 1 = pages with dots (wheel or dots flip).
    pub overflow: u8,
    /// Keyboard shortcut of this box: 0 = none, 1..9 = Ctrl+Alt+F2..F10 (brings it into view, then
    /// rolls it up or down).
    pub hotkey: u8,
    /// A soft shadow under the box (a click-through window below it, see shadow.rs).
    pub shadow: bool,
    /// "Raised look": edge lighting and a sheen on the glass.
    #[serde(default)]
    pub raised: bool,
    /// Header: 0 none, 1 tinted band behind the title, 2 thin line under it.
    pub header_style: u8,
    /// Title placement along a top or bottom title: 0 centre, 1 left, 2 right.
    pub title_align: u8,
    /// A symbol before the title: a Segoe Fluent Icons character or a typed emoji (empty = none).
    pub title_icon: String,
    /// Own colour of the title (None = the box's text colour).
    pub title_color: Option<[u8; 3]>,
    /// 0 normal, 1 bold, 2 light.
    pub title_weight: u8,
    /// A small badge with the number of items at the end of the title.
    pub show_count: bool,
    /// The title only shows while the mouse is over the box.
    pub title_on_hover: bool,
    /// UPPERCASE title with a little letter spacing.
    pub title_caps: bool,
    /// A colour fade over the box (None = off).
    pub gradient: Option<Gradient>,
    /// Faint acrylic noise over the box.
    pub texture: bool,
    /// While the mouse is over the box: brighter border and stronger drop shadow.
    pub hover_lift: bool,
    /// Border in Windows' accent colour: 0 off, 1 always, 2 only while hovered.
    pub accent_border: u8,
    /// A soft backing behind each icon caption.
    pub name_backing: bool,
    /// The icon under the mouse is drawn a little larger.
    pub hover_zoom: bool,
    /// Icons slightly dimmed until hovered or selected.
    pub calm_icons: bool,
    /// This box's own auto-sort tab rules (None = follow the default list in Settings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub categories: Option<Vec<CategoryRule>>,
    /// A folder box split into tabs by kind (using the auto-sort tab rules). Folder boxes only.
    #[serde(default)]
    pub kind_tabs: bool,
}

impl Fence {
    /// True when this box's tabs come from the tab rules: an auto-sort box, or a folder box
    /// split by kind.
    pub fn rule_tabs(&self) -> bool {
        self.auto_sort || (self.folder.is_some() && self.kind_tabs)
    }

    /// A folder box can't also be a collector: older layouts that had both become "by kind".
    pub fn normalize_kind_tabs(&mut self) {
        if self.folder.is_some() && self.auto_sort {
            self.auto_sort = false;
            self.kind_tabs = true;
        }
    }


    /// The auto-sort rules this box uses: its own list, or `default` (the app setting).
    pub fn rules<'a>(&'a self, default: &'a [CategoryRule]) -> &'a [CategoryRule] {
        self.categories.as_deref().unwrap_or(default)
    }
}

/// Moves the element at `from` so it ends up at index `to` (both indices of the list as it is now
/// before the move; `to` is clamped). Returns false when nothing moved.
pub fn move_item<T>(v: &mut Vec<T>, from: usize, to: usize) -> bool {
    if from >= v.len() {
        return false;
    }
    let to = to.min(v.len() - 1);
    if from == to {
        return false;
    }
    let x = v.remove(from);
    v.insert(to, x);
    true
}

/// A fade from clear (at the top / left) to `color2` with opacity `alpha2` (at the bottom / right).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Gradient {
    pub color2: [u8; 3],
    pub alpha2: u8,
    /// 0 vertical, 1 horizontal, 2 diagonal.
    pub angle: u8,
}

impl Default for Gradient {
    fn default() -> Self {
        Self { color2: [60, 90, 160], alpha2: 90, angle: 0 }
    }
}

impl Default for Fence {
    fn default() -> Self {
        Self {
            id: 0,
            name: "New box".into(),
            rect: [80.0, 80.0, 520.0, 300.0],
            expanded: true,
            locked: false,
            auto_sort: false,
            title_pos: TitlePos::Left,
            tab_pos: TabPos::Right,
            tabs: vec![Tab { name: "Main".into(), items: vec![] }],
            sel_tab: 0,
            sort: SortKind::Manual,
            color: [0, 0, 0],
            // App-wide defaults read by the original: box_color 0xFF000000 (0x1801e4470),
            // box_alpha 50 of 255 (0x1801e43f0), show_border on (0x1801e44f0).
            alpha: 50,
            border_color: [255, 255, 255, 40],
            // roundCorner is 0 unless Windows reports a major version above 10 (0x1801e40f0 /
            // 0x1802aea40); we use Windows 11's own 8 px so boxes match the system look.
            corner: 8.0,
            icon_size: 48,
            widget: None,
            blur: 16,
            monitor_role: 0,
            monitor_name: String::new(),
            monitor_offset: [0.0, 0.0],
            border_width: 1.0,
            text_color: [255, 255, 255],
            text_shadow: true,
            font: String::new(),
            title_size: 13.0,
            caption_size: 12.0,
            show_captions: true,
            icon_spacing: 0.0,
            folder: None,
            page: 0,
            overflow: 0,
            hotkey: 0,
            shadow: false,
            raised: false,
            header_style: 0,
            title_align: 0,
            title_icon: String::new(),
            title_color: None,
            title_weight: 0,
            show_count: false,
            title_on_hover: false,
            title_caps: false,
            gradient: None,
            texture: false,
            hover_lift: false,
            accent_border: 0,
            name_backing: false,
            hover_zoom: false,
            calm_icons: false,
            categories: None,
            kind_tabs: false,
        }
    }
}

impl Fence {
    /// Takes over another box's whole look (colours, glass, border, text, icon size and
    /// spacing, title and tab placement) — for "Apply this look to all boxes".
    pub fn copy_look_from(&mut self, o: &Fence) {
        self.color = o.color;
        self.alpha = o.alpha;
        self.border_color = o.border_color;
        self.border_width = o.border_width;
        self.corner = o.corner;
        self.blur = o.blur;
        self.text_color = o.text_color;
        self.text_shadow = o.text_shadow;
        self.font = o.font.clone();
        self.title_size = o.title_size;
        self.caption_size = o.caption_size;
        self.show_captions = o.show_captions;
        self.icon_size = o.icon_size;
        self.icon_spacing = o.icon_spacing;
        self.title_pos = o.title_pos;
        self.tab_pos = o.tab_pos;
        self.overflow = o.overflow;
        self.shadow = o.shadow;
        self.raised = o.raised;
        self.header_style = o.header_style;
        self.title_align = o.title_align;
        self.title_icon = o.title_icon.clone();
        self.title_color = o.title_color;
        self.title_weight = o.title_weight;
        self.show_count = o.show_count;
        self.title_on_hover = o.title_on_hover;
        self.title_caps = o.title_caps;
        self.gradient = o.gradient.clone();
        self.texture = o.texture;
        self.hover_lift = o.hover_lift;
        self.accent_border = o.accent_border;
        self.name_backing = o.name_backing;
        self.hover_zoom = o.hover_zoom;
        self.calm_icons = o.calm_icons;
    }

    pub fn remove_item(&mut self, path: &str) {
        for t in &mut self.tabs {
            t.items.retain(|p| !p.eq_ignore_ascii_case(path));
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Frosted glass: how strongly the desktop behind the boxes is blurred (0 = clear glass;
    /// like the original's blur effect, `effectDegree`).
    pub blur: u8,
    /// How boxes follow monitors when displays change.
    pub monitor_mode: MonitorMode,
    /// Rules that name the auto-sort box's tabs: a name and the file extensions it takes
    /// (like the original's user categories: STRID_DESK_CATEGORY_TMP_NAME / _EXT, 0x1801ecbc0).
    pub categories: Vec<CategoryRule>,
    /// Hide Windows' own desktop icons while boxes show them (restored on exit or crash).
    pub hide_desktop_icons: bool,
    /// Pause (no redraws, shortcuts given back) while a full-screen app or game is running.
    pub pause_fullscreen: bool,
    /// Rolled-up boxes open while the mouse is over them.
    pub expand_on_hover: bool,
    pub start_with_windows: bool,
    /// Lower this program's CPU priority so it never competes with other apps.
    pub low_priority: bool,
    /// How often the desktop folder is re-read, in seconds.
    pub rescan_secs: u64,
    /// Moving or resizing a box snaps its edges to other boxes and the screen edges (hold Alt
    /// to place it freely).
    pub snap: bool,
    /// Space left between snapped boxes and inside the screen edges, in points.
    pub snap_gap: f32,
    /// Also snap to an invisible grid (when no box or screen edge is close), and its size in
    /// points.
    pub snap_grid: bool,
    pub grid_size: f32,
    /// Quick-search shortcut: index into `runloop::SEARCH_HOTKEYS` (0 = off).
    pub search_hotkey: usize,
    /// Typing on the desktop (while it has the focus and Windows' icons are hidden):
    /// 0 = nothing, 1 = jump to the icon whose name starts with what was typed, 2 = quick search.
    pub desktop_typing: u8,
    /// Hide/show-all-boxes shortcut: index into `runloop::SEARCH_HOTKEYS` (0 = off; 5 = Ctrl+Alt+D).
    pub hide_hotkey: usize,
    /// Remember box positions separately for each monitor setup (laptop alone, docked, …).
    pub per_setup: bool,
    /// "DesktopShepherd ▸" in the desktop's right-click menu (new box / widget there).
    pub desktop_menu: bool,
    /// Quick search also lists Start menu programs.
    pub search_apps: bool,
    /// Boxes fade while the mouse is away from them.
    pub fade_idle: bool,
    /// How visible a faded box stays (percent) and after how many seconds away it fades.
    pub fade_opacity: u8,
    pub fade_delay: u32,
    /// Each Windows virtual desktop shows its own page of boxes (desktop 1 = page 1…).
    pub link_vdesks: bool,
    /// Language: 0 = automatic (Windows), 1 = English, 2 = Spanish, 3 = Catalan.
    pub language: u8,
    /// Look for new versions on GitHub (at start and once a day).
    pub check_updates: bool,
    /// The person was asked once whether to look for new versions (nothing is checked before).
    pub updates_asked: bool,
    /// A version the person chose to skip ("Later" on the update note).
    pub skipped_version: String,
    /// Highlights use Windows' accent colour.
    pub follow_accent: bool,
    /// Dark text in boxes that sit on a bright wallpaper.
    pub auto_ink: bool,
    /// Menus and settings windows: 0 = like Windows, 1 = dark, 2 = light.
    pub theme: u8,
    /// Keep the layout in OneDrive, shared by this user's PCs.
    pub sync_onedrive: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            blur: 16,
            monitor_mode: MonitorMode::Role,
            categories: default_categories(),
            // The boxes replace the desktop icons while DesktopShepherd runs (original: organize_hide).
            hide_desktop_icons: true,
            pause_fullscreen: true,
            expand_on_hover: true,
            start_with_windows: false,
            low_priority: true,
            rescan_secs: 4,
            snap: true,
            snap_gap: 8.0,
            snap_grid: false,
            grid_size: 20.0,
            search_hotkey: 1,
            desktop_typing: 1,
            hide_hotkey: 5,
            per_setup: false,
            desktop_menu: true,
            search_apps: true,
            fade_idle: false,
            fade_opacity: 30,
            fade_delay: 3,
            link_vdesks: false,
            language: 0,
            check_updates: false,
            updates_asked: false,
            skipped_version: String::new(),
            follow_accent: true,
            auto_ink: true,
            theme: 0,
            sync_onedrive: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Layout {
    pub version: u32,
    pub next_id: u64,
    pub settings: Settings,
    pub fences: Vec<Fence>,
    /// Times each item was opened from a box (lower-case path), for 'Sort by times opened'.
    pub open_counts: std::collections::HashMap<String, u32>,
    /// Box positions remembered per monitor setup (see `desktop::setup_key`): setup ->
    /// box id -> rectangle. Used when Settings.per_setup is on.
    pub setup_positions: std::collections::HashMap<String, std::collections::HashMap<u64, [f32; 4]>>,
    /// The page of boxes shown (0 = the first).
    pub current_page: u32,
    /// The person's own saved looks (Personalize > Presets). Built-in ones live in code.
    pub look_presets: Vec<LookPreset>,
}

/// A saved box look. `look` is a Fence used only for its look fields (no items, no widget).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LookPreset {
    pub name: String,
    pub look: Fence,
}

/// Most looks the person can save.
pub const MAX_LOOK_PRESETS: usize = 30;

impl LookPreset {
    /// A preset holding the look of `f` (everything else cleared).
    pub fn from_fence(name: &str, f: &Fence) -> LookPreset {
        let mut look = f.clone();
        look.name = String::new();
        look.tabs = Vec::new();
        look.widget = None;
        look.folder = None;
        look.sel_tab = 0;
        LookPreset { name: name.to_string(), look }
    }
}

/// The looks that come with the program (not saved in the layout, can't be deleted).
pub const BUILTIN_LOOK_NAMES: [&str; 5] = ["Glass", "Solid dark", "Minimal", "Light frost", "Accent"];

pub fn builtin_look_presets() -> Vec<LookPreset> {
    let base = Fence::default();
    let glass = base.clone();
    let solid_dark = Fence {
        color: [24, 26, 32],
        alpha: 235,
        blur: 0,
        border_color: [255, 255, 255, 30],
        border_width: 1.0,
        ..base.clone()
    };
    let minimal = Fence {
        alpha: 20,
        blur: 8,
        border_width: 0.0,
        title_on_hover: true,
        header_style: 0,
        name_backing: true,
        ..base.clone()
    };
    let light_frost = Fence {
        color: [236, 241, 248],
        alpha: 150,
        blur: 24,
        border_color: [255, 255, 255, 120],
        text_color: [24, 28, 36],
        text_shadow: false,
        ..base.clone()
    };
    let accent = Fence {
        accent_border: 1,
        border_width: 1.5,
        header_style: 2,
        title_weight: 1,
        raised: true,
        ..base
    };
    [glass, solid_dark, minimal, light_frost, accent]
        .into_iter()
        .zip(BUILTIN_LOOK_NAMES)
        .map(|(f, n)| LookPreset::from_fence(n, &f))
        .collect()
}

impl Layout {
    pub fn new_id(&mut self) -> u64 {
        self.next_id = self.next_id.max(1) + 1;
        self.next_id
    }

    /// A copy for the OneDrive sync file: agenda links can hold secret calendar addresses, so
    /// they never leave this PC.
    pub fn for_sync(&self) -> Layout {
        let mut l = self.clone();
        for f in &mut l.fences {
            if let Some(w) = f.widget.as_mut().filter(|w| w.kind == "agenda") {
                if let Some(o) = w.data.as_object_mut() {
                    o.remove("urls");
                }
            }
        }
        l
    }

    /// After loading a synced layout: agenda boxes keep this PC's own links (by box id).
    pub fn keep_agenda_urls_from(&mut self, local: &Layout) {
        for f in &mut self.fences {
            let Some(w) = f.widget.as_mut().filter(|w| w.kind == "agenda") else { continue };
            let mine = local
                .fences
                .iter()
                .find(|g| g.id == f.id)
                .and_then(|g| g.widget.as_ref())
                .filter(|lw| lw.kind == "agenda")
                .and_then(|lw| lw.data.get("urls").cloned());
            if let Some(u) = mine {
                if !w.data.is_object() {
                    w.data = serde_json::json!({});
                }
                if let Some(o) = w.data.as_object_mut() {
                    o.insert("urls".to_string(), u);
                }
            }
        }
    }
}

/// What kind of desktop item a file is; used by auto-sort boxes for their tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Category {
    Apps,
    Folders,
    Files,
    Archives,
    Pictures,
    Other,
}

#[allow(dead_code)]
impl Category {
    pub const ALL: [Category; 6] = [
        Category::Apps,
        Category::Folders,
        Category::Files,
        Category::Archives,
        Category::Pictures,
        Category::Other,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Category::Apps => "Apps",
            Category::Folders => "Folders",
            Category::Files => "Files",
            Category::Archives => "Archives",
            Category::Pictures => "Pictures",
            Category::Other => "Other",
        }
    }
    pub fn of(ext: &str, is_dir: bool) -> Category {
        if is_dir {
            return Category::Folders;
        }
        match ext {
            "lnk" | "url" | "exe" | "appref-ms" | "msi" | "bat" | "cmd" | "website" => Category::Apps,
            "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "iso" | "cab" | "zst" => Category::Archives,
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico" | "heic" | "tif" | "tiff"
            | "avif" | "psd" => Category::Pictures,
            "pdf" | "doc" | "docx" | "xls" | "xlsx" | "ppt" | "pptx" | "txt" | "md" | "rtf" | "odt"
            | "ods" | "csv" | "json" | "xml" | "ini" | "cfg" | "log" | "html" | "htm" | "epub" => {
                Category::Files
            }
            _ => Category::Other,
        }
    }
}

/// One file, folder or shortcut found on the desktop (or referenced by a box).
#[derive(Clone, Debug)]
pub struct DeskItem {
    pub path: String,
    pub name: String,
    pub ext: String,
    pub is_dir: bool,
    pub category: Category,
    /// Found in a desktop folder (as opposed to a file dropped into a box from elsewhere).
    pub on_desktop: bool,
    pub size: u64,
    /// Seconds since 1970, for sorting by date.
    pub modified: u64,
}

/// A rule for the auto-sort box: one tab per rule that has items. An item matches when every
/// condition that is set matches: its file type, its name (pattern with * and ?), and how
/// recently it changed. Rules are checked in order; the first match wins.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct CategoryRule {
    pub name: String,
    /// Lower-case extensions without the dot, e.g. ["zip", "rar"] (empty = any type).
    pub exts: Vec<String>,
    /// Name pattern, e.g. "*invoice*" or "screenshot*" (empty = any name).
    #[serde(default)]
    pub pattern: String,
    /// Only items changed in the last this-many days (0 = any age).
    #[serde(default)]
    pub days: u32,
}

impl CategoryRule {
    /// Whether `item` belongs to this rule's tab (`now`: seconds since 1970).
    pub fn matches(&self, item: &DeskItem, now: u64) -> bool {
        if self.exts.is_empty() && self.pattern.trim().is_empty() && self.days == 0 {
            return false; // an empty rule takes nothing
        }
        let ext_ok = self.exts.is_empty() || self.exts.iter().any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&item.ext));
        let name_ok = self.pattern.trim().is_empty() || wildcard(&self.pattern.trim().to_lowercase(), &item.name.to_lowercase());
        let age_ok = self.days == 0 || now.saturating_sub(item.modified) <= self.days as u64 * 86_400;
        ext_ok && name_ok && age_ok
    }
}

/// Simple wildcard match: `*` = any run of characters, `?` = one character.
pub fn wildcard(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0usize, 0usize, None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn rule(name: &str, exts: &[&str]) -> CategoryRule {
    CategoryRule { name: name.into(), exts: exts.iter().map(|e| e.to_string()).collect(), ..Default::default() }
}

pub fn default_categories() -> Vec<CategoryRule> {
    vec![
        rule("Apps", &["lnk", "url", "exe", "appref-ms", "msi", "bat", "cmd", "website"]),
        rule("Files", &["pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "txt", "md", "rtf", "odt", "ods", "csv", "json", "xml", "ini", "cfg", "log", "html", "htm", "epub"]),
        rule("Archives", &["zip", "rar", "7z", "tar", "gz", "bz2", "xz", "iso", "cab", "zst"]),
        rule("Pictures", &["png", "jpg", "jpeg", "gif", "bmp", "webp", "svg", "ico", "heic", "tif", "tiff", "avif", "psd"]),
    ]
}

/// The auto-sort tab an item belongs to: the first rule listing its extension; folders go to
/// "Folders" and everything else to "Other" (original item kinds: 0x1801942f0).
pub fn tab_name_for(item: &DeskItem, rules: &[CategoryRule]) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    // Folders only match rules without file types (name or date rules).
    if let Some(r) = rules.iter().find(|r| (!item.is_dir || r.exts.is_empty()) && r.matches(item, now)) {
        return r.name.clone();
    }
    if item.is_dir { "Folders".into() } else { "Other".into() }
}

#[cfg(test)]
mod rule_tests {
    use super::*;

    fn item(name: &str, ext: &str, days_old: u64) -> DeskItem {
        DeskItem { path: format!("C:\\D\\{name}.{ext}"), name: name.into(), ext: ext.into(), is_dir: false, category: Category::Other, on_desktop: true, size: 0, modified: 1_000_000_000 - days_old * 86_400 }
    }

    #[test]
    fn wildcards() {
        assert!(wildcard("*invoice*", "my invoice 2024"));
        assert!(wildcard("screenshot*", "screenshot 12"));
        assert!(!wildcard("screenshot*", "my screenshot"));
        assert!(wildcard("a?c", "abc"));
        assert!(!wildcard("a?c", "abbc"));
    }

    #[test]
    fn rules_by_type_name_and_age() {
        let now = 1_000_000_000;
        let week = CategoryRule { name: "This week".into(), days: 7, ..Default::default() };
        let inv = CategoryRule { name: "Invoices".into(), pattern: "*invoice*".into(), exts: vec!["pdf".into()], ..Default::default() };
        assert!(week.matches(&item("x", "txt", 2), now));
        assert!(!week.matches(&item("x", "txt", 10), now));
        assert!(inv.matches(&item("March invoice", "pdf", 100), now));
        assert!(!inv.matches(&item("March invoice", "docx", 100), now));
        assert!(!CategoryRule::default().matches(&item("x", "txt", 0), now));
    }
}
/// How boxes are tied to monitors.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MonitorMode {
    /// Boxes belong to a monitor role (main, second, ...): if the main monitor changes, the
    /// boxes move with the role.
    #[default]
    Role,
    /// Boxes stay on their physical monitor, whatever its role.
    Fixed,
}
/// Where the entry that was at `sel` ends up after `move_item(v, from, to)`.
pub fn follow_index(sel: usize, from: usize, to: usize) -> usize {
    if sel == from {
        to
    } else if from < sel && sel <= to {
        sel - 1
    } else if to <= sel && sel < from {
        sel + 1
    } else {
        sel
    }
}

#[cfg(test)]
mod move_tests {
    use super::*;

    /// A saved look, written to the layout file and read back, gives a box every look option
    /// of the box it was saved from (corners, border, glass, text, effects…).
    #[test]
    fn preset_keeps_every_look_option() {
        let mut src = Fence::default();
        src.color = [10, 20, 30];
        src.alpha = 77;
        src.border_color = [1, 2, 3, 4];
        src.border_width = 2.5;
        src.corner = 13.0;
        src.blur = 33;
        src.text_color = [40, 50, 60];
        src.text_shadow = !src.text_shadow;
        src.font = "Bahnschrift".into();
        src.title_size = 21.0;
        src.caption_size = 9.5;
        src.show_captions = !src.show_captions;
        src.icon_size = 72;
        src.icon_spacing = 101.0;
        src.title_pos = TitlePos::Bottom;
        src.tab_pos = TabPos::Top;
        src.overflow = 1;
        src.shadow = !src.shadow;
        src.raised = !src.raised;
        src.header_style = 2;
        src.title_align = 1;
        src.title_icon = "★".into();
        src.title_color = Some([7, 8, 9]);
        src.title_weight = 1;
        src.show_count = !src.show_count;
        src.title_on_hover = !src.title_on_hover;
        src.title_caps = !src.title_caps;
        src.gradient = Some(Gradient { color2: [9, 9, 9], alpha2: 99, angle: 2 });
        src.texture = !src.texture;
        src.hover_lift = !src.hover_lift;
        src.accent_border = 2;
        src.name_backing = !src.name_backing;
        src.hover_zoom = !src.hover_zoom;
        src.calm_icons = !src.calm_icons;

        let saved = serde_json::to_string(&LookPreset::from_fence("Mine", &src)).expect("save");
        let preset: LookPreset = serde_json::from_str(&saved).expect("load");
        let mut dst = Fence::default();
        dst.copy_look_from(&preset.look);

        // Everything that isn't part of the look is taken from the source too, so any look
        // option that the preset forgets shows up as a difference.
        dst.id = src.id;
        dst.name = src.name.clone();
        dst.rect = src.rect;
        dst.expanded = src.expanded;
        dst.locked = src.locked;
        dst.auto_sort = src.auto_sort;
        dst.tabs = src.tabs.clone();
        dst.sel_tab = src.sel_tab;
        dst.sort = src.sort;
        dst.widget = src.widget.clone();
        dst.monitor_role = src.monitor_role;
        dst.monitor_name = src.monitor_name.clone();
        dst.monitor_offset = src.monitor_offset;
        dst.folder = src.folder.clone();
        dst.page = src.page;
        dst.hotkey = src.hotkey;
        dst.categories = src.categories.clone();
        dst.kind_tabs = src.kind_tabs;
        assert_eq!(serde_json::to_value(&dst).expect("dst"), serde_json::to_value(&src).expect("src"));
    }

    #[test]
    fn moves_and_follows() {
        let mut v = vec!['a', 'b', 'c', 'd'];
        assert!(move_item(&mut v, 0, 2));
        assert_eq!(v, vec!['b', 'c', 'a', 'd']);
        assert!(move_item(&mut v, 3, 0));
        assert_eq!(v, vec!['d', 'b', 'c', 'a']);
        assert!(!move_item(&mut v, 1, 1));
        assert!(!move_item(&mut v, 9, 0));
        assert!(move_item(&mut v, 0, 99));
        assert_eq!(v, vec!['b', 'c', 'a', 'd']);
        // The selected entry keeps pointing at the same tab.
        for from in 0..4 {
            for to in 0..4 {
                for sel in 0..4 {
                    let names = ['a', 'b', 'c', 'd'];
                    let mut w = names.to_vec();
                    move_item(&mut w, from, to);
                    assert_eq!(w[follow_index(sel, from, to)], names[sel]);
                }
            }
        }
    }

    #[test]
    fn box_without_own_rules_deserializes_to_none() {
        let f: Fence = serde_json::from_str(r#"{"id": 3, "name": "x"}"#).unwrap();
        assert!(f.categories.is_none());
        let def = default_categories();
        assert_eq!(f.rules(&def).len(), def.len());
        assert!(!serde_json::to_string(&f).unwrap().contains("categories"));
    }

    #[test]
    fn folder_box_with_auto_sort_becomes_kind_tabs() {
        let mut f = Fence { folder: Some("C://x".into()), auto_sort: true, ..Fence::default() };
        f.normalize_kind_tabs();
        assert!(!f.auto_sort && f.kind_tabs && f.rule_tabs());
        let mut c = Fence { auto_sort: true, ..Fence::default() };
        c.normalize_kind_tabs();
        assert!(c.auto_sort && !c.kind_tabs && c.rule_tabs());
        let g: Fence = serde_json::from_str(r#"{"id": 3}"#).unwrap();
        assert!(!g.kind_tabs && !g.rule_tabs());
    }
}
