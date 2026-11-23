//! Desktop widgets: a box can show a widget instead of icons.
//!
//! Adding a new widget later = write one struct implementing `DeskWidget` and add one line to
//! `REGISTRY`. Each widget keeps its own saved state in a free-form JSON value, so the layout
//! file never needs a format change, and a widget only repaints when it asks to (`repaint_after`),
//! so idle widgets cost nothing.

use crate::i18n::{t, tf};
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
    WidgetKind { id: "laptop", label: "Laptop status", icon: '\u{E83F}', default_size: [300.0, 150.0], make: || Box::new(Laptop) },
    WidgetKind { id: "media", label: "Now playing", icon: '\u{E8D6}', default_size: [360.0, 140.0], make: || Box::new(Media) },
    WidgetKind { id: "weather", label: "Weather", icon: '\u{E706}', default_size: [340.0, 330.0], make: || Box::new(crate::weather::Weather) },
    WidgetKind { id: "timer", label: "Timer", icon: '\u{E916}', default_size: [260.0, 190.0], make: || Box::new(crate::timer::Timer) },
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
                crate::ink::halo_label(ui, time, 44.0, crate::ink::strong());
                ui.label(egui::RichText::new(crate::i18n::long_date(now.date_naive())).size(14.0).color(crate::ink::gray(200)));
            })
            .response;
        resp.context_menu(|ui| {
            if ui.checkbox(&mut data_bool(data, "h24", true), t("24-hour")).clicked() {
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
        if ui.checkbox(&mut h24, t("24-hour clock")).changed() {
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
                .hint_text(t("Type a note…"))
                .frame(false)
                .text_color(crate::ink::gray(235)),
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
            crate::ink::strong(),
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
            if ui.small_button(t("Clear")).clicked() {
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
    crate::ink::adapt(level_color_raw(f))
}

fn level_color_raw(f: f32) -> Color32 {
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
        ui.add_sized([40.0, 18.0], egui::Label::new(egui::RichText::new(label).size(13.0).color(crate::ink::gray(210))));
        let w = (ui.available_width() - 100.0).max(30.0);
        let (r, _) = ui.allocate_exact_size(egui::vec2(w, 8.0), Sense::hover());
        let p = ui.painter();
        p.rect_filled(r, 4.0, crate::ink::white_alpha(28));
        let mut fill = r;
        fill.set_width(r.width() * frac.clamp(0.0, 1.0));
        p.rect_filled(fill, 4.0, level_color(frac));
        ui.label(egui::RichText::new(text).size(12.5).color(crate::ink::gray(225)));
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
                meter(ui, name, *frac, &tf("{} GB free", &[&format!("{free:.0}")]));
            }
        });
        WidgetOutput { changed: false, repaint_after: Some(SAMPLE_EVERY) }
    }
}

// ---- Calendar -----------------------------------------------------------------------------

