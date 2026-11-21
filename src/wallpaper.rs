//! "Glass" boxes without relying on window transparency (unreliable with some graphics
//! drivers): each box draws the piece of the desktop wallpaper that lies behind it, then its
//! tinted background on top. Boxes always sit on the desktop, so what is behind them is the
//! wallpaper. The picture is decoded once on a background thread and re-read only when
//! Windows changes it (checked every 10 s by file date).

use eframe::egui::{self, Color32, ColorImage, Pos2, Rect, TextureHandle, TextureOptions, Vec2};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, SystemTime};

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fit {
    Fill,
    Fit,
    Stretch,
    Center,
    Span,
}

struct Loaded {
    image: Option<ColorImage>,
    /// Top-left of a desktop capture in screen pixels (None = decoded from the wallpaper file).
    origin: Option<Pos2>,
    /// A tiny copy (64 px wide) for the average colour shown while a box is dragged.
    thumb: Option<(usize, usize, Vec<Color32>)>,
    fit: Fit,
    solid: Color32,
}

pub struct Wallpaper {
    rx: Receiver<Loaded>,
    tex: Option<TextureHandle>,
    thumb: Option<(usize, usize, Vec<Color32>)>,
    /// Full-size copy kept on the CPU, for building see-through snapshots of dragged boxes.
    full: Option<std::sync::Arc<ColorImage>>,
    origin: Option<Pos2>,
    size: Vec2,
    /// Changes every time a new desktop picture arrives (invalidates the per-box glass).
    generation: u64,
    /// Per-box frosted glass: box id -> (what it was made for, texture, blurred pixels).
    glass: std::collections::HashMap<u64, (GlassKey, TextureHandle, std::sync::Arc<ColorImage>)>,
    /// The whole desktop picture blurred once per strength in use (made in the background).
    blurred: std::collections::HashMap<u32, std::sync::Arc<ColorImage>>,
    /// When each blurred picture was last used (unused ones are freed to save memory).
    blurred_used: std::collections::HashMap<u32, u64>,
    /// Counts drawing passes (boxes ask for their glass on every pass).
    pass: u64,
    blur_tx: std::sync::mpsc::Sender<(u64, u32, ColorImage)>,
    blur_rx: Receiver<(u64, u32, ColorImage)>,
    blur_busy: bool,
    ctx: egui::Context,
    /// Small picture of the desktop for the layout preview (made only when shown).
    preview: Option<TextureHandle>,
    fit: Fit,
    solid: Color32,
}

fn wallpaper_file() -> Option<PathBuf> {
    // Windows keeps a copy of the current wallpaper here (also for slideshows / Spotlight).
    let p = dirs::config_dir()?.join("Microsoft\\Windows\\Themes\\TranscodedWallpaper");
    p.is_file().then_some(p)
}

fn read_style() -> (Fit, Color32) {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        let hk = winreg::RegKey::predef(HKEY_CURRENT_USER);
        let desk = hk.open_subkey("Control Panel\\Desktop").ok();
        let get = |k: &str| desk.as_ref().and_then(|d| d.get_value::<String, _>(k).ok()).unwrap_or_default();
        let fit = match get("WallpaperStyle").trim() {
            "6" => Fit::Fit,
            "2" => Fit::Stretch,
            "22" => Fit::Span,
            "0" if get("TileWallpaper").trim() != "1" => Fit::Center,
            _ => Fit::Fill,
        };
        let solid = hk
            .open_subkey("Control Panel\\Colors")
            .ok()
            .and_then(|c| c.get_value::<String, _>("Background").ok())
            .and_then(|s| {
                let v: Vec<u8> = s.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                (v.len() == 3).then(|| Color32::from_rgb(v[0], v[1], v[2]))
            })
            .unwrap_or(Color32::from_rgb(20, 24, 32));
        (fit, solid)
    }
    #[cfg(not(windows))]
    {
        (Fit::Fill, Color32::from_rgb(20, 24, 32))
    }
}

