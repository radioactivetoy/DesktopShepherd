//! Windows' own right-click menu for desktop items ("More options…" in the icon menu): the same
//! menu Explorer shows, with "Open with", "Send to", and what other programs add (7-Zip, the
//! antivirus, …).
//!
//! It runs inside this program, not inside Explorer: if another program's menu add-on
//! misbehaves, only DeskBoxes is affected (the supervisor restarts it), never Explorer.
//! The menu is modal: it runs its own message loop until a choice is made or it is dismissed.

#![cfg(windows)]

use std::cell::RefCell;
use windows::core::{Interface, HSTRING, PCSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    IContextMenu, IContextMenu2, IContextMenu3, IShellFolder, SHBindToParent, SHParseDisplayName, CMF_NORMAL, CMINVOKECOMMANDINFO,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreatePopupMenu, DestroyMenu, GetCursorPos, PostMessageW, SetForegroundWindow, TrackPopupMenuEx, SW_SHOWNORMAL, TPM_RETURNCMD,
    TPM_RIGHTBUTTON, WM_DRAWITEM, WM_INITMENUPOPUP, WM_MEASUREITEM, WM_MENUCHAR, WM_NULL,
};

thread_local! {
    /// The menu being shown: its owner window forwards drawing messages to it, so submenus like
    /// "Send to" and "Open with" fill in and show their icons.
    static OPEN: RefCell<Option<IContextMenu>> = const { RefCell::new(None) };
}

/// Called by the owner window's procedure: lets the open menu handle its own messages.
pub fn handle_menu_msg(msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    if !matches!(msg, WM_INITMENUPOPUP | WM_DRAWITEM | WM_MEASUREITEM | WM_MENUCHAR) {
        return None;
    }
    OPEN.with(|o| {
        let o = o.try_borrow().ok()?;
        let cm = o.as_ref()?;
        unsafe {
            if let Ok(cm3) = cm.cast::<IContextMenu3>() {
                let mut result = LRESULT(0);
                if cm3.HandleMenuMsg2(msg, wp, lp, Some(&mut result)).is_ok() {
                    return Some(result);
                }
            } else if let Ok(cm2) = cm.cast::<IContextMenu2>() {
                if cm2.HandleMenuMsg(msg, wp, lp).is_ok() {
                    return Some(LRESULT(0));
                }
            }
        }
        None
    })
}

/// A desktop item's shell identity: its parent folder and its own last ID (freed on drop).
struct Item {
    full: *mut ITEMIDLIST,
}

impl Drop for Item {
    fn drop(&mut self) {
        if !self.full.is_null() {
            unsafe { CoTaskMemFree(Some(self.full as *const _)) };
        }
    }
}

/// Shows Windows' menu for `paths` (items of one folder; others are left out) at the mouse,
/// owned by `owner` (a window of this program), and runs the chosen command. Never panics: any
/// failure is logged and the menu simply doesn't show.
pub fn show(owner: isize, paths: &[String]) {
    if let Err(e) = unsafe { show_inner(HWND(owner as *mut _), paths) } {
        crate::storage::log_line(&format!("Windows menu could not be shown: {e:?}"));
    }
}

unsafe fn show_inner(owner: HWND, paths: &[String]) -> windows::core::Result<()> {
    // Parse every path to a shell ID; keep those in the same folder as the first one.
    let mut items: Vec<Item> = Vec::new();
    for p in paths {
        let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
        if SHParseDisplayName(&HSTRING::from(p.as_str()), None, &mut pidl, 0, None).is_ok() && !pidl.is_null() {
            items.push(Item { full: pidl });
        }
    }
    let Some(first) = items.first() else { return Ok(()) };
    let mut last: *mut ITEMIDLIST = std::ptr::null_mut();
    let folder: IShellFolder = SHBindToParent(first.full, Some(&mut last))?;
    let parent_of = |p: &std::path::Path| p.parent().map(|x| x.to_string_lossy().to_lowercase());
    let first_parent = parent_of(std::path::Path::new(&paths[0]));
    let mut children: Vec<*const ITEMIDLIST> = vec![last as *const _];
    for (i, it) in items.iter().enumerate().skip(1) {
        if paths.get(i).map(|p| parent_of(std::path::Path::new(p))) == Some(first_parent.clone()) {
            let mut l: *mut ITEMIDLIST = std::ptr::null_mut();
            if SHBindToParent::<IShellFolder>(it.full, Some(&mut l)).is_ok() && !l.is_null() {
                children.push(l as *const _);
            }
        }
    }
    let cm: IContextMenu = folder.GetUIObjectOf(owner, &children, None)?;
    let menu = CreatePopupMenu()?;
    const FIRST: u32 = 1;
    const LAST: u32 = 0x7FFF;
    let shown = (|| -> windows::core::Result<()> {
        cm.QueryContextMenu(menu, 0, FIRST, LAST, CMF_NORMAL)?;
        let mut at = POINT::default();
        let _ = GetCursorPos(&mut at);
        OPEN.with(|o| *o.borrow_mut() = Some(cm.clone()));
        // The owner must be in front, or the menu won't close when clicking elsewhere.
        let _ = SetForegroundWindow(owner);
        let cmd = TrackPopupMenuEx(menu, (TPM_RETURNCMD | TPM_RIGHTBUTTON).0, at.x, at.y, owner, None).0 as u32;
        let _ = PostMessageW(owner, WM_NULL, WPARAM(0), LPARAM(0));
        OPEN.with(|o| *o.borrow_mut() = None);
        if cmd >= FIRST {
            let info = CMINVOKECOMMANDINFO {
                cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
                hwnd: owner,
                lpVerb: PCSTR((cmd - FIRST) as usize as *const u8),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            cm.InvokeCommand(&info)?;
        }
        Ok(())
    })();
    let _ = DestroyMenu(menu);
    shown
}