fn arrow_button(ui: &mut Ui, glyph: &str) -> bool {
    ui.add(egui::Button::new(egui::RichText::new(glyph).size(20.0).color(crate::ink::strong())).frame(false)).clicked()
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
                    let title = egui::RichText::new(crate::i18n::month_year(first)).size(15.0).color(crate::ink::strong());
                    let w = (ui.available_width() - 24.0).max(40.0);
                    if ui.add_sized([w, 20.0], egui::Button::new(title).frame(false)).on_hover_text(t("Back to today")).clicked() {
                        shift = 0;
                    }
                    if arrow_button(ui, "›") {
                        shift += 1;
                    }
                });
                let names: [&str; 7] = if sunday_first {
                    [6, 0, 1, 2, 3, 4, 5].map(crate::i18n::weekday_short)
                } else {
                    [0, 1, 2, 3, 4, 5, 6].map(crate::i18n::weekday_short)
                };
                let cw = ui.available_width() / 7.0;
                let ch = ((ui.available_height() - 20.0) / 6.0).clamp(14.0, 34.0);
                let (head, _) = ui.allocate_exact_size(egui::vec2(cw * 7.0, 18.0), Sense::hover());
                for (i, n) in names.iter().enumerate() {
                    let c = Pos2::new(head.min.x + cw * (i as f32 + 0.5), head.center().y);
                    ui.painter().text(c, egui::Align2::CENTER_CENTER, *n, egui::FontId::proportional(11.5), crate::ink::gray(215));
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
                    let col = if day == today { crate::ink::strong() } else { crate::ink::gray(230) };
                    ui.painter().text(c, egui::Align2::CENTER_CENTER, day.day().to_string(), egui::FontId::proportional(13.0), col);
                }
            })
            .response;
        resp.context_menu(|ui| {
            if ui.checkbox(&mut data_bool(data, "sunday", false), t("Week starts on Sunday")).clicked() {
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
        if ui.checkbox(&mut sunday, t("Week starts on Sunday")).changed() {
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
        // The add field is hidden until the "+" in the footer is clicked (always shown while the
        // list is empty), so it takes no room most of the time.
        let aid = ui.make_persistent_id("todo-adding");
        let empty = data["items"].as_array().map(|a| a.is_empty()).unwrap_or(true);
        let mut adding: bool = ui.ctx().data(|d| d.get_temp(aid)).unwrap_or(false);
        let mut focus_now = false;
        if adding || empty {
            let hint = egui::RichText::new(t("Add a task, then Enter")).color(crate::ink::soft());
            let r = egui::Frame::new()
                .fill(crate::ink::field())
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(6, 1))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(
                        egui::TextEdit::singleline(&mut new)
                            .frame(false)
                            .font(egui::FontId::proportional(13.0))
                            .text_color(crate::ink::text())
                            .hint_text(hint)
                            .desired_width(f32::INFINITY),
                    )
                })
                .inner;
            // Esc counts only while this field has the keyboard (not for the whole window).
            let has_kb = r.has_focus() || r.lost_focus();
            let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), has_kb && i.key_pressed(egui::Key::Escape)));
            // The pointer is on the "+" (its place from the last pass): a click there closes the
            // field itself, so losing the focus must not close it first.
            let over_plus = ui
                .ctx()
                .data(|d| d.get_temp::<egui::Rect>(aid.with("plus")))
                .zip(ui.input(|i| i.pointer.hover_pos()))
                .map(|(r, p)| r.contains(p))
                .unwrap_or(false);
            if ui.ctx().data(|d| d.get_temp::<bool>(aid.with("focus"))).unwrap_or(false) {
                r.request_focus();
                ui.ctx().data_mut(|d| d.remove::<bool>(aid.with("focus")));
            }
            if r.lost_focus() && enter && !new.trim().is_empty() {
                if let Some(a) = data["items"].as_array_mut() {
                    a.push(json!({ "t": new.trim(), "d": false }));
                }
                new.clear();
                out.changed = true;
                // Keep the field open (also when it was only shown because the list was empty).
                adding = true;
                r.request_focus();
            } else if esc || (r.lost_focus() && new.trim().is_empty() && !over_plus) {
                new.clear();
                adding = false;
            }
            ui.add_space(4.0);
        }
        ui.ctx().data_mut(|d| d.insert_temp(nid, new));

        // Display order: open tasks first, then finished ones (stored order is kept).
        let (order, left, done_count): (Vec<usize>, usize, usize) = {
            let items = data["items"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
            let is_done = |i: usize| items[i]["d"].as_bool().unwrap_or(false);
            let mut o: Vec<usize> = (0..items.len()).filter(|&i| !is_done(i)).collect();
            let left = o.len();
            o.extend((0..items.len()).filter(|&i| is_done(i)));
            (o, left, items.len() - left)
        };

        let mut remove: Option<usize> = None;
        let mut toggle: Option<usize> = None;
        let accent = crate::app::accent();
        let lum = 0.2126 * accent.r() as f32 + 0.7152 * accent.g() as f32 + 0.0722 * accent.b() as f32;
        let check_col = if lum > 150.0 { Color32::from_gray(20) } else { Color32::WHITE };
        let list_h = (ui.available_height() - 30.0).max(20.0);
        egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let items = data["items"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
            for &i in &order {
                let done = items[i]["d"].as_bool().unwrap_or(false);
                let text = items[i]["t"].as_str().unwrap_or("");
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 28.0), egui::Sense::click());
                let x_rect = egui::Rect::from_center_size(egui::pos2(rect.right() - 14.0, rect.center().y), egui::vec2(22.0, 22.0));
                let x_resp = ui.interact(x_rect, resp.id.with("x"), egui::Sense::click());
                let hovered = ui.rect_contains_pointer(rect);
                let p = ui.painter();
                if hovered {
                    p.rect_filled(rect, 7, crate::ink::row_hover());
                }
                // Round check box.
                let c = egui::pos2(rect.left() + 16.0, rect.center().y);
                if done {
                    p.circle_filled(c, 8.0, accent);
                    let st = egui::Stroke::new(1.8_f32, check_col);
                    p.line_segment([c + egui::vec2(-3.6, 0.2), c + egui::vec2(-1.0, 2.9)], st);
                    p.line_segment([c + egui::vec2(-1.0, 2.9), c + egui::vec2(3.8, -2.8)], st);
                } else {
                    p.circle_stroke(c, 7.5, egui::Stroke::new(1.5_f32, if hovered { crate::ink::text() } else { crate::ink::soft() }));
                }
                // Text.
                let col = if done { crate::ink::soft() } else { crate::ink::text() };
                let tw = (rect.width() - 34.0 - 28.0).max(20.0);
                let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), egui::FontId::proportional(14.0), col);
                job.wrap.max_width = tw;
                job.wrap.max_rows = 1;
                job.wrap.break_anywhere = true;
                let galley = ui.fonts(|f| f.layout_job(job));
                let tp = egui::pos2(rect.left() + 34.0, rect.center().y - galley.size().y / 2.0);
                let gw = galley.size().x;
                let gh = galley.size().y;
                let p = ui.painter();
                p.galley(tp, galley, col);
                if done {
                    let y = tp.y + gh * 0.55;
                    p.line_segment([egui::pos2(tp.x, y), egui::pos2(tp.x + gw, y)], egui::Stroke::new(1.0_f32, col));
                }
                // Remove "x" while hovered.
                if hovered {
                    let xc = if x_resp.hovered() { crate::ink::strong() } else { crate::ink::faint() };
                    let xs = egui::Stroke::new(1.5_f32, xc);
                    let xcp = x_rect.center();
                    p.line_segment([xcp + egui::vec2(-4.0, -4.0), xcp + egui::vec2(4.0, 4.0)], xs);
                    p.line_segment([xcp + egui::vec2(-4.0, 4.0), xcp + egui::vec2(4.0, -4.0)], xs);
                }
                if x_resp.clicked() {
                    remove = Some(i);
                } else if resp.clicked() {
                    toggle = Some(i);
                }
            }
        });
        if let Some(a) = data["items"].as_array_mut() {
            if let Some(i) = toggle {
                if let Some(mut it) = (i < a.len()).then(|| a.remove(i)) {
                    let d = it["d"].as_bool().unwrap_or(false);
                    it["d"] = json!(!d);
                    if d {
                        // Re-opened: back to the end of the open tasks.
                        let pos = a.iter().rposition(|x| !x["d"].as_bool().unwrap_or(false)).map(|p| p + 1).unwrap_or(0);
                        a.insert(pos, it);
                    } else {
                        a.push(it);
                    }
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

        // Footer.
        ui.horizontal(|ui| {
            // "+": shows the add field.
            let (pr, presp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
            let pc = if presp.hovered() { crate::ink::strong() } else { crate::ink::soft() };
            if presp.hovered() {
                ui.painter().circle_filled(pr.center(), 9.0, crate::ink::row_hover());
                ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
            }
            let ps = egui::Stroke::new(1.6_f32, pc);
            ui.painter().line_segment([pr.center() - egui::vec2(5.0, 0.0), pr.center() + egui::vec2(5.0, 0.0)], ps);
            ui.painter().line_segment([pr.center() - egui::vec2(0.0, 5.0), pr.center() + egui::vec2(0.0, 5.0)], ps);
            ui.ctx().data_mut(|d| d.insert_temp(aid.with("plus"), pr));
            if presp.clicked() {
                adding = !adding;
                focus_now = adding;
            }
            ui.label(egui::RichText::new(crate::i18n::tf("{} left", &[&left])).size(12.0).color(crate::ink::faint()));
            if done_count > 0 {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let r = ui.add(egui::Label::new(egui::RichText::new(t("Remove finished tasks")).size(12.0).color(crate::ink::soft())).sense(egui::Sense::click()));
                    if r.hovered() {
                        ui.output_mut(|o| o.cursor_icon = egui::CursorIcon::PointingHand);
                        let y = r.rect.bottom();
                        ui.painter().line_segment([egui::pos2(r.rect.left(), y), egui::pos2(r.rect.right(), y)], egui::Stroke::new(1.0_f32, crate::ink::soft()));
                    }
                    if r.clicked() {
                        if let Some(a) = data["items"].as_array_mut() {
                            a.retain(|it| !it["d"].as_bool().unwrap_or(false));
                            out.changed = true;
                        }
                    }
                });
            }
        });
        ui.ctx().data_mut(|d| {
            d.insert_temp(aid, adding);
            if focus_now {
                d.insert_temp(aid.with("focus"), true);
            }
        });
        if focus_now {
            ui.ctx().request_repaint();
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
        let mut out = WidgetOutput { changed: false, repaint_after: Some(Duration::from_secs(60)) };
        let urls = agenda_urls(data);
        let days = data.get("days").and_then(|v| v.as_u64()).unwrap_or(7).clamp(1, 14) as i64;
        let force = crate::agenda::take_refresh();
        let cals = crate::agenda::view(&urls, force);
        if force || cals.iter().any(|c| c.loading) {
            // Look again soon for the download's result (it also wakes the app when done).
            out.repaint_after = Some(Duration::from_secs(2));
        }
        if urls.is_empty() {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(t("Right-click here and choose \"Widget settings…\" to add a calendar link (iCal / .ics).")).size(13.0).color(crate::ink::gray(220)));
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(t("Google Calendar: Settings > your calendar > \"Secret address in iCal format\". Outlook: Settings > Calendar > Shared calendars > Publish."))
                    .size(11.5)
                    .color(crate::ink::gray(170)),
            );
            return out;
        }
        let names: Vec<String> = cals
            .iter()
            .zip(urls.iter())
            .map(|(c, u)| c.name.clone().unwrap_or_else(|| u.split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or(u).to_string()))
            .collect();
        // 0 = month, 1 = week, 2 = agenda list (the dots at the bottom switch).
        let view = data.get("view").and_then(|v| v.as_u64()).unwrap_or(2).min(2) as usize;
        let full = ui.max_rect();
        let content = egui::Rect::from_min_max(full.min, Pos2::new(full.max.x, full.max.y - 16.0));
        let did = ui.make_persistent_id("agenda-details");
        let details: Option<AgendaDetails> = ui.ctx().data(|d| d.get_temp(did));
        let mut open: Option<AgendaDetails> = None;
        let mut close_details = false;
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(egui::Layout::top_down(egui::Align::Min)));
        match &details {
            Some(d) => close_details = agenda_details(&mut child, d, &names),
            None => {
                match view {
                    0 => agenda_month(&mut child, &cals, &mut open),
                    1 => agenda_week(&mut child, &cals, &mut open),
                    _ => agenda_list(&mut child, &cals, days, &mut open),
                }
                let failed = cals.iter().filter(|c| c.error.is_some()).count();
                if failed > 0 {
                    let first = cals.iter().find_map(|c| c.error.clone()).unwrap_or_default();
                    child.label(egui::RichText::new(tf("⚠ {} calendar(s) not updated: {}", &[&failed, &first])).size(11.0).color(crate::ink::adapt(Color32::from_rgb(255, 190, 130)))).on_hover_text(first);
                } else if cals.iter().any(|c| c.loading && c.events.is_empty()) {
                    child.label(egui::RichText::new(t("Loading…")).size(11.5).color(crate::ink::gray(170)));
                }
                if cals.iter().any(|c| c.truncated) {
                    child.label(egui::RichText::new(t("The calendar is too large; only the first events are shown.")).size(11.0).color(crate::ink::gray(170)));
                }
            }
        }
        if close_details || (details.is_some() && ui.input(|i| i.key_pressed(egui::Key::Escape))) {
            ui.ctx().data_mut(|d| d.remove::<AgendaDetails>(did));
            ui.ctx().request_repaint();
        }
        if let Some(d) = open {
            ui.ctx().data_mut(|m| m.insert_temp(did, d));
            ui.ctx().request_repaint();
        }
        // The three dots: month, week, agenda.
        let gap = 14.0;
        let y = full.max.y - 7.0;
        for (i, name) in [t("Month"), t("Week"), t("Agenda")].iter().enumerate() {
            let c = Pos2::new(full.center().x + (i as f32 - 1.0) * gap, y);
            let r = egui::Rect::from_center_size(c, egui::vec2(gap, 14.0));
            let resp = ui.interact(r, ui.id().with(("agenda-dot", i)), Sense::click()).on_hover_text(*name);
            if resp.clicked() {
                data["view"] = json!(i);
                out.changed = true;
                ui.ctx().data_mut(|d| d.remove::<AgendaDetails>(did));
                ui.ctx().request_repaint();
            }
            let alpha = if i == view { 235 } else if resp.hovered() { 160 } else { 90 };
            ui.painter().circle_filled(c, if i == view { 3.5 } else { 3.0 }, crate::ink::white_alpha(alpha));
        }
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
            ui.label(egui::RichText::new(t("Calendar links (iCal / .ics)")).strong());
            let mut remove = None;
            for (i, u) in urls.iter().enumerate() {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 16.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.5, CAL_COLORS[i % CAL_COLORS.len()]);
                    let name = cals.get(i).and_then(|c| c.name.clone()).unwrap_or_else(|| {
                        // The server name is enough to tell links apart (the rest is a secret).
                        u.split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or(u).to_string()
                    });
                    ui.add(egui::Label::new(name).truncate()).on_hover_text(t("The full link stays on this PC only: in your layout file and its backups."));
                    if ui.small_button(t("Remove")).clicked() {
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
                let r = ui.add(egui::TextEdit::singleline(&mut new).hint_text(t("Paste a link: https://… or webcal://…")).desired_width(420.0));
                add |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                add |= ui.button(t("Add")).clicked();
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
                ui.label(t("Show the next"));
                for n in [3u64, 7, 14] {
                    if ui.selectable_label(days as u64 == n, tf("{} days", &[&n])).clicked() {
                        data["days"] = json!(n);
                        out.changed = true;
                    }
                }
            });
            if ui.button(t("Refresh now")).clicked() {
                crate::agenda::request_refresh();
            }
            ui.label(egui::RichText::new(t("Calendars are downloaded every 30 minutes while this widget is shown.")).size(11.0).color(crate::ink::gray(150)));
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
            let img = image::open(&p).map_err(|e| tf("can't open the picture ({})", &[&e]))?;
            let img = if img.width().max(img.height()) > 1600 { img.thumbnail(1600, 1600) } else { img };
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            Ok(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
        })
        .unwrap_or_else(|_| Err(t("can't read that picture").to_string()));
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
            ui.label(egui::RichText::new(t("Right-click here and choose \"Widget settings…\" to pick a picture.")).size(13.0).color(crate::ink::gray(220)));
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
                    ui.label(egui::RichText::new(format!("⚠ {e}")).size(12.5).color(crate::ink::adapt(Color32::from_rgb(255, 190, 130))));
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
        ui.label(egui::RichText::new(t("Picture (PNG or JPEG)")).strong());
        ui.horizontal(|ui| {
            if ui.add(egui::TextEdit::singleline(&mut path).hint_text("C:\\Users\\…\\Pictures\\photo.jpg").desired_width(420.0)).changed() {
                data["path"] = json!(path.trim());
                changed = true;
            }
            let picking = PICKING.load(std::sync::atomic::Ordering::SeqCst);
            if ui.add_enabled(!picking, egui::Button::new(t("Choose…"))).clicked() {
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
            if ui.radio(!fill, t("Show the whole picture")).clicked() && fill {
                fill = false;
                changed = true;
            }
            if ui.radio(fill, t("Fill the box (edges may be cut)")).clicked() && !fill {
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

/// What the agenda's details view shows: a heading and the events (calendar index, event).
type AgendaDetails = (String, Vec<(usize, crate::agenda::Occurrence)>);

/// The events touching a day, all-day ones first, then by start time.
fn agenda_day_events(cals: &[crate::agenda::CalView], day: chrono::NaiveDate) -> Vec<(usize, crate::agenda::Occurrence)> {
    use chrono::NaiveTime;
    let (d0, d1) = (day.and_time(NaiveTime::MIN), (day + chrono::Duration::days(1)).and_time(NaiveTime::MIN));
    let mut v: Vec<(usize, crate::agenda::Occurrence)> = Vec::new();
    for (ci, c) in cals.iter().enumerate() {
        for e in c.events.iter() {
            let touches = if e.all_day {
                e.start < d1 && e.end > d0
            } else {
                (e.start >= d0 && e.start < d1) || (e.start < d0 && e.end > d0)
            };
            if touches {
                v.push((ci, e.clone()));
            }
        }
    }
    v.sort_by(|a, b| b.1.all_day.cmp(&a.1.all_day).then(a.1.start.cmp(&b.1.start)).then(a.1.title.cmp(&b.1.title)));
    v
}

/// "Sun" in the language in use.
fn wd_abbr(d: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    crate::i18n::weekday_name(d.weekday().num_days_from_monday()).chars().take(3).collect()
}

/// "Sun 22 Nov" in the language in use.
fn wd_date(d: chrono::NaiveDate) -> String {
    format!("{} {}", wd_abbr(d), crate::i18n::short_date(d))
}

fn agenda_day_title(day: chrono::NaiveDate) -> String {
    let today = chrono::Local::now().date_naive();
    match (day - today).num_days() {
        0 => tf("Today, {}", &[&crate::i18n::day_month(day)]),
        1 => tf("Tomorrow, {}", &[&crate::i18n::day_month(day)]),
        _ => crate::i18n::long_date(day),
    }
}

/// A small "‹ title ›" header; returns -1 / 0 / +1 for the arrows (0 also for a click on the
/// title, meaning "back to now", returned as Some(0)).
fn agenda_nav(ui: &mut Ui, title: &str) -> Option<i32> {
    let mut step = None;
    ui.horizontal(|ui| {
        if ui.add(egui::Button::new(egui::RichText::new("‹").size(18.0).color(crate::ink::strong())).frame(false)).clicked() {
            step = Some(-1);
        }
        let w = (ui.available_width() - 24.0).max(40.0);
        if ui.add_sized([w, 20.0], egui::Button::new(egui::RichText::new(title).size(14.0).color(crate::ink::strong())).frame(false)).on_hover_text(t("Back to today")).clicked() {
            step = Some(0);
        }
        if ui.add(egui::Button::new(egui::RichText::new("›").size(18.0).color(crate::ink::strong())).frame(false)).clicked() {
            step = Some(1);
        }
    });
    step
}

/// Month view: the days of a month; dots under days with events (one per calendar, at most
/// three); a click on a day shows its events.
fn agenda_month(ui: &mut Ui, cals: &[crate::agenda::CalView], open: &mut Option<AgendaDetails>) {
    use chrono::{Datelike, NaiveDate};
    let today = chrono::Local::now().date_naive();
    let sid = ui.make_persistent_id("agenda-month");
    let mut shift: i32 = ui.ctx().data(|d| d.get_temp(sid)).unwrap_or(0);
    let months = today.year() * 12 + today.month0() as i32 + shift;
    let (y, m) = (months.div_euclid(12), months.rem_euclid(12) as u32 + 1);
    let Some(first) = NaiveDate::from_ymd_opt(y, m, 1) else { return };
    match agenda_nav(ui, &crate::i18n::month_year(first)) {
        Some(0) => shift = 0,
        Some(s) => shift += s,
        None => {}
    }
    ui.ctx().data_mut(|d| d.insert_temp(sid, shift));
    let cw = ui.available_width() / 7.0;
    let ch = ((ui.available_height() - 40.0) / 6.0).clamp(18.0, 44.0);
    let (head, _) = ui.allocate_exact_size(egui::vec2(cw * 7.0, 16.0), Sense::hover());
    for (i, n) in [0, 1, 2, 3, 4, 5, 6].map(crate::i18n::weekday_short).iter().enumerate() {
        ui.painter().text(Pos2::new(head.min.x + cw * (i as f32 + 0.5), head.center().y), egui::Align2::CENTER_CENTER, *n, egui::FontId::proportional(11.0), crate::ink::gray(200));
    }
    let lead = first.weekday().num_days_from_monday() as i64;
    let (grid, _) = ui.allocate_exact_size(egui::vec2(cw * 7.0, ch * 6.0), Sense::hover());
    for cell in 0..42i64 {
        let day = first + chrono::Duration::days(cell - lead);
        if day.month() != m {
            continue;
        }
        let r = egui::Rect::from_min_size(Pos2::new(grid.min.x + cw * (cell % 7) as f32, grid.min.y + ch * (cell / 7) as f32), egui::vec2(cw, ch));
        let evs = agenda_day_events(cals, day);
        let resp = ui.interact(r, ui.id().with(("agenda-day", day)), Sense::click());
        if resp.hovered() && !evs.is_empty() {
            ui.painter().rect_filled(r.shrink(1.0), 4.0, crate::ink::white_alpha(22));
        }
        let num_c = Pos2::new(r.center().x, r.min.y + (ch * 0.42).min(14.0));
        if day == today {
            ui.painter().circle_filled(num_c, 9.5, Color32::from_rgb(70, 140, 230));
        }
        ui.painter().text(num_c, egui::Align2::CENTER_CENTER, day.day().to_string(), egui::FontId::proportional(12.5), crate::ink::gray(if day == today { 255 } else { 230 }));
        // One dot per calendar with events that day (at most three).
        let mut seen: Vec<usize> = Vec::new();
        for (ci, _) in &evs {
            if !seen.contains(ci) && seen.len() < 3 {
                seen.push(*ci);
            }
        }
        let dy = (num_c.y + 11.0).min(r.max.y - 3.0);
        for (k, ci) in seen.iter().enumerate() {
            let x = r.center().x + (k as f32 - (seen.len() as f32 - 1.0) / 2.0) * 6.0;
            ui.painter().circle_filled(Pos2::new(x, dy), 2.2, CAL_COLORS[ci % CAL_COLORS.len()]);
        }
        if resp.clicked() && !evs.is_empty() {
            *open = Some((agenda_day_title(day), evs));
        }
    }
}

/// Week view: seven columns, each day's events as small coloured chips; a click on a chip shows
/// that event, a click on a day's name shows the whole day.
fn agenda_week(ui: &mut Ui, cals: &[crate::agenda::CalView], open: &mut Option<AgendaDetails>) {
    use chrono::Datelike;
    let today = chrono::Local::now().date_naive();
    let sid = ui.make_persistent_id("agenda-week");
    let mut shift: i64 = ui.ctx().data(|d| d.get_temp(sid)).unwrap_or(0);
    let monday = today - chrono::Duration::days(today.weekday().num_days_from_monday() as i64) + chrono::Duration::weeks(shift);
    let sunday = monday + chrono::Duration::days(6);
    let title = if monday.month() == sunday.month() {
        format!("{} – {} {}", monday.day(), sunday.day(), crate::i18n::month_name(sunday.month()))
    } else {
        format!("{} – {}", crate::i18n::short_date(monday), crate::i18n::short_date(sunday))
    };
    match agenda_nav(ui, &title) {
        Some(0) => shift = 0,
        Some(s) => shift += s as i64,
        None => {}
    }
    ui.ctx().data_mut(|d| d.insert_temp(sid, shift));
    let cw = ui.available_width() / 7.0;
    let top = ui.cursor().min.y;
    let height = (ui.available_height() - 4.0).max(40.0);
    let (area, _) = ui.allocate_exact_size(egui::vec2(cw * 7.0, height), Sense::hover());
    let chip_h = 17.0;
    for i in 0..7 {
        let day = monday + chrono::Duration::days(i);
        let col = egui::Rect::from_min_size(Pos2::new(area.min.x + cw * i as f32, top), egui::vec2(cw, height));
        if i > 0 {
            ui.painter().line_segment([col.left_top(), col.left_bottom()], egui::Stroke::new(1.0_f32, crate::ink::white_alpha(18)));
        }
        let evs = agenda_day_events(cals, day);
        // Day name and number (today highlighted); click = the whole day.
        let head = egui::Rect::from_min_size(col.min, egui::vec2(cw, 30.0));
        let hresp = ui.interact(head, ui.id().with(("agenda-wday", day)), Sense::click());
        if day == today {
            ui.painter().rect_filled(head.shrink(2.0), 4.0, Color32::from_rgb(70, 140, 230).linear_multiply(0.6));
        } else if hresp.hovered() && !evs.is_empty() {
            ui.painter().rect_filled(head.shrink(2.0), 4.0, crate::ink::white_alpha(22));
        }
        ui.painter().text(Pos2::new(head.center().x, head.min.y + 8.0), egui::Align2::CENTER_CENTER, wd_abbr(day), egui::FontId::proportional(10.5), crate::ink::gray(205));
        ui.painter().text(Pos2::new(head.center().x, head.min.y + 21.0), egui::Align2::CENTER_CENTER, day.day().to_string(), egui::FontId::proportional(13.0), crate::ink::strong());
        if hresp.clicked() && !evs.is_empty() {
            *open = Some((agenda_day_title(day), evs.clone()));
        }
        // The events, as many as fit; then "+n".
        let fit = (((height - 34.0) / (chip_h + 2.0)).floor() as usize).max(1);
        let shown = if evs.len() > fit { fit.saturating_sub(1) } else { evs.len() };
        for (k, (ci, e)) in evs.iter().take(shown).enumerate() {
            let r = egui::Rect::from_min_size(Pos2::new(col.min.x + 2.0, col.min.y + 34.0 + k as f32 * (chip_h + 2.0)), egui::vec2(cw - 4.0, chip_h));
            let color = CAL_COLORS[ci % CAL_COLORS.len()];
            let resp = ui.interact(r, ui.id().with(("agenda-chip", day, k)), Sense::click());
            ui.painter().rect_filled(r, 3.0, color.linear_multiply(if resp.hovered() { 0.75 } else { 0.5 }));
            let text = if e.all_day || e.start.date() != day { e.title.clone() } else { format!("{} {}", e.start.format("%H:%M"), e.title) };
            let g = ui.painter().layout_no_wrap(text, egui::FontId::proportional(11.0), crate::ink::strong());
            ui.painter().with_clip_rect(r.shrink(2.0)).galley(Pos2::new(r.min.x + 3.0, r.center().y - g.size().y / 2.0), g, crate::ink::strong());
            if resp.clicked() {
                *open = Some((agenda_day_title(day), vec![(*ci, e.clone())]));
            }
            resp.on_hover_text(&e.title);
        }
        if evs.len() > shown {
            let r = egui::Rect::from_min_size(Pos2::new(col.min.x + 2.0, col.min.y + 34.0 + shown as f32 * (chip_h + 2.0)), egui::vec2(cw - 4.0, chip_h));
            let resp = ui.interact(r, ui.id().with(("agenda-more", day)), Sense::click());
            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, format!("+{}", evs.len() - shown), egui::FontId::proportional(11.5), crate::ink::gray(if resp.hovered() { 255 } else { 200 }));
            if resp.clicked() {
                *open = Some((agenda_day_title(day), evs.clone()));
            }
        }
    }
}

/// Agenda view: the coming days as a list, each day under its heading; a click on an event shows it.
fn agenda_list(ui: &mut Ui, cals: &[crate::agenda::CalView], days: i64, open: &mut Option<AgendaDetails>) {
    use chrono::{Duration as CDuration, NaiveTime};
    let today = chrono::Local::now().date_naive();
    let now = chrono::Local::now().naive_local();
    let end = (today + CDuration::days(days)).and_time(NaiveTime::MIN);
    let mut rows: Vec<(chrono::NaiveDate, usize, &crate::agenda::Occurrence)> = Vec::new();
    for (ci, c) in cals.iter().enumerate() {
        for e in c.events.iter() {
            let over = e.end <= now && !(e.end == e.start && e.start >= now);
            if over || e.start >= end {
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
    let status_h = if cals.iter().any(|c| c.error.is_some() || c.truncated || (c.loading && c.events.is_empty())) { 20.0 } else { 0.0 };
    egui::ScrollArea::vertical().max_height((ui.available_height() - status_h).max(30.0)).auto_shrink([false, true]).show(ui, |ui| {
        if rows.is_empty() && cals.iter().all(|c| !c.loading) {
            ui.label(egui::RichText::new(tf("Nothing in the next {} days.", &[&days])).size(13.0).color(crate::ink::gray(190)));
        }
        let mut last_day = None;
        for (k, (day, ci, e)) in rows.iter().enumerate() {
            if last_day != Some(*day) {
                last_day = Some(*day);
                let label = match (*day - today).num_days() {
                    0 => t("Today").to_string(),
                    1 => t("Tomorrow").to_string(),
                    _ => crate::i18n::long_date(*day),
                };
                ui.add_space(3.0);
                ui.label(egui::RichText::new(label).size(12.0).strong().color(crate::ink::gray(200)));
            }
            let (r, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 19.0), Sense::click());
            let resp = ui.interact(r, ui.id().with(("agenda-row", k)), Sense::click()).union(resp);
            if resp.hovered() {
                ui.painter().rect_filled(r, 3.0, crate::ink::white_alpha(22));
            }
            ui.painter().circle_filled(Pos2::new(r.min.x + 5.0, r.center().y), 3.5, CAL_COLORS[ci % CAL_COLORS.len()]);
            let when = if e.all_day { t("all day").to_string() } else { e.start.format("%H:%M").to_string() };
            ui.painter().text(Pos2::new(r.min.x + 14.0, r.center().y), egui::Align2::LEFT_CENTER, when, egui::FontId::proportional(12.5), crate::ink::gray(185));
            let happening = !e.all_day && e.start <= now && now < e.end;
            let g = ui.painter().layout_no_wrap(e.title.clone(), egui::FontId::proportional(13.0), if happening { crate::ink::strong() } else { crate::ink::gray(230) });
            let text_r = egui::Rect::from_min_max(Pos2::new(r.min.x + 62.0, r.min.y), r.max);
            ui.painter().with_clip_rect(text_r).galley(Pos2::new(text_r.min.x, r.center().y - g.size().y / 2.0), g, crate::ink::strong());
            if resp.clicked() {
                *open = Some((agenda_day_title(*day), vec![(*ci, (*e).clone())]));
            }
        }
    });
}

/// The details of one or more events. Returns true when "‹ Back" was clicked.
fn agenda_details(ui: &mut Ui, (heading, evs): &AgendaDetails, names: &[String]) -> bool {
    let mut back = false;
    ui.horizontal(|ui| {
        if ui.add(egui::Button::new(egui::RichText::new(t("‹ Back")).size(13.0).color(crate::ink::gray(220))).frame(false)).clicked() {
            back = true;
        }
        ui.label(egui::RichText::new(heading).size(13.0).strong().color(crate::ink::strong()));
    });
    ui.add_space(2.0);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        for (i, (ci, e)) in evs.iter().enumerate() {
            if i > 0 {
                ui.add_space(4.0);
                ui.separator();
            }
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 18.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, CAL_COLORS[ci % CAL_COLORS.len()]);
                ui.add(egui::Label::new(egui::RichText::new(&e.title).size(14.5).strong().color(crate::ink::strong())).wrap());
            });
            let when = if e.all_day {
                let last = e.end.date() - chrono::Duration::days(1);
                if last > e.start.date() {
                    tf("All day, {} to {}", &[&wd_date(e.start.date()), &wd_date(last)])
                } else {
                    tf("All day, {}", &[&crate::i18n::long_date(e.start.date())])
                }
            } else if e.end.date() != e.start.date() {
                tf(
                    "{} to {}",
                    &[&format!("{}, {}", wd_date(e.start.date()), e.start.format("%H:%M")), &format!("{}, {}", wd_date(e.end.date()), e.end.format("%H:%M"))],
                )
            } else if e.end > e.start {
                format!("{}, {} – {}", wd_date(e.start.date()), e.start.format("%H:%M"), e.end.format("%H:%M"))
            } else {
                format!("{}, {}", wd_date(e.start.date()), e.start.format("%H:%M"))
            };
            ui.label(egui::RichText::new(when).size(12.5).color(crate::ink::gray(215)));
            if let Some(n) = names.get(*ci) {
                ui.label(egui::RichText::new(n).size(11.5).color(crate::ink::gray(165)));
            }
            if !e.location.is_empty() {
                ui.add(egui::Label::new(egui::RichText::new(tf("Where: {}", &[&e.location])).size(12.5).color(crate::ink::gray(225))).wrap());
            }
            if !e.description.is_empty() {
                ui.add_space(2.0);
                ui.add(egui::Label::new(egui::RichText::new(&*e.description).size(12.5).color(crate::ink::gray(205))).wrap());
            }
        }
    });
    back
}

