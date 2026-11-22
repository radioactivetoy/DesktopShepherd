//! Real file icons (and picture thumbnails) loaded on one low-priority background thread, so
//! drawing never waits for the shell. Results are cached as textures and reused.

use egui::{self, ColorImage, TextureHandle, TextureOptions};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

type Key = (String, u32);
type Answer = (Key, Option<ColorImage>);
/// What the loader thread is busy with right now, and since when (None = waiting for work).
type Working = Arc<Mutex<Option<(Key, Instant)>>>;

/// A load that takes longer than this is treated as hung (e.g. a shortcut to an offline drive).
const STUCK_AFTER: Duration = Duration::from_secs(5);

pub struct IconCache {
    tx: Sender<Key>,
    rx: Receiver<Answer>,
    working: Working,
    textures: HashMap<Key, Option<TextureHandle>>,
    pending: HashSet<Key>,
    /// When the loader was last replaced, so a broken shell can't make us spawn threads in a loop.
    last_restart: Option<Instant>,
}

/// Starts one loader thread. Returns the request sender, the result receiver and the shared
/// "working on" note the watchdog looks at. If the thread can't start, sends just go nowhere and
/// the watchdog never fires (nothing is marked as working).
fn spawn_loader(ctx: &egui::Context) -> (Sender<Key>, Receiver<Answer>, Working) {
    let (tx, req_rx) = channel::<Key>();
    let (res_tx, rx) = channel::<Answer>();
    let working: Working = Arc::new(Mutex::new(None));
    let note = working.clone();
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("icon-loader".into())
        .spawn(move || {
            platform::thread_init();
            while let Ok(key) = req_rx.recv() {
                if let Ok(mut w) = note.lock() {
                    *w = Some((key.clone(), Instant::now()));
                }
                let img = std::panic::catch_unwind(|| platform::load(&key.0, key.1)).ok().flatten();
                if let Ok(mut w) = note.lock() {
                    *w = None;
                }
                // If the cache replaced this loader while it was stuck, nobody listens any more:
                // the send fails and this old thread simply ends.
                if res_tx.send((key, img)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        })
        .ok();
    (tx, rx, working)
}

impl IconCache {
    pub fn new(ctx: &egui::Context) -> Self {
        let (tx, rx, working) = spawn_loader(ctx);
        Self { tx, rx, working, textures: HashMap::new(), pending: HashSet::new(), last_restart: None }
    }

    /// Moves finished icons into textures. Call once per frame. Returns true when any arrived
    /// (the boxes showing them must be drawn again). Also acts as the watchdog: a loader stuck
    /// on one file for too long is replaced so the other icons still load.
    pub fn poll(&mut self, ctx: &egui::Context) -> bool {
        let mut any = false;
        while let Ok((key, img)) = self.rx.try_recv() {
            self.pending.remove(&key);
            let tex = img.map(|i| ctx.load_texture(format!("icon:{}:{}", key.0, key.1), i, TextureOptions::LINEAR));
            self.textures.insert(key, tex);
            any = true;
        }
        if self.pending.is_empty() {
            return any; // idle: no timers, no wake-ups
        }

        let current = self.working.lock().ok().and_then(|w| w.clone());
        let mut wake = STUCK_AFTER;
        if let Some((key, since)) = current {
            let busy_for = since.elapsed();
            let may_restart = self.last_restart.map_or(true, |t| t.elapsed() >= STUCK_AFTER);
            if busy_for >= STUCK_AFTER && may_restart {
                crate::storage::log_line(&format!("icon loading stuck on {}; starting a new loader", key.0));
                // Never ask for this one again; a placeholder shows instead.
                self.pending.remove(&key);
                self.textures.insert(key, None);
                // Replacing rx/tx drops the old receiver and sender, so the old thread ends
                // as soon as its hung call (if ever) returns.
                let (tx, rx, working) = spawn_loader(ctx);
                self.tx = tx;
                self.rx = rx;
                self.working = working;
                self.last_restart = Some(Instant::now());
                for k in self.pending.iter() {
                    let _ = self.tx.send(k.clone());
                }
            } else if busy_for < STUCK_AFTER {
                wake = STUCK_AFTER - busy_for;
            }
        }
        // Only while icons are still loading: come back to check on the loader.
        ctx.request_repaint_after(wake.max(Duration::from_millis(250)));
        any
    }

    /// `px`: the size the icon is shown at, in real pixels (points × display scale). It is
    /// loaded at exactly that size: sharp on any display, and no bigger in memory than needed
    /// (it used to be twice the size, four times the memory).
    pub fn get(&mut self, path: &str, px: u32) -> Option<egui::TextureId> {
        let size = px.clamp(16, 256);
        let key = (path.to_string(), size);
        if let Some(t) = self.textures.get(&key) {
            return t.as_ref().map(|t| t.id());
        }
        if self.pending.insert(key.clone()) {
            let _ = self.tx.send(key);
        }
        None
    }

    /// Drops every texture (after a graphics reset they are made again on demand).
    pub fn clear(&mut self) {
        self.textures.clear();
        self.pending.clear();
    }

    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Drops icons of sizes no box shows any more (after a View size change). Only call it when
    /// the set of sizes in use changes, so icons of boxes that just weren't redrawn stay.
    pub fn keep_only_sizes(&mut self, sizes: &HashSet<u32>) {
        let keep = |s: &u32| sizes.contains(&(*s).clamp(16, 256));
        self.textures.retain(|(_, s), _| keep(s));
        self.pending.retain(|(_, s)| keep(s));
    }

    /// Forget an icon so it is fetched again (e.g. after a file changed).
    pub fn forget_missing(&mut self, keep: &HashSet<String>) {
        self.textures.retain(|(p, _), _| keep.contains(&p.to_lowercase()));
    }
}

/// Loads an icon right away (for the picture that flies with a dragged icon).
pub fn load_now(path: &str, size: u32) -> Option<ColorImage> {
    platform::load(path, size.clamp(16, 256))
}

#[cfg(windows)]
mod platform {
    use egui::{Color32, ColorImage};
    use windows::core::HSTRING;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
        BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_LOWEST};
    use windows::Win32::UI::Shell::{IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK};

    pub fn thread_init() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
            let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_LOWEST);
        }
    }

    pub fn load(path: &str, size: u32) -> Option<ColorImage> {
        unsafe {
            let p = HSTRING::from(path);
            let factory: IShellItemImageFactory = SHCreateItemFromParsingName(&p, None).ok()?;
            let hbm = factory
                .GetImage(SIZE { cx: size as i32, cy: size as i32 }, SIIGBF_BIGGERSIZEOK)
                .ok()?;
            let obj: HGDIOBJ = hbm.into();
            let mut bm = BITMAP::default();
            let got = GetObjectW(obj, std::mem::size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _));
            if got == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 || bm.bmWidth > 1024 || bm.bmHeight > 1024 {
                let _ = DeleteObject(obj);
                return None;
            }
            let (w, h) = (bm.bmWidth as usize, bm.bmHeight as usize);
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32), // top-down rows
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut buf = vec![0u8; w * h * 4];
            let dc = CreateCompatibleDC(None);
            let lines = GetDIBits(dc, hbm, 0, h as u32, Some(buf.as_mut_ptr() as *mut _), &mut info, DIB_RGB_COLORS);
            let _ = DeleteDC(dc);
            let _ = DeleteObject(obj);
            if lines == 0 {
                return None;
            }
            let any_alpha = buf.chunks_exact(4).any(|px| px[3] != 0);
            let pixels: Vec<Color32> = buf
                .chunks_exact(4)
                .map(|px| {
                    let (b, g, r, a) = (px[0], px[1], px[2], px[3]);
                    if any_alpha {
                        // Shell bitmaps come with premultiplied alpha.
                        Color32::from_rgba_premultiplied(r.min(a), g.min(a), b.min(a), a)
                    } else {
                        Color32::from_rgb(r, g, b)
                    }
                })
                .collect();
            Some(ColorImage { size: [w, h], pixels })
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use egui::ColorImage;
    pub fn thread_init() {}
    pub fn load(_path: &str, _size: u32) -> Option<ColorImage> {
        None
    }
}