fn decode(path: &PathBuf) -> Option<ColorImage> {
    // The file has no extension: recognise the format from its bytes.
    let img = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?;
    // Never keep more than needed: limit to 2560 px wide.
    let img = if img.width() > 2560 { img.resize(2560, 2560 * img.height() / img.width().max(1), image::imageops::FilterType::Triangle) } else { img };
    let rgba = img.to_rgba8();
    Some(ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw()))
}

impl Wallpaper {
    /// `blur`: frosted-glass strength (pixels), changeable at any time.
    /// `capture`: copy the desktop as Windows draws it (needed only when boxes paint their own
    /// glass; DirectX boxes get their glass from Windows, so the copy is skipped to save memory).
    pub fn new(ctx: &egui::Context, blur: std::sync::Arc<std::sync::atomic::AtomicU32>, capture: bool) -> Self {
        let (tx, rx) = channel();
        let ctx_keep = ctx.clone();
        let ctx = ctx.clone();
        let _ = std::thread::Builder::new().name("wallpaper".into()).spawn(move || {
            let mut last: Option<(Option<SystemTime>, Fit, Color32, [i32; 4], bool, u32)> = None;
            loop {
                let file = wallpaper_file();
                let stamp = file.as_ref().and_then(|f| std::fs::metadata(f).ok()).and_then(|m| m.modified().ok());
                let (fit, solid) = read_style();
                let vs = crate::desktop::virtual_screen();
                let icons = crate::desktop::desktop_icons_visible();
                let radius = blur.load(std::sync::atomic::Ordering::Relaxed);
                let key = (stamp, fit, solid, [vs[0] as i32, vs[1] as i32, vs[2] as i32, vs[3] as i32], icons, radius);
                if last.as_ref() != Some(&key) {
                    // Let Windows finish showing a new wallpaper (and hiding icons) first.
                    std::thread::sleep(Duration::from_millis(1500));
                    // Preferred: the desktop exactly as Windows draws it (same colours and
                    // sharpness as what shows through while a box is dragged). With the
                    // desktop icons showing, the wallpaper file is used instead.
                    let captured = if !capture || crate::desktop::desktop_icons_visible() {
                        None
                    } else {
                        std::panic::catch_unwind(crate::desktop::capture_desktop).ok().flatten()
                    };
                    let (image, origin) = match captured {
                        Some((o, img)) => (Some(img), Some(o)),
                        // DirectX boxes: Windows draws the glass, so no wallpaper copy is kept at all.
                        None if !capture => (None, None),
                        None => (file.as_ref().and_then(|f| std::panic::catch_unwind(|| decode(f)).ok().flatten()), None),
                    };
                    // Frosted glass: blur the desktop picture once, here in the background.
                    let image = image.map(|mut img| {
                        if radius > 0 {
                            let r = if origin.is_some() { radius as usize } else { (radius as usize / 2).max(1) };
                            blur_image(&mut img, r);
                        }
                        img
                    });
                    let thumb = image.as_ref().map(make_thumb);
                    if tx.send(Loaded { image, origin, thumb, fit, solid }).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                    last = Some(key);
                }
                for _ in 0..10 {
                    std::thread::sleep(Duration::from_secs(1));
                    if blur.load(std::sync::atomic::Ordering::Relaxed) != radius {
                        break;
                    }
                }
            }
        });
        let (blur_tx, blur_rx) = channel();
        Self { rx, tex: None, thumb: None, full: None, origin: None, size: Vec2::ZERO, generation: 0, glass: Default::default(), blurred: Default::default(), blurred_used: Default::default(), pass: 0, blur_tx, blur_rx, blur_busy: false, ctx: ctx_keep, preview: None, fit: Fit::Fill, solid: Color32::from_rgb(20, 24, 32) }
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        self.pass += 1;
        while let Ok((generation, radius, img)) = self.blur_rx.try_recv() {
            self.blur_busy = false;
            if generation == self.generation {
                self.blurred_used.insert(radius, self.pass);
                self.blurred.insert(radius, std::sync::Arc::new(img));
                self.glass.clear();
            }
        }
        while let Ok(l) = self.rx.try_recv() {
            self.fit = l.fit;
            self.solid = l.solid;
            self.thumb = l.thumb;
            self.origin = l.origin;
            self.generation += 1;
            self.glass.clear();
            self.blurred.clear();
            match l.image {
                Some(img) => {
                    self.size = Vec2::new(img.size[0] as f32, img.size[1] as f32);
                    self.full = Some(std::sync::Arc::new(img.clone()));
                    // A desktop capture is cut into box-sized pieces as needed (see box_glass);
                    // only the wallpaper-file fallback needs a whole-picture texture.
                    self.tex = if l.origin.is_none() { Some(ctx.load_texture("wallpaper", img, TextureOptions::LINEAR)) } else { None };
                    self.preview = None;
                }
                None => {
                    self.tex = None;
                    self.full = None;
                }
            }
        }
    }