// ---- Laptop status (battery, Wi-Fi, Bluetooth) -----------------------------------------------

#[derive(Clone, Default)]
struct LaptopInfo {
    /// Battery: percent, charging (plugged in), minutes left (if Windows knows). None = no battery.
    battery: Option<(u8, bool, Option<u32>)>,
    /// Wi-Fi: Some(Some((network, signal 0-100))) connected, Some(None) not connected, None = no Wi-Fi.
    wifi: Option<Option<(String, u32)>>,
    /// Bluetooth: None = no Bluetooth, Some((on, connected device names)).
    bluetooth: Option<(bool, Vec<String>)>,
}

/// Last reading (and when), read on a helper thread at most every 5 s while a laptop widget shows.
static LAPTOP: std::sync::Mutex<Option<(std::time::Instant, LaptopInfo)>> = std::sync::Mutex::new(None);
static LAPTOP_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(windows)]
fn read_laptop() -> LaptopInfo {
    let mut info = LaptopInfo::default();
    unsafe {
        use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
        let mut s = SYSTEM_POWER_STATUS::default();
        // BatteryFlag 128 = no battery, 255 = unknown.
        if GetSystemPowerStatus(&mut s).is_ok() && s.BatteryFlag != 128 && s.BatteryFlag != 255 && s.BatteryLifePercent <= 100 {
            let left = (s.BatteryLifeTime != u32::MAX).then(|| s.BatteryLifeTime / 60);
            info.battery = Some((s.BatteryLifePercent, s.ACLineStatus == 1, left));
        }
    }
    unsafe {
        use windows::Win32::Foundation::HANDLE;
        use windows::Win32::NetworkManagement::WiFi::*;
        let mut ver = 0u32;
        let mut h = HANDLE::default();
        if WlanOpenHandle(2, None, &mut ver, &mut h) == 0 {
            let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
            if WlanEnumInterfaces(h, None, &mut list) == 0 && !list.is_null() {
                let n = (*list).dwNumberOfItems as usize;
                if n > 0 {
                    info.wifi = Some(None);
                    let first = (*list).InterfaceInfo.as_ptr();
                    for i in 0..n {
                        let iface = &*first.add(i);
                        if iface.isState != wlan_interface_state_connected {
                            continue;
                        }
                        let mut size = 0u32;
                        let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
                        if WlanQueryInterface(h, &iface.InterfaceGuid, wlan_intf_opcode_current_connection, None, &mut size, &mut data, None) == 0 && !data.is_null() {
                            let a = &*(data as *const WLAN_CONNECTION_ATTRIBUTES);
                            let ssid = &a.wlanAssociationAttributes.dot11Ssid;
                            let len = (ssid.uSSIDLength as usize).min(32);
                            let name = String::from_utf8_lossy(&ssid.ucSSID[..len]).to_string();
                            info.wifi = Some(Some((name, a.wlanAssociationAttributes.wlanSignalQuality.min(100))));
                            WlanFreeMemory(data);
                            break;
                        }
                    }
                }
                WlanFreeMemory(list as *const _);
            }
            let _ = WlanCloseHandle(h, None);
        }
    }
    unsafe {
        use windows::Win32::Devices::Bluetooth::*;
        use windows::Win32::Foundation::{CloseHandle, BOOL, HANDLE};
        let params = BLUETOOTH_FIND_RADIO_PARAMS { dwSize: std::mem::size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32 };
        let mut radio = HANDLE::default();
        if let Ok(find) = BluetoothFindFirstRadio(&params, &mut radio) {
            let on = BluetoothIsConnectable(radio).as_bool();
            let mut names = Vec::new();
            if on {
                // Only devices connected right now; no search (that would take seconds).
                let sp = BLUETOOTH_DEVICE_SEARCH_PARAMS {
                    dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
                    fReturnAuthenticated: BOOL(0),
                    fReturnRemembered: BOOL(0),
                    fReturnUnknown: BOOL(0),
                    fReturnConnected: BOOL(1),
                    fIssueInquiry: BOOL(0),
                    cTimeoutMultiplier: 0,
                    hRadio: radio,
                };
                let mut di = BLUETOOTH_DEVICE_INFO { dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32, ..Default::default() };
                if let Ok(df) = BluetoothFindFirstDevice(&sp, &mut di) {
                    loop {
                        if di.fConnected.as_bool() {
                            let len = di.szName.iter().position(|c| *c == 0).unwrap_or(di.szName.len());
                            names.push(String::from_utf16_lossy(&di.szName[..len]));
                        }
                        di = BLUETOOTH_DEVICE_INFO { dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32, ..Default::default() };
                        if BluetoothFindNextDevice(df, &mut di).is_err() || names.len() > 20 {
                            break;
                        }
                    }
                    let _ = BluetoothFindDeviceClose(df);
                }
            }
            let _ = CloseHandle(radio);
            let _ = BluetoothFindRadioClose(find);
            info.bluetooth = Some((on, names));
        }
    }
    info
}
#[cfg(not(windows))]
fn read_laptop() -> LaptopInfo {
    LaptopInfo::default()
}

