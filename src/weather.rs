//! The weather widget: Open-Meteo (free, no account, no key). The place is chosen in the widget's
//! settings (looked up by name); the forecast is read on a helper thread every 30 minutes while a
//! weather widget is shown.

use crate::i18n::t;
use crate::widgets::{DeskWidget, WidgetOutput};
use egui::{Color32, Pos2, Ui};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[derive(Clone, Default)]
pub struct Forecast {
    pub temp: f64,
    pub code: i64,
    pub wind: f64,
    pub is_day: bool,
    /// The coming days: (date, code, max, min).
    pub days: Vec<(chrono::NaiveDate, i64, f64, f64)>,
}

struct Slot {
    at: Option<Instant>,
    data: Option<Forecast>,
    error: Option<String>,
    busy: bool,
}

/// Forecasts by "lat,lon,unit".
static CACHE: Mutex<Option<HashMap<String, Slot>>> = Mutex::new(None);
/// Places found for the name typed in the settings: (query, results or error).
static FOUND: Mutex<Option<(String, Result<Vec<(String, f64, f64)>, String>)>> = Mutex::new(None);

/// A weather code (WMO) -> (symbol, short description). The symbols come from the emoji font
/// built into the program, so they are always there.
pub fn describe(code: i64, day: bool) -> (&'static str, &'static str) {
    match code {
        0 => (if day { "\u{2600}" } else { "\u{1F319}" }, t("Clear")),
        1 | 2 => (if day { "\u{26C5}" } else { "\u{2601}" }, t("Partly cloudy")),
        3 => ("\u{2601}", t("Cloudy")),
        45 | 48 => ("\u{2601}", t("Fog")),
        51..=57 => ("\u{2614}", t("Drizzle")),
        61..=67 | 80..=82 => ("\u{2614}", t("Rain")),
        71..=77 | 85 | 86 => ("\u{2744}", t("Snow")),
        95..=99 => ("\u{26A1}", t("Thunderstorm")),
        _ => ("\u{2601}", ""),
    }
}

fn fetch(lat: f64, lon: f64, fahrenheit: bool) -> Result<Forecast, String> {
    let unit = if fahrenheit { "&temperature_unit=fahrenheit&wind_speed_unit=mph" } else { "" };
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat:.4}&longitude={lon:.4}&current=temperature_2m,weather_code,wind_speed_10m,is_day&daily=weather_code,temperature_2m_max,temperature_2m_min&timezone=auto&forecast_days=5{unit}"
    );
    let v: Value = serde_json::from_str(&crate::net::get_text(&url)?).map_err(|_| "the weather service answered strangely".to_string())?;
    let c = &v["current"];
    let d = &v["daily"];
    let mut days = Vec::new();
    if let (Some(ts), Some(codes), Some(mx), Some(mn)) = (d["time"].as_array(), d["weather_code"].as_array(), d["temperature_2m_max"].as_array(), d["temperature_2m_min"].as_array()) {
        for i in 0..ts.len().min(codes.len()).min(mx.len()).min(mn.len()) {
            if let Some(date) = ts[i].as_str().and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()) {
                days.push((date, codes[i].as_i64().unwrap_or(-1), mx[i].as_f64().unwrap_or(0.0), mn[i].as_f64().unwrap_or(0.0)));
            }
        }
    }
    Ok(Forecast {
        temp: c["temperature_2m"].as_f64().ok_or("no temperature in the answer")?,
        code: c["weather_code"].as_i64().unwrap_or(-1),
        wind: c["wind_speed_10m"].as_f64().unwrap_or(0.0),
        is_day: c["is_day"].as_i64().unwrap_or(1) == 1,
        days,
    })
}