    /// Where the whole wallpaper picture lies on screen (points) for a monitor rectangle.
    fn image_rect(&self, monitor: Rect, virtual_screen: Rect, ppp: f32) -> Rect {
        if let Some(o) = self.origin {
            // A desktop capture covers all monitors at 1:1 pixels.
            return Rect::from_min_size(Pos2::new(o.x / ppp, o.y / ppp), self.size / ppp);
        }
        let (iw, ih) = (self.size.x.max(1.0), self.size.y.max(1.0));
        let area = if self.fit == Fit::Span { virtual_screen } else { monitor };
        match self.fit {
            Fit::Stretch => area,
            Fit::Center => Rect::from_center_size(area.center(), self.size),
            Fit::Fit => {
                let s = (area.width() / iw).min(area.height() / ih);
                Rect::from_center_size(area.center(), Vec2::new(iw * s, ih * s))
            }
            Fit::Fill | Fit::Span => {
                let s = (area.width() / iw).max(area.height() / ih);
                Rect::from_center_size(area.center(), Vec2::new(iw * s, ih * s))
            }
        }
    }

    /// Paints the wallpaper part found at `screen_rect` (screen points) into `local_rect`.
    pub fn paint(&self, painter: &egui::Painter, local_rect: Rect, screen_rect: Rect, ppp: f32) {
        painter.rect_filled(local_rect, 0.0, self.solid);
        let Some(tex) = &self.tex else { return };
        let (monitor, virt) = monitor_of(screen_rect, ppp);
        let img = self.image_rect(monitor, virt, ppp);
        let to_uv = |p: Pos2| Pos2::new((p.x - img.min.x) / img.width(), (p.y - img.min.y) / img.height());
        // Only the part that overlaps the picture (Center/Fit can leave solid-colour borders).
        let vis = screen_rect.intersect(img);
        if vis.width() <= 0.0 || vis.height() <= 0.0 {
            return;
        }
        let offset = local_rect.min - screen_rect.min;
        let dst = vis.translate(offset);
        painter.image(tex.id(), dst, Rect::from_min_max(to_uv(vis.min), to_uv(vis.max)), Color32::WHITE);
    }


    pub fn paint_preview(&mut self, painter: &egui::Painter, dst: Rect, screen: Rect, scale: f32) {
        painter.rect_filled(dst, 0.0, self.solid);
        if self.tex.is_none() && self.preview.is_none() {
            if let Some(full) = self.full.clone() {
                let small = half_size(&half_size(&full));
                self.preview = Some(self.ctx.load_texture("desktop-preview", small, TextureOptions::LINEAR));
            }
        }
        let Some(tex) = self.tex.as_ref().or(self.preview.as_ref()) else { return };
        let img = self.image_rect(screen, screen, crate::desktop::system_scale());
        let to_dst = |p: Pos2| dst.min + (p - screen.min) * scale;
        let vis = img.intersect(screen);
        let uv = Rect::from_min_max(
            Pos2::new((vis.min.x - img.min.x) / img.width(), (vis.min.y - img.min.y) / img.height()),
            Pos2::new((vis.max.x - img.min.x) / img.width(), (vis.max.y - img.min.y) / img.height()),
        );
        painter.image(tex.id(), Rect::from_min_max(to_dst(vis.min), to_dst(vis.max)), uv, Color32::from_gray(150));
    }
}

