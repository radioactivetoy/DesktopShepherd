//! Drawing one box (fence): background, title, tabs, icon grid. Used both by the box's own
//! window (interactive) and by the layout preview (scaled, read-only).

use crate::icons::IconCache;
use crate::model::{tab_name_for, CategoryRule, DeskItem, Fence, SortKind, TabPos, TitlePos};
use egui::{self, text::LayoutJob, Align, Color32, CornerRadius, FontId, Pos2, Rect, Stroke, Vec2};
use std::collections::HashMap;

pub const TITLE_STRIP: f32 = 26.0; // width of the vertical title strip / height of a top title
pub const TAB_STRIP: f32 = 26.0;
pub const ROLLED_HEIGHT: f32 = 30.0;

/// Fonts a box can use: the name shown in the menu and its file in the Windows Fonts folder.
/// Only the fonts boxes actually use are loaded (see `setup_style` in app.rs).
pub const FONTS: &[(&str, &str)] = &[
    ("Segoe UI", "segoeui.ttf"),
    ("Segoe UI Light", "segoeuil.ttf"),
    ("Segoe UI Semibold", "seguisb.ttf"),
    ("Bahnschrift", "bahnschrift.ttf"),
    ("Calibri", "calibri.ttf"),
    ("Arial", "arial.ttf"),
    ("Verdana", "verdana.ttf"),
    ("Tahoma", "tahoma.ttf"),
    ("Trebuchet MS", "trebuc.ttf"),
    ("Georgia", "georgia.ttf"),
    ("Consolas", "consola.ttf"),
    ("Segoe Print", "segoepr.ttf"),
];

/// Fonts already loaded into the drawing context. A box asking for one that isn't loaded yet
/// draws in the default font: egui must never be asked for a font it doesn't have.
static LOADED_FONTS: std::sync::RwLock<Vec<String>> = std::sync::RwLock::new(Vec::new());

pub fn set_loaded_fonts(names: &[String]) {
    if let Ok(mut l) = LOADED_FONTS.write() {
        *l = names.to_vec();
    }
}

/// The font family a box's text uses.
pub fn family(fence: &Fence) -> egui::FontFamily {
    if !fence.font.is_empty() && LOADED_FONTS.read().map(|l| l.iter().any(|n| *n == fence.font)).unwrap_or(false) {
        egui::FontFamily::Name(fence.font.as_str().into())
    } else {
        egui::FontFamily::Proportional
    }
}

/// How a box draws its icon captions.
#[derive(Clone)]
pub struct TextStyle {
    pub family: egui::FontFamily,
    pub color: Color32,
    pub shadow: bool,
    pub captions: bool,
    pub size: f32,
    pub backing: bool,
    pub zoom: bool,
    pub calm: bool,
}

pub fn text_style(fence: &Fence) -> TextStyle {
    let [r, g, b] = ink_rgb(fence.text_color);
    TextStyle { family: family(fence), color: Color32::from_rgb(r, g, b), shadow: fence.text_shadow, captions: fence.show_captions, size: fence.caption_size, backing: fence.name_backing, zoom: fence.hover_zoom, calm: fence.calm_icons }
}

/// Everything about the desktop a box needs to draw itself.
pub struct DeskState<'a> {
    pub items: &'a HashMap<String, DeskItem>,
    /// Paths held by boxes that are not auto-sort (lower-case), for the auto-sort box.
    pub claimed: &'a std::collections::HashSet<String>,
    pub spacing: (f32, f32),
    pub rules: &'a [CategoryRule],
    pub open_counts: &'a HashMap<String, u32>,
    /// Changes whenever anything `visible_tabs` reads changes (scan result, which items the
    /// boxes hold, the rules; once a minute while an age rule exists). See `tabs_key`.
    pub rev: u64,
}

/// Sorts items the way the original's menu offers (0x180223170): name, size, type, date
/// modified, times opened; ascending by default (sort_upper = 1 in the constructor 0x180197810).
pub fn sort_items(v: &mut [DeskItem], sort: SortKind, open_counts: &HashMap<String, u32>) {
    // Keys are built once per item (not inside the comparator, which would lower-case names
    // over and over).
    let name = |a: &DeskItem| a.name.to_lowercase();
    match sort {
        SortKind::Manual => {}
        SortKind::Name => v.sort_by_cached_key(name),
        SortKind::Size => v.sort_by_cached_key(|a| (a.is_dir, a.size, name(a))),
        SortKind::Type => v.sort_by_cached_key(|a| (!a.is_dir, a.ext.clone(), name(a))),
        SortKind::Modified => v.sort_by_cached_key(|a| (std::cmp::Reverse(a.modified), name(a))),
        SortKind::OpenCount => v.sort_by_cached_key(|a| (std::cmp::Reverse(open_counts.get(&a.path.to_lowercase()).copied().unwrap_or(0)), name(a))),
    }
}