/// The forecast known now for a place; starts a download when there is none or it is 30 min old.
fn forecast(lat: f64, lon: f64, fahrenheit: bool) -> (Option<Forecast>, Option<String>) {
    let key = format!("{lat:.3},{lon:.3},{fahrenheit}");
    let Ok(mut g) = CACHE.lock() else { return (None, None) };
    let map = g.get_or_insert_with(HashMap::new);
    let slot = map.entry(key.clone()).or_insert(Slot { at: None, data: None, error: None, busy: false });
    let wait = if slot.error.is_some() { Duration::from_secs(300) } else { Duration::from_secs(1800) };
    if !slot.busy && slot.at.map(|t| t.elapsed() >= wait).unwrap_or(true) {
        slot.busy = true;
        let _ = std::thread::Builder::new().name("weather".into()).spawn(move || {
            let r = std::panic::catch_unwind(|| fetch(lat, lon, fahrenheit)).unwrap_or_else(|_| Err("weather data could not be read".into()));
            if let Ok(mut g) = CACHE.lock() {
                if let Some(s) = g.get_or_insert_with(HashMap::new).get_mut(&key) {
                    s.busy = false;
                    s.at = Some(Instant::now());
                    match r {
                        Ok(f) => {
                            s.data = Some(f);
                            s.error = None;
                        }
                        Err(e) => s.error = Some(e),
                    }
                }
            }
            #[cfg(windows)]
            crate::runloop::wake_after(Duration::ZERO);
        });
    }
    (slot.data.clone(), slot.error.clone())
}

/// Looks up places by name on a helper thread (results in FOUND).
fn search_places(q: String) {
    let _ = std::thread::Builder::new().name("weather-place".into()).spawn(move || {
        let lang = match crate::i18n::lang() {
            crate::i18n::Lang::Es => "es",
            crate::i18n::Lang::Ca => "ca",
            crate::i18n::Lang::En => "en",
        };
        let url = format!("https://geocoding-api.open-meteo.com/v1/search?name={}&count=6&language={lang}&format=json", crate::net::url_encode(q.trim()));
        let r = crate::net::get_text(&url).and_then(|txt| {
            let v: Value = serde_json::from_str(&txt).map_err(|_| "the place service answered strangely".to_string())?;
            Ok(v["results"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|p| {
                            let name = p["name"].as_str()?;
                            let region = [p["admin1"].as_str(), p["country"].as_str()].into_iter().flatten().collect::<Vec<_>>().join(", ");
                            let label = if region.is_empty() { name.to_string() } else { format!("{name} ({region})") };
                            Some((label, p["latitude"].as_f64()?, p["longitude"].as_f64()?))
                        })
                        .collect()
                })
                .unwrap_or_default())
        });
        if let Ok(mut f) = FOUND.lock() {
            *f = Some((q, r));
        }
        #[cfg(windows)]
        crate::runloop::wake_after(Duration::ZERO);
    });
}

pub struct Weather;