/// The monitor holding most of `r`, and the whole desktop, both in points.
fn monitor_of(r: Rect, ppp: f32) -> (Rect, Rect) {
    let vs = crate::desktop::virtual_screen();
    let virt = Rect::from_min_size(Pos2::new(vs[0], vs[1]) / ppp, Vec2::new(vs[2], vs[3]) / ppp);
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromRect, MONITORINFO, MONITOR_DEFAULTTONEAREST};
        let pr = RECT {
            left: (r.min.x * ppp) as i32,
            top: (r.min.y * ppp) as i32,
            right: (r.max.x * ppp) as i32,
            bottom: (r.max.y * ppp) as i32,
        };
        let hm = MonitorFromRect(&pr, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if GetMonitorInfoW(hm, &mut mi).as_bool() {
            let m = mi.rcMonitor;
            let mon = Rect::from_min_max(
                Pos2::new(m.left as f32, m.top as f32) / ppp,
                Pos2::new(m.right as f32, m.bottom as f32) / ppp,
            );
            return (mon, virt);
        }
    }
    let [w, h] = crate::desktop::primary_screen_points();
    (Rect::from_min_size(Pos2::ZERO, Vec2::new(w, h)), virt)
}

/// Shrinks the wallpaper to 64 px wide (box-filter average) for quick colour sampling.
fn make_thumb(img: &ColorImage) -> (usize, usize, Vec<Color32>) {
    let (w, h) = (img.size[0].max(1), img.size[1].max(1));
    let tw = 64usize;
    let th = (h * tw / w).max(1);
    let mut out = Vec::with_capacity(tw * th);
    for ty in 0..th {
        for tx in 0..tw {
            let (x0, x1) = (tx * w / tw, ((tx + 1) * w / tw).max(tx * w / tw + 1));
            let (y0, y1) = (ty * h / th, ((ty + 1) * h / th).max(ty * h / th + 1));
            let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
            for y in (y0..y1.min(h)).step_by(2) {
                for x in (x0..x1.min(w)).step_by(2) {
                    let c = img.pixels[y * w + x];
                    r += c.r() as u64;
                    g += c.g() as u64;
                    b += c.b() as u64;
                    n += 1;
                }
            }
            let n = n.max(1);
            out.push(Color32::from_rgb((r / n) as u8, (g / n) as u8, (b / n) as u8));
        }
    }
    (tw, th, out)
}

