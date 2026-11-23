//! Text colours for things drawn ON boxes. On a bright wallpaper the box being drawn switches to
//! dark ink (`set(true)` just before the box draws, `set(false)` after); menus and settings
//! panels never use this module (they have `theme.rs`).

use egui::Color32;
use std::sync::atomic::{AtomicBool, Ordering};

static DARK: AtomicBool = AtomicBool::new(false);

/// The box that draws next sits on a bright backdrop (dark ink).
pub fn set(dark: bool) {
    DARK.store(dark, Ordering::Relaxed);
}

pub fn is_dark() -> bool {
    DARK.load(Ordering::Relaxed)
}

fn pick(light: Color32, dark: Color32) -> Color32 {
    if is_dark() {
        dark
    } else {
        light
    }
}

/// Main text.
pub fn text() -> Color32 {
    pick(Color32::from_gray(230), Color32::from_gray(20))
}

/// Strongest text (white / black).
pub fn strong() -> Color32 {
    pick(Color32::WHITE, Color32::BLACK)
}

/// Secondary text.
pub fn soft() -> Color32 {
    pick(Color32::from_gray(222), Color32::from_gray(45))
}

/// Faint text and hints.
pub fn faint() -> Color32 {
    pick(Color32::from_gray(200), Color32::from_gray(65))
}

/// Thin lines and separators.
pub fn line() -> Color32 {
    pick(Color32::from_white_alpha(28), Color32::from_black_alpha(40))
}

/// The halo / shadow drawn 1 px behind text.
pub fn halo() -> Color32 {
    pick(Color32::from_black_alpha(200), Color32::from_white_alpha(150))
}

/// A light-on-glass colour mapped to its dark-ink equivalent when the box is on a bright
/// backdrop. Light greys/whites become dark greys (alpha kept); saturated colours stay, only
/// very light ones are darkened a little.
pub fn adapt(c: Color32) -> Color32 {
    if !is_dark() {
        return c;
    }
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    let mx = r.max(g).max(b) as i32;
    let mn = r.min(g).min(b) as i32;
    let lum = (0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32) as i32;
    if mx - mn < 28 {
        // A grey: mirror it (white -> black, 165 -> ~80, 230 -> ~20).
        if lum < 90 {
            return c; // already dark
        }
        let v = ((255 - lum) as f32 * 0.9 - 2.0).clamp(0.0, 255.0) as u8;
        return Color32::from_rgba_unmultiplied(v, v, v, a);
    }
    if lum > 190 {
        let k = 0.72;
        return Color32::from_rgba_unmultiplied((r as f32 * k) as u8, (g as f32 * k) as u8, (b as f32 * k) as u8, a);
    }
    c
}

/// `Color32::from_gray(v)` for text/lines on glass: mirrored to a dark grey on bright backdrops.
pub fn gray(v: u8) -> Color32 {
    adapt(Color32::from_gray(v))
}

/// `Color32::from_white_alpha(a)` for fills and lines: black on bright backdrops (a little stronger).
pub fn white_alpha(a: u8) -> Color32 {
    if is_dark() {
        Color32::from_black_alpha(((a as f32) * 1.4).min(255.0) as u8)
    } else {
        Color32::from_white_alpha(a)
    }
}

/// A label with a 1 px halo behind it (reads on busy or bright glass). Centred in the row like
/// `ui.label` would be in a centred layout; use inside `vertical_centered`.
pub fn halo_label(ui: &mut egui::Ui, text: String, size: f32, color: Color32) {
    let g = ui.painter().layout_no_wrap(text, egui::FontId::proportional(size), color);
    let (rect, _) = ui.allocate_exact_size(g.size(), egui::Sense::hover());
    ui.painter().galley_with_override_text_color(rect.min + egui::vec2(1.0, 1.0), g.clone(), halo());
    ui.painter().galley(rect.min, g, color);
}