/// A fingerprint of everything `visible_tabs` depends on for this box, to tell whether a
/// result kept from an earlier frame is still right.
pub fn tabs_key(fence: &Fence, desk: &DeskState) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (desk.rev, std::mem::discriminant(&fence.sort), fence.auto_sort, fence.kind_tabs, &fence.folder).hash(&mut h);
    if fence.sort == SortKind::OpenCount {
        // Order-independent sum over the counts.
        let mut sum: u64 = 0;
        for (k, v) in desk.open_counts.iter() {
            let mut eh = std::collections::hash_map::DefaultHasher::new();
            (k, v).hash(&mut eh);
            sum = sum.wrapping_add(eh.finish());
        }
        (sum, desk.open_counts.len()).hash(&mut h);
    }
    if fence.folder.is_none() && !fence.auto_sort {
        for t in &fence.tabs {
            (&t.name, &t.items).hash(&mut h);
        }
    } else if fence.rule_tabs() {
        if let Some(rs) = &fence.categories {
            for r in rs {
                (&r.name, &r.exts, &r.pattern, r.days).hash(&mut h);
            }
        }
    }
    h.finish()
}

/// Splits `items` into tabs by the box's tab rules (rules in order, then Folders, then Other),
/// each sorted with the box's sort (Manual counts as Name). No items: one empty tab `empty_name`.
fn group_by_rules(fence: &Fence, desk: &DeskState, items: Vec<DeskItem>, empty_name: String) -> Vec<(String, Vec<DeskItem>)> {
    let rules = fence.rules(desk.rules);
    let mut by_tab: HashMap<String, Vec<DeskItem>> = HashMap::new();
    for it in items {
        by_tab.entry(tab_name_for(&it, rules)).or_default().push(it);
    }
    let mut order: Vec<String> = rules.iter().map(|r| r.name.clone()).collect();
    order.push("Folders".into());
    order.push("Other".into());
    let sort = if fence.sort == SortKind::Manual { SortKind::Name } else { fence.sort };
    let mut tabs = Vec::new();
    for name in order {
        if let Some(mut v) = by_tab.remove(&name) {
            sort_items(&mut v, sort, desk.open_counts);
            tabs.push((name, v));
        }
    }
    if tabs.is_empty() {
        tabs.push((empty_name, vec![]));
    }
    tabs
}

/// The tabs as shown: names and the items in each, already filtered and sorted.
pub fn visible_tabs(fence: &Fence, desk: &DeskState) -> Vec<(String, Vec<DeskItem>)> {
    if let Some(dir) = &fence.folder {
        // A folder box: whatever is in the folder right now.
        let dir_l = dir.trim_end_matches(['\\', '/']).to_lowercase();
        let mut v: Vec<DeskItem> = desk
            .items
            .values()
            .filter(|it| std::path::Path::new(&it.path).parent().map(|p| p.to_string_lossy().to_lowercase() == dir_l).unwrap_or(false))
            .cloned()
            .collect();
        let name = std::path::Path::new(dir).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| dir.clone());
        if fence.kind_tabs {
            return group_by_rules(fence, desk, v, name);
        }
        let sort = if fence.sort == SortKind::Manual { SortKind::Name } else { fence.sort };
        sort_items(&mut v, sort, desk.open_counts);
        return vec![(name, v)];
    }
    if fence.auto_sort {
        let v: Vec<DeskItem> = desk.items.iter().filter(|(key, it)| !desk.claimed.contains(*key) && it.on_desktop).map(|(_, it)| it.clone()).collect();
        return group_by_rules(fence, desk, v, "Desktop".to_string());
    }
    fence
        .tabs
        .iter()
        .map(|t| {
            // Each file once, even if the layout lists it twice (see dedupe_items in app.rs).
            let mut seen = std::collections::HashSet::new();
            let mut v: Vec<DeskItem> = t
                .items
                .iter()
                .filter_map(|p| {
                    let k = p.to_lowercase();
                    if seen.insert(k.clone()) { desk.items.get(&k).cloned() } else { None }
                })
                .collect();
            sort_items(&mut v, fence.sort, desk.open_counts);
            (t.name.clone(), v)
        })
        .collect()
}
pub fn cell_size(fence: &Fence, spacing: (f32, f32)) -> Vec2 {
    let icon = fence.icon_size as f32;
    // Horizontal pitch = the box's own spacing, or Windows desktop icon spacing (75 px by
    // default); vertical pitch fits the icon plus two lines of caption (≈100 px at 48 px icons
    // and 12 pt text, measured on the original's layout picture), or just the icon without captions.
    let x = if fence.icon_spacing > 0.0 { fence.icon_spacing.max(icon + 8.0) } else { spacing.0.max(icon + 20.0) };
    let y = if fence.show_captions { icon + 28.0 + 2.0 * fence.caption_size } else { icon + 16.0 };
    Vec2::new(x, y)
}

/// Areas of a box: where title, tabs and content go.
pub struct FenceAreas {
    pub title: Rect,
    pub tabs: Option<Rect>,
    pub content: Rect,
}

pub fn areas(fence: &Fence, rect: Rect, tab_count: usize, scale: f32) -> FenceAreas {
    areas_x(fence, fence.expanded, rect, tab_count, scale)
}

