//! Light or dark look for menus and settings windows (the boxes keep their own colours). Follows
//! Windows' "app mode" (Settings > Personalization > Colors) unless the person chose one.

use egui::Color32;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// 0 = like Windows, 1 = dark, 2 = light.
static CHOICE: AtomicU8 = AtomicU8::new(0);
/// Windows' app mode is light (read at start and when Windows says its colours changed).
static WINDOWS_LIGHT: AtomicBool = AtomicBool::new(false);

/// Reads Windows' app mode again. Returns true if the look of menus and settings changed.
pub fn refresh() -> bool {
    let before = is_light();
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        let light = winreg::RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
            .and_then(|k| k.get_value::<u32, _>("AppsUseLightTheme"))
            .map(|v| v != 0)
            .unwrap_or(false);
        WINDOWS_LIGHT.store(light, Ordering::Relaxed);
    }
    before != is_light()
}

/// Sets the person's choice (0 = like Windows, 1 = dark, 2 = light). Returns true if the look changed.
pub fn set_choice(c: u8) -> bool {
    let before = is_light();
    CHOICE.store(c.min(2), Ordering::Relaxed);
    before != is_light()
}

/// Menus and settings windows are drawn light.
pub fn is_light() -> bool {
    match CHOICE.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => WINDOWS_LIGHT.load(Ordering::Relaxed),
    }
}

/// Picks the dark or the light colour.
fn pick(dark: Color32, light: Color32) -> Color32 {
    if is_light() {
        light
    } else {
        dark
    }
}

// ---- The palette of menus and settings windows ----------------------------------------------

/// Window background.
pub fn bg() -> Color32 {
    pick(Color32::from_rgb(32, 33, 36), Color32::from_rgb(243, 243, 243))
}
/// A window's top bar / side bar (a little darker than the background in dark mode).
pub fn bar() -> Color32 {
    pick(Color32::from_rgb(24, 25, 28), Color32::from_rgb(232, 232, 234))
}
/// The deepest background (lists, the settings window's content).
pub fn deep() -> Color32 {
    pick(Color32::from_rgb(16, 17, 19), Color32::from_rgb(250, 250, 250))
}
/// Menus, popups, small notes.
pub fn menu() -> Color32 {
    pick(Color32::from_rgb(44, 44, 46), Color32::from_rgb(249, 249, 249))
}
/// Cards and groups inside a window.
pub fn card() -> Color32 {
    pick(Color32::from_rgb(40, 40, 43), Color32::from_rgb(255, 255, 255))
}
/// Main text.
pub fn text() -> Color32 {
    pick(Color32::from_gray(235), Color32::from_gray(25))
}
/// Strong text (titles).
pub fn strong() -> Color32 {
    pick(Color32::WHITE, Color32::BLACK)
}
/// Secondary text.
pub fn soft() -> Color32 {
    pick(Color32::from_gray(190), Color32::from_gray(70))
}
/// Hints and small print.
pub fn faint() -> Color32 {
    pick(Color32::from_gray(150), Color32::from_gray(105))
}
/// Disabled text and icons.
pub fn disabled() -> Color32 {
    pick(Color32::from_gray(110), Color32::from_gray(160))
}
/// Thin lines and borders.
pub fn line() -> Color32 {
    pick(Color32::from_white_alpha(26), Color32::from_black_alpha(26))
}
/// A soft highlight (hovered row), drawn over the background.
pub fn hover() -> Color32 {
    pick(Color32::from_white_alpha(20), Color32::from_black_alpha(14))
}
/// A warning text colour.
pub fn warn() -> Color32 {
    pick(Color32::from_rgb(255, 190, 130), Color32::from_rgb(170, 80, 0))
}

/// The egui look for menus and settings windows, made from the dark look the boxes use.
pub fn panel_visuals(dark: &egui::Visuals) -> egui::Visuals {
    if !is_light() {
        return dark.clone();
    }
    use egui::Stroke;
    let mut v = egui::Visuals::light();
    v.window_fill = menu();
    v.panel_fill = bg();
    v.extreme_bg_color = Color32::WHITE;
    v.faint_bg_color = Color32::from_gray(238);
    v.window_corner_radius = dark.window_corner_radius;
    v.menu_corner_radius = dark.menu_corner_radius;
    v.window_stroke = Stroke::new(1.0_f32, Color32::from_black_alpha(30));
    v.popup_shadow = egui::Shadow { color: Color32::from_black_alpha(50), ..dark.popup_shadow };
    v.window_shadow = egui::Shadow { color: Color32::from_black_alpha(45), ..dark.window_shadow };
    v.selection.bg_fill = dark.hyperlink_color.linear_multiply(0.35);
    v.selection.stroke = Stroke::new(1.0_f32, Color32::BLACK);
    v.hyperlink_color = darker(dark.hyperlink_color);
    v.slider_trailing_fill = true;
    v.widgets = dark.widgets.clone();
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = bg();
    w.noninteractive.weak_bg_fill = bg();
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, Color32::from_black_alpha(22));
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, text());
    w.inactive.weak_bg_fill = Color32::from_rgb(253, 253, 253);
    w.inactive.bg_fill = Color32::from_rgb(220, 220, 222);
    w.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_black_alpha(30));
    w.inactive.fg_stroke = Stroke::new(1.0_f32, Color32::from_gray(30));
    w.hovered.weak_bg_fill = Color32::from_black_alpha(14);
    w.hovered.bg_fill = Color32::from_black_alpha(20);
    w.hovered.fg_stroke = Stroke::new(1.0_f32, Color32::BLACK);
    w.active.weak_bg_fill = Color32::from_black_alpha(24);
    w.active.bg_fill = Color32::from_black_alpha(30);
    w.active.fg_stroke = Stroke::new(1.0_f32, Color32::BLACK);
    w.open.weak_bg_fill = Color32::from_black_alpha(14);
    w.open.bg_fill = Color32::from_black_alpha(20);
    w.open.fg_stroke = Stroke::new(1.0_f32, Color32::BLACK);
    v
}

/// The accent colour made dark enough to read on a light background.
fn darker(c: Color32) -> Color32 {
    let f = |x: u8| (x as f32 * 0.62) as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}