fn laptop_now() -> LaptopInfo {
    let current = LAPTOP.lock().ok().and_then(|g| g.clone());
    let stale = current.as_ref().map(|(t, _)| t.elapsed() >= Duration::from_secs(5)).unwrap_or(true);
    if stale && !LAPTOP_BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
        let spawned = std::thread::Builder::new().name("laptop-status".into()).spawn(|| {
            let info = std::panic::catch_unwind(read_laptop).unwrap_or_default();
            if let Ok(mut g) = LAPTOP.lock() {
                *g = Some((std::time::Instant::now(), info));
            }
            LAPTOP_BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
            #[cfg(windows)]
            crate::runloop::wake_after(Duration::ZERO);
        });
        if spawned.is_err() {
            LAPTOP_BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
    current.map(|(_, i)| i).unwrap_or_default()
}

/// One status line: icon, text, and a small detail on the right.
fn status_line(ui: &mut Ui, icon: char, text: &str, detail: &str, color: Color32) {
    ui.horizontal(|ui| {
        ui.add_sized([24.0, 22.0], egui::Label::new(egui::RichText::new(icon.to_string()).size(17.0).color(crate::ink::adapt(color))));
        ui.add(egui::Label::new(egui::RichText::new(text).size(13.5).color(crate::ink::gray(235))).truncate());
        if !detail.is_empty() {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(detail).size(12.0).color(crate::ink::gray(175)));
            });
        }
    });
}

