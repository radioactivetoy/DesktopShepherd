//! The saved layout: boxes (fences), their tabs and items, and app-wide settings.
//! Field set follows the original's per-box record (see MECHANICS.md).

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TitlePos {
    #[default]
    Left,
    Top,
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
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            blur: 16,
            monitor_mode: MonitorMode::Role,
            categories: default_categories(),
            // The boxes replace the desktop icons while DeskBoxes runs (original: organize_hide).
            hide_desktop_icons: true,
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
}

impl Layout {
    pub fn new_id(&mut self) -> u64 {
        self.next_id = self.next_id.max(1) + 1;
        self.next_id
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

/// A named group of file extensions; the auto-sort box shows one tab per rule that has items.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CategoryRule {
    pub name: String,
    /// Lower-case extensions without the dot, e.g. ["zip", "rar"].
    pub exts: Vec<String>,
}

fn rule(name: &str, exts: &[&str]) -> CategoryRule {
    CategoryRule { name: name.into(), exts: exts.iter().map(|e| e.to_string()).collect() }
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
    if item.is_dir {
        return "Folders".into();
    }
    rules
        .iter()
        .find(|r| r.exts.iter().any(|e| e.trim_start_matches('.').eq_ignore_ascii_case(&item.ext)))
        .map(|r| r.name.clone())
        .unwrap_or_else(|| "Other".into())
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