//! File actions for the keyboard in boxes: Recycle Bin (Delete), copy / cut / paste with the
//! clipboard in the same format Explorer uses, so files can go to and come from any folder.
//! Windows' own file operation does the work (progress, conflicts, undo), each on its own thread
//! so a slow copy or a confirmation dialog never freezes the boxes.

#![cfg(windows)]

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{BOOL, HANDLE, HWND, POINT};
use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, RegisterClipboardFormatW, SetClipboardData};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::UI::Shell::{DragQueryFileW, SHFileOperationW, DROPFILES, HDROP, SHFILEOPSTRUCTW};

const CF_HDROP: u32 = 15;
const FO_MOVE: u32 = 1;
const FO_COPY: u32 = 2;
const FO_DELETE: u32 = 3;
const FOF_ALLOWUNDO: u16 = 0x40;
const FOF_RENAMEONCOLLISION: u16 = 0x08;

/// Paths as one double-zero-terminated wide string (what Windows' file operation expects).
fn multi_sz(paths: &[String]) -> Vec<u16> {
    let mut v: Vec<u16> = Vec::new();
    for p in paths {
        v.extend(p.encode_utf16());
        v.push(0);
    }
    v.push(0);
    v
}

fn file_op(func: u32, from: &[String], to: Option<&str>, flags: u16) -> bool {
    let from_w = multi_sz(from);
    let to_w = to.map(|t| multi_sz(&[t.to_string()]));
    let mut op = SHFILEOPSTRUCTW {
        hwnd: HWND::default(),
        wFunc: func,
        pFrom: PCWSTR(from_w.as_ptr()),
        pTo: to_w.as_ref().map(|t| PCWSTR(t.as_ptr())).unwrap_or(PCWSTR::null()),
        fFlags: flags,
        ..Default::default()
    };
    unsafe { SHFileOperationW(&mut op) == 0 && !op.fAnyOperationsAborted.as_bool() }
}

/// Sends files to the Recycle Bin (Windows asks for confirmation if the person set it so),
/// then calls `done`.
pub fn recycle(paths: Vec<String>, done: impl FnOnce() + Send + 'static) {
    let _ = std::thread::Builder::new().name("recycle".into()).spawn(move || {
        let _ = file_op(FO_DELETE, &paths, None, FOF_ALLOWUNDO);
        done();
    });
}

/// Puts files on the clipboard like Explorer's Copy / Cut (pasting in a folder copies or
/// moves them). `owner`: a window of this program.
pub fn set_clipboard_files(owner: isize, paths: &[String], cut: bool) -> bool {
    if paths.is_empty() {
        return false;
    }
    unsafe {
        if OpenClipboard(HWND(owner as *mut _)).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let list = multi_sz(paths);
        let header = std::mem::size_of::<DROPFILES>();
        let mut ok = false;
        if let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, header + list.len() * 2) {
            let p = GlobalLock(mem) as *mut u8;
            if !p.is_null() {
                let df = DROPFILES { pFiles: header as u32, pt: POINT::default(), fNC: BOOL(0), fWide: BOOL(1) };
                std::ptr::write_unaligned(p as *mut DROPFILES, df);
                std::ptr::copy_nonoverlapping(list.as_ptr() as *const u8, p.add(header), list.len() * 2);
                let _ = GlobalUnlock(mem);
                ok = SetClipboardData(CF_HDROP, HANDLE(mem.0)).is_ok();
            }
            if !ok {
                // The clipboard didn't take it: the memory is still ours to free.
                let _ = windows::Win32::Foundation::GlobalFree(mem);
            }
        }
        // "Preferred DropEffect": 2 = move (cut), 1 = copy.
        let effect_fmt = RegisterClipboardFormatW(w!("Preferred DropEffect"));
        if effect_fmt != 0 {
            if let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, 4) {
                let p = GlobalLock(mem) as *mut u32;
                if !p.is_null() {
                    *p = if cut { 2 } else { 1 };
                    let _ = GlobalUnlock(mem);
                    if SetClipboardData(effect_fmt, HANDLE(mem.0)).is_err() {
                        let _ = windows::Win32::Foundation::GlobalFree(mem);
                    }
                } else {
                    let _ = windows::Win32::Foundation::GlobalFree(mem);
                }
            }
        }
        let _ = CloseClipboard();
        ok
    }
}

/// Files on the clipboard (copied or cut in Explorer or here) and whether they were cut.
fn clipboard_files(owner: isize) -> (Vec<String>, bool) {
    let mut files = Vec::new();
    let mut cut = false;
    unsafe {
        if OpenClipboard(HWND(owner as *mut _)).is_err() {
            return (files, cut);
        }
        if let Ok(h) = GetClipboardData(CF_HDROP) {
            let drop = HDROP(h.0);
            let n = DragQueryFileW(drop, u32::MAX, None);
            for i in 0..n.min(10_000) {
                let len = DragQueryFileW(drop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(drop, i, Some(&mut buf));
                files.push(String::from_utf16_lossy(&buf[..len]));
            }
        }
        let effect_fmt = RegisterClipboardFormatW(w!("Preferred DropEffect"));
        if effect_fmt != 0 {
            if let Ok(h) = GetClipboardData(effect_fmt) {
                let p = GlobalLock(windows::Win32::Foundation::HGLOBAL(h.0)) as *const u32;
                if !p.is_null() {
                    cut = *p & 2 != 0;
                    let _ = GlobalUnlock(windows::Win32::Foundation::HGLOBAL(h.0));
                }
            }
        }
        let _ = CloseClipboard();
    }
    (files, cut)
}

/// Pastes files from the clipboard into `dest` (a desktop folder): copies them, or moves them
/// if they were cut. Then calls `done` with the new paths that exist.
pub fn paste_into(owner: isize, dest: String, done: impl FnOnce(Vec<String>) + Send + 'static) {
    let (files, cut) = clipboard_files(owner);
    if files.is_empty() {
        return;
    }
    let _ = std::thread::Builder::new().name("paste".into()).spawn(move || {
        let flags = FOF_ALLOWUNDO | FOF_RENAMEONCOLLISION;
        // What is in the folder before, so the new files are known exactly afterwards (a copy of a
        // file that is already there gets a new name, e.g. "x - Copy").
        let list = |d: &str| -> std::collections::HashSet<String> {
            std::fs::read_dir(d).map(|rd| rd.flatten().map(|e| e.path().to_string_lossy().to_lowercase()).collect()).unwrap_or_default()
        };
        let before = list(&dest);
        let dest_l = dest.trim_end_matches(['\\', '/']).to_lowercase();
        // Files already in this folder that were cut: nothing to move, just place them. (Copied
        // ones are copied again, as Explorer does: "x - Copy".)
        let in_place: Vec<String> = files
            .iter()
            .filter(|_| cut)
            .filter(|f| std::path::Path::new(f).parent().map(|p| p.to_string_lossy().to_lowercase() == dest_l).unwrap_or(false))
            .cloned()
            .collect();
        let to_do: Vec<String> = files.iter().filter(|f| !in_place.contains(f)).cloned().collect();
        if !to_do.is_empty() {
            let _ = file_op(if cut { FO_MOVE } else { FO_COPY }, &to_do, Some(&dest), flags);
        }
        let mut landed: Vec<String> = std::fs::read_dir(&dest)
            .map(|rd| rd.flatten().map(|e| e.path().to_string_lossy().to_string()).filter(|p| !before.contains(&p.to_lowercase())).collect())
            .unwrap_or_default();
        landed.extend(in_place.into_iter().filter(|p| std::path::Path::new(p).exists()));
        done(landed);
    });
}
