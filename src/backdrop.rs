//! How bright is the wallpaper behind a box? (for the "darker text on bright wallpapers" option)
//!
//! A helper thread finds each monitor's wallpaper (Windows' `IDesktopWallpaper`; fallbacks: the
//! `TranscodedWallpaper` file, `CachedFiles`, or the plain background colour), shrinks it to a
//! small brightness grid per monitor and keeps it. Every 30 s it only looks at file dates and
//! sizes (cheap); the picture is decoded again only when something changed. Asleep otherwise.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, SystemTime};

const GW: usize = 48;
const GH: usize = 27;

struct Mon {
    rect: [i32; 4],
    /// GW*GH luminance cells (0..1), row by row; empty = unknown (no adaptation).
    grid: Vec<f32>,
}

static STATE: Mutex<Vec<Mon>> = Mutex::new(Vec::new());
static GEN: AtomicU64 = AtomicU64::new(0);
static ENABLED: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);
static POKE: (Mutex<bool>, Condvar) = (Mutex::new(false), Condvar::new());

/// Changes whenever the brightness grids were recomputed.
pub fn generation() -> u64 {
    GEN.load(Ordering::Relaxed)
}

/// Turns the feature on (starts the helper thread once) or off.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
    if on {
        if !STARTED.swap(true, Ordering::SeqCst) {
            let r = std::thread::Builder::new().name("backdrop".into()).spawn(worker);
            if r.is_err() {
                STARTED.store(false, Ordering::SeqCst);
            }
        }
        poke();
    }
}

/// Check right away (the wallpaper may have changed).
pub fn poke() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let (m, c) = &POKE;
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    *g = true;
    c.notify_all();
}

/// Average brightness (0 dark .. 1 bright) of the wallpaper under a rectangle in physical screen
/// pixels `[left, top, right, bottom]`; `None` when unknown.
pub fn brightness(rect_px: [i32; 4]) -> Option<f32> {
    let st = STATE.lock().ok()?;
    let (mut sum, mut n) = (0.0_f32, 0u32);
    let mut best: Option<(i64, usize)> = None; // monitor with most overlap
    for (mi, m) in st.iter().enumerate() {
        if m.grid.len() != GW * GH {
            continue;
        }
        let ox = (rect_px[2].min(m.rect[2]) - rect_px[0].max(m.rect[0])).max(0) as i64;
        let oy = (rect_px[3].min(m.rect[3]) - rect_px[1].max(m.rect[1])).max(0) as i64;
        if best.map(|b| ox * oy > b.0).unwrap_or(true) {
            best = Some((ox * oy, mi));
        }
        let (mw, mh) = ((m.rect[2] - m.rect[0]).max(1) as f32, (m.rect[3] - m.rect[1]).max(1) as f32);
        for j in 0..GH {
            let cy = m.rect[1] as f32 + (j as f32 + 0.5) * mh / GH as f32;
            if cy < rect_px[1] as f32 || cy >= rect_px[3] as f32 {
                continue;
            }
            for i in 0..GW {
                let cx = m.rect[0] as f32 + (i as f32 + 0.5) * mw / GW as f32;
                if cx >= rect_px[0] as f32 && cx < rect_px[2] as f32 {
                    sum += m.grid[j * GW + i];
                    n += 1;
                }
            }
        }
    }
    if n > 0 {
        return Some(sum / n as f32);
    }
    // A box smaller than one cell: take the cell under its centre.
    let (_, mi) = best?;
    let m = st.get(mi)?;
    let (cx, cy) = ((rect_px[0] + rect_px[2]) as f32 / 2.0, (rect_px[1] + rect_px[3]) as f32 / 2.0);
    let (mw, mh) = ((m.rect[2] - m.rect[0]).max(1) as f32, (m.rect[3] - m.rect[1]).max(1) as f32);
    let i = (((cx - m.rect[0] as f32) / mw * GW as f32) as i64).clamp(0, GW as i64 - 1) as usize;
    let j = (((cy - m.rect[1] as f32) / mh * GH as f32) as i64).clamp(0, GH as i64 - 1) as usize;
    m.grid.get(j * GW + i).copied()
}

// ---------------------------------------------------------------------------------------------

