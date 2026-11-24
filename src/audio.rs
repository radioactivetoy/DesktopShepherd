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
    /// Id of the default output device.
    pub device_id: String,
    /// Every active output device: (id, friendly name), refreshed when the menu asks (`want_devices`).
    pub devices: Vec<(String, String)>,
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

/// The id of the output device the person picked in the menu; the helper makes it the default.
static DEFAULT_REQ: Mutex<Option<String>> = Mutex::new(None);
/// When the device list was last read (the menu asks for a fresh one when it is older than 5 s).
static LIST_AT: Mutex<Option<std::time::Instant>> = Mutex::new(None);
static LIST_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Called while the output menu is open: true when the helper should be woken to read the list
/// of output devices again.
pub fn want_devices() -> bool {
    let old = LIST_AT.lock().ok().and_then(|g| *g).map(|t| t.elapsed() >= std::time::Duration::from_secs(5)).unwrap_or(true);
    if old {
        LIST_ASKED.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    old
}

/// Make this device (by id) the default output; the helper does it.
pub fn set_default(id: &str) {
    if let Ok(mut d) = DEFAULT_REQ.lock() {
        *d = Some(id.to_string());
    }
}

/// Opens Windows' Sound settings page.
pub fn open_sound_settings() {
    let _ = std::thread::Builder::new().name("open-sound".into()).spawn(|| {
        #[cfg(windows)]
        unsafe {
            use windows::core::{w, PCWSTR};
            use windows::Win32::Foundation::HWND;
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
            use windows::Win32::UI::Shell::ShellExecuteW;
            use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
            let _ = ShellExecuteW(HWND::default(), w!("open"), w!("ms-settings:sound"), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        }
    });
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::cell::RefCell;
    use windows::core::{Interface, IUnknown, IUnknown_Vtbl, GUID, HRESULT, PCWSTR};
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{eCommunications, eConsole, eMultimedia, eRender, ERole, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE};
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, STGM_READ};

    /// Windows' unofficial IPolicyConfig interface for changing the default output device (the
    /// one EarTrumpet and SoundSwitch use; Windows has no official way). Only `SetDefaultEndpoint`
    /// is called; the entries before it only hold their places in the table.
    #[repr(C)]
    struct PolicyVtbl {
        base: IUnknown_Vtbl,
        _before: [usize; 10], // GetMixFormat .. SetPropertyValue
        set_default_endpoint: unsafe extern "system" fn(*mut core::ffi::c_void, PCWSTR, ERole) -> HRESULT,
        _after: usize, // SetEndpointVisibility
    }

    const IID_POLICY_CONFIG: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);

    const CLSID_POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

    struct Kept {
        enumerator: IMMDeviceEnumerator,
        id: String,
        vol: Option<IAudioEndpointVolume>,
        name: String,
        devices: Vec<(String, String)>,
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

    /// Every active output device: (id, friendly name).
    unsafe fn list_devices(en: &IMMDeviceEnumerator) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Ok(col) = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) {
            let n = col.GetCount().unwrap_or(0).min(32);
            for i in 0..n {
                if let Ok(d) = col.Item(i) {
                    if let Some(id) = device_id(&d) {
                        out.push((id, friendly_name(&d)));
                    }
                }
            }
        }
        out
    }

    /// Makes the device the default for every role; false when Windows refuses.
    unsafe fn make_default(id: &str) -> bool {
        let Ok(unk) = CoCreateInstance::<_, IUnknown>(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_ALL) else { return false };
        let mut raw: *mut core::ffi::c_void = std::ptr::null_mut();
        if unk.query(&IID_POLICY_CONFIG, &mut raw).is_err() || raw.is_null() {
            return false;
        }
        let policy = IUnknown::from_raw(raw); // released when dropped
        let vt = &**(raw as *const *const PolicyVtbl);
        let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
        let mut ok = true;
        for role in [eConsole, eMultimedia, eCommunications] {
            if (vt.set_default_endpoint)(raw, PCWSTR(wide.as_ptr()), role).is_err() {
                ok = false;
            }
        }
        drop(policy);
        ok
    }

    /// One reading (and the pending commands). Returns true when the shown state changed.
    pub fn tick() -> bool {
        let new_state: Option<Volume> = KEPT.with(|k| {
            let mut k = k.borrow_mut();
            unsafe {
                if k.is_none() {
                    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
                    *k = Some(Kept { enumerator: en, id: String::new(), vol: None, name: String::new(), devices: Vec::new() });
                }
                let kept = k.as_mut()?;
                // A device picked in the menu becomes the default (all roles).
                let mut relist = LIST_ASKED.swap(false, std::sync::atomic::Ordering::SeqCst) || kept.devices.is_empty();
                if let Some(want) = DEFAULT_REQ.lock().ok().and_then(|mut d| d.take()) {
                    if !make_default(&want) {
                        static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                        if !LOGGED.swap(true, std::sync::atomic::Ordering::SeqCst) {
                            crate::storage::log_line("could not change the default sound output; opening Sound settings");
                        }
                        super::open_sound_settings();
                    }
                    relist = true;
                }
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
                    relist = true;
                }
                if relist {
                    kept.devices = list_devices(&kept.enumerator);
                    if let Ok(mut t) = LIST_AT.lock() {
                        *t = Some(std::time::Instant::now());
                    }
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
                    (Ok(level), Ok(muted)) => Some(Volume { level, muted: muted.as_bool(), device: kept.name.clone(), device_id: kept.id.clone(), devices: kept.devices.clone() }),
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
