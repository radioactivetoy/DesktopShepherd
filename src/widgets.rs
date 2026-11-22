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
    WidgetKind { id: "sysmon", label: "System monitor", icon: '\u{E9D9}', default_size: [310.0, 270.0], make: || Box::new(SysMon) },
    WidgetKind { id: "calendar", label: "Calendar", icon: '\u{E787}', default_size: [290.0, 270.0], make: || Box::new(Calendar) },
    WidgetKind { id: "todo", label: "To-do list", icon: '\u{E73A}', default_size: [290.0, 280.0], make: || Box::new(Todo) },
    // Drawn by the box itself (it needs the desktop's icons): see `search_widget` in app.rs.
    WidgetKind { id: "search", label: "Search box", icon: '\u{E721}', default_size: [380.0, 280.0], make: || Box::new(Nothing) },
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

// ---- (search box placeholder) -------------------------------------------------------------

struct Nothing;

impl DeskWidget for Nothing {
    fn ui(&self, _ui: &mut Ui, _data: &mut Value) -> WidgetOutput {
        WidgetOutput::default()
    }
}

// ---- System monitor -----------------------------------------------------------------------

/// The last readings, shared by every monitor widget. Read at most every 2 s, and only while a
/// monitor is on screen (a hidden widget isn't drawn, so it reads nothing).
struct Readings {
    at: std::time::Instant,
    idle: u64,
    total: u64,
    cpu: f32,
    history: std::collections::VecDeque<f32>,
    mem_load: f32,
    mem_used_gb: f32,
    mem_total_gb: f32,
    disks_at: Option<std::time::Instant>,
    disks: Vec<(String, f32, f32)>, // drive, used fraction, free GB
}

static READINGS: std::sync::Mutex<Option<Readings>> = std::sync::Mutex::new(None);

const SAMPLE_EVERY: Duration = Duration::from_secs(2);

#[cfg(windows)]
fn cpu_times() -> Option<(u64, u64)> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetSystemTimes;
    let (mut i, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetSystemTimes(Some(&mut i), Some(&mut k), Some(&mut u)).ok()? };
    let f = |t: FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
    // Kernel time includes idle time.
    Some((f(i), f(k) + f(u)))
}
#[cfg(not(windows))]
fn cpu_times() -> Option<(u64, u64)> {
    None
}

#[cfg(windows)]
fn memory() -> Option<(f32, f32, f32)> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { GlobalMemoryStatusEx(&mut m).ok()? };
    let gb = 1024.0 * 1024.0 * 1024.0;
    Some((m.dwMemoryLoad as f32 / 100.0, (m.ullTotalPhys - m.ullAvailPhys) as f32 / gb, m.ullTotalPhys as f32 / gb))
}
#[cfg(not(windows))]
fn memory() -> Option<(f32, f32, f32)> {
    None
}

/// Every fixed (local) drive: name, used fraction, free GB.
#[cfg(windows)]
fn disks() -> Vec<(String, f32, f32)> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives};
    let mut out = Vec::new();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root: Vec<u16> = format!("{letter}:\\").encode_utf16().chain(Some(0)).collect();
        // 3 = DRIVE_FIXED: no network, CD or card readers (they can be slow to answer).
        if unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } != 3 {
            continue;
        }
        let (mut free, mut total) = (0u64, 0u64);
        if unsafe { GetDiskFreeSpaceExW(PCWSTR(root.as_ptr()), Some(&mut free), Some(&mut total), None) }.is_ok() && total > 0 {
            out.push((format!("{letter}:"), 1.0 - free as f32 / total as f32, free as f32 / (1024.0 * 1024.0 * 1024.0)));
        }
    }
    out
}
#[cfg(not(windows))]
fn disks() -> Vec<(String, f32, f32)> {
    Vec::new()
}

type Snapshot = (f32, Vec<f32>, f32, f32, f32, Vec<(String, f32, f32)>);

