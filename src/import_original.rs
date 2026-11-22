//! Reads the layout saved by the original organizer (its `cache\desk.cache`) so the person can
//! bring their existing boxes over and compare. The file is Blowfish-ECB encrypted with the
//! fixed key "FindWindow" (Native-x64.dll 0x18024f8b0 → 0x1801b0330; key schedule 0x1801ad170,
//! block decrypt 0x1801ac6b0, which reads each 4-byte half as a little-endian word), and holds
//! JSON text. Nothing is ever written back to the original's files.

use blowfish::cipher::{generic_array::GenericArray, BlockDecrypt, KeyInit};
use std::path::PathBuf;

pub fn original_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("Coodesker"))
}

/// Decrypts one of the original's `.cache` files to its JSON text.
pub fn decrypt(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() || bytes.len() % 8 != 0 {
        return None;
    }
    let bf = <blowfish::Blowfish>::new_from_slice(b"FindWindow").ok()?;
    let mut out = Vec::with_capacity(bytes.len());
    for chunk in bytes.chunks_exact(8) {
        // The original treats each half as a little-endian word; the crate reads big-endian,
        // so swap each half's bytes before and after.
        let mut b = [0u8; 8];
        for i in 0..2 {
            for j in 0..4 {
                b[i * 4 + j] = chunk[i * 4 + 3 - j];
            }
        }
        let mut block = GenericArray::clone_from_slice(&b);
        bf.decrypt_block(&mut block);
        for i in 0..2 {
            for j in 0..4 {
                out.push(block[i * 4 + 3 - j]);
            }
        }
    }
    // Padding: trailing zero bytes.
    while out.last() == Some(&0) {
        out.pop();
    }
    String::from_utf8(out).ok()
}

pub fn read_cache(name: &str) -> Option<serde_json::Value> {
    let path = original_dir()?.join("cache").join(name);
    let bytes = std::fs::read(path).ok()?;
    let text = decrypt(&bytes)?;
    serde_json::from_str(text.trim_end_matches('\0')).ok()
}

fn num(v: &serde_json::Value, k: &str) -> f64 {
    v.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0)
}

/// Builds our boxes from the original's layout: every box with its name, position, colour,
/// opacity, corners, blur, lock/roll-up state and its items in their order; its auto-sort box
/// becomes ours. Settings and widgets of the current layout are kept. None if the original's
/// layout isn't there or can't be read.
pub fn boxes_from_original(current: &crate::model::Layout) -> Option<crate::model::Layout> {
    use crate::model::{Fence, Tab};
    let root = read_cache("desk.cache")?;
    let mut out = current.clone();
    out.fences.retain(|f| f.widget.is_some());
    let mut added = 0;
    for display in root.get("fence")?.as_array()? {
        let scale = display.get("category").map(|c| num(c, "factor")).filter(|f| *f >= 50.0).unwrap_or(100.0) as f32 / 100.0;
        for b in display.get("folders").and_then(|f| f.as_array()).into_iter().flatten() {
            let Some(c) = b.get("category") else { continue };
            let (l, t, r, btm) = (num(c, "pos_left"), num(c, "pos_top"), num(c, "pos_right"), num(c, "pos_bottom"));
            if r <= l || btm <= t {
                continue;
            }
            let mut f = Fence::default();
            f.id = out.new_id();
            f.name = c.get("name").and_then(|n| n.as_str()).unwrap_or("Box").to_string();
            f.rect = [l as f32 / scale, t as f32 / scale, (r - l) as f32 / scale, (btm - t) as f32 / scale];
            f.expanded = c.get("expand").and_then(|x| x.as_bool()).unwrap_or(true);
            f.locked = c.get("lock").and_then(|x| x.as_bool()).unwrap_or(false);
            // Colour is ARGB; 0xFFFFFFFF means "app default" (black at 50 of 255).
            let argb = num(c, "color") as u64 as u32;
            if argb != u32::MAX {
                f.color = [(argb >> 16) as u8, (argb >> 8) as u8, argb as u8];
                f.alpha = (argb >> 24) as u8;
            }
            let box_alpha = num(c, "boxAlpha") as u64 as u32;
            if box_alpha <= 255 {
                f.alpha = box_alpha as u8;
            }
            let corner = num(c, "corner") as u64 as u32;
            if corner <= 64 {
                f.corner = corner as f32;
            }
            f.blur = num(c, "effectDegree").clamp(0.0, 40.0) as u8;
            // category_type 4 = the box that collects every other desktop item.
            f.auto_sort = num(c, "category_type") as i64 == 4;
            let mut items: Vec<(f64, String)> = Vec::new();
            let mut collect = |cat: &serde_json::Value| {
                for a in cat.get("apps").and_then(|a| a.as_array()).into_iter().flatten() {
                    if let Some(p) = a.get("file_path").and_then(|p| p.as_str()) {
                        if !p.to_ascii_lowercase().ends_with("desktop.ini") {
                            items.push((num(a, "position"), p.to_string()));
                        }
                    }
                }
            };
            collect(c);
            if !f.auto_sort {
                for tab in b.get("folders").and_then(|t| t.as_array()).into_iter().flatten() {
                    if let Some(tc) = tab.get("category") {
                        collect(tc);
                    }
                }
            }
            items.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            f.tabs = vec![Tab { name: "Main".into(), items: if f.auto_sort { vec![] } else { items.into_iter().map(|i| i.1).collect() } }];
            out.fences.push(f);
            added += 1;
        }
    }
    (added > 0).then_some(out)
}