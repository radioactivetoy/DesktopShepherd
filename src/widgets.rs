//! Desktop widgets: a box can show a widget instead of icons.
//!
//! Adding a new widget later = write one struct implementing `DeskWidget` and add one line to
//! `REGISTRY`. Each widget keeps its own saved state in a free-form JSON value, so the layout
//! file never needs a format change, and a widget only repaints when it asks to (`repaint_after`),
//! so idle widgets cost nothing.

use egui::{self, Color32, Pos2, Sense, Stroke, Ui};
use serde_json::{json, Value};
use std::time::Duration;

#[derive(Default)]
pub struct WidgetOutput {
    /// The widget's saved data changed: save the layout.
    pub changed: bool,
    /// Ask to be drawn again after this long (a clock); `None` = only on input.
    pub repaint_after: Option<Duration>,
}

pub trait DeskWidget {
    fn ui(&self, ui: &mut Ui, data: &mut Value) -> WidgetOutput;
}

pub struct WidgetKind {
    pub id: &'static str,
    pub label: &'static str,
    /// A Segoe Fluent Icons glyph shown in menus.
    pub icon: char,
    pub default_size: [f32; 2],
    pub make: fn() -> Box<dyn DeskWidget>,
}

pub const REGISTRY: &[WidgetKind] = &[
    WidgetKind { id: "clock", label: "Clock", icon: '\u{E823}', default_size: [260.0, 130.0], make: || Box::new(Clock) },
    WidgetKind { id: "note", label: "Sticky note", icon: '\u{E70B}', default_size: [280.0, 220.0], make: || Box::new(Note) },
    WidgetKind { id: "sketch", label: "Drawing pad", icon: '\u{E70F}', default_size: [360.0, 280.0], make: || Box::new(Sketch) },
];

pub fn find(id: &str) -> Option<&'static WidgetKind> {
    REGISTRY.iter().find(|k| k.id == id)
}

// ---- Clock --------------------------------------------------------------------------------

struct Clock;

impl DeskWidget for Clock {
    fn ui(&self, ui: &mut Ui, data: &mut Value) -> WidgetOutput {
        let now = chrono::Local::now();
        let h24 = data.get("h24").and_then(|v| v.as_bool()).unwrap_or(true);
        let time = if h24 { now.format("%H:%M").to_string() } else { now.format("%I:%M %p").to_string() };
        let mut changed = false;
        let resp = ui
            .vertical_centered(|ui| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(time).size(44.0).color(Color32::WHITE));
                ui.label(egui::RichText::new(now.format("%A, %d %B").to_string()).size(14.0).color(Color32::from_gray(200)));
            })
            .response;
        resp.context_menu(|ui| {
            if ui.checkbox(&mut data_bool(data, "h24", true), "24-hour").clicked() {
                data["h24"] = json!(!h24);
                changed = true;
                ui.close_menu();
            }
        });
        // Wake up exactly when the minute changes; nothing in between.
        let secs_left = 60 - (now.timestamp() % 60) as u64;
        WidgetOutput { changed, repaint_after: Some(Duration::from_millis(secs_left * 1000 + 50)) }
    }
}

fn data_bool(data: &Value, key: &str, default: bool) -> bool {
    data.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

// ---- Sticky note --------------------------------------------------------------------------

struct Note;

impl DeskWidget for Note {
    fn ui(&self, ui: &mut Ui, data: &mut Value) -> WidgetOutput {
        let mut text = data.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let r = ui.add_sized(
            ui.available_size(),
            egui::TextEdit::multiline(&mut text)
                .hint_text("Type a note…")
                .frame(false)
                .text_color(Color32::from_gray(235)),
        );
        let mut out = WidgetOutput::default();
        if r.changed() {
            data["text"] = json!(text);
            out.changed = true;
        }
        out
    }
}

// ---- Drawing pad --------------------------------------------------------------------------

struct Sketch;

impl DeskWidget for Sketch {
    fn ui(&self, ui: &mut Ui, data: &mut Value) -> WidgetOutput {
        let mut out = WidgetOutput::default();
        if !data.get("strokes").map(|s| s.is_array()).unwrap_or(false) {
            data["strokes"] = json!([]);
        }
        let colors = [
            Color32::WHITE,
            Color32::from_rgb(255, 99, 99),
            Color32::from_rgb(255, 210, 80),
            Color32::from_rgb(110, 220, 120),
            Color32::from_rgb(90, 170, 255),
        ];
        let sel = data.get("pen").and_then(|v| v.as_u64()).unwrap_or(0) as usize % colors.len();
        ui.horizontal(|ui| {
            for (i, c) in colors.iter().enumerate() {
                let (rect, r) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), Sense::click());
                ui.painter().circle_filled(rect.center(), if i == sel { 7.5 } else { 5.5 }, *c);
                if r.clicked() {
                    data["pen"] = json!(i);
                    out.changed = true;
                }
            }
            if ui.small_button("Clear").clicked() {
                data["strokes"] = json!([]);
                out.changed = true;
            }
        });
        let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::drag());
        let painter = ui.painter_at(rect);
        let to_screen = |p: &Value| -> Option<Pos2> {
            let a = p.as_array()?;
            Some(Pos2::new(
                rect.min.x + a.first()?.as_f64()? as f32 * rect.width(),
                rect.min.y + a.get(1)?.as_f64()? as f32 * rect.height(),
            ))
        };
        if resp.drag_started() {
            if let Some(arr) = data["strokes"].as_array_mut() {
                arr.push(json!({ "c": sel, "p": [] }));
            }
        }
        if resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let nx = ((pos.x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
                let ny = ((pos.y - rect.min.y) / rect.height()).clamp(0.0, 1.0);
                if let Some(last) = data["strokes"].as_array_mut().and_then(|a| a.last_mut()) {
                    if let Some(pts) = last["p"].as_array_mut() {
                        pts.push(json!([(nx * 1000.0).round() / 1000.0, (ny * 1000.0).round() / 1000.0]));
                    }
                }
            }
        }
        if resp.drag_stopped() {
            out.changed = true;
        }
        if let Some(strokes) = data["strokes"].as_array() {
            for s in strokes {
                let c = colors[s["c"].as_u64().unwrap_or(0) as usize % colors.len()];
                let pts: Vec<Pos2> = s["p"].as_array().map(|a| a.iter().filter_map(to_screen).collect()).unwrap_or_default();
                if pts.len() == 1 {
                    painter.circle_filled(pts[0], 1.5, c);
                } else if pts.len() > 1 {
                    painter.add(egui::Shape::line(pts, Stroke::new(2.5_f32, c)));
                }
            }
        }
        out
    }
}