/// Takes new readings if the last ones are 2 s old (disks every 60 s), and returns a copy.
fn read_now() -> Snapshot {
    let Ok(mut g) = READINGS.lock() else { return (0.0, Vec::new(), 0.0, 0.0, 0.0, Vec::new()) };
    let now = std::time::Instant::now();
    let r = g.get_or_insert_with(|| {
        let (idle, total) = cpu_times().unwrap_or((0, 0));
        Readings {
            at: now.checked_sub(SAMPLE_EVERY).unwrap_or(now),
            idle,
            total,
            cpu: 0.0,
            history: Default::default(),
            mem_load: 0.0,
            mem_used_gb: 0.0,
            mem_total_gb: 0.0,
            disks_at: None,
            disks: Vec::new(),
        }
    });
    if now.duration_since(r.at) >= SAMPLE_EVERY - Duration::from_millis(150) {
        if let Some((idle, total)) = cpu_times() {
            let (di, dt) = (idle.saturating_sub(r.idle), total.saturating_sub(r.total));
            if dt > 0 {
                r.cpu = (1.0 - di as f32 / dt as f32).clamp(0.0, 1.0);
                if r.history.len() >= 60 {
                    r.history.pop_front();
                }
                r.history.push_back(r.cpu);
            }
            r.idle = idle;
            r.total = total;
        }
        if let Some((load, used, total)) = memory() {
            r.mem_load = load;
            r.mem_used_gb = used;
            r.mem_total_gb = total;
        }
        r.at = now;
    }
    if r.disks_at.map(|t| now.duration_since(t) >= Duration::from_secs(60)).unwrap_or(true) {
        r.disks = disks();
        r.disks_at = Some(now);
    }
    (r.cpu, r.history.iter().copied().collect(), r.mem_load, r.mem_used_gb, r.mem_total_gb, r.disks.clone())
}

fn level_color(f: f32) -> Color32 {
    if f >= 0.9 {
        Color32::from_rgb(255, 110, 100)
    } else if f >= 0.7 {
        Color32::from_rgb(255, 200, 90)
    } else {
        Color32::from_rgb(110, 190, 255)
    }
}

