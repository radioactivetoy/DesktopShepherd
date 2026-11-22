//! Drawing one box (fence): background, title, tabs, icon grid. Used both by the box's own
//! window (interactive) and by the layout preview (scaled, read-only).

use crate::icons::IconCache;
use crate::model::{tab_name_for, CategoryRule, DeskItem, Fence, SortKind, TabPos, TitlePos};
use egui::{self, text::LayoutJob, Align, Color32, CornerRadius, FontId, Pos2, Rect, Stroke, Vec2};
use std::collections::HashMap;

pub const TITLE_STRIP: f32 = 26.0; // width of the vertical title strip / height of a top title
pub const TAB_STRIP: f32 = 26.0;
pub const ROLLED_HEIGHT: f32 = 30.0;

/// Everything about the desktop a box needs to draw itself.
pub struct DeskState<'a> {
    pub items: &'a HashMap<String, DeskItem>,
    /// Paths held by boxes that are not auto-sort (lower-case), for the auto-sort box.
    pub claimed: &'a std::collections::HashSet<String>,
    pub spacing: (f32, f32),
    pub rules: &'a [CategoryRule],
    pub open_counts: &'a HashMap<String, u32>,
}

/// Sorts items the way the original's menu offers (0x180223170): name, size, type, date
/// modified, times opened; ascending by default (sort_upper = 1 in the constructor 0x180197810).
pub fn sort_items(v: &mut [DeskItem], sort: SortKind, open_counts: &HashMap<String, u32>) {
    let name = |a: &DeskItem| a.name.to_lowercase();
    match sort {
        SortKind::Manual => {}
        SortKind::Name => v.sort_by_key(name),
        SortKind::Size => v.sort_by(|a, b| (a.is_dir, a.size, name(a)).cmp(&(b.is_dir, b.size, name(b)))),
        SortKind::Type => v.sort_by(|a, b| (!a.is_dir, a.ext.as_str(), name(a)).cmp(&(!b.is_dir, b.ext.as_str(), name(b)))),
        SortKind::Modified => v.sort_by(|a, b| b.modified.cmp(&a.modified).then_with(|| name(a).cmp(&name(b)))),
        SortKind::OpenCount => v.sort_by(|a, b| {
            let ca = open_counts.get(&a.path.to_lowercase()).copied().unwrap_or(0);
            let cb = open_counts.get(&b.path.to_lowercase()).copied().unwrap_or(0);
            cb.cmp(&ca).then_with(|| name(a).cmp(&name(b)))
        }),
    }
}

/// The tabs as shown: names and the items in each, already filtered and sorted.
pub fn visible_tabs(fence: &Fence, desk: &DeskState) -> Vec<(String, Vec<DeskItem>)> {
    if fence.auto_sort {
        let mut by_tab: HashMap<String, Vec<DeskItem>> = HashMap::new();
        for (key, it) in desk.items.iter() {
            if !desk.claimed.contains(key) && it.on_desktop {
                by_tab.entry(tab_name_for(it, desk.rules)).or_default().push(it.clone());
            }
        }
        let mut order: Vec<String> = desk.rules.iter().map(|r| r.name.clone()).collect();
        order.push("Folders".into());
        order.push("Other".into());
        let mut tabs = Vec::new();
        for name in order {
            if let Some(mut v) = by_tab.remove(&name) {
                let sort = if fence.sort == SortKind::Manual { SortKind::Name } else { fence.sort };
                sort_items(&mut v, sort, desk.open_counts);
                tabs.push((name, v));
            }
        }
        if tabs.is_empty() {
            tabs.push(("Desktop".to_string(), vec![]));
        }
        return tabs;
    }
    fence
        .tabs
        .iter()
        .map(|t| {
            let mut v: Vec<DeskItem> =
                t.items.iter().filter_map(|p| desk.items.get(&p.to_lowercase()).cloned()).collect();
            sort_items(&mut v, fence.sort, desk.open_counts);
            (t.name.clone(), v)
        })
        .collect()
}
pub fn cell_size(fence: &Fence, spacing: (f32, f32)) -> Vec2 {
    let icon = fence.icon_size as f32;
    // Horizontal pitch = Windows desktop icon spacing (75 px by default); vertical pitch fits
    // the icon plus two lines of caption (≈100 px at 48 px icons, measured on the original's
    // layout picture).
    Vec2::new(spacing.0.max(icon + 20.0), icon + 52.0)
}