struct Laptop;

impl DeskWidget for Laptop {
    fn ui(&self, ui: &mut Ui, _data: &mut Value) -> WidgetOutput {
        let info = laptop_now();
        ui.add_space(2.0);
        match info.battery {
            Some((pct, charging, left)) => {
                let icon = if charging { '\u{E83E}' } else { '\u{E83F}' };
                let detail = match (charging, left) {
                    (true, _) if pct >= 100 => t("charged").to_string(),
                    (true, _) => t("charging").to_string(),
                    (false, Some(m)) => tf("{}h {}m left", &[&(m / 60), &format!("{:02}", m % 60)]),
                    (false, None) => String::new(),
                };
                status_line(ui, icon, &tf("Battery {} %", &[&pct]), &detail, level_color(1.0 - pct as f32 / 100.0));
                // The charge as a bar.
                let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 6.0), Sense::hover());
                ui.painter().rect_filled(r, 3.0, crate::ink::white_alpha(28));
                let mut fill = r;
                fill.set_width(r.width() * pct as f32 / 100.0);
                ui.painter().rect_filled(fill, 3.0, if pct <= 15 { Color32::from_rgb(255, 110, 100) } else { Color32::from_rgb(110, 210, 120) });
                ui.add_space(4.0);
            }
            None => status_line(ui, '\u{E83E}', t("No battery (plugged in)"), "", crate::ink::gray(170)),
        }
        match &info.wifi {
            Some(Some((name, q))) => {
                let bars = match q {
                    0..=25 => "▂",
                    26..=50 => "▂▄",
                    51..=75 => "▂▄▆",
                    _ => "▂▄▆█",
                };
                status_line(ui, '\u{E701}', name, &format!("{bars} {q} %"), Color32::from_rgb(110, 190, 255));
            }
            Some(None) => status_line(ui, '\u{EB5E}', t("Wi-Fi: not connected"), "", crate::ink::gray(170)),
            None => status_line(ui, '\u{EB5E}', t("No Wi-Fi"), "", crate::ink::gray(140)),
        }
        match &info.bluetooth {
            Some((true, names)) if !names.is_empty() => status_line(ui, '\u{E702}', &names.join(", "), t("connected"), Color32::from_rgb(110, 190, 255)),
            Some((true, _)) => status_line(ui, '\u{E702}', t("Bluetooth on"), t("nothing connected"), Color32::from_rgb(110, 190, 255)),
            Some((false, _)) => status_line(ui, '\u{E702}', t("Bluetooth off"), "", crate::ink::gray(150)),
            None => status_line(ui, '\u{E702}', t("No Bluetooth"), "", crate::ink::gray(140)),
        }
        WidgetOutput { changed: false, repaint_after: Some(Duration::from_secs(5)) }
    }
}