fn worker() {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut last_sig = String::new();
    let mut cache = ThumbCache::default();
    loop {
        if ENABLED.load(Ordering::Relaxed) {
            check(&mut last_sig, &mut cache);
        }
        let (m, c) = &POKE;
        let g = m.lock().unwrap_or_else(|e| e.into_inner());
        let (mut g, _) = c.wait_timeout_while(g, Duration::from_secs(30), |p| !*p).unwrap_or_else(|e| e.into_inner());
        let poked = std::mem::replace(&mut *g, false);
        drop(g);
        if poked {
            // Windows announces a new wallpaper before the files are final: look again shortly.
            if ENABLED.load(Ordering::Relaxed) {
                check(&mut last_sig, &mut cache);
            }
            std::thread::sleep(Duration::from_secs(2));
            if ENABLED.load(Ordering::Relaxed) {
                check(&mut last_sig, &mut cache);
            }
        }
    }
}

struct Source {
    rect: [i32; 4],
    path: String,
}

struct Scan {
    pos: i32,
    mons: Vec<Source>,
    bg: Option<f32>,
    sig: String,
}

fn stat_sig(p: &std::path::Path) -> String {
    match std::fs::metadata(p) {
        Ok(m) => {
            let t = m.modified().ok().and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
            format!("{}:{}:{}", p.display(), m.len(), t)
        }
        Err(_) => format!("{}:-", p.display()),
    }
}

fn themes_dir() -> Option<std::path::PathBuf> {
    let a = std::env::var_os("APPDATA")?;
    Some(std::path::PathBuf::from(a).join("Microsoft").join("Windows").join("Themes"))
}

fn newest_cached() -> Option<std::path::PathBuf> {
    let dir = themes_dir()?.join("CachedFiles");
    let mut best: Option<(SystemTime, std::path::PathBuf)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let Ok(md) = e.metadata() else { continue };
        if !md.is_file() {
            continue;
        }
        let t = md.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if best.as_ref().map(|b| t > b.0).unwrap_or(true) {
            best = Some((t, e.path()));
        }
    }
    best.map(|b| b.1)
}

/// The plain background colour (Control Panel\Colors\Background, "R G B") as a brightness.
fn background_colour() -> Option<f32> {
    #[cfg(windows)]
    {
        use winreg::enums::HKEY_CURRENT_USER;
        let key = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey("Control Panel\\Colors").ok()?;
        let s: String = key.get_value("Background").ok()?;
        let v: Vec<f32> = s.split_whitespace().filter_map(|x| x.parse::<f32>().ok()).collect();
        if v.len() >= 3 {
            return Some(lum(v[0], v[1], v[2]) / 255.0);
        }
    }
    None
}

fn lum(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

#[cfg(windows)]
fn virtual_screen() -> [i32; 4] {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN};
    unsafe {
        let (x, y) = (GetSystemMetrics(SM_XVIRTUALSCREEN), GetSystemMetrics(SM_YVIRTUALSCREEN));
        let (w, h) = (GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1), GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1));
        [x, y, x + w, y + h]
    }
}

#[cfg(not(windows))]
fn virtual_screen() -> [i32; 4] {
    [0, 0, 1920, 1080]
}

/// Asks Windows for the monitors and their wallpapers. (`pos`, monitors); empty list on failure.
#[cfg(windows)]
fn ask_windows() -> (i32, Vec<Source>) {
    use windows::core::PCWSTR;
    use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL};
    use windows::Win32::UI::Shell::{DesktopWallpaper, IDesktopWallpaper};
    let mut out = Vec::new();
    let mut pos = 4;
    unsafe {
        let Ok(dw) = CoCreateInstance::<_, IDesktopWallpaper>(&DesktopWallpaper, None, CLSCTX_ALL) else { return (pos, out) };
        if let Ok(p) = dw.GetPosition() {
            pos = p.0;
        }
        let n = dw.GetMonitorDevicePathCount().unwrap_or(0);
        for i in 0..n {
            let Ok(id) = dw.GetMonitorDevicePathAt(i) else { continue };
            let idc = PCWSTR(id.0 as *const u16);
            let rect = dw.GetMonitorRECT(idc);
            let path = match dw.GetWallpaper(idc) {
                Ok(w) => {
                    let s = w.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(w.0 as *const _));
                    s
                }
                Err(_) => String::new(),
            };
            CoTaskMemFree(Some(id.0 as *const _));
            if let Ok(r) = rect {
                if r.right > r.left && r.bottom > r.top {
                    out.push(Source { rect: [r.left, r.top, r.right, r.bottom], path });
                }
            }
        }
    }
    (pos, out)
}

