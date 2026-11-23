//! The timer widget: a countdown (presets or any minutes) and a Pomodoro mode (25 minutes of
//! work, 5 of rest, a long rest every fourth time). When the time is up it chimes and the widget
//! flashes until clicked. The end time is saved, so a running timer survives a restart. It only
//! redraws while running (once a second).

use crate::i18n::{t, tf};
use crate::widgets::{DeskWidget, WidgetOutput};
use egui::{Color32, Pos2, Sense, Ui};
use serde_json::{json, Value};
use std::time::Duration;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn chime() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Diagnostics::Debug::MessageBeep;
        use windows::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK;
        let _ = MessageBeep(MB_ICONASTERISK);
    }
}

fn mmss(ms: i64) -> String {
    let s = (ms.max(0) + 999) / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

pub struct Timer;

impl DeskWidget for Timer {
    fn ui(&self, ui: &mut Ui, data: &mut Value) -> WidgetOutput {
        let mut out = WidgetOutput::default();
        // Saved state: length (ms), end (epoch ms, 0 = stopped), left when paused (ms), done flag,
        // pomodoro on and which round.
        let len = data["len"].as_i64().unwrap_or(25 * 60_000).max(1000);
        let end = data["end"].as_i64().unwrap_or(0);
        let paused = data["paused"].as_i64().unwrap_or(0);
        let pomodoro = data["pomodoro"].as_bool().unwrap_or(false);
        let round = data["round"].as_i64().unwrap_or(0);
        let mut done = data["done"].as_bool().unwrap_or(false);
        let now = now_ms();
        let running = end > 0;
        let left = if running { end - now } else if paused > 0 { paused } else { len };
        if running && left <= 0 {
            // Time is up.
            chime();
            done = true;
            data["done"] = json!(true);
            data["end"] = json!(0);
            data["paused"] = json!(0);
            if pomodoro {
                // Next: rest after work (long rest every 4th), work after rest.
                let next_round = round + 1;
                let work = next_round % 2 == 0;
                let next_len = if work { 25 } else if (next_round / 2) % 4 == 3 { 15 } else { 5 } as i64 * 60_000;
                data["round"] = json!(next_round);
                data["len"] = json!(next_len);
            }
            out.changed = true;
        }
        let full = ui.max_rect();
        let flash = done && (now / 500) % 2 == 0;
        if done {
            ui.painter().rect_filled(full, 6.0, Color32::from_rgba_unmultiplied(230, 90, 70, if flash { 90 } else { 40 }));
        }
        let label = if pomodoro {
            if round % 2 == 0 { t("Work") } else { t("Break") }
        } else if done {
            t("Time's up!")
        } else {
            t("Timer")
        };
        let p = ui.painter();
        p.text(Pos2::new(full.center().x, full.min.y + 12.0), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(12.5), crate::ink::gray(200));
        let shown = if running { left } else if paused > 0 { paused } else { data["len"].as_i64().unwrap_or(len) };
        let big = (full.height() * 0.32).clamp(22.0, 54.0);
        p.text(Pos2::new(full.center().x, full.min.y + 18.0 + big * 0.7), egui::Align2::CENTER_CENTER, mmss(shown), egui::FontId::proportional(big), crate::ink::strong());
        // Progress ring would be nice; a thin bar is lighter.
        let bar_y = full.min.y + 22.0 + big * 1.35;
        let bar = egui::Rect::from_min_size(Pos2::new(full.min.x + 8.0, bar_y), egui::vec2(full.width() - 16.0, 3.0));
        p.rect_filled(bar, 1.5, crate::ink::white_alpha(35));
        let total = data["len"].as_i64().unwrap_or(len).max(1) as f32;
        let mut fill = bar;
        fill.set_width(bar.width() * (1.0 - shown as f32 / total).clamp(0.0, 1.0));
        p.rect_filled(fill, 1.5, Color32::from_rgb(110, 190, 255));
        // Buttons: start / pause, reset; presets below.
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(Pos2::new(full.min.x, bar_y + 8.0), full.max)).layout(egui::Layout::top_down(egui::Align::Center)));
        child.horizontal(|ui| {
            let start_label = if running { t("Pause") } else if paused > 0 { t("Resume") } else { t("Start") };
            if ui.button(start_label).clicked() {
                if running {
                    data["paused"] = json!((end - now).max(0));
                    data["end"] = json!(0);
                } else {
                    let l = if paused > 0 { paused } else { data["len"].as_i64().unwrap_or(len) };
                    data["end"] = json!(now + l);
                    data["paused"] = json!(0);
                    data["done"] = json!(false);
                }
                out.changed = true;
            }
            if ui.button(t("Reset")).clicked() {
                data["end"] = json!(0);
                data["paused"] = json!(0);
                data["done"] = json!(false);
                if pomodoro {
                    data["round"] = json!(0);
                    data["len"] = json!(25 * 60_000);
                }
                out.changed = true;
            }
        });
        if !pomodoro && !running && full.height() > 150.0 {
            child.horizontal(|ui| {
                for m in [5i64, 10, 25] {
                    if ui.small_button(tf("{} min", &[&m])).clicked() {
                        data["len"] = json!(m * 60_000);
                        data["paused"] = json!(0);
                        data["done"] = json!(false);
                        out.changed = true;
                    }
                }
            });
        }
        // A click anywhere stops the flashing.
        if done && ui.interact(full, ui.id().with("timer-ack"), Sense::click()).clicked() {
            data["done"] = json!(false);
            out.changed = true;
        }
        out.repaint_after = if running {
            Some(Duration::from_millis((left % 1000).clamp(50, 1000) as u64))
        } else if done {
            Some(Duration::from_millis(500))
        } else {
            None
        };
        out
    }

    fn has_settings(&self) -> bool {
        true
    }

    fn settings(&self, ui: &mut Ui, data: &mut Value) -> bool {
        let mut changed = false;
        let mut pomodoro = data["pomodoro"].as_bool().unwrap_or(false);
        if ui.checkbox(&mut pomodoro, t("Pomodoro: 25 minutes of work, 5 of rest, a longer rest every fourth time")).changed() {
            data["pomodoro"] = json!(pomodoro);
            data["round"] = json!(0);
            data["len"] = json!(25 * 60_000);
            data["end"] = json!(0);
            data["paused"] = json!(0);
            changed = true;
        }
        if !pomodoro {
            let mut mins = (data["len"].as_i64().unwrap_or(25 * 60_000) / 60_000).max(1);
            ui.horizontal(|ui| {
                ui.label(t("Countdown length"));
                if ui.add(egui::DragValue::new(&mut mins).range(1..=600).suffix(" min")).changed() {
                    data["len"] = json!(mins * 60_000);
                    data["paused"] = json!(0);
                    changed = true;
                }
            });
        }
        changed
    }
}