// ---- Media (what's playing) ------------------------------------------------------------------

#[derive(Clone, Default)]
struct MediaInfo {
    app: String,
    title: String,
    artist: String,
    playing: bool,
    /// Position and length in seconds, and when they were read (the bar moves on from there).
    timeline: Option<(f64, f64, std::time::Instant)>,
    /// Cover picture (decoded, at most 256 px), with a number that changes with the song.
    art: Option<(u64, std::sync::Arc<egui::ColorImage>)>,
    /// (song key, tries) of reading the cover when there was none: after a few tries the stream is
    /// not opened again for that song.
    art_tries: (u64, u8),
}

/// What's playing, read on a helper thread at most every 2 s while a media widget shows (Windows'
/// own media controls: Spotify, browsers, VLC… anything that shows in the volume flyout).
/// Every player Windows knows (the one it calls "current" first).
static MEDIA: std::sync::Mutex<Option<(std::time::Instant, Vec<MediaInfo>)>> = std::sync::Mutex::new(None);
static MEDIA_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(windows)]
fn media_sessions() -> Vec<windows::Media::Control::GlobalSystemMediaTransportControlsSession> {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager as M;
    let Some(m) = M::RequestAsync().ok().and_then(|op| op.get().ok()) else { return Vec::new() };
    let mut v = Vec::new();
    let current = m.GetCurrentSession().ok();
    let cur_id = current.as_ref().and_then(|s| s.SourceAppUserModelId().ok()).map(|h| h.to_string());
    if let Some(c) = current {
        v.push(c);
    }
    if let Ok(list) = m.GetSessions() {
        for s in list {
            let id = s.SourceAppUserModelId().ok().map(|h| h.to_string());
            if id != cur_id {
                v.push(s);
            }
        }
    }
    v
}