/// Areas of a box: where title, tabs and content go.
pub struct FenceAreas {
    pub title: Rect,
    pub tabs: Option<Rect>,
    pub content: Rect,
}

pub fn areas(fence: &Fence, rect: Rect, tab_count: usize, scale: f32) -> FenceAreas {
    let ts = TITLE_STRIP * scale;
    let tb = TAB_STRIP * scale;
    if !fence.expanded {
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
    let [r, g, b] = fence.color;
    let fill = Color32::from_rgba_unmultiplied(r, g, b, fence.alpha);
    let [br, bg, bb, ba] = fence.border_color;
    let radius = CornerRadius::same((fence.corner * scale).round().clamp(0.0, 40.0) as u8);
    painter.rect(
        rect.shrink(0.5),
        radius,
        fill,
        Stroke::new(1.0_f32, Color32::from_rgba_unmultiplied(br, bg, bb, ba)),
        egui::StrokeKind::Inside,
    );
}

/// Text rotated a quarter turn clockwise, centred in `strip`, read top to bottom
/// (like the original's vertical titles and tabs).
pub fn vertical_text(painter: &egui::Painter, strip: Rect, text: &str, size: f32, color: Color32) {
    let galley = painter.layout_no_wrap(text.to_string(), FontId::proportional(size), color);
    let w = galley.size().x.min(strip.height() - 6.0);
    let h = galley.size().y;
    let pos = Pos2::new(strip.center().x + h / 2.0, strip.center().y - w / 2.0);
    let clip = painter.with_clip_rect(strip);
    clip.add(egui::epaint::TextShape::new(pos, galley, color).with_angle(std::f32::consts::FRAC_PI_2));
}

pub fn paint_title(painter: &egui::Painter, fence: &Fence, title: Rect, scale: f32, hovered: bool) {
    let color = if hovered { Color32::WHITE } else { Color32::from_gray(215) };
    let size = 13.0 * scale;
    if fence.expanded && fence.title_pos == TitlePos::Left {
        vertical_text(painter, title, &fence.name, size, color);
    } else {
        let galley = painter.layout_no_wrap(fence.name.clone(), FontId::proportional(size), color);
        let pos = Pos2::new(title.center().x - galley.size().x / 2.0, title.center().y - galley.size().y / 2.0);
        painter.with_clip_rect(title).galley(pos, galley, color);
    }
    if fence.locked {
        let p = Pos2::new(title.max.x - 9.0 * scale, title.min.y + 9.0 * scale);
        painter.circle_stroke(p, 3.0 * scale, Stroke::new(1.0_f32, Color32::from_gray(160)));
    }
}

/// Paints the tab strip and returns each tab's rectangle.
pub fn paint_tabs(painter: &egui::Painter, fence: &Fence, strip: Rect, names: &[String], sel: usize, scale: f32) -> Vec<Rect> {
    let n = names.len().max(1) as f32;
    let mut rects = Vec::new();
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
        let color = if active { Color32::WHITE } else { Color32::from_gray(170) };
        match fence.tab_pos {
            TabPos::Right => {
                vertical_text(painter, r, name, 12.0 * scale, color);
                if active {
                    painter.line_segment(
                        [Pos2::new(r.min.x + 1.0, r.min.y + 4.0), Pos2::new(r.min.x + 1.0, r.max.y - 4.0)],
                        Stroke::new(1.5 * scale, Color32::from_gray(230)),
                    );
                }
            }
            TabPos::Top => {
                let g = painter.layout_no_wrap(name.clone(), FontId::proportional(12.0 * scale), color);
                let pos = Pos2::new(r.center().x - g.size().x / 2.0, r.center().y - g.size().y / 2.0);
                painter.with_clip_rect(r).galley(pos, g, color);
                if active {
                    painter.line_segment(
                        [Pos2::new(r.min.x + 6.0, r.max.y - 1.0), Pos2::new(r.max.x - 6.0, r.max.y - 1.0)],
                        Stroke::new(1.5 * scale, Color32::from_gray(230)),
                    );
                }
            }
        }
        rects.push(r);
    }
    rects
}