impl Wallpaper {
    /// The average wallpaper colour behind screen_rect: a calm, detail-free fill used while a
    /// box is being dragged (a detailed picture would visibly lag behind fast moves).
    pub fn average(&self, screen_rect: Rect, ppp: f32) -> Color32 {
        let Some((tw, th, px)) = &self.thumb else { return self.solid };
        let (monitor, virt) = monitor_of(screen_rect, ppp);
        let img = self.image_rect(monitor, virt, ppp);
        let vis = screen_rect.intersect(img);
        if vis.width() <= 0.0 || vis.height() <= 0.0 {
            return self.solid;
        }
        let fx = |x: f32| (((x - img.min.x) / img.width()) * *tw as f32).clamp(0.0, *tw as f32 - 1.0) as usize;
        let fy = |y: f32| (((y - img.min.y) / img.height()) * *th as f32).clamp(0.0, *th as f32 - 1.0) as usize;
        let (x0, x1, y0, y1) = (fx(vis.min.x), fx(vis.max.x), fy(vis.min.y), fy(vis.max.y));
        let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let c = px[y * tw + x];
                r += c.r() as u32;
                g += c.g() as u32;
                b += c.b() as u32;
                n += 1;
            }
        }
        let n = n.max(1);
        Color32::from_rgb((r / n) as u8, (g / n) as u8, (b / n) as u8)
    }
}
impl Wallpaper {
    /// A lookup of the wallpaper colour at a screen point (points) for the monitor holding
    /// screen_rect, with the same placement the boxes draw with.
    pub fn sampler(&self, screen_rect: Rect, ppp: f32) -> impl Fn(f32, f32) -> [u8; 3] {
        let (monitor, virt) = monitor_of(screen_rect, ppp);
        let img_rect = self.image_rect(monitor, virt, ppp);
        let full = self.full.clone();
        let solid = self.solid;
        move |x: f32, y: f32| {
            let Some(img) = &full else { return [solid.r(), solid.g(), solid.b()] };
            if !img_rect.contains(Pos2::new(x, y)) {
                return [solid.r(), solid.g(), solid.b()];
            }
            let (w, h) = (img.size[0], img.size[1]);
            let u = ((x - img_rect.min.x) / img_rect.width() * w as f32).clamp(0.0, w as f32 - 1.0) as usize;
            let v = ((y - img_rect.min.y) / img_rect.height() * h as f32).clamp(0.0, h as f32 - 1.0) as usize;
            let c = img.pixels[v * w + u];
            [c.r(), c.g(), c.b()]
        }
    }
}
/// Gaussian-like blur (three box-blur passes each way), in place. Runs on the background thread.
pub fn blur_image(img: &mut ColorImage, r: usize) {
    let (w, h) = (img.size[0], img.size[1]);
    if r == 0 || w < 2 || h < 2 {
        return;
    }
    let mut planes: [Vec<u8>; 3] = [
        img.pixels.iter().map(|c| c.r()).collect(),
        img.pixels.iter().map(|c| c.g()).collect(),
        img.pixels.iter().map(|c| c.b()).collect(),
    ];
    let mut tmp = vec![0u8; w * h];
    for p in planes.iter_mut() {
        for _ in 0..3 {
            box_pass(p, &mut tmp, w, h, r, true);
            box_pass(&tmp.clone(), p, w, h, r, false);
        }
    }
    for (i, c) in img.pixels.iter_mut().enumerate() {
        *c = Color32::from_rgb(planes[0][i], planes[1][i], planes[2][i]);
    }
}

/// One running-average pass along rows (horizontal) or columns, edges clamped.
fn box_pass(src: &[u8], dst: &mut [u8], w: usize, h: usize, r: usize, horizontal: bool) {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let win = (2 * r + 1) as u32;
    for line in 0..lines {
        let at = |i: usize| -> usize { if horizontal { line * w + i } else { i * w + line } };
        let get = |i: isize| -> u32 { src[at(i.clamp(0, len as isize - 1) as usize)] as u32 };
        let mut sum: u32 = (-(r as isize)..=(r as isize)).map(get).sum();
        for i in 0..len {
            dst[at(i)] = ((sum + win / 2) / win) as u8;
            sum += get(i as isize + r as isize + 1);
            sum -= get(i as isize - r as isize);
        }
    }
}
/// What a box's blurred glass was made for: desktop picture, box area (pixels), strength.
#[derive(Clone, Copy, PartialEq)]
pub struct GlassKey {
    generation: u64,
    area: [i32; 4],
    radius: u32,
}