/// `areas` for a box shown open (or rolled up) regardless of its saved `expanded` flag (hover-open).
pub fn areas_x(fence: &Fence, expanded: bool, rect: Rect, tab_count: usize, scale: f32) -> FenceAreas {
    let ts = TITLE_STRIP * scale;
    let tb = TAB_STRIP * scale;
    if !expanded {
        return FenceAreas { title: rect, tabs: None, content: Rect::NOTHING };
    }
    let (title, mut rest) = match fence.title_pos {
        TitlePos::Left => (
            Rect::from_min_max(rect.min, Pos2::new(rect.min.x + ts, rect.max.y)),
            Rect::from_min_max(Pos2::new(rect.min.x + ts, rect.min.y), rect.max),
        ),
        TitlePos::Top => (
            Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + ts)),
            Rect::from_min_max(Pos2::new(rect.min.x, rect.min.y + ts), rect.max),
        ),
        TitlePos::Bottom => (
            Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - ts), rect.max),
            Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.max.y - ts)),
        ),
        TitlePos::Hidden => {
            // A thin grab strip at the top (no text).
            let g = 10.0 * scale;
            (Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.min.y + g)), Rect::from_min_max(Pos2::new(rect.min.x, rect.min.y + g), rect.max))
        }
    };
    let tabs = if tab_count > 1 {
        Some(match fence.tab_pos {
            TabPos::Right => {
                let r = Rect::from_min_max(Pos2::new(rest.max.x - tb, rest.min.y), rest.max);
                rest.max.x -= tb;
                r
            }
            TabPos::Top => {
                let r = Rect::from_min_max(rest.min, Pos2::new(rest.max.x, rest.min.y + tb));
                rest.min.y += tb;
                r
            }
        })
    } else {
        None
    };
    FenceAreas { title, tabs, content: rest.shrink(4.0 * scale) }
}

pub fn paint_background(painter: &egui::Painter, fence: &Fence, rect: Rect, scale: f32) {
    paint_background_h(painter, fence, rect, scale, false)
}

/// `paint_background` with the mouse-over state (for the accent border and the hover lift).
pub fn paint_background_h(painter: &egui::Painter, fence: &Fence, rect: Rect, scale: f32, hover: bool) {
    let [r, g, b] = fence.color;
    let fill = Color32::from_rgba_unmultiplied(r, g, b, fence.alpha);
    let radius = CornerRadius::same((fence.corner * scale).round().clamp(0.0, 40.0) as u8);
    painter.rect_filled(rect, radius, fill);
    paint_body_overlays(painter, fence, rect, scale);
    painter.rect_stroke(rect, radius, border_stroke_h(fence, scale, hover), egui::StrokeKind::Inside);
    if fence.raised {
        paint_raised(painter, rect.shrink(0.5 * scale), (fence.corner * scale).clamp(0.0, 40.0), scale);
    }
}

/// The optional colour fade and acrylic noise over the box body (on top of the glass tint).
pub fn paint_body_overlays(painter: &egui::Painter, fence: &Fence, rect: Rect, scale: f32) {
    if let Some(g) = &fence.gradient {
        let pts = rounded_points(rect, (fence.corner * scale).clamp(0.0, 40.0), 8);
        let t_at = |p: Pos2| {
            let u = ((p.x - rect.min.x) / rect.width().max(1.0)).clamp(0.0, 1.0);
            let v = ((p.y - rect.min.y) / rect.height().max(1.0)).clamp(0.0, 1.0);
            match g.angle {
                1 => u,
                2 => (u + v) / 2.0,
                _ => v,
            }
        };
        let col = |p: Pos2| Color32::from_rgba_unmultiplied(g.color2[0], g.color2[1], g.color2[2], (g.alpha2 as f32 * t_at(p)).round().clamp(0.0, 255.0) as u8);
        let mut mesh = egui::Mesh::default();
        let c = rect.center();
        mesh.colored_vertex(c, col(c));
        for p in &pts {
            mesh.colored_vertex(*p, col(*p));
        }
        let n = pts.len() as u32;
        for i in 0..n {
            mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
        }
        painter.add(egui::Shape::mesh(mesh));
    }
    if fence.texture {
        paint_noise(painter, rect.shrink(1.0 * scale));
    }
}

/// Side of the noise tile in pixels.
const NOISE: usize = 128;

/// The noise picture: grey specks at about 4-6 % opacity, made once per drawing context.
fn noise_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::new("acrylic-noise");
    if let Some(h) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return h;
    }
    let mut s: u32 = 0x9E37_79B9;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        s
    };
    let mut px = Vec::with_capacity(NOISE * NOISE);
    for _ in 0..NOISE * NOISE {
        let r = next();
        // Mid greys only (no black or white specks) and very faint: a hint of grain, not sandpaper.
        let v = 112 + (r & 0x1F) as u8; // 112..143
        let a = 3 + ((r >> 8) & 0x3) as u8; // 3..6 of 255
        px.push(Color32::from_rgba_unmultiplied(v, v, v, a));
    }
    let img = egui::ColorImage { size: [NOISE, NOISE], pixels: px };
    let h = ctx.load_texture("acrylic-noise", img, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, h.clone()));
    h
}