#[cfg(windows)]
fn read_media(previous: &[MediaInfo]) -> Vec<MediaInfo> {
    media_sessions().iter().filter_map(|s| read_session(s, previous)).collect()
}

#[cfg(windows)]
fn read_session(s: &windows::Media::Control::GlobalSystemMediaTransportControlsSession, previous: &[MediaInfo]) -> Option<MediaInfo> {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus as S;
    let props = s.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    let title = props.Title().map(|t| t.to_string()).unwrap_or_default();
    let artist = props.Artist().map(|t| t.to_string()).unwrap_or_default();
    let app = s.SourceAppUserModelId().map(|t| t.to_string()).unwrap_or_default();
    let playing = s.GetPlaybackInfo().ok().and_then(|p| p.PlaybackStatus().ok()).map(|st| st == S::Playing).unwrap_or(false);
    let timeline = s.GetTimelineProperties().ok().and_then(|t| {
        let pos = t.Position().ok()?.Duration as f64 / 1e7;
        let end = t.EndTime().ok()?.Duration as f64 / 1e7;
        (end > 1.0).then_some((pos, end, std::time::Instant::now()))
    });
    // Same song as before: keep its picture (reading it again costs more than the rest).
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&app, &title, &artist).hash(&mut h);
        h.finish()
    };
    let tried = previous.iter().find(|p| p.art_tries.0 == key).map(|p| p.art_tries.1).unwrap_or(0);
    let art = match previous.iter().find_map(|p| p.art.clone().filter(|(k, _)| *k == key)) {
        Some(a) => Some(a),
        None if tried >= 3 => None,
        None =>(|| -> Option<(u64, std::sync::Arc<egui::ColorImage>)> {
            use windows::Storage::Streams::DataReader;
            let stream = props.Thumbnail().ok()?.OpenReadAsync().ok()?.get().ok()?;
            let size = stream.Size().ok()? as u32;
            if size == 0 || size > 8 * 1024 * 1024 {
                return None;
            }
            let reader = DataReader::CreateDataReader(&stream).ok()?;
            reader.LoadAsync(size).ok()?.get().ok()?;
            let mut buf = vec![0u8; size as usize];
            reader.ReadBytes(&mut buf).ok()?;
            let img = image::load_from_memory(&buf).ok()?.thumbnail(256, 256).to_rgba8();
            let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
            Some((key, std::sync::Arc::new(ci)))
        })(),
    };
    // A cover that is not there (yet) is tried only a few times per song (players fill it in late).
    let art_tries = if art.is_some() { (key, 0) } else { (key, tried.saturating_add(1)) };
    Some(MediaInfo { app, title, artist, playing, timeline, art, art_tries })
}
#[cfg(not(windows))]
fn read_media(_previous: &[MediaInfo]) -> Vec<MediaInfo> {
    Vec::new()
}

fn refresh_media(force: bool) {
    let current = MEDIA.lock().ok().and_then(|g| g.clone());
    let stale = current.as_ref().map(|(t, _)| t.elapsed() >= Duration::from_secs(2)).unwrap_or(true);
    if (stale || force) && !MEDIA_BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
        let prev = current.map(|(_, m)| m).unwrap_or_default();
        let spawned = std::thread::Builder::new().name("media-status".into()).spawn(move || {
            let info = std::panic::catch_unwind(|| read_media(&prev)).unwrap_or_default();
            if let Ok(mut g) = MEDIA.lock() {
                *g = Some((std::time::Instant::now(), info));
            }
            MEDIA_BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
            #[cfg(windows)]
            crate::runloop::wake_after(Duration::ZERO);
        });
        if spawned.is_err() {
            MEDIA_BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }
}

/// Play/pause, next or previous, on a helper thread (the player answers when it likes).
fn media_command(which: u8, app: String) {
    let _ = std::thread::Builder::new().name("media-command".into()).spawn(move || {
        #[cfg(windows)]
        if let Some(s) = media_sessions().into_iter().find(|s| s.SourceAppUserModelId().map(|h| h.to_string() == app).unwrap_or(false)) {
            let r = match which {
                0 => s.TrySkipPreviousAsync(),
                1 => s.TryTogglePlayPauseAsync(),
                _ => s.TrySkipNextAsync(),
            };
            if let Ok(op) = r {
                let _ = op.get();
            }
        }
        std::thread::sleep(Duration::from_millis(300));
        if let Ok(mut g) = MEDIA.lock() {
            if let Some((t, _)) = g.as_mut() {
                *t = std::time::Instant::now() - Duration::from_secs(10); // read again now
            }
        }
        #[cfg(windows)]
        crate::runloop::wake_after(Duration::ZERO);
    });
}