impl Wallpaper {
    /// The glass behind one box: the desktop under it, sharp (strength 0) or blurred with that
    /// box's own strength. Made once and reused until the box moves or resizes, its strength
    /// changes or the desktop picture changes. Only this box-sized piece is sent to the graphics
    /// card, never the whole desktop. `None` = no desktop capture yet (or still blurring).
    fn box_glass(&mut self, ctx: &egui::Context, id: u64, screen_rect: Rect, ppp: f32, radius: u32) -> Option<(TextureHandle, std::sync::Arc<ColorImage>)> {
        let origin = self.origin?; // only for a pixel-exact desktop capture
        let area = [
            (screen_rect.min.x * ppp).round() as i32,
            (screen_rect.min.y * ppp).round() as i32,
            (screen_rect.width() * ppp).round().max(1.0) as i32,
            (screen_rect.height() * ppp).round().max(1.0) as i32,
        ];
        let key = GlassKey { generation: self.generation, area, radius };
        if let Some((k, t, img)) = self.glass.get(&id) {
            if *k == key {
                self.blurred_used.insert(radius, self.pass);
                return Some((t.clone(), img.clone()));
            }
        }
        let (pic, _, scale) = self.frosted_picture(radius)?;
        let out = crop_scaled(&pic, origin, area, scale);
        let img = std::sync::Arc::new(out.clone());
        let tex = ctx.load_texture(format!("glass-{id}"), out, TextureOptions::NEAREST);
        self.glass.insert(id, (key, tex.clone(), img.clone()));
        Some((tex, img))
    }