#[cfg(not(windows))]
fn ask_windows() -> (i32, Vec<Source>) {
    (4, Vec::new())
}

fn scan() -> Scan {
    let (pos, mut mons) = ask_windows();
    if mons.is_empty() {
        mons.push(Source { rect: virtual_screen(), path: String::new() });
    }
    let bg = background_colour();
    let mut sig = format!("pos{pos};bg{bg:?};");
    for m in &mons {
        sig.push_str(&format!("{:?}|{}|{};", m.rect, m.path, if m.path.is_empty() { String::new() } else { stat_sig(std::path::Path::new(&m.path)) }));
    }
    if let Some(d) = themes_dir() {
        sig.push_str(&stat_sig(&d.join("TranscodedWallpaper")));
    }
    if let Some(c) = newest_cached() {
        sig.push_str(&stat_sig(&c));
    }
    Scan { pos, mons, bg, sig }
}

struct Thumb {
    w: usize,
    h: usize,
    iw: f32,
    ih: f32,
    lum: Vec<f32>,
}

fn load_thumb(path: &std::path::Path) -> Option<Thumb> {
    let bytes = std::fs::read(path).ok()?;
    let img = image::load_from_memory(&bytes).ok()?;
    drop(bytes);
    let (iw, ih) = (img.width() as f32, img.height() as f32);
    // Keep only the small thumbnail: the full picture is dropped at once.
    let t = img.thumbnail(320, 320).to_rgb8();
    drop(img);
    let (w, h) = (t.width() as usize, t.height() as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let lum_v = t.pixels().map(|p| lum(p.0[0] as f32, p.0[1] as f32, p.0[2] as f32) / 255.0).collect();
    Some(Thumb { w, h, iw, ih, lum: lum_v })
}

/// Thumbnails already made, by (file, modified time, size), so the same wallpaper on two monitors
/// (or seen again after a change of layout) is decoded once. At most 4 are kept.
#[derive(Default)]
struct ThumbCache {
    items: Vec<((std::path::PathBuf, Option<SystemTime>, u64), Option<std::sync::Arc<Thumb>>)>,
}

impl ThumbCache {
    fn get(&mut self, path: &std::path::Path) -> Option<std::sync::Arc<Thumb>> {
        let md = std::fs::metadata(path).ok();
        let key = (path.to_path_buf(), md.as_ref().and_then(|m| m.modified().ok()), md.as_ref().map(|m| m.len()).unwrap_or(0));
        if let Some((_, v)) = self.items.iter().find(|(k, _)| *k == key) {
            return v.clone();
        }
        let v = load_thumb(path).map(std::sync::Arc::new);
        if self.items.len() >= 4 {
            self.items.remove(0);
        }
        self.items.push((key, v.clone()));
        v
    }
}

/// Brightness grid of `th` laid over `target` (screen pixels) as `pos` says, seen through `mon`.
fn make_grid(th: &Thumb, mon: [i32; 4], target: [i32; 4], pos: i32, bg: f32) -> Vec<f32> {
    let (mw, mh) = ((mon[2] - mon[0]).max(1) as f32, (mon[3] - mon[1]).max(1) as f32);
    let (tw, th_px) = ((target[2] - target[0]).max(1) as f32, (target[3] - target[1]).max(1) as f32);
    // Scale and offset of the image in target pixels.
    let (scale, ox, oy, tile) = match pos {
        0 => (1.0, (tw - th.iw) / 2.0, (th_px - th.ih) / 2.0, false), // center
        1 => (1.0, 0.0, 0.0, true),                                    // tile
        2 => (0.0, 0.0, 0.0, false),                                   // stretch (handled below)
        3 => {
            let s = (tw / th.iw).min(th_px / th.ih);
            (s, (tw - th.iw * s) / 2.0, (th_px - th.ih * s) / 2.0, false)
        }
        _ => {
            let s = (tw / th.iw).max(th_px / th.ih); // fill / span
            (s, (tw - th.iw * s) / 2.0, (th_px - th.ih * s) / 2.0, false)
        }
    };
    let (kx, ky) = (th.w as f32 / th.iw, th.h as f32 / th.ih); // image px -> thumb px
    let mut grid = vec![bg; GW * GH];
    for j in 0..GH {
        for i in 0..GW {
            // The cell's two corners in target pixels.
            let x0 = mon[0] as f32 + i as f32 * mw / GW as f32 - target[0] as f32;
            let x1 = mon[0] as f32 + (i + 1) as f32 * mw / GW as f32 - target[0] as f32;
            let y0 = mon[1] as f32 + j as f32 * mh / GH as f32 - target[1] as f32;
            let y1 = mon[1] as f32 + (j + 1) as f32 * mh / GH as f32 - target[1] as f32;
            // To image pixels.
            let (mut u0, mut u1, mut v0, mut v1) = if scale == 0.0 && !tile {
                (x0 / tw * th.iw, x1 / tw * th.iw, y0 / th_px * th.ih, y1 / th_px * th.ih)
            } else if tile {
                let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
                let (u, v) = (cx.rem_euclid(th.iw), cy.rem_euclid(th.ih));
                (u, u, v, v)
            } else {
                ((x0 - ox) / scale, (x1 - ox) / scale, (y0 - oy) / scale, (y1 - oy) / scale)
            };
            if u1 <= 0.0 || v1 <= 0.0 || u0 >= th.iw || v0 >= th.ih {
                continue; // outside the picture: background colour
            }
            u0 = u0.max(0.0);
            v0 = v0.max(0.0);
            u1 = u1.min(th.iw);
            v1 = v1.min(th.ih);
            let px0 = ((u0 * kx) as usize).min(th.w - 1);
            let py0 = ((v0 * ky) as usize).min(th.h - 1);
            let px1 = (((u1 * kx).ceil() as usize).max(px0 + 1)).min(th.w);
            let py1 = (((v1 * ky).ceil() as usize).max(py0 + 1)).min(th.h);
            let (mut s, mut n) = (0.0_f32, 0u32);
            for y in py0..py1 {
                for x in px0..px1 {
                    s += th.lum[y * th.w + x];
                    n += 1;
                }
            }
            if n > 0 {
                grid[j * GW + i] = s / n as f32;
            }
        }
    }
    grid
}

fn check(last_sig: &mut String, cache: &mut ThumbCache) {
    let sc = scan();
    if sc.sig == *last_sig {
        return;
    }
    *last_sig = sc.sig.clone();
    let vs = virtual_screen();
    let bg = sc.bg.unwrap_or(0.0);
    // Fallback picture (shared): the transcoded wallpaper, else the newest cached one.
    let mut fallback: Option<Option<std::sync::Arc<Thumb>>> = None;
    let mut out = Vec::new();
    for m in &sc.mons {
        let own = if m.path.is_empty() { None } else { cache.get(std::path::Path::new(&m.path)) };
        // No picture path, or a format we cannot read: the transcoded wallpaper / cached file.
        let thumb: Option<&Thumb> = match &own {
            Some(t) => Some(&**t),
            None => {
                if fallback.is_none() {
                    fallback = Some(themes_dir().and_then(|d| cache.get(&d.join("TranscodedWallpaper"))).or_else(|| newest_cached().and_then(|p| cache.get(&p))));
                }
                fallback.as_ref().and_then(|f| f.as_deref())
            }
        };
        let grid = match thumb {
            Some(t) => {
                let target = if sc.pos == 5 { vs } else { m.rect };
                make_grid(t, m.rect, target, sc.pos, bg)
            }
            None => match sc.bg {
                Some(b) if m.path.is_empty() => vec![b; GW * GH],
                _ => Vec::new(),
            },
        };
        if std::env::var_os("DESKTOPSHEPHERD_DEBUG_INK").is_some() {
            let avg = if grid.is_empty() { -1.0 } else { grid.iter().sum::<f32>() / grid.len() as f32 };
            crate::storage::log_line(&format!("backdrop: monitor {:?} wallpaper {:?} own {} avg {avg:.2}", m.rect, m.path, own.is_some()));
        }
        out.push(Mon { rect: m.rect, grid });
    }
    if let Ok(mut st) = STATE.lock() {
        *st = out;
    }
    GEN.fetch_add(1, Ordering::Relaxed);
    #[cfg(windows)]
    crate::runloop::wake_after(Duration::ZERO);
}
