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
    /// Whether the widget has options for the settings window (Personalize > Widget settings).
    fn has_settings(&self) -> bool {
        false
    }
    /// The widget's options, drawn in the settings window. Returns true if `data` changed.
    fn settings(&self, _ui: &mut Ui, _data: &mut Value) -> bool {
        false
    }
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
    WidgetKind { id: "agenda", label: "Agenda (online calendars)", icon: '\u{E8BF}', default_size: [320.0, 300.0], make: || Box::new(Agenda) },
    WidgetKind { id: "picture", label: "Picture frame", icon: '\u{EB9F}', default_size: [320.0, 240.0], make: || Box::new(Picture) },
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

    fn has_settings(&self) -> bool {
        true
    }

    fn settings(&self, ui: &mut Ui, data: &mut Value) -> bool {
        let mut h24 = data_bool(data, "h24", true);
        if ui.checkbox(&mut h24, "24-hour clock").changed() {
            data["h24"] = json!(h24);
            return true;
        }
        false
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
                // At most 300 strokes: the oldest go first.
                if arr.len() > 300 {
                    let extra = arr.len() - 300;
                    arr.drain(..extra);
                }
            }
        }
        if resp.dragged() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let nx = ((pos.x - rect.min.x) / rect.width()).clamp(0.0, 1.0);
                let ny = ((pos.y - rect.min.y) / rect.height()).clamp(0.0, 1.0);
                if let Some(last) = data["strokes"].as_array_mut().and_then(|a| a.last_mut()) {
                    if let Some(pts) = last["p"].as_array_mut() {
                        // Keep the saved drawing small: skip points closer than ~2 px to the last
                        // one, and at most 2000 points per stroke.
                        let (px, py) = ((nx * 1000.0).round() / 1000.0, (ny * 1000.0).round() / 1000.0);
                        let near = pts.last().and_then(|l| l.as_array()).map(|l| {
                            let (lx, ly) = (l.first().and_then(|v| v.as_f64()).unwrap_or(-1.0) as f32, l.get(1).and_then(|v| v.as_f64()).unwrap_or(-1.0) as f32);
                            ((px - lx) * rect.width()).abs() < 2.0 && ((py - ly) * rect.height()).abs() < 2.0
                        });
                        if !near.unwrap_or(false) && pts.len() < 2000 {
                            pts.push(json!([px, py]));
                        }
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
        // A sleeping or stuck drive can take seconds to answer: ask on a helper thread, never
        // here on the thread that draws every box. The answer shows on the next update.
        r.disks_at = Some(now);
        let _ = std::thread::Builder::new().name("drive-space".into()).spawn(|| {
            let found = disks();
            if let Ok(mut g) = READINGS.lock() {
                if let Some(r) = g.as_mut() {
                    r.disks = found;
                }
            }
        });
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

    fn has_settings(&self) -> bool {
        true
    }

    fn settings(&self, ui: &mut Ui, data: &mut Value) -> bool {
        let mut sunday = data_bool(data, "sunday", false);
        if ui.checkbox(&mut sunday, "Week starts on Sunday").changed() {
            data["sunday"] = json!(sunday);
            return true;
        }
        false
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

// ---- Agenda (online calendars) --------------------------------------------------------------

/// A colour per calendar link (dots in the list).
const CAL_COLORS: [Color32; 6] = [
    Color32::from_rgb(90, 170, 255),
    Color32::from_rgb(110, 220, 120),
    Color32::from_rgb(255, 170, 80),
    Color32::from_rgb(240, 110, 170),
    Color32::from_rgb(180, 140, 255),
    Color32::from_rgb(255, 220, 90),
];

struct Agenda;

fn agenda_urls(data: &Value) -> Vec<String> {
    data.get("urls").and_then(|v| v.as_array()).map(|a| a.iter().filter_map(|u| u.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

impl DeskWidget for Agenda {
    fn ui(&self, ui: &mut Ui, data: &mut Value) -> WidgetOutput {
        use chrono::{Duration as CDuration, NaiveTime};
        let mut out = WidgetOutput { changed: false, repaint_after: Some(Duration::from_secs(60)) };
        let urls = agenda_urls(data);
        let days = data.get("days").and_then(|v| v.as_u64()).unwrap_or(7).clamp(1, 14) as i64;
        let force = crate::agenda::take_refresh();
        let cals = crate::agenda::view(&urls, force);
        let today = chrono::Local::now().date_naive();
        let now = chrono::Local::now().naive_local();
        let end = (today + CDuration::days(days)).and_time(NaiveTime::MIN);

        if force || cals.iter().any(|c| c.loading) {
            // Look again soon for the download's result (it also wakes the app when done).
            out.repaint_after = Some(Duration::from_secs(2));
        }
        let resp = ui
            .vertical(|ui| {
                if urls.is_empty() {
                    ui.add_space(6.0);
                    ui.label(egui::RichText::new("Right-click here and choose \"Widget settings…\" to add a calendar link (iCal / .ics).").size(13.0).color(Color32::from_gray(220)));
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("Google Calendar: Settings > your calendar > \"Secret address in iCal format\". Outlook: Settings > Calendar > Shared calendars > Publish.")
                            .size(11.5)
                            .color(Color32::from_gray(170)),
                    );
                    return;
                }
                // Every event of the next days, each day under its own heading.
                let mut rows: Vec<(chrono::NaiveDate, usize, &crate::agenda::Occurrence)> = Vec::new();
                for (ci, c) in cals.iter().enumerate() {
                    for e in c.events.iter() {
                        if e.end <= now && !(e.all_day && e.end.date() > today) {
                            if !(e.end == e.start && e.start.date() == today && e.start >= now) {
                                continue; // already over
                            }
                        }
                        if e.start >= end {
                            continue;
                        }
                        if e.all_day {
                            // A trip over several days is listed on each day it covers.
                            let mut day = e.start.date().max(today);
                            while day < e.end.date() && day.and_time(NaiveTime::MIN) < end {
                                rows.push((day, ci, e));
                                day += CDuration::days(1);
                            }
                        } else {
                            rows.push((e.start.date().max(today), ci, e));
                        }
                    }
                }
                rows.sort_by(|a, b| a.0.cmp(&b.0).then(b.2.all_day.cmp(&a.2.all_day)).then(a.2.start.cmp(&b.2.start)));
                let status_h = if cals.iter().any(|c| c.error.is_some() || (c.loading && c.events.is_empty())) { 20.0 } else { 0.0 };
                egui::ScrollArea::vertical().max_height((ui.available_height() - status_h).max(30.0)).auto_shrink([false, true]).show(ui, |ui| {
                    if rows.is_empty() && cals.iter().all(|c| !c.loading) {
                        ui.label(egui::RichText::new(format!("Nothing in the next {days} days.")).size(13.0).color(Color32::from_gray(190)));
                    }
                    let mut last_day = None;
                    for (day, ci, e) in &rows {
                        if last_day != Some(*day) {
                            last_day = Some(*day);
                            let label = match (*day - today).num_days() {
                                0 => "Today".to_string(),
                                1 => "Tomorrow".to_string(),
                                _ => day.format("%A %e %B").to_string(),
                            };
                            ui.add_space(3.0);
                            ui.label(egui::RichText::new(label).size(12.0).strong().color(Color32::from_gray(200)));
                        }
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 16.0), Sense::hover());
                            ui.painter().circle_filled(r.center(), 3.5, CAL_COLORS[ci % CAL_COLORS.len()]);
                            let when = if e.all_day { "all day".to_string() } else { e.start.format("%H:%M").to_string() };
                            ui.add_sized([46.0, 16.0], egui::Label::new(egui::RichText::new(when).size(12.5).color(Color32::from_gray(185))));
                            let happening = !e.all_day && e.start <= now && now < e.end;
                            let col = if happening { Color32::WHITE } else { Color32::from_gray(230) };
                            let mut t = egui::RichText::new(&e.title).size(13.0).color(col);
                            if happening {
                                t = t.strong();
                            }
                            ui.add(egui::Label::new(t).truncate());
                        });
                    }
                });
                let failed = cals.iter().filter(|c| c.error.is_some()).count();
                if failed > 0 {
                    let first = cals.iter().find_map(|c| c.error.clone()).unwrap_or_default();
                    ui.label(egui::RichText::new(format!("⚠ {failed} calendar(s) not updated: {first}")).size(11.0).color(Color32::from_rgb(255, 190, 130)))
                        .on_hover_text(first);
                } else if cals.iter().any(|c| c.loading && c.events.is_empty()) {
                    ui.label(egui::RichText::new("Loading…").size(11.5).color(Color32::from_gray(170)));
                }
            })
            .response;

        let _ = resp;
        out
    }

    fn has_settings(&self) -> bool {
        true
    }

    fn settings(&self, ui: &mut Ui, data: &mut Value) -> bool {
        let urls = agenda_urls(data);
        let days = data.get("days").and_then(|v| v.as_u64()).unwrap_or(7).clamp(1, 14) as i64;
        let cals = crate::agenda::view(&urls, false);
        agenda_edit(ui, data, &urls, &cals, days)
    }
}

/// The agenda's settings (in the settings window): calendar links and options. Returns true if
/// the saved data changed.
fn agenda_edit(ui: &mut Ui, data: &mut Value, urls: &[String], cals: &[crate::agenda::CalView], days: i64) -> bool {
    let mut out = WidgetOutput::default();
    egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
            ui.label(egui::RichText::new("Calendar links (iCal / .ics)").strong());
            let mut remove = None;
            for (i, u) in urls.iter().enumerate() {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 16.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.5, CAL_COLORS[i % CAL_COLORS.len()]);
                    let name = cals.get(i).and_then(|c| c.name.clone()).unwrap_or_else(|| {
                        // The server name is enough to tell links apart (the rest is a secret).
                        u.split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or(u).to_string()
                    });
                    ui.add(egui::Label::new(name).truncate()).on_hover_text("The full link stays on this PC only: in your layout file and its backups.");
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            }
            if let Some(i) = remove {
                if let Some(a) = data.get_mut("urls").and_then(|v| v.as_array_mut()) {
                    if i < a.len() {
                        a.remove(i);
                        out.changed = true;
                    }
                }
            }
            let nid = ui.make_persistent_id("agenda-new");
            let mut new: String = ui.ctx().data(|d| d.get_temp(nid)).unwrap_or_default();
            let mut add = false;
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut new).hint_text("Paste a link: https://… or webcal://…").desired_width(420.0));
                add |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                add |= ui.button("Add").clicked();
            });
            let link = new.trim().to_string();
            if add && !link.is_empty() {
                let ok = ["https://", "http://", "webcal://", "webcals://"].iter().any(|p| link.to_ascii_lowercase().starts_with(p));
                if ok {
                    if !data.get("urls").map(|v| v.is_array()).unwrap_or(false) {
                        data["urls"] = json!([]);
                    }
                    if let Some(a) = data["urls"].as_array_mut() {
                        if !a.iter().any(|x| x.as_str() == Some(link.as_str())) {
                            a.push(json!(link));
                            out.changed = true;
                        }
                    }
                    new.clear();
                }
            }
            ui.ctx().data_mut(|d| d.insert_temp(nid, new));
            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Show the next");
                for n in [3u64, 7, 14] {
                    if ui.selectable_label(days as u64 == n, format!("{n} days")).clicked() {
                        data["days"] = json!(n);
                        out.changed = true;
                    }
                }
            });
            if ui.button("Refresh now").clicked() {
                crate::agenda::request_refresh();
            }
            ui.label(egui::RichText::new("Calendars are downloaded every 30 minutes while this widget is shown. This is the only part of DesktopShepherd that uses the internet.").size(11.0).color(Color32::from_gray(150)));
    });
    out.changed
}

