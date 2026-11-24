//! Volume, mute and the name of the default output device, through Windows' Core Audio.
//!
//! Everything COM lives on the media helper thread (see `widgets.rs`): the device enumerator and
//! the endpoint volume interface are created once and kept; they are created again only when
//! the default output device changes. The UI thread never touches COM: it reads `current()` and
//! leaves commands with `set_level` / `set_mute` for the helper to carry out.

use std::sync::Mutex;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Volume {
    /// Master volume, 0..1.
    pub level: f32,
    pub muted: bool,
    /// For example "Headphones (WH-1000XM5)".
    pub device: String,
}

static STATE: Mutex<Option<Volume>> = Mutex::new(None);
/// Commands waiting for the helper: (new level, new mute state). The latest one wins.
static PENDING: Mutex<(Option<f32>, Option<bool>)> = Mutex::new((None, None));

/// The last reading (None until the first one, or when there is no output device).
pub fn current() -> Option<Volume> {
    STATE.lock().ok().and_then(|g| g.clone())
}

/// Ask for a new volume (0..1). The shown value changes at once; the helper applies it.
pub fn set_level(level: f32) {
    let level = level.clamp(0.0, 1.0);
    if let Ok(mut s) = STATE.lock() {
        if let Some(v) = s.as_mut() {
            v.level = level;
        }
    }
    if let Ok(mut p) = PENDING.lock() {
        p.0 = Some(level);
    }
}

pub fn set_mute(muted: bool) {
    if let Ok(mut s) = STATE.lock() {
        if let Some(v) = s.as_mut() {
            v.muted = muted;
        }
    }
    if let Ok(mut p) = PENDING.lock() {
        p.1 = Some(muted);
    }
}

/// Opens Windows' own output picker (quick settings audio list), else the Sound settings page.
/// Windows has no official way to switch the default device from a program.
pub fn open_output_picker() {
    let _ = std::thread::Builder::new().name("open-sound".into()).spawn(|| {
        #[cfg(windows)]
        unsafe {
            use windows::core::{w, PCWSTR};
            use windows::Win32::Foundation::HWND;
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
            use windows::Win32::UI::Shell::ShellExecuteW;
            use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
            let r = ShellExecuteW(HWND::default(), w!("open"), w!("ms-actioncenter:controlcenter/audio"), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
            if (r.0 as isize) <= 32 {
                let _ = ShellExecuteW(HWND::default(), w!("open"), w!("ms-settings:sound"), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
            }
        }
    });
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::cell::RefCell;
    use windows::core::GUID;
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{eMultimedia, eRender, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator};
    use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, STGM_READ};
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;

    struct Kept {
        enumerator: IMMDeviceEnumerator,
        id: String,
        vol: Option<IAudioEndpointVolume>,
        name: String,
    }

    thread_local! {
        static KEPT: RefCell<Option<Kept>> = const { RefCell::new(None) };
    }

    unsafe fn device_id(d: &IMMDevice) -> Option<String> {
        let p = d.GetId().ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }

    unsafe fn friendly_name(d: &IMMDevice) -> String {
        let go = || -> Option<String> {
            let store = d.OpenPropertyStore(STGM_READ).ok()?;
            let pv = store.GetValue(&windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName).ok()?;
            let p = PropVariantToStringAlloc(&pv).ok()?;
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        };
        go().unwrap_or_default()
    }

    /// One reading (and the pending commands). Returns true when the shown state changed.
    pub fn tick() -> bool {
        let new_state: Option<Volume> = KEPT.with(|k| {
            let mut k = k.borrow_mut();
            unsafe {
                if k.is_none() {
                    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
                    *k = Some(Kept { enumerator: en, id: String::new(), vol: None, name: String::new() });
                }
                let kept = k.as_mut()?;
                let dev = match kept.enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia) {
                    Ok(d) => d,
                    Err(_) => {
                        kept.vol = None;
                        kept.id.clear();
                        return None; // no output device
                    }
                };
                let id = device_id(&dev).unwrap_or_default();
                if kept.vol.is_none() || id != kept.id {
                    kept.vol = dev.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None).ok();
                    kept.name = friendly_name(&dev);
                    kept.id = id;
                }
                let vol = kept.vol.clone()?;
                let (lvl, mute) = PENDING.lock().map(|mut p| std::mem::take(&mut *p)).unwrap_or((None, None));
                let none: *const GUID = std::ptr::null();
                if let Some(l) = lvl {
                    let _ = vol.SetMasterVolumeLevelScalar(l, none);
                }
                if let Some(m) = mute {
                    let _ = vol.SetMute(m, none);
                }
                match (vol.GetMasterVolumeLevelScalar(), vol.GetMute()) {
                    (Ok(level), Ok(muted)) => Some(Volume { level, muted: muted.as_bool(), device: kept.name.clone() }),
                    _ => {
                        kept.vol = None; // device went away: ask again next time
                        None
                    }
                }
            }
        });
        match STATE.lock() {
            Ok(mut s) => {
                let changed = *s != new_state;
                *s = new_state;
                changed
            }
            Err(_) => false,
        }
    }
}

/// Called by the media helper thread (COM must be initialised there) with each media reading.
pub fn tick() -> bool {
    #[cfg(windows)]
    {
        imp::tick()
    }
    #[cfg(not(windows))]
    {
        false
    }
}