/// Tiles the noise over `rect` (one texel per screen pixel). The renderer clamps texture
/// coordinates, so each tile is its own quad; all tiles go into one mesh.
fn paint_noise(painter: &egui::Painter, rect: Rect) {
    let tex = noise_texture(painter.ctx());
    let ppp = painter.ctx().pixels_per_point().max(0.5);
    let tile = NOISE as f32 / ppp;
    let mut mesh = egui::Mesh::with_texture(tex.id());
    let mut y = rect.min.y;
    while y < rect.max.y {
        let h = tile.min(rect.max.y - y);
        let mut x = rect.min.x;
        while x < rect.max.x {
            let w = tile.min(rect.max.x - x);
            let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(w / tile, h / tile));
            mesh.add_rect_with_uv(Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h)), uv, Color32::WHITE);
            x += tile;
        }
        y += tile;
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// The points of a rounded rectangle, clockwise from the start of the top-left corner; `n`
/// points per corner.
fn rounded_points(r: Rect, rad: f32, n: usize) -> Vec<Pos2> {
    let rad = rad.min(r.width() / 2.0).min(r.height() / 2.0).max(0.0);
    let corners = [
        (Pos2::new(r.min.x + rad, r.min.y + rad), std::f32::consts::PI),
        (Pos2::new(r.max.x - rad, r.min.y + rad), 1.5 * std::f32::consts::PI),
        (Pos2::new(r.max.x - rad, r.max.y - rad), 0.0),
        (Pos2::new(r.min.x + rad, r.max.y - rad), 0.5 * std::f32::consts::PI),
    ];
    let mut pts = Vec::with_capacity(4 * n);
    for (c, start) in corners {
        for i in 0..n {
            let a = start + (i as f32 / (n - 1).max(1) as f32) * std::f32::consts::FRAC_PI_2;
            pts.push(Pos2::new(c.x + rad * a.cos(), c.y + rad * a.sin()));
        }
    }
    pts
}

/// "Raised look": a bevel all round the inside edge (top brightest, sides softly lit, bottom dark,
/// blending through the corners), a soft sheen on the upper glass and a thin top highlight, so the
/// box reads as a raised panel. Drawn with the box (no extra windows, nothing kept in memory).
pub fn paint_raised(painter: &egui::Painter, rect: Rect, rad: f32, scale: f32) {
    let pts = rounded_points(rect, rad, 8);
    // Sheen: a fan from the centre; each point's brightness fades from the top to 60 % height.
    let shade = |p: Pos2| {
        let t = ((p.y - rect.min.y) / (rect.height() * 0.6)).clamp(0.0, 1.0);
        Color32::from_white_alpha(((1.0 - t) * (1.0 - t) * 40.0) as u8)
    };
    let mut mesh = egui::Mesh::default();
    let c = rect.center();
    mesh.colored_vertex(c, shade(c));
    for p in &pts {
        mesh.colored_vertex(*p, shade(*p));
    }
    let n = pts.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    painter.add(egui::Shape::mesh(mesh));
    // Bevel: a band inside the edge, lit from the top-left. Each edge point gets its colour from
    // which way the edge faces (light where it faces the light, dark on the far side) and fades to
    // nothing towards the inside, like a raised button.
    let d = (10.0 * scale).max(4.0);
    let inner = rounded_points(rect.shrink(d), (rad - d).max(0.0), 8);
    let mut bevel = egui::Mesh::default();
    for (o, i) in pts.iter().zip(inner.iter()) {
        let n = (*o - *i).normalized();
        // Top brightest, both sides softly lit (the same left and right, so no seams), bottom
        // dark; continuous all the way round, so the corners blend.
        let f = if n.y <= 0.0 { 0.45 + 0.55 * -n.y } else { 0.45 - 1.45 * n.y };
        let c = if f >= 0.0 { Color32::from_white_alpha((f * 150.0) as u8) } else { Color32::from_black_alpha((-f * 130.0) as u8) };
        bevel.colored_vertex(*o, c);
        bevel.colored_vertex(*i, Color32::TRANSPARENT);
    }
    let m = pts.len() as u32;
    for k in 0..m {
        let (a, b) = (2 * k, 2 * ((k + 1) % m));
        bevel.add_triangle(a, a + 1, b);
        bevel.add_triangle(b, a + 1, b + 1);
    }
    painter.add(egui::Shape::mesh(bevel));
    // A crisp highlight on the very top edge.
    let q = pts.len() / 4;
    // (only the straight part and the start of each top corner, so it fades into the sides)
    let top: Vec<Pos2> = pts[q - 3..q + 3].to_vec();
    painter.add(egui::Shape::line(top, Stroke::new((1.5 * scale).max(1.0), Color32::from_white_alpha(170))));
}