// ---- Picture frame --------------------------------------------------------------------------

/// Pictures being read or ready, by file: decoded on a helper thread (a big photo would freeze
/// the boxes), made at most 1600 px wide or tall, then turned into a texture once and dropped.
static PICTURES: std::sync::Mutex<Option<std::collections::HashMap<String, Result<Option<egui::ColorImage>, String>>>> = std::sync::Mutex::new(None);
/// A picture chosen in Windows' file picker (opened from the settings window), not yet used.
static PICKED_PICTURE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);
static PICKING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn start_picture_load(path: String) {
    let p = path.clone();
    let spawned = std::thread::Builder::new().name("picture".into()).spawn(move || {
        let result = std::panic::catch_unwind(|| -> Result<egui::ColorImage, String> {
            let img = image::open(&p).map_err(|e| format!("can't open the picture ({e})"))?;
            let img = if img.width().max(img.height()) > 1600 { img.thumbnail(1600, 1600) } else { img };
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            Ok(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
        })
        .unwrap_or_else(|_| Err("can't read that picture".to_string()));
        if let Ok(mut g) = PICTURES.lock() {
            // Only if still wanted (the picture wasn't changed meanwhile): else it is dropped here.
            let map = g.get_or_insert_with(Default::default);
            if matches!(map.get(&p), Some(Ok(None))) {
                map.insert(p, result.map(Some));
            }
        }
        #[cfg(windows)]
        crate::runloop::wake_after(Duration::ZERO);
    });
    if spawned.is_err() {
        if let Ok(mut g) = PICTURES.lock() {
            g.get_or_insert_with(Default::default).remove(&path);
        }
    }
}

struct Picture;

impl DeskWidget for Picture {
    fn ui(&self, ui: &mut Ui, data: &mut Value) -> WidgetOutput {
        let mut out = WidgetOutput::default();
        let path = data.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let fill = data_bool(data, "fill", false);
        let rect = ui.max_rect();
        if path.is_empty() {
            ui.add_space(6.0);
            ui.label(egui::RichText::new("Right-click here and choose \"Widget settings…\" to pick a picture.").size(13.0).color(Color32::from_gray(220)));
            return out;
        }
        // The texture, once made, lives in this box's memory; the decoded copy is dropped.
        // One texture per picture frame: replaced (the old one freed) when the picture changes.
        let tid = ui.make_persistent_id("picture-tex");
        let held: Option<(String, egui::TextureHandle)> = ui.ctx().data(|d| d.get_temp(tid));
        let tex = match held.filter(|(p, _)| *p == path).map(|(_, t)| t) {
            Some(t) => Some(t),
            None => {
                let mut error = None;
                let mut made = None;
                if let Ok(mut g) = PICTURES.lock() {
                    let map = g.get_or_insert_with(Default::default);
                    match map.get_mut(&path) {
                        None => {
                            map.insert(path.clone(), Ok(None));
                            start_picture_load(path.clone());
                        }
                        Some(Ok(slot)) => {
                            if let Some(img) = slot.take() {
                                let t = ui.ctx().load_texture(format!("picture:{path}"), img, egui::TextureOptions::LINEAR);
                                ui.ctx().data_mut(|d| d.insert_temp(tid, (path.clone(), t.clone())));
                                map.remove(&path);
                                made = Some(t);
                            }
                        }
                        Some(Err(e)) => error = Some(e.clone()),
                    }
                }
                if let Some(e) = error {
                    ui.label(egui::RichText::new(format!("⚠ {e}")).size(12.5).color(Color32::from_rgb(255, 190, 130)));
                    return out;
                }
                if made.is_none() {
                    out.repaint_after = Some(Duration::from_millis(300)); // still being read
                }
                made
            }
        };
        let Some(tex) = tex else { return out };
        let [tw, th] = tex.size().map(|v| v.max(1) as f32);
        let (rw, rh) = (rect.width().max(1.0), rect.height().max(1.0));
        let full = egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        let (draw, uv) = if fill {
            // Fill the box, cropping the picture's edges.
            let scale = (rw / tw).max(rh / th);
            let (vw, vh) = (rw / (tw * scale), rh / (th * scale));
            (rect, egui::Rect::from_center_size(Pos2::new(0.5, 0.5), egui::vec2(vw, vh)))
        } else {
            // The whole picture, as big as fits.
            let scale = (rw / tw).min(rh / th);
            (egui::Rect::from_center_size(rect.center(), egui::vec2(tw * scale, th * scale)), full)
        };
        ui.painter().with_clip_rect(rect).image(tex.id(), draw, uv, Color32::WHITE);
        let _ = full;
        out
    }

    fn has_settings(&self) -> bool {
        true
    }

    fn settings(&self, ui: &mut Ui, data: &mut Value) -> bool {
        let mut changed = false;
        if let Some(p) = PICKED_PICTURE.lock().ok().and_then(|mut p| p.take()) {
            data["path"] = json!(p);
            changed = true;
        }
        let mut path = data.get("path").and_then(|v| v.as_str()).unwrap_or("").to_string();
        ui.label(egui::RichText::new("Picture (PNG or JPEG)").strong());
        ui.horizontal(|ui| {
            if ui.add(egui::TextEdit::singleline(&mut path).hint_text("C:\\Users\\…\\Pictures\\photo.jpg").desired_width(420.0)).changed() {
                data["path"] = json!(path.trim());
                changed = true;
            }
            let picking = PICKING.load(std::sync::atomic::Ordering::SeqCst);
            if ui.add_enabled(!picking, egui::Button::new("Choose…")).clicked() {
                PICKING.store(true, std::sync::atomic::Ordering::SeqCst);
                // Windows' file picker waits for the person: on its own thread, never the boxes'.
                let _ = std::thread::Builder::new().name("picture-picker".into()).spawn(|| {
                    let picked = crate::desktop::pick_picture();
                    if let (Some(p), Ok(mut slot)) = (picked, PICKED_PICTURE.lock()) {
                        *slot = Some(p);
                    }
                    PICKING.store(false, std::sync::atomic::Ordering::SeqCst);
                    #[cfg(windows)]
                    crate::runloop::wake_after(Duration::ZERO);
                });
            }
        });
        let mut fill = data_bool(data, "fill", false);
        ui.horizontal(|ui| {
            if ui.radio(!fill, "Show the whole picture").clicked() && fill {
                fill = false;
                changed = true;
            }
            if ui.radio(fill, "Fill the box (edges may be cut)").clicked() && !fill {
                fill = true;
                changed = true;
            }
        });
        data["fill"] = json!(fill);
        if changed {
            // Read the (new) file again on the next draw.
            if let Ok(mut g) = PICTURES.lock() {
                g.get_or_insert_with(Default::default).clear();
            }
            ui.ctx().request_repaint();
        }
        changed
    }
}