/// A friendly app name from Windows' id for the player: "Spotify.exe" -> "Spotify",
/// "AppleInc.AppleMusicWin_nzyj…!App" -> "Apple Music", "Microsoft.ZuneMusic_…!Microsoft.ZuneMusic"
/// -> "Media Player". Browsers report a code: "Browser".
fn app_label(id: &str) -> String {
    let lower = id.to_lowercase();
    for (k, v) in [("zunemusic", "Media Player"), ("spotify", "Spotify"), ("chrome", "Chrome"), ("msedge", "Edge"), ("firefox", "Firefox"), ("vlc", "VLC"), ("applemusic", "Apple Music")] {
        if lower.contains(k) {
            return v.to_string();
        }
    }
    // Packaged app: the package name, before "_" and "!", after the publisher ("Publisher.Name").
    let pkg = id.split('!').next().unwrap_or(id);
    let pkg = pkg.split('_').next().unwrap_or(pkg);
    let base = pkg.rsplit(['\\', '/']).next().unwrap_or(pkg);
    let base = base.trim_end_matches(".exe").trim_end_matches(".EXE");
    let base = base.rsplit('.').next().unwrap_or(base);
    if base.is_empty() || (base.len() > 14 && base.chars().all(|c| c.is_ascii_hexdigit())) {
        return t("Browser").into(); // Chrome / Edge sometimes report a code instead of a name
    }
    let base = base.strip_suffix("Win").unwrap_or(base);
    // "AppleMusic" -> "Apple Music"
    let mut out = String::new();
    for (i, c) in base.chars().enumerate() {
        if i > 0 && c.is_uppercase() && !out.ends_with(' ') {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

fn mmss(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

struct Media;

impl DeskWidget for Media {
    fn ui(&self, ui: &mut Ui, _data: &mut Value) -> WidgetOutput {
        refresh_media(false);
        let all: Vec<MediaInfo> = MEDIA.lock().ok().and_then(|g| g.as_ref().map(|(_, m)| m.clone())).unwrap_or_default();
        // Nothing playing: an empty box (it fills again when music starts).
        if all.is_empty() {
            return WidgetOutput { changed: false, repaint_after: Some(Duration::from_secs(3)) };
        }
        // Several players: the one chosen with the dots (by its app), else Windows' current one.
        let sid = ui.make_persistent_id("media-player");
        let chosen: Option<String> = ui.ctx().data(|d| d.get_temp(sid));
        let idx = chosen.and_then(|c| all.iter().position(|m| m.app == c)).unwrap_or(0);
        let m = all[idx].clone();
        let mut full = ui.max_rect();
        if all.len() > 1 {
            // The dots, one per player, at the bottom.
            let gap = 14.0;
            let y = full.max.y - 6.0;
            let n = all.len();
            for (i, p) in all.iter().enumerate() {
                let c = Pos2::new(full.center().x + (i as f32 - (n as f32 - 1.0) / 2.0) * gap, y);
                let r = egui::Rect::from_center_size(c, egui::vec2(gap, 12.0));
                let resp = ui.interact(r, ui.id().with(("media-dot", i)), Sense::click()).on_hover_text(format!("{} — {}", app_label(&p.app), p.title));
                if resp.clicked() {
                    ui.ctx().data_mut(|d| d.insert_temp(sid, p.app.clone()));
                    ui.ctx().request_repaint();
                }
                let alpha = if i == idx { 235 } else if resp.hovered() { 160 } else { 90 };
                ui.painter().circle_filled(c, if i == idx { 3.5 } else { 3.0 }, crate::ink::white_alpha(alpha));
            }
            full.max.y -= 12.0;
        }
        let side = (full.height() - 8.0).min(full.width() * 0.42).max(32.0);
        let art_rect = egui::Rect::from_min_size(Pos2::new(full.min.x, full.center().y - side / 2.0), egui::vec2(side, side));
        // The cover with rounded corners: one texture per widget, replaced when the song changes.
        let tid = ui.make_persistent_id("media-art");
        match &m.art {
            Some((key, img)) => {
                let held: Option<(u64, egui::TextureHandle)> = ui.ctx().data(|d| d.get_temp(tid));
                let tex = match held.filter(|(k, _)| k == key) {
                    Some((_, t)) => t,
                    None => {
                        let t = ui.ctx().load_texture("media-art", (**img).clone(), egui::TextureOptions::LINEAR);
                        ui.ctx().data_mut(|d| d.insert_temp(tid, (*key, t.clone())));
                        t
                    }
                };
                egui::Image::new((tex.id(), art_rect.size())).corner_radius(8).paint_at(ui, art_rect);
            }
            None => {
                ui.ctx().data_mut(|d| d.remove::<(u64, egui::TextureHandle)>(tid));
                ui.painter().rect_filled(art_rect, 8.0, crate::ink::white_alpha(18));
                ui.painter().text(art_rect.center(), egui::Align2::CENTER_CENTER, "\u{E8D6}", egui::FontId::proportional(side * 0.4), crate::ink::gray(170));
            }
        }
        // Right side: app, title, artist, controls, progress; centred vertically.
        let x0 = art_rect.max.x + 14.0;
        let w = (full.max.x - x0 - 4.0).max(40.0);
        let block_h = 15.0 + 22.0 + if m.artist.is_empty() { 0.0 } else { 19.0 } + 44.0 + if m.timeline.is_some() { 22.0 } else { 0.0 };
        let mut y = (full.center().y - block_h / 2.0).max(full.min.y);
        let p = ui.painter().with_clip_rect(egui::Rect::from_min_max(Pos2::new(x0, full.min.y), full.max));
        let line = |p: &egui::Painter, y: f32, text: &str, size: f32, color: Color32| {
            let g = p.layout_no_wrap(text.to_string(), egui::FontId::proportional(size), color);
            p.galley(Pos2::new(x0, y), g, color);
        };
        line(&p, y, &app_label(&m.app).to_uppercase(), 10.5, crate::ink::gray(150));
        y += 15.0;
        line(&p, y, if m.title.is_empty() { t("(no title)") } else { &m.title }, 16.0, crate::ink::strong());
        y += 22.0;
        if !m.artist.is_empty() {
            line(&p, y, &m.artist, 13.0, crate::ink::gray(200));
            y += 19.0;
        }
        // Controls: previous, a round play/pause button, next.
        let cy = y + 22.0;
        let mid = Pos2::new(x0 + 58.0, cy);
        let button = |ui: &mut Ui, c: Pos2, r: f32, idn: &str| -> egui::Response {
            ui.interact(egui::Rect::from_center_size(c, egui::vec2(r * 2.0, r * 2.0)), ui.id().with(("media", idn)), Sense::click())
        };
        let prev = button(ui, Pos2::new(mid.x - 44.0, cy), 15.0, "prev").on_hover_text(t("Previous"));
        let play = button(ui, mid, 19.0, "play").on_hover_text(if m.playing { t("Pause") } else { t("Play") });
        let next = button(ui, Pos2::new(mid.x + 44.0, cy), 15.0, "next").on_hover_text(t("Next"));
        let pa = ui.painter();
        let dim = |r: &egui::Response| if r.hovered() { crate::ink::strong() } else { crate::ink::gray(210) };
        pa.text(Pos2::new(mid.x - 44.0, cy), egui::Align2::CENTER_CENTER, "\u{E892}", egui::FontId::proportional(17.0), dim(&prev));
        pa.text(Pos2::new(mid.x + 44.0, cy), egui::Align2::CENTER_CENTER, "\u{E893}", egui::FontId::proportional(17.0), dim(&next));
        pa.circle_filled(mid, if play.hovered() { 19.0 } else { 18.0 }, crate::ink::strong());
        pa.text(mid + egui::vec2(if m.playing { 0.0 } else { 1.5 }, 0.0), egui::Align2::CENTER_CENTER, if m.playing { "\u{E769}" } else { "\u{E768}" }, egui::FontId::proportional(16.0), Color32::from_rgb(20, 20, 24));
        if prev.clicked() {
            media_command(0, m.app.clone());
        }
        if play.clicked() {
            media_command(1, m.app.clone());
        }
        if next.clicked() {
            media_command(2, m.app.clone());
        }
        y += 44.0;
        // Progress: where in the song, moving on by itself while playing.
        let mut repaint = Duration::from_secs(2);
        if let Some((pos, end, read_at)) = m.timeline {
            let now = if m.playing { pos + read_at.elapsed().as_secs_f64() } else { pos };
            let now = now.min(end);
            let bar = egui::Rect::from_min_size(Pos2::new(x0, y + 4.0), egui::vec2(w, 4.0));
            pa.rect_filled(bar, 2.0, crate::ink::white_alpha(40));
            let mut done = bar;
            done.set_width(bar.width() * (now / end).clamp(0.0, 1.0) as f32);
            pa.rect_filled(done, 2.0, crate::ink::gray(230));
            pa.text(Pos2::new(x0, y + 10.0), egui::Align2::LEFT_TOP, mmss(now), egui::FontId::proportional(10.5), crate::ink::gray(160));
            pa.text(Pos2::new(x0 + w, y + 10.0), egui::Align2::RIGHT_TOP, mmss(end), egui::FontId::proportional(10.5), crate::ink::gray(160));
            if m.playing {
                repaint = Duration::from_secs(1); // the seconds move on
            }
        }
        WidgetOutput { changed: false, repaint_after: Some(repaint) }
    }
}