/// The box border (colour, thickness; none at thickness 0) with the optional accent colour and hover lift (brighter while the mouse is over).
pub fn border_stroke_h(fence: &Fence, scale: f32, hover: bool) -> Stroke {
    let [br, bg, bb, ba] = fence.border_color;
    let accent = fence.accent_border == 1 || (fence.accent_border == 2 && hover);
    if fence.border_width <= 0.0 && !accent {
        return Stroke::NONE;
    }
    let width = if accent { fence.border_width.max(1.0) } else { fence.border_width };
    let mut c = if accent { crate::app::accent() } else { Color32::from_rgba_unmultiplied(br, bg, bb, ba) };
    if hover && fence.hover_lift {
        let [r, g, b, a] = c.to_srgba_unmultiplied();
        let up = |v: u8| (v as f32 + (255.0 - v as f32) * 0.35).round() as u8;
        c = Color32::from_rgba_unmultiplied(up(r), up(g), up(b), (a as u16 + 90).min(255) as u8);
    }
    Stroke::new(width * scale, c)
}

/// On a bright backdrop (dark ink) a light text colour turns dark; a dark or coloured choice stays.
fn ink_rgb(c: [u8; 3]) -> [u8; 3] {
    if crate::ink::is_dark() && c.iter().all(|v| *v > 170) {
        [20, 20, 20]
    } else {
        c
    }
}

/// The box's text colour, dimmed (`f` < 1) for less important text.
fn text_color(fence: &Fence, f: f32) -> Color32 {
    let [r, g, b] = ink_rgb(fence.text_color);
    if crate::ink::is_dark() && [r, g, b] == [20, 20, 20] {
        // Dark ink: dimming means lighter, towards the backdrop.
        let v = (20.0 + (1.0 - f) * 90.0).round().clamp(0.0, 255.0) as u8;
        return Color32::from_rgb(v, v, v);
    }
    let m = |v: u8| (v as f32 * f).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(m(r), m(g), m(b))
}

/// Text rotated a quarter turn clockwise, centred in `strip`, read top to bottom
/// (like the original's vertical titles and tabs).
pub fn vertical_text(painter: &egui::Painter, strip: Rect, text: &str, font: FontId, color: Color32) {
    let galley = painter.layout_no_wrap(text.to_string(), font, color);
    vertical_galley(painter, strip, galley, color, Vec2::ZERO);
}

fn vertical_galley(painter: &egui::Painter, strip: Rect, galley: std::sync::Arc<egui::Galley>, color: Color32, nudge: Vec2) {
    let w = galley.size().x.min(strip.height() - 6.0);
    let h = galley.size().y;
    let pos = Pos2::new(strip.center().x + h / 2.0, strip.center().y - w / 2.0) + nudge;
    let clip = painter.with_clip_rect(strip);
    clip.add(egui::epaint::TextShape::new(pos, galley, color).with_angle(std::f32::consts::FRAC_PI_2));
}

pub fn paint_title(painter: &egui::Painter, fence: &Fence, title: Rect, scale: f32, hovered: bool) {
    // (the layout preview has no mouse: a title that shows on hover is drawn)
    paint_title_x(painter, fence, fence.expanded, title, scale, hovered, true, None)
}

/// The title's colour: the box's text colour (or the title's own), dimmed when not hovered
/// and for the light weight.
fn title_color(fence: &Fence, hovered: bool) -> Color32 {
    let f = if hovered { 1.0 } else { 0.85 } * if fence.title_weight == 2 { 0.7 } else { 1.0 };
    match fence.title_color {
        Some([r, g, b]) => {
            let m = |v: u8| (v as f32 * f).round().clamp(0.0, 255.0) as u8;
            Color32::from_rgb(m(r), m(g), m(b))
        }
        None => text_color(fence, f),
    }
}

/// The header band or line behind / under the title.
fn paint_header(painter: &egui::Painter, fence: &Fence, title: Rect, scale: f32) {
    match fence.header_style {
        1 => {
            // The band always contrasts with the title: darker behind light text, lighter behind
            // dark text (adaptive ink, or a dark title colour chosen in Personalize).
            let tc = fence.title_color.unwrap_or(fence.text_color);
            let dark_text = crate::ink::is_dark() || (tc[0] as u32 + tc[1] as u32 + tc[2] as u32) < 300;
            let fill = if dark_text { Color32::from_white_alpha(90) } else { Color32::from_black_alpha(70) };
            let bw = fence.border_width.max(0.0) * scale;
            let rad = ((fence.corner * scale).round().clamp(0.0, 40.0) - bw).max(0.0) as u8;
            let cr = match fence.title_pos {
                TitlePos::Left => CornerRadius { nw: rad, sw: rad, ne: 0, se: 0 },
                TitlePos::Top => CornerRadius { nw: rad, ne: rad, sw: 0, se: 0 },
                TitlePos::Bottom => CornerRadius { sw: rad, se: rad, nw: 0, ne: 0 },
                TitlePos::Hidden => return,
            };
            painter.rect_filled(title.shrink(bw), cr, fill);
        }
        2 => {
            let m = 8.0 * scale;
            let (a, b) = match fence.title_pos {
                TitlePos::Left => (Pos2::new(title.max.x, title.min.y + m), Pos2::new(title.max.x, title.max.y - m)),
                TitlePos::Top => (Pos2::new(title.min.x + m, title.max.y), Pos2::new(title.max.x - m, title.max.y)),
                TitlePos::Bottom => (Pos2::new(title.min.x + m, title.min.y), Pos2::new(title.max.x - m, title.min.y)),
                TitlePos::Hidden => return,
            };
            painter.line_segment([a, b], Stroke::new(scale.max(1.0), crate::ink::white_alpha(50)));
        }
        _ => {}
    }
}

/// `paint_title` with the open/rolled-up state given separately (see `areas_x`), whether the
/// mouse is over the whole box (`box_hover`, for titles that only show on hover) and the number of
/// items for the count badge (None = no badge).
#[allow(clippy::too_many_arguments)]
pub fn paint_title_x(painter: &egui::Painter, fence: &Fence, expanded: bool, title: Rect, scale: f32, hovered: bool, box_hover: bool, count: Option<usize>) {
    let color = title_color(fence, hovered);
    if expanded && fence.title_pos == TitlePos::Hidden {
        // No title: only a faint handle while the mouse is over the strip.
        if hovered {
            let w = (title.width() * 0.2).clamp(24.0 * scale, 80.0 * scale);
            let bar = Rect::from_center_size(title.center(), Vec2::new(w, 3.0 * scale));
            painter.rect_filled(bar, 2.0 * scale, text_color(fence, 0.6));
        }
        return;
    }
    if expanded {
        paint_header(painter, fence, title, scale);
    }
    let show_text = !(fence.title_on_hover && expanded && !box_hover);
    if show_text {
        let font = FontId::new(fence.title_size * scale, family(fence));
        let mut text = if fence.title_caps { fence.name.to_uppercase() } else { fence.name.clone() };
        if !fence.title_icon.is_empty() {
            text = format!("{}  {}", fence.title_icon, text);
        }
        let spacing = if fence.title_caps { 1.2 * scale } else { 0.0 };
        let mut job = LayoutJob::default();
        job.append(&text, 0.0, egui::TextFormat { font_id: font, color, extra_letter_spacing: spacing, ..Default::default() });
        let galley = painter.layout_job(job);
        // The count badge: a small rounded pill with the number.
        let badge = count.filter(|_| fence.show_count && fence.widget.is_none()).map(|n| {
            let g = painter.layout_no_wrap(n.to_string(), FontId::new((fence.title_size * 0.7).max(8.0) * scale, family(fence)), color);
            let pill = Vec2::new((g.size().x + 8.0 * scale).max(g.size().y + 2.0 * scale), g.size().y + 2.0 * scale);
            (g, pill)
        });
        let bold = fence.title_weight == 1;
        let pill_at = |painter: &egui::Painter, rect: Rect, g: std::sync::Arc<egui::Galley>| {
            // Contrasting pill: dark behind light text, light behind dark text.
            let light_text = color.r() as u32 + color.g() as u32 + color.b() as u32 > 380;
            let fill = if light_text { Color32::from_black_alpha(110) } else { Color32::from_white_alpha(150) };
            painter.rect_filled(rect, rect.height() / 2.0, fill);
            painter.galley(rect.center() - g.size() / 2.0, g, color);
        };
        if expanded && fence.title_pos == TitlePos::Left {
            let mut strip = title;
            if let Some((g, pill)) = badge {
                let pill = Vec2::new(pill.x.min(title.width() - 2.0), pill.y);
                strip.max.y -= pill.y + 8.0 * scale;
                let r = Rect::from_center_size(Pos2::new(title.center().x, title.max.y - pill.y / 2.0 - 4.0 * scale), pill);
                pill_at(&painter.with_clip_rect(title), r, g);
            }
            if bold {
                vertical_galley(painter, strip, galley.clone(), color, Vec2::new(0.0, 0.5 * scale));
            }
            vertical_galley(painter, strip, galley, color, Vec2::ZERO);
        } else {
            let gap = 6.0 * scale;
            let total = galley.size().x + badge.as_ref().map(|(_, p)| gap + p.x).unwrap_or(0.0);
            let lock_pad = if fence.locked { 14.0 * scale } else { 0.0 };
            let x0 = match fence.title_align {
                1 => title.min.x + 10.0 * scale,
                2 => title.max.x - 10.0 * scale - lock_pad - total,
                _ => title.center().x - total / 2.0,
            }
            .max(title.min.x + 2.0 * scale);
            let clip = painter.with_clip_rect(title);
            let pos = Pos2::new(x0, title.center().y - galley.size().y / 2.0);
            if bold {
                clip.galley(pos + Vec2::new(0.5 * scale, 0.0), galley.clone(), color);
            }
            let end = pos.x + galley.size().x;
            clip.galley(pos, galley, color);
            if let Some((g, pill)) = badge {
                let r = Rect::from_min_size(Pos2::new(end + gap, title.center().y - pill.y / 2.0), pill);
                pill_at(&clip, r, g);
            }
        }
    }
    if fence.locked {
        let p = Pos2::new(title.max.x - 9.0 * scale, title.min.y + 9.0 * scale);
        painter.circle_stroke(p, 3.0 * scale, Stroke::new(1.0_f32, text_color(fence, 0.65)));
    }
}