    /// Paints a box's glass (sharp or blurred with its own strength).
    pub fn paint_box(&mut self, ctx: &egui::Context, painter: &egui::Painter, id: u64, local_rect: Rect, screen_rect: Rect, ppp: f32, radius: u32) {
        let glass = self.box_glass(ctx, id, screen_rect, ppp, radius).or_else(|| {
            // Blur still being made: show the sharp desktop for that moment.
            if radius > 0 { self.box_glass(ctx, id ^ (1 << 63), screen_rect, ppp, 0) } else { None }
        });
        match glass {
            Some((tex, _)) => {
                painter.image(tex.id(), local_rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            }
            None => self.paint(painter, local_rect, screen_rect, ppp),
        }
    }

    /// The glass colour behind pixel (x, y) of a box (pixels from its top-left), exactly as
    /// `paint_box` draws it.
    pub fn box_sampler(&mut self, ctx: &egui::Context, id: u64, screen_rect: Rect, ppp: f32, radius: u32) -> Box<dyn Fn(usize, usize) -> [u8; 3]> {
        if let Some((_, img)) = self.box_glass(ctx, id, screen_rect, ppp, radius) {
            let w = img.size[0];
            let h = img.size[1];
            return Box::new(move |x: usize, y: usize| {
                let c = img.pixels[y.min(h - 1) * w + x.min(w - 1)];
                [c.r(), c.g(), c.b()]
            });
        }
        let s = self.sampler(screen_rect, ppp);
        let o = screen_rect.min;
        Box::new(move |x: usize, y: usize| s(o.x + (x as f32 + 0.5) / ppp, o.y + (y as f32 + 0.5) / ppp))
    }

    /// Forget a box's glass (e.g. the box was deleted).
    #[allow(dead_code)]
    pub fn forget(&mut self, id: u64) {
        self.glass.remove(&id);
    }

    /// The whole desktop blurred with `radius`, stored at half size (the blur hides the lost
    /// detail and it takes a quarter of the memory); made once in the background. `None` until
    /// it is ready.
    fn blurred_full(&mut self, radius: u32) -> Option<std::sync::Arc<ColorImage>> {
        if let Some(b) = self.blurred.get(&radius) {
            self.blurred_used.insert(radius, self.pass);
            return Some(b.clone());
        }
        if !self.blur_busy {
            if let Some(full) = self.full.clone() {
                self.blur_busy = true;
                let tx = self.blur_tx.clone();
                let ctx = self.ctx.clone();
                let generation = self.generation;
                let _ = std::thread::Builder::new().name("blur".into()).spawn(move || {
                    let mut img = half_size(&full);
                    blur_image(&mut img, (radius as usize / 2).max(1));
                    let _ = tx.send((generation, radius, img));
                    ctx.request_repaint();
                });
            }
        }
        None
    }

    /// The desktop picture for a box's glass and for its moving copy: sharp (strength 0) or
    /// blurred, its top-left in screen pixels, and its scale (1 = full size, 2 = half size).
    pub fn frosted_picture(&mut self, radius: u32) -> Option<(std::sync::Arc<ColorImage>, Pos2, u32)> {
        let origin = self.origin?;
        if radius == 0 {
            return Some((self.full.clone()?, origin, 1));
        }
        Some((self.blurred_full(radius)?, origin, 2))
    }
}

/// Halves a picture's size (each pixel = average of 2x2).
fn half_size(img: &ColorImage) -> ColorImage {
    let (w, h) = (img.size[0], img.size[1]);
    let (hw, hh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = ColorImage::new([hw, hh], Color32::BLACK);
    for y in 0..hh {
        for x in 0..hw {
            let p = |dx: usize, dy: usize| img.pixels[((y * 2 + dy).min(h - 1)) * w + (x * 2 + dx).min(w - 1)];
            let (a, b, c, d) = (p(0, 0), p(1, 0), p(0, 1), p(1, 1));
            let avg = |f: fn(&Color32) -> u8| ((f(&a) as u32 + f(&b) as u32 + f(&c) as u32 + f(&d) as u32 + 2) / 4) as u8;
            out.pixels[y * hw + x] = Color32::from_rgb(avg(|c| c.r()), avg(|c| c.g()), avg(|c| c.b()));
        }
    }
    out
}

/// The part of a desktop picture under `area` (screen pixels: x, y, width, height). `scale` 2
/// means the picture is stored at half size: it is smoothly enlarged. Used by both the resting
/// glass and the moving copy, so a box looks identical in both.
pub fn crop_scaled(pic: &ColorImage, origin: Pos2, area: [i32; 4], scale: u32) -> ColorImage {
    let (pw, ph) = (pic.size[0] as i32, pic.size[1] as i32);
    let mut out = ColorImage::new([area[2] as usize, area[3] as usize], Color32::BLACK);
    if scale <= 1 {
        for y in 0..area[3] {
            let sy = (area[1] - origin.y as i32 + y).clamp(0, ph - 1) as usize;
            for x in 0..area[2] {
                let sx = (area[0] - origin.x as i32 + x).clamp(0, pw - 1) as usize;
                out.pixels[(y * area[2] + x) as usize] = pic.pixels[sy * pw as usize + sx];
            }
        }
        return out;
    }
    let s = scale as f32;
    for y in 0..area[3] {
        let fy = ((area[1] as f32 - origin.y + y as f32 + 0.5) / s - 0.5).clamp(0.0, (ph - 1) as f32);
        let (y0, ty) = (fy.floor() as i32, fy.fract());
        let y1 = (y0 + 1).min(ph - 1);
        for x in 0..area[2] {
            let fx = ((area[0] as f32 - origin.x + x as f32 + 0.5) / s - 0.5).clamp(0.0, (pw - 1) as f32);
            let (x0, tx) = (fx.floor() as i32, fx.fract());
            let x1 = (x0 + 1).min(pw - 1);
            let g = |xx: i32, yy: i32| pic.pixels[(yy * pw + xx) as usize];
            let (a, b, c, d) = (g(x0, y0), g(x1, y0), g(x0, y1), g(x1, y1));
            let mix = |f: fn(&Color32) -> u8| {
                let top = f(&a) as f32 * (1.0 - tx) + f(&b) as f32 * tx;
                let bot = f(&c) as f32 * (1.0 - tx) + f(&d) as f32 * tx;
                (top * (1.0 - ty) + bot * ty).round() as u8
            };
            out.pixels[(y * area[2] + x) as usize] = Color32::from_rgb(mix(|c| c.r()), mix(|c| c.g()), mix(|c| c.b()));
        }
    }
    out
}
impl Wallpaper {
    /// Keeps the blurred desktop pictures for exactly the blur levels boxes are set to, and
    /// frees the others (e.g. levels tried on the slider). Called on every pass with the boxes'
    /// current levels, so a picture in use is never freed, even mid-drag.
    pub fn keep_blur_levels(&mut self, levels: &std::collections::HashSet<u32>) {
        self.blurred.retain(|r, _| levels.contains(r));
    }
}