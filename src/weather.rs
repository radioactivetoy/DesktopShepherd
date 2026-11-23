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
    pub feels: f64,
    pub code: i64,
    pub wind: f64,
    /// Where the wind comes from, in degrees (0 = north).
    pub wind_dir: f64,
    pub humidity: f64,
    pub is_day: bool,
    /// The coming days.
    pub days: Vec<Day>,
    /// The coming hours: (local time, code, temperature, is day, chance of rain %).
    pub hours: Vec<(chrono::NaiveDateTime, i64, f64, bool, f64)>,
}

#[derive(Clone)]
pub struct Day {
    pub date: chrono::NaiveDate,
    pub code: i64,
    pub max: f64,
    pub min: f64,
    /// Chance of rain, %.
    pub rain: f64,
    pub uv: f64,
    pub sunrise: Option<chrono::NaiveDateTime>,
    pub sunset: Option<chrono::NaiveDateTime>,
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

// ---- Icons (Meteocons by Bas Milius, MIT licence: assets/meteocons/LICENSE) -------------------

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../assets/meteocons/", $name, ".png")) as &[u8])),*]
    };
}

static ICONS: &[(&str, &[u8])] = icons![
    "clear-day", "clear-night", "partly-cloudy-day", "partly-cloudy-night", "overcast-day", "overcast-night", "overcast",
    "cloudy", "fog-day", "fog-night", "fog", "drizzle", "partly-cloudy-day-drizzle", "partly-cloudy-night-drizzle", "rain",
    "partly-cloudy-day-rain", "partly-cloudy-night-rain", "snow", "partly-cloudy-day-snow", "partly-cloudy-night-snow", "sleet",
    "partly-cloudy-day-sleet", "partly-cloudy-night-sleet", "thunderstorms-day", "thunderstorms-night", "thunderstorms-day-rain",
    "thunderstorms-night-rain", "hail", "humidity", "wind", "raindrop", "raindrops", "umbrella", "sunrise", "sunset",
    "uv-index", "thermometer", "barometer", "not-available",
];

/// The icon picture for a name (decoded once per window, kept by egui).
fn icon(ctx: &egui::Context, name: &str) -> Option<egui::TextureHandle> {
    let id = egui::Id::new(("meteocon", name));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(t);
    }
    let bytes = ICONS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)?;
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
    let t = ctx.load_texture(format!("meteocon:{name}"), ci, egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, t.clone()));
    Some(t)
}

fn draw_icon(ui: &Ui, name: &str, center: Pos2, size: f32) {
    if let Some(t) = icon(ui.ctx(), name) {
        let r = egui::Rect::from_center_size(center, egui::vec2(size, size));
        ui.painter().image(t.id(), r, egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
    }
}

/// A weather code (WMO) -> (icon name, short description).
pub fn describe(code: i64, day: bool) -> (&'static str, &'static str) {
    let dn = |d: &'static str, n: &'static str| if day { d } else { n };
    match code {
        0 => (dn("clear-day", "clear-night"), t("Clear")),
        1 => (dn("partly-cloudy-day", "partly-cloudy-night"), t("Mostly clear")),
        2 => (dn("partly-cloudy-day", "partly-cloudy-night"), t("Partly cloudy")),
        3 => ("overcast", t("Cloudy")),
        45 | 48 => (dn("fog-day", "fog-night"), t("Fog")),
        51..=55 => ("drizzle", t("Drizzle")),
        56 | 57 | 66 | 67 => ("sleet", t("Sleet")),
        61..=65 => ("rain", t("Rain")),
        71..=77 => ("snow", t("Snow")),
        80..=82 => (dn("partly-cloudy-day-rain", "partly-cloudy-night-rain"), t("Showers")),
        85 | 86 => (dn("partly-cloudy-day-snow", "partly-cloudy-night-snow"), t("Snow showers")),
        95 => (dn("thunderstorms-day", "thunderstorms-night"), t("Thunderstorm")),
        96 | 99 => (dn("thunderstorms-day-rain", "thunderstorms-night-rain"), t("Thunderstorm")),
        _ => ("not-available", ""),
    }
}

fn fetch(lat: f64, lon: f64, fahrenheit: bool) -> Result<Forecast, String> {
    let unit = if fahrenheit { "&temperature_unit=fahrenheit&wind_speed_unit=mph" } else { "" };
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat:.4}&longitude={lon:.4}\
         &current=temperature_2m,apparent_temperature,relative_humidity_2m,weather_code,wind_speed_10m,wind_direction_10m,is_day\
         &hourly=temperature_2m,weather_code,is_day,precipitation_probability\
         &daily=weather_code,temperature_2m_max,temperature_2m_min,precipitation_probability_max,uv_index_max,sunrise,sunset\
         &timezone=auto&forecast_days=7&forecast_hours=30{unit}"
    );
    let v: Value = serde_json::from_str(&crate::net::get_text(&url)?).map_err(|_| "the weather service answered strangely".to_string())?;
    let c = &v["current"];
    let time = |x: &Value| x.as_str().and_then(|s| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M").ok());
    let num = |a: &Value, i: usize| a.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0);
    let d = &v["daily"];
    let mut days = Vec::new();
    if let Some(ts) = d["time"].as_array() {
        for (i, day) in ts.iter().enumerate() {
            let Some(date) = day.as_str().and_then(|s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()) else { continue };
            days.push(Day {
                date,
                code: d["weather_code"].get(i).and_then(|x| x.as_i64()).unwrap_or(-1),
                max: num(&d["temperature_2m_max"], i),
                min: num(&d["temperature_2m_min"], i),
                rain: num(&d["precipitation_probability_max"], i),
                uv: num(&d["uv_index_max"], i),
                sunrise: d["sunrise"].get(i).and_then(time),
                sunset: d["sunset"].get(i).and_then(time),
            });
        }
    }
    let h = &v["hourly"];
    let now = time(&c["time"]);
    let mut hours = Vec::new();
    if let Some(ts) = h["time"].as_array() {
        for (i, x) in ts.iter().enumerate() {
            let Some(at) = time(x) else { continue };
            // From the current hour on.
            if now.map(|n| at + chrono::Duration::minutes(59) < n).unwrap_or(false) {
                continue;
            }
            let day = h["is_day"].get(i).and_then(|x| x.as_i64()).unwrap_or(1) == 1;
            hours.push((at, h["weather_code"].get(i).and_then(|x| x.as_i64()).unwrap_or(-1), num(&h["temperature_2m"], i), day, num(&h["precipitation_probability"], i)));
        }
    }
    Ok(Forecast {
        temp: c["temperature_2m"].as_f64().ok_or("no temperature in the answer")?,
        feels: c["apparent_temperature"].as_f64().unwrap_or(f64::NAN),
        code: c["weather_code"].as_i64().unwrap_or(-1),
        wind: c["wind_speed_10m"].as_f64().unwrap_or(0.0),
        wind_dir: c["wind_direction_10m"].as_f64().unwrap_or(f64::NAN),
        humidity: c["relative_humidity_2m"].as_f64().unwrap_or(f64::NAN),
        is_day: c["is_day"].as_i64().unwrap_or(1) == 1,
        days,
        hours,
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

/// The widget's places: (name, lat, lon). A layout from before several places were possible
/// (a single "place"/"lat"/"lon") is moved into the list.
fn places(data: &mut Value) -> Vec<(String, f64, f64)> {
    if !data["places"].is_array() {
        let mut v = Vec::new();
        if let (Some(lat), Some(lon)) = (data["lat"].as_f64(), data["lon"].as_f64()) {
            v.push(json!({ "place": data["place"].as_str().unwrap_or(""), "lat": lat, "lon": lon }));
        }
        data["places"] = json!(v);
    }
    data["places"]
        .as_array()
        .map(|a| a.iter().filter_map(|p| Some((p["place"].as_str().unwrap_or("").to_string(), p["lat"].as_f64()?, p["lon"].as_f64()?))).collect())
        .unwrap_or_default()
}

pub struct Weather;

impl DeskWidget for Weather {
    fn ui(&self, ui: &mut Ui, data: &mut Value) -> WidgetOutput {
        let out = WidgetOutput { changed: false, repaint_after: Some(Duration::from_secs(600)) };
        let list = places(data);
        if list.is_empty() {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(t("Right-click here and choose \"Widget settings…\" to pick a place.")).size(13.0).color(crate::ink::gray(220)));
            return out;
        }
        // Several places: dots at the bottom switch between them.
        let sid = ui.make_persistent_id("weather-place");
        let mut idx: usize = ui.ctx().data(|d| d.get_temp(sid)).unwrap_or(0).min(list.len() - 1);
        if list.len() > 1 {
            let full = ui.max_rect();
            let gap = 14.0;
            let y = full.max.y - 6.0;
            let n = list.len();
            for (i, (name, _, _)) in list.iter().enumerate() {
                let c = Pos2::new(full.center().x + (i as f32 - (n as f32 - 1.0) / 2.0) * gap, y);
                let r = egui::Rect::from_center_size(c, egui::vec2(gap, 12.0));
                let resp = ui.interact(r, ui.id().with(("weather-dot", i)), egui::Sense::click()).on_hover_text(name);
                if resp.clicked() {
                    idx = i;
                    ui.ctx().data_mut(|d| d.insert_temp(sid, i));
                    ui.ctx().request_repaint();
                }
                let alpha = if i == idx { 235 } else if resp.hovered() { 160 } else { 90 };
                ui.painter().circle_filled(c, if i == idx { 3.5 } else { 3.0 }, crate::ink::white_alpha(alpha));
            }
        }
        let (place, lat, lon) = list[idx].clone();
        let fahrenheit = data["fahrenheit"].as_bool().unwrap_or(false);
        // The look chosen in the settings ("" = adapts to the box size).
        let view = data["view"].as_str().unwrap_or("").to_string();
        let (f, err) = forecast(lat, lon, fahrenheit);
        let Some(f) = f else {
            let text = err.unwrap_or_else(|| t("Loading…").to_string());
            ui.label(egui::RichText::new(text).size(12.0).color(crate::ink::gray(170)));
            return WidgetOutput { changed: false, repaint_after: Some(Duration::from_secs(2)) };
        };
        let unit = if fahrenheit { "F" } else { "C" };
        let (icon_name, what) = describe(f.code, f.is_day);
        let mut full = ui.max_rect();
        if list.len() > 1 {
            full.max.y -= 12.0; // room for the dots
        }
        let p = ui.painter().clone();
        let white = crate::ink::adapt(Color32::from_rgb(250, 250, 252));
        let soft = crate::ink::soft();
        let faint = crate::ink::faint();
        let blue = Color32::from_rgb(130, 190, 255);
        let line = |y: f32| p.line_segment([Pos2::new(full.min.x + 4.0, y), Pos2::new(full.max.x - 4.0, y)], egui::Stroke::new(1.0_f32, crate::ink::line()));
        let font = |s: f32| egui::FontId::proportional(s);
        let x0 = full.min.x;
        let y0 = full.min.y;
        let today = f.days.first();
        let day_name = |i: usize, d: &Day| {
            use chrono::Datelike;
            if i == 0 {
                t("Today").to_string()
            } else {
                crate::i18n::weekday_short(d.date.weekday().num_days_from_monday()).to_string()
            }
        };

        // Now: big icon, temperature and "feels like"; place, sky and today's range on the right.
        draw_icon(ui, icon_name, Pos2::new(x0 + 34.0, y0 + 34.0), 72.0);
        let temp = p.layout_no_wrap(format!("{:.0}°", f.temp), font(40.0), white);
        let temp_w = temp.size().x;
        p.galley_with_override_text_color(Pos2::new(x0 + 73.0, y0 + 5.0), temp.clone(), crate::ink::halo());
        p.galley(Pos2::new(x0 + 72.0, y0 + 4.0), temp, white);
        p.text(Pos2::new(x0 + 74.0 + temp_w, y0 + 14.0), egui::Align2::LEFT_TOP, unit, font(15.0), soft);
        if f.feels.is_finite() {
            p.text(Pos2::new(x0 + 74.0, y0 + 54.0), egui::Align2::LEFT_TOP, crate::i18n::tf("Feels like {}°", &[&format!("{:.0}", f.feels)]), font(12.0), faint);
        }
        let rx = full.max.x - 4.0;
        p.text(Pos2::new(rx + 1.0, y0 + 9.0), egui::Align2::RIGHT_TOP, &place, font(15.0), crate::ink::halo());
        p.text(Pos2::new(rx, y0 + 8.0), egui::Align2::RIGHT_TOP, &place, font(15.0), white);
        p.text(Pos2::new(rx, y0 + 30.0), egui::Align2::RIGHT_TOP, what, font(12.5), soft);
        if let Some(d) = today {
            p.text(Pos2::new(rx, y0 + 50.0), egui::Align2::RIGHT_TOP, format!("↑ {:.0}°   ↓ {:.0}°", d.max, d.min), font(12.0), faint);
        }
        let mut y = y0 + 76.0;

        // Details: humidity, wind, chance of rain, next sunrise or sunset, UV (as many as fit).
        let auto = !matches!(view.as_str(), "compact" | "standard" | "detailed");
        let show_details = if auto { full.max.y - y >= 110.0 } else { view == "detailed" && full.max.y - y >= 50.0 };
        if show_details {
            line(y);
            let wind = if fahrenheit { format!("{:.0} mph", f.wind) } else { format!("{:.0} km/h", f.wind) };
            let mut items: Vec<(&str, String, &str)> = Vec::new();
            if f.humidity.is_finite() {
                items.push(("humidity", format!("{:.0}%", f.humidity), t("Humidity")));
            }
            items.push(("wind", wind, t("Wind")));
            if let Some(d) = today {
                items.push(("umbrella", format!("{:.0}%", d.rain), t("Rain")));
                let now = chrono::Local::now().naive_local();
                let next_sun = match (d.sunrise, d.sunset) {
                    (Some(r), _) if now < r => Some(("sunrise", r, t("Sunrise"))),
                    (_, Some(s)) if now < s => Some(("sunset", s, t("Sunset"))),
                    _ => f.days.get(1).and_then(|n| n.sunrise).map(|r| ("sunrise", r, t("Sunrise"))),
                };
                if let Some((ic, at, label)) = next_sun {
                    items.push((ic, at.format("%H:%M").to_string(), label));
                }
                if d.uv >= 1.0 {
                    items.push(("uv-index", format!("{:.0}", d.uv), t("UV index")));
                }
            }
            let fit = ((full.width() / 80.0).floor() as usize).clamp(2, 5);
            items.truncate(fit);
            let cw = full.width() / items.len() as f32;
            for (i, (ic, value, label)) in items.iter().enumerate() {
                let cx = x0 + cw * (i as f32 + 0.5);
                draw_icon(ui, ic, Pos2::new(cx - 20.0, y + 24.0), 34.0);
                p.text(Pos2::new(cx - 2.0, y + 11.0), egui::Align2::LEFT_TOP, value, font(13.0), white);
                p.text(Pos2::new(cx - 2.0, y + 28.0), egui::Align2::LEFT_TOP, *label, font(10.5), faint);
                if *ic == "wind" && f.wind_dir.is_finite() {
                    // A small arrow showing where the wind blows to.
                    let a = (f.wind_dir as f32 + 180.0).to_radians();
                    let dir = egui::vec2(a.sin(), -a.cos());
                    let c = Pos2::new(cx - 20.0, y + 42.0);
                    let tip = c + dir * 5.0;
                    let back = c - dir * 5.0;
                    let side = egui::vec2(-dir.y, dir.x) * 3.0;
                    let st = egui::Stroke::new(1.3_f32, soft);
                    p.line_segment([back, tip], st);
                    p.line_segment([tip, tip - dir * 3.0 + side], st);
                    p.line_segment([tip, tip - dir * 3.0 - side], st);
                }
            }
            y += 52.0;
        }

        // The coming hours (when the box is tall enough).
        let show_hours = if auto { full.max.y - y >= 170.0 } else { view == "detailed" && full.max.y - y >= 90.0 };
        if show_hours && !f.hours.is_empty() {
            line(y);
            let n = ((full.width() / 52.0).floor() as usize).clamp(3, 8);
            let step = if f.hours.len() >= n * 3 { 3 } else if f.hours.len() >= n * 2 { 2 } else { 1 };
            let cw = full.width() / n as f32;
            for (i, (at, code, temp, day, rain)) in f.hours.iter().step_by(step).take(n).enumerate() {
                let cx = x0 + cw * (i as f32 + 0.5);
                let label = if i == 0 { t("Now").to_string() } else { at.format("%H:%M").to_string() };
                p.text(Pos2::new(cx, y + 8.0), egui::Align2::CENTER_TOP, label, font(11.0), faint);
                draw_icon(ui, describe(*code, *day).0, Pos2::new(cx, y + 38.0), 38.0);
                p.text(Pos2::new(cx, y + 56.0), egui::Align2::CENTER_TOP, format!("{temp:.0}°"), font(13.0), white);
                if *rain >= 20.0 {
                    p.text(Pos2::new(cx, y + 73.0), egui::Align2::CENTER_TOP, format!("{rain:.0}%"), font(10.0), blue);
                }
            }
            y += 90.0;
        }

        // The coming days.
        let days: Vec<&Day> = f.days.iter().collect();
        if !days.is_empty() && view != "compact" && full.max.y - y >= 60.0 {
            line(y);
            let room = full.max.y - y;
            if (auto && room > 120.0) || (view == "detailed" && room > 40.0) {
                // Rows with a bar for each day's range (enough height).
                let rows = (((room - 6.0) / 26.0).floor() as usize).clamp(1, days.len());
                let lo = days.iter().take(rows).map(|d| d.min).fold(f64::INFINITY, f64::min);
                let hi = days.iter().take(rows).map(|d| d.max).fold(f64::NEG_INFINITY, f64::max);
                let span = (hi - lo).max(1.0);
                let icon_x = x0 + full.width() * 0.27;
                let bar_l = x0 + full.width() * 0.50;
                let bar_r = full.max.x - 42.0;
                for (i, d) in days.iter().take(rows).enumerate() {
                    let cy = y + 17.0 + i as f32 * 26.0;
                    p.text(Pos2::new(x0 + 6.0, cy), egui::Align2::LEFT_CENTER, day_name(i, d), font(12.5), soft);
                    draw_icon(ui, describe(d.code, true).0, Pos2::new(icon_x, cy), 30.0);
                    if d.rain >= 20.0 {
                        p.text(Pos2::new(icon_x + 16.0, cy), egui::Align2::LEFT_CENTER, format!("{:.0}%", d.rain), font(10.0), blue);
                    }
                    p.text(Pos2::new(bar_l - 6.0, cy), egui::Align2::RIGHT_CENTER, format!("{:.0}°", d.min), font(12.0), faint);
                    p.text(Pos2::new(full.max.x - 6.0, cy), egui::Align2::RIGHT_CENTER, format!("{:.0}°", d.max), font(12.0), white);
                    let track = egui::Rect::from_min_max(Pos2::new(bar_l, cy - 2.0), Pos2::new(bar_r, cy + 2.0));
                    p.rect_filled(track, egui::CornerRadius::same(2), crate::ink::white_alpha(30));
                    let a = bar_l + ((d.min - lo) / span) as f32 * (bar_r - bar_l);
                    let b = bar_l + ((d.max - lo) / span) as f32 * (bar_r - bar_l);
                    let fill = egui::Rect::from_min_max(Pos2::new(a, cy - 2.0), Pos2::new(b.max(a + 4.0), cy + 2.0));
                    p.rect_filled(fill, egui::CornerRadius::same(2), Color32::from_rgb(255, 196, 90));
                }
            } else {
                // Columns (a short box).
                let n = ((full.width() / 60.0).floor() as usize).clamp(3, 7).min(days.len());
                let cw = full.width() / n as f32;
                for (i, d) in days.iter().take(n).enumerate() {
                    let cx = x0 + cw * (i as f32 + 0.5);
                    p.text(Pos2::new(cx, y + 6.0), egui::Align2::CENTER_TOP, day_name(i, d), font(11.5), faint);
                    draw_icon(ui, describe(d.code, true).0, Pos2::new(cx, y + 32.0), 34.0);
                    let mx = p.layout_no_wrap(format!("{:.0}°", d.max), font(12.0), white);
                    let mn = p.layout_no_wrap(format!(" {:.0}°", d.min), font(12.0), faint);
                    let mxw = mx.size().x;
                    let w = mxw + mn.size().x;
                    let ty = y + 48.0;
                    p.galley(Pos2::new(cx - w / 2.0, ty), mx, white);
                    p.galley(Pos2::new(cx - w / 2.0 + mxw, ty), mn, faint);
                }
            }
        }
        out
    }

    fn has_settings(&self) -> bool {
        true
    }

    fn settings(&self, ui: &mut Ui, data: &mut Value) -> bool {
        let mut changed = false;
        // The look: picking one also gives the box a size that fits it.
        ui.label(egui::RichText::new(t("Look")).strong());
        let view = data["view"].as_str().unwrap_or("").to_string();
        ui.horizontal_wrapped(|ui| {
            for (key, label, size) in [
                ("compact", t("Compact"), Some([300.0, 92.0])),
                ("standard", t("Standard"), Some([330.0, 170.0])),
                ("detailed", t("Detailed"), Some([340.0, 450.0])),
                ("", t("Adapts to the box size"), None),
            ] {
                if ui.radio(view == key, label).clicked() && view != key {
                    data["view"] = json!(key);
                    if let Some([w, h]) = size {
                        data["resize_to"] = json!([w, h]);
                    }
                    changed = true;
                }
            }
        });
        ui.separator();
        ui.label(egui::RichText::new(t("Places")).strong());
        let list = places(data);
        let mut remove = None;
        for (i, (name, _, _)) in list.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(name).size(14.0));
                if ui.small_button(t("Remove")).clicked() {
                    remove = Some(i);
                }
            });
        }
        if let Some(i) = remove {
            if let Some(a) = data["places"].as_array_mut() {
                if i < a.len() {
                    a.remove(i);
                    changed = true;
                }
            }
        }
        ui.label(egui::RichText::new(t("Add a place:")).size(12.5).color(crate::theme::soft()));
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
                            if let Some(a) = data["places"].as_array_mut() {
                                a.push(json!({ "place": short, "lat": lat, "lon": lon }));
                            }
                            changed = true;
                            if let Ok(mut f) = FOUND.lock() {
                                *f = None;
                            }
                        }
                    }
                }
                Err(e) => {
                    ui.label(egui::RichText::new(e).color(crate::theme::warn()));
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
        ui.label(egui::RichText::new(t("Weather data by Open-Meteo.com, updated every 30 minutes while the widget is shown.")).size(11.0).color(crate::theme::faint()));
        changed
    }
}