/// Paints the tab strip and returns each tab's rectangle.
pub fn paint_tabs(painter: &egui::Painter, fence: &Fence, strip: Rect, names: &[String], sel: usize, scale: f32) -> Vec<Rect> {
    let n = names.len().max(1) as f32;
    let mut rects = Vec::new();
    let font = FontId::new(fence.caption_size * scale, family(fence));
    for (i, name) in names.iter().enumerate() {
        let r = match fence.tab_pos {
            TabPos::Right => {
                let h = (strip.height() / n).min(70.0 * scale);
                Rect::from_min_size(Pos2::new(strip.min.x, strip.min.y + i as f32 * h), Vec2::new(strip.width(), h))
            }
            TabPos::Top => {
                let w = (strip.width() / n).min(90.0 * scale);
                Rect::from_min_size(Pos2::new(strip.min.x + i as f32 * w, strip.min.y), Vec2::new(w, strip.height()))
            }
        };
        let active = i == sel;
        let color = if active { text_color(fence, 1.0) } else { text_color(fence, 0.67) };
        let mark = text_color(fence, 0.9);
        match fence.tab_pos {
            TabPos::Right => {
                vertical_text(painter, r, name, font.clone(), color);
                if active {
                    painter.line_segment(
                        [Pos2::new(r.min.x + 1.0, r.min.y + 4.0), Pos2::new(r.min.x + 1.0, r.max.y - 4.0)],
                        Stroke::new(1.5 * scale, mark),
                    );
                }
            }
            TabPos::Top => {
                let g = painter.layout_no_wrap(name.clone(), font.clone(), color);
                let pos = Pos2::new(r.center().x - g.size().x / 2.0, r.center().y - g.size().y / 2.0);
                painter.with_clip_rect(r).galley(pos, g, color);
                if active {
                    painter.line_segment(
                        [Pos2::new(r.min.x + 6.0, r.max.y - 1.0), Pos2::new(r.max.x - 6.0, r.max.y - 1.0)],
                        Stroke::new(1.5 * scale, mark),
                    );
                }
            }
        }
        rects.push(r);
    }
    rects
}

pub fn caption_galley(painter: &egui::Painter, text: &str, width: f32, font: FontId) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::simple(text.to_string(), font, Color32::WHITE, width);
    // Two lines at most, broken between words (a word too long for a whole line is still split);
    // a name longer than that ends with "…".
    job.wrap.max_rows = 2;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = Some('…');
    job.halign = Align::Center;
    painter.layout_job(job)
}

/// Paints one icon cell (icon + caption). Returns nothing; interaction is done by the caller.
#[allow(clippy::too_many_arguments)]
pub fn paint_item(painter: &egui::Painter, icons: &mut IconCache, item: &DeskItem, cell: Rect, icon: f32, scale: f32, highlight: u8, style: &TextStyle, hot: bool, selected: bool) {
    if highlight > 0 {
        painter.rect_filled(cell.shrink(2.0 * scale), CornerRadius::same((4.0 * scale) as u8), if crate::ink::is_dark() { Color32::from_black_alpha(highlight) } else { Color32::from_white_alpha(highlight) });
    }
    let icon_rect = Rect::from_center_size(Pos2::new(cell.center().x, cell.min.y + 6.0 * scale + icon / 2.0), Vec2::splat(icon));
    // Hover zoom: the icon under the mouse a little larger (the caption stays where it was).
    let draw_rect = if style.zoom && hot { Rect::from_center_size(icon_rect.center(), icon_rect.size() * 1.08) } else { icon_rect };
    // Calm icons: slightly dimmed until hovered or selected.
    let tint = if style.calm && !hot && !selected { Color32::from_rgba_unmultiplied(200, 200, 200, 210) } else { Color32::WHITE };
    // In real pixels at the box's own size (the layout preview reuses the boxes' icons).
    let px = (icon / scale.max(0.01) * painter.ctx().pixels_per_point()).round() as u32;
    match icons.get(&item.path, px) {
        Some(tex) => {
            painter.image(tex, draw_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), tint);
        }
        None => {
            // Placeholder while the real icon loads.
            let c = if item.is_dir { Color32::from_rgb(232, 190, 80) } else { Color32::from_gray(120) };
            painter.rect_filled(draw_rect.shrink(icon * 0.12), CornerRadius::same((3.0 * scale) as u8), if tint == Color32::WHITE { c } else { c.gamma_multiply(0.85) });
        }
    }
    if !style.captions {
        return;
    }
    let font = FontId::new(style.size * scale, style.family.clone());
    let galley = caption_galley(painter, &item.name, cell.width() - 4.0 * scale, font);
    let pos = Pos2::new(cell.center().x, icon_rect.max.y + 4.0 * scale);
    let clip = painter.with_clip_rect(cell);
    if style.backing {
        // A soft rounded backing behind the caption: dark behind light text, light behind dark ink.
        let r = galley.rect.translate(pos.to_vec2()).expand2(Vec2::new(4.0 * scale, 1.0 * scale));
        clip.rect_filled(r, CornerRadius::same((5.0 * scale) as u8), if crate::ink::is_dark() { Color32::from_white_alpha(110) } else { Color32::from_black_alpha(110) });
    }
    if style.shadow {
        clip.galley_with_override_text_color(pos + Vec2::splat(1.0 * scale.max(0.6)), galley.clone(), crate::ink::halo());
    }
    // The galley is laid out white: the colour is given here (the box's text colour, or dark ink).
    clip.galley_with_override_text_color(pos, galley, style.color);
}