pub fn caption_galley(painter: &egui::Painter, text: &str, width: f32, size: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::simple(text.to_string(), FontId::proportional(size), Color32::WHITE, width);
    job.wrap.max_rows = 2;
    job.wrap.break_anywhere = true;
    job.halign = Align::Center;
    painter.layout_job(job)
}

/// Paints one icon cell (icon + caption). Returns nothing; interaction is done by the caller.
pub fn paint_item(painter: &egui::Painter, icons: &mut IconCache, item: &DeskItem, cell: Rect, icon: f32, scale: f32, highlight: u8) {
    if highlight > 0 {
        painter.rect_filled(cell.shrink(2.0 * scale), CornerRadius::same((4.0 * scale) as u8), Color32::from_white_alpha(highlight));
    }
    let icon_rect = Rect::from_center_size(Pos2::new(cell.center().x, cell.min.y + 6.0 * scale + icon / 2.0), Vec2::splat(icon));
    // In real pixels at the box's own size (the layout preview reuses the boxes' icons).
    let px = (icon / scale.max(0.01) * painter.ctx().pixels_per_point()).round() as u32;
    match icons.get(&item.path, px) {
        Some(tex) => {
            painter.image(tex, icon_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        None => {
            // Placeholder while the real icon loads.
            let c = if item.is_dir { Color32::from_rgb(232, 190, 80) } else { Color32::from_gray(120) };
            painter.rect_filled(icon_rect.shrink(icon * 0.12), CornerRadius::same((3.0 * scale) as u8), c);
        }
    }
    let size = 12.0 * scale;
    let galley = caption_galley(painter, &item.name, cell.width() - 4.0 * scale, size);
    let pos = Pos2::new(cell.center().x, icon_rect.max.y + 4.0 * scale);
    let clip = painter.with_clip_rect(cell);
    clip.galley_with_override_text_color(pos + Vec2::splat(1.0 * scale.max(0.6)), galley.clone(), Color32::from_black_alpha(220));
    clip.galley(pos, galley, Color32::WHITE);
}

/// Non-interactive drawing of a whole box at `rect` (used by the layout preview).
pub fn paint_static(painter: &egui::Painter, icons: &mut IconCache, fence: &Fence, desk: &DeskState, rect: Rect, scale: f32) {
    paint_background(painter, fence, rect, scale);
    if let Some(wc) = &fence.widget {
        let a = areas(fence, rect, 1, scale);
        paint_title(painter, fence, a.title, scale, false);
        let label = crate::widgets::find(&wc.kind).map(|k| k.label).unwrap_or("Widget");
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
    let cols = ((a.content.width() / cell.x).floor() as usize).max(1);
    let clip = painter.with_clip_rect(a.content);
    if let Some((_, items)) = tabs.get(sel) {
        for (i, it) in items.iter().enumerate() {
            let (c, r) = (i % cols, i / cols);
            let min = a.content.min + Vec2::new(c as f32 * cell.x, r as f32 * cell.y);
            if min.y > a.content.max.y {
                break;
            }
            paint_item(&clip, icons, it, Rect::from_min_size(min, cell), fence.icon_size as f32 * scale, scale, 0);
        }
    }
}