impl DeskWidget for Weather {
    fn ui(&self, ui: &mut Ui, data: &mut Value) -> WidgetOutput {
        let out = WidgetOutput { changed: false, repaint_after: Some(Duration::from_secs(600)) };
        let (Some(lat), Some(lon)) = (data["lat"].as_f64(), data["lon"].as_f64()) else {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(t("Right-click here and choose \"Widget settings…\" to pick a place.")).size(13.0).color(Color32::from_gray(220)));
            return out;
        };
        let fahrenheit = data["fahrenheit"].as_bool().unwrap_or(false);
        let place = data["place"].as_str().unwrap_or("").to_string();
        let (f, err) = forecast(lat, lon, fahrenheit);
        let Some(f) = f else {
            let text = err.unwrap_or_else(|| t("Loading…").to_string());
            ui.label(egui::RichText::new(text).size(12.0).color(Color32::from_gray(170)));
            return WidgetOutput { changed: false, repaint_after: Some(Duration::from_secs(2)) };
        };
        let unit = if fahrenheit { "°F" } else { "°C" };
        let (icon, what) = describe(f.code, f.is_day);
        let full = ui.max_rect();
        let p = ui.painter();
        // Now: big icon and temperature, place and description.
        let icon_c = Pos2::new(full.min.x + 30.0, full.min.y + 34.0);
        p.text(icon_c, egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(40.0), Color32::from_rgb(255, 214, 110));
        p.text(Pos2::new(full.min.x + 62.0, full.min.y + 34.0), egui::Align2::LEFT_CENTER, format!("{:.0}{unit}", f.temp), egui::FontId::proportional(34.0), Color32::WHITE);
        let right_x = full.max.x - 4.0;
        p.text(Pos2::new(right_x, full.min.y + 18.0), egui::Align2::RIGHT_CENTER, &place, egui::FontId::proportional(13.0), Color32::from_gray(225));
        p.text(Pos2::new(right_x, full.min.y + 36.0), egui::Align2::RIGHT_CENTER, what, egui::FontId::proportional(12.0), Color32::from_gray(190));
        let wind = if fahrenheit { format!("{:.0} mph", f.wind) } else { format!("{:.0} km/h", f.wind) };
        p.text(Pos2::new(right_x, full.min.y + 52.0), egui::Align2::RIGHT_CENTER, wind, egui::FontId::proportional(11.0), Color32::from_gray(160));
        // The coming days, as columns.
        let days: Vec<_> = f.days.iter().take(5).collect();
        if !days.is_empty() && full.height() > 110.0 {
            let top = full.min.y + 76.0;
            let cw = full.width() / days.len() as f32;
            for (i, (date, code, mx, mn)) in days.iter().enumerate() {
                use chrono::Datelike;
                let cx = full.min.x + cw * (i as f32 + 0.5);
                let name = if i == 0 { t("Today").to_string() } else { crate::i18n::weekday_short(date.weekday().num_days_from_monday()).to_string() };
                p.text(Pos2::new(cx, top), egui::Align2::CENTER_TOP, name, egui::FontId::proportional(11.5), Color32::from_gray(190));
                p.text(Pos2::new(cx, top + 18.0), egui::Align2::CENTER_TOP, describe(*code, true).0, egui::FontId::proportional(18.0), Color32::from_rgb(255, 214, 110));
                p.text(Pos2::new(cx, top + 42.0), egui::Align2::CENTER_TOP, format!("{mx:.0}° {mn:.0}°"), egui::FontId::proportional(11.5), Color32::from_gray(220));
            }
        }
        out
    }

    fn has_settings(&self) -> bool {
        true
    }

    fn settings(&self, ui: &mut Ui, data: &mut Value) -> bool {
        let mut changed = false;
        ui.label(egui::RichText::new(t("Place")).strong());
        if let Some(p) = data["place"].as_str() {
            ui.label(egui::RichText::new(p).size(14.0));
        }
        let qid = ui.make_persistent_id("weather-query");
        let mut q: String = ui.ctx().data(|d| d.get_temp(qid)).unwrap_or_default();
        let mut go = false;
        ui.horizontal(|ui| {
            let r = ui.add(egui::TextEdit::singleline(&mut q).hint_text(t("Type a town or city")).desired_width(300.0));
            go |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            go |= ui.button(t("Search")).clicked();
        });
        if go && q.trim().len() >= 2 {
            search_places(q.clone());
        }
        ui.ctx().data_mut(|d| d.insert_temp(qid, q.clone()));
        let found = FOUND.lock().ok().and_then(|f| f.clone());
        if let Some((fq, r)) = found {
            match r {
                Ok(list) if list.is_empty() => {
                    ui.label(crate::i18n::tf("Nothing found for \"{}\".", &[&fq]));
                }
                Ok(list) => {
                    for (label, lat, lon) in list {
                        if ui.button(&label).clicked() {
                            let short = label.split(" (").next().unwrap_or(&label).to_string();
                            data["place"] = json!(short);
                            data["lat"] = json!(lat);
                            data["lon"] = json!(lon);
                            changed = true;
                            if let Ok(mut f) = FOUND.lock() {
                                *f = None;
                            }
                        }
                    }
                }
                Err(e) => {
                    ui.label(egui::RichText::new(e).color(Color32::from_rgb(255, 190, 130)));
                }
            }
        }
        ui.separator();
        let mut fahrenheit = data["fahrenheit"].as_bool().unwrap_or(false);
        ui.horizontal(|ui| {
            if ui.radio(!fahrenheit, "°C").clicked() && fahrenheit {
                fahrenheit = false;
                changed = true;
            }
            if ui.radio(fahrenheit, "°F").clicked() && !fahrenheit {
                fahrenheit = true;
                changed = true;
            }
        });
        data["fahrenheit"] = json!(fahrenheit);
        ui.label(egui::RichText::new(t("Weather data by Open-Meteo.com, updated every 30 minutes while the widget is shown.")).size(11.0).color(Color32::from_gray(150)));
        changed
    }
}