/// Non-interactive drawing of a whole box at `rect` (used by the layout preview).
pub fn paint_static(painter: &egui::Painter, icons: &mut IconCache, fence: &Fence, desk: &DeskState, rect: Rect, scale: f32) {
    paint_background(painter, fence, rect, scale);
    if let Some(wc) = &fence.widget {
        let a = areas(fence, rect, 1, scale);
        paint_title(painter, fence, a.title, scale, false);
        let label = crate::widgets::find(&wc.kind).map(|k| crate::i18n::t(k.label)).unwrap_or(crate::i18n::t("Widget"));
        let g = painter.layout_no_wrap(label.to_string(), FontId::proportional(14.0 * scale), Color32::from_gray(200));
        painter.galley(a.content.center() - g.size() / 2.0, g, Color32::from_gray(200));
        return;
    }
    let tabs = visible_tabs(fence, desk);
    let a = areas(fence, rect, tabs.len(), scale);
    paint_title(painter, fence, a.title, scale, false);
    if !fence.expanded {
        return;
    }
    let sel = fence.sel_tab.min(tabs.len().saturating_sub(1));
    if let Some(strip) = a.tabs {
        let names: Vec<String> = tabs.iter().map(|t| t.0.clone()).collect();
        paint_tabs(painter, fence, strip, &names, sel, scale);
    }
    let cell = cell_size(fence, desk.spacing) * scale;
    let style = text_style(fence);
    let cols = ((a.content.width() / cell.x).floor() as usize).max(1);
    let clip = painter.with_clip_rect(a.content);
    if let Some((_, items)) = tabs.get(sel) {
        for (i, it) in items.iter().enumerate() {
            let (c, r) = (i % cols, i / cols);
            let min = a.content.min + Vec2::new(c as f32 * cell.x, r as f32 * cell.y);
            if min.y > a.content.max.y {
                break;
            }
            paint_item(&clip, icons, it, Rect::from_min_size(min, cell), fence.icon_size as f32 * scale, scale, 0, &style, false, false);
        }
    }
}


/// Windows' fonts folder (normally C:\Windows\Fonts).
pub fn fonts_dir() -> std::path::PathBuf {
    std::env::var_os("WINDIR").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::PathBuf::from(r"C:\Windows")).join("Fonts")
}

/// The names in `FONTS` whose file is on this PC (checked once, not on every redraw).
pub fn installed_fonts() -> &'static [&'static str] {
    static LIST: std::sync::OnceLock<Vec<&'static str>> = std::sync::OnceLock::new();
    LIST.get_or_init(|| {
        let dir = fonts_dir();
        FONTS.iter().filter(|(n, f)| *n == "Segoe UI" || dir.join(f).exists()).map(|(n, _)| *n).collect()
    })
}

#[cfg(test)]
mod kind_tab_tests {
    use super::*;
    use crate::model::Category;

    fn item(dir: &str, name: &str, ext: &str) -> (String, DeskItem) {
        let path = format!("{dir}\\{name}.{ext}");
        (path.to_lowercase(), DeskItem { path, name: name.into(), ext: ext.into(), is_dir: false, category: Category::Other, on_desktop: false, size: 0, modified: 0 })
    }

    #[test]
    fn kind_tabs_folder_box_splits_by_rules() {
        let items: HashMap<String, DeskItem> = [
            item("C:\\Dl", "b", "png"),
            item("C:\\Dl", "a", "png"),
            item("C:\\Dl", "notes", "zzz"),
            item("C:\\Other", "elsewhere", "png"),
        ]
        .into_iter()
        .collect();
        let claimed = std::collections::HashSet::new();
        let rules = vec![crate::model::CategoryRule { name: "Pictures".into(), exts: vec!["png".into()], ..Default::default() }];
        let counts = HashMap::new();
        let desk = DeskState { items: &items, claimed: &claimed, spacing: (75.0, 100.0), rules: &rules, open_counts: &counts, rev: 0 };
        let mut f = Fence { folder: Some("C:\\Dl".into()), ..Fence::default() };
        let plain = visible_tabs(&f, &desk);
        assert_eq!(plain.len(), 1);
        assert_eq!(plain[0].1.len(), 3);
        let k0 = tabs_key(&f, &desk);
        f.kind_tabs = true;
        assert_ne!(k0, tabs_key(&f, &desk));
        let tabs = visible_tabs(&f, &desk);
        let names: Vec<&str> = tabs.iter().map(|t| t.0.as_str()).collect();
        assert_eq!(names, ["Pictures", "Other"]);
        assert_eq!(tabs[0].1.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["a", "b"]);
    }
}