/// One line: label, a bar, and the text on the right.
fn meter(ui: &mut Ui, label: &str, frac: f32, text: &str) {
    ui.horizontal(|ui| {
        ui.add_sized([40.0, 18.0], egui::Label::new(egui::RichText::new(label).size(13.0).color(Color32::from_gray(210))));
        let w = (ui.available_width() - 100.0).max(30.0);
        let (r, _) = ui.allocate_exact_size(egui::vec2(w, 8.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(r, 4.0, Color32::from_white_alpha(28));
        let mut fill = r;
        fill.set_width(r.width() * frac.clamp(0.0, 1.0));
        p.rect_filled(fill, 4.0, level_color(frac));
        ui.label(egui::RichText::new(text).size(12.5).color(Color32::from_gray(225)));
    });
}

struct SysMon;

impl DeskWidget for SysMon {
    fn ui(&self, ui: &mut Ui, _data: &mut Value) -> WidgetOutput {
        let (cpu, hist, load, used, total, disks) = read_now();
        ui.add_space(2.0);
        meter(ui, "CPU", cpu, &format!("{:.0} %", cpu * 100.0));
        // The last two minutes of CPU use.
        let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(r, 3.0, Color32::from_black_alpha(45));
        if hist.len() > 1 {
            let step = r.width() / 59.0;
            let x0 = r.max.x - step * (hist.len() - 1) as f32;
            let pts: Vec<Pos2> = hist.iter().enumerate().map(|(i, v)| Pos2::new(x0 + i as f32 * step, r.max.y - 2.0 - v * (r.height() - 4.0))).collect();
            p.add(egui::Shape::line(pts, Stroke::new(1.5_f32, Color32::from_rgb(110, 190, 255))));
        }
        ui.add_space(4.0);
        meter(ui, "RAM", load, &format!("{used:.1}/{total:.0} GB"));
        // Many drives: the list scrolls.
        egui::ScrollArea::vertical().id_salt("disks").auto_shrink([false, true]).show(ui, |ui| {
            for (name, frac, free) in &disks {
                meter(ui, name, *frac, &format!("{free:.0} GB free"));
            }
        });
        WidgetOutput { changed: false, repaint_after: Some(SAMPLE_EVERY) }
    }
}

// ---- Calendar -----------------------------------------------------------------------------

fn arrow_button(ui: &mut Ui, glyph: &str) -> bool {
    ui.add(egui::Button::new(egui::RichText::new(glyph).size(20.0).color(Color32::WHITE)).frame(false)).clicked()
}

struct Calendar;

impl DeskWidget for Calendar {
    fn ui(&self, ui: &mut Ui, data: &mut Value) -> WidgetOutput {
        use chrono::{Datelike, NaiveDate};
        let mut out = WidgetOutput::default();
        let today = chrono::Local::now().date_naive();
        let sunday_first = data_bool(data, "sunday", false);
        // Months away from this one (arrows), not saved.
        let mid = ui.make_persistent_id("cal-month");
        let mut shift: i32 = ui.ctx().data(|d| d.get_temp(mid)).unwrap_or(0);
        let months = today.year() * 12 + today.month0() as i32 + shift;
        let (y, m) = (months.div_euclid(12), months.rem_euclid(12) as u32 + 1);
        let Some(first) = NaiveDate::from_ymd_opt(y, m, 1) else { return out };
        let resp = ui
            .vertical(|ui| {
                ui.horizontal(|ui| {
                    if arrow_button(ui, "‹") {
                        shift -= 1;
                    }
                    let title = egui::RichText::new(first.format("%B %Y").to_string()).size(15.0).color(Color32::WHITE);
                    let w = (ui.available_width() - 24.0).max(40.0);
                    if ui.add_sized([w, 20.0], egui::Button::new(title).frame(false)).on_hover_text("Back to today").clicked() {
                        shift = 0;
                    }
                    if arrow_button(ui, "›") {
                        shift += 1;
                    }
                });
                let names = if sunday_first { ["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"] } else { ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"] };
                let cw = ui.available_width() / 7.0;
                let ch = ((ui.available_height() - 20.0) / 6.0).clamp(14.0, 34.0);
                let (head, _) = ui.allocate_exact_size(egui::vec2(cw * 7.0, 18.0), Sense::hover());
                for (i, n) in names.iter().enumerate() {
                    let c = Pos2::new(head.min.x + cw * (i as f32 + 0.5), head.center().y);
                    ui.painter().text(c, egui::Align2::CENTER_CENTER, *n, egui::FontId::proportional(11.5), Color32::from_gray(215));
                }
                let lead = if sunday_first { first.weekday().num_days_from_sunday() } else { first.weekday().num_days_from_monday() } as i64;
                let (grid, _) = ui.allocate_exact_size(egui::vec2(cw * 7.0, ch * 6.0), Sense::hover());
                for cell in 0..42i64 {
                    let day = first + chrono::Duration::days(cell - lead);
                    let c = Pos2::new(grid.min.x + cw * ((cell % 7) as f32 + 0.5), grid.min.y + ch * ((cell / 7) as f32 + 0.5));
                    let this_month = day.month() == m;
                    if !this_month {
                        continue; // only this month's days
                    }
                    if day == today {
                        ui.painter().circle_filled(c, (ch.min(cw) / 2.0 - 1.0).max(7.0), Color32::from_rgb(70, 140, 230));
                    }
                    let col = if day == today { Color32::WHITE } else { Color32::from_gray(230) };
                    ui.painter().text(c, egui::Align2::CENTER_CENTER, day.day().to_string(), egui::FontId::proportional(13.0), col);
                }
            })
            .response;
        resp.context_menu(|ui| {
            if ui.checkbox(&mut data_bool(data, "sunday", false), "Week starts on Sunday").clicked() {
                data["sunday"] = json!(!sunday_first);
                out.changed = true;
                ui.close_menu();
            }
        });
        ui.ctx().data_mut(|d| d.insert_temp(mid, shift));
        // Next redraw: just after midnight (the day changes), nothing in between.
        let now = chrono::Local::now().naive_local();
        let next = (today + chrono::Duration::days(1)).and_hms_opt(0, 0, 5).unwrap_or(now);
        out.repaint_after = Some((next - now).to_std().unwrap_or(Duration::from_secs(3600)));
        out
    }
}

// ---- To-do list ---------------------------------------------------------------------------

struct Todo;

impl DeskWidget for Todo {
    fn ui(&self, ui: &mut Ui, data: &mut Value) -> WidgetOutput {
        let mut out = WidgetOutput::default();
        if !data.get("items").map(|s| s.is_array()).unwrap_or(false) {
            data["items"] = json!([]);
        }
        // The task being typed (not saved until it is added).
        let nid = ui.make_persistent_id("todo-new");
        let mut new: String = ui.ctx().data(|d| d.get_temp(nid)).unwrap_or_default();
        let r = ui.add(egui::TextEdit::singleline(&mut new).hint_text("Add a task, then Enter").desired_width(f32::INFINITY));
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !new.trim().is_empty() {
            if let Some(a) = data["items"].as_array_mut() {
                a.push(json!({ "t": new.trim(), "d": false }));
            }
            new.clear();
            out.changed = true;
            r.request_focus();
        }
        ui.ctx().data_mut(|d| d.insert_temp(nid, new));
        ui.add_space(4.0);
        let mut remove: Option<usize> = None;
        let mut toggle: Option<usize> = None;
        let any_done = data["items"].as_array().map(|a| a.iter().any(|it| it["d"].as_bool().unwrap_or(false))).unwrap_or(false);
        let list_h = (ui.available_height() - if any_done { 26.0 } else { 0.0 }).max(20.0);
        egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
            if let Some(items) = data["items"].as_array() {
                for (i, it) in items.iter().enumerate() {
                    let done = it["d"].as_bool().unwrap_or(false);
                    let text = it["t"].as_str().unwrap_or("");
                    ui.horizontal(|ui| {
                        let mut d = done;
                        if ui.checkbox(&mut d, "").changed() {
                            toggle = Some(i);
                        }
                        let mut t = egui::RichText::new(text).size(13.5);
                        t = if done { t.strikethrough().color(Color32::from_gray(130)) } else { t.color(Color32::from_gray(235)) };
                        let w = (ui.available_width() - 22.0).max(20.0);
                        ui.allocate_ui_with_layout(egui::vec2(w, 18.0), egui::Layout::left_to_right(egui::Align::Center), |ui| ui.add(egui::Label::new(t).truncate()));
                        if ui.add(egui::Button::new(egui::RichText::new("✕").size(11.0).color(Color32::from_gray(150))).frame(false)).on_hover_text("Delete").clicked() {
                            remove = Some(i);
                        }
                    });
                }
            }
        });
        if let Some(a) = data["items"].as_array_mut() {
            if let Some(i) = toggle {
                if let Some(it) = a.get_mut(i) {
                    let d = it["d"].as_bool().unwrap_or(false);
                    it["d"] = json!(!d);
                    out.changed = true;
                }
            }
            if let Some(i) = remove {
                if i < a.len() {
                    a.remove(i);
                    out.changed = true;
                }
            }
        }
        if any_done && ui.small_button("Remove finished tasks").clicked() {
            if let Some(a) = data["items"].as_array_mut() {
                a.retain(|it| !it["d"].as_bool().unwrap_or(false));
                out.changed = true;
            }
        }
        out
    }
}
