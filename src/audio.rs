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
    /// The playing app's own volume (its Windows audio session), when one was matched to the
    /// target set with `set_target`: level 0..1, mute.
    pub app_matched: bool,
    pub app_level: f32,
    pub app_mute: bool,
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

/// App volume commands waiting for the helper (applied to every matched session).
static APP_PENDING: Mutex<(Option<f32>, Option<bool>)> = Mutex::new((None, None));
/// The player whose own volume is wanted (Windows' id for it, e.g. "Spotify.exe"); None = only
/// the whole PC's volume is wanted.
static TARGET: Mutex<Option<String>> = Mutex::new(None);

/// Tell the helper which app's volume to follow (None: none). True when it changed.
pub fn set_target(app: Option<&str>) -> bool {
    match TARGET.lock() {
        Ok(mut t) => {
            if t.as_deref() == app {
                false
            } else {
                *t = app.map(|a| a.to_string());
                true
            }
        }
        Err(_) => false,
    }
}

pub fn set_app_level(level: f32) {
    let level = level.clamp(0.0, 1.0);
    if let Ok(mut s) = STATE.lock() {
        if let Some(v) = s.as_mut() {
            v.app_level = level;
        }
    }
    if let Ok(mut p) = APP_PENDING.lock() {
        p.0 = Some(level);
    }
}

pub fn set_app_mute(muted: bool) {
    if let Ok(mut s) = STATE.lock() {
        if let Some(v) = s.as_mut() {
            v.app_mute = muted;
        }
    }
    if let Ok(mut p) = APP_PENDING.lock() {
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
    use windows::Win32::Media::Audio::{AudioSessionStateExpired, IAudioSessionControl2, IAudioSessionManager2, ISimpleAudioVolume, eCommunications, eConsole, eMultimedia, eRender, ERole, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, DEVICE_STATE_ACTIVE};
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
        /// Session manager of the default device (kept; asked for again when the device changes).
        /// One session manager per active output (an app may play on a device that isn't the default).
        mgrs: Vec<(String, IAudioSessionManager2)>,
        /// The player followed, its matched sessions, and when they were looked for.
        app_target: String,
        app_vols: Vec<ISimpleAudioVolume>,
        app_at: Option<std::time::Instant>,
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

    /// Does the process `pid` belong to the player Windows calls `target`? A packaged app's id
    /// ("Publisher.App_hash!Entry") is compared with the process' package family name; otherwise
    /// the id is an exe name ("Spotify.exe", "chrome") compared with the process' exe name.
    unsafe fn process_matches(pid: u32, target: &str) -> bool {
        use windows::core::PWSTR;
        use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS};
        use windows::Win32::Storage::Packaging::Appx::GetPackageFamilyName;
        use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION};
        if pid == 0 || target.is_empty() {
            return false;
        }
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let result = (|| {
            if let Some((family, _)) = target.split_once('!') {
                let mut len = 256u32;
                let mut buf = [0u16; 256];
                if GetPackageFamilyName(h, &mut len, PWSTR(buf.as_mut_ptr())) != ERROR_SUCCESS || len == 0 {
                    return false;
                }
                let have = String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1).min(255)]);
                return have.eq_ignore_ascii_case(family);
            }
            let mut len = 520u32;
            let mut buf = [0u16; 520];
            if QueryFullProcessImageNameW(h, PROCESS_NAME_FORMAT(0), PWSTR(buf.as_mut_ptr()), &mut len).is_err() {
                return false;
            }
            let path = String::from_utf16_lossy(&buf[..(len as usize).min(519)]);
            let exe = path.rsplit(['\\', '/']).next().unwrap_or("");
            let strip = |s: &str| -> String {
                let l = s.to_lowercase();
                l.strip_suffix(".exe").map(|x| x.to_string()).unwrap_or(l)
            };
            let t = target.rsplit(['\\', '/']).next().unwrap_or(target);
            !strip(exe).is_empty() && strip(exe) == strip(t)
        })();
        let _ = CloseHandle(h);
        result
    }

    /// Finds the audio sessions of the target player (a browser may have several).
    unsafe fn find_sessions(mgr: &IAudioSessionManager2, target: &str) -> Vec<ISimpleAudioVolume> {
        let mut out = Vec::new();
        let Ok(en) = mgr.GetSessionEnumerator() else { return out };
        let n = en.GetCount().unwrap_or(0).clamp(0, 64);
        for i in 0..n {
            let Ok(ctl) = en.GetSession(i) else { continue };
            if ctl.GetState().map(|s| s == AudioSessionStateExpired).unwrap_or(true) {
                continue;
            }
            let Ok(c2) = ctl.cast::<IAudioSessionControl2>() else { continue };
            let Ok(pid) = c2.GetProcessId() else { continue };
            if process_matches(pid, target) {
                if let Ok(v) = ctl.cast::<ISimpleAudioVolume>() {
                    out.push(v);
                }
            }
        }
        out
    }

    /// One reading (and the pending commands). Returns true when the shown state changed.
    pub fn tick() -> bool {
        let new_state: Option<Volume> = KEPT.with(|k| {
            let mut k = k.borrow_mut();
            unsafe {
                if k.is_none() {
                    let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
                    *k = Some(Kept { enumerator: en, id: String::new(), vol: None, name: String::new(), devices: Vec::new(), mgrs: Vec::new(), app_target: String::new(), app_vols: Vec::new(), app_at: None });
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
                    kept.app_vols.clear();
                    kept.app_at = None;
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
                // The target player's own volume (only when the widget wants it).
                let target = TARGET.lock().ok().and_then(|t| t.clone());
                let (app_cmd_l, app_cmd_m) = APP_PENDING.lock().map(|mut p| std::mem::take(&mut *p)).unwrap_or((None, None));
                let (mut app_matched, mut app_level, mut app_mute) = (false, 0.0f32, false);
                match target {
                    None => {
                        kept.app_vols.clear();
                        kept.app_target.clear();
                        kept.app_at = None;
                    }
                    Some(t) => {
                        // Look for the sessions again when the player changed, or every 10 s
                        // (3 s while none was found yet).
                        let wait = if kept.app_vols.is_empty() { 3 } else { 10 };
                        let stale = kept.app_at.map(|a| a.elapsed() >= std::time::Duration::from_secs(wait)).unwrap_or(true);
                        if t != kept.app_target || stale {
                            // A session manager for every active output, kept while the output exists.
                            let ids: Vec<String> = kept.devices.iter().map(|d| d.0.clone()).collect();
                            kept.mgrs.retain(|(id, _)| ids.contains(id));
                            for id in &ids {
                                if !kept.mgrs.iter().any(|(i, _)| i == id) {
                                    if let Ok(d) = kept.enumerator.GetDevice(&windows::core::HSTRING::from(id.as_str())) {
                                        if let Ok(m) = d.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) {
                                            kept.mgrs.push((id.clone(), m));
                                        }
                                    }
                                }
                            }
                            let found: Vec<ISimpleAudioVolume> = kept.mgrs.iter().flat_map(|(_, m)| find_sessions(m, &t)).collect();
                            if t != kept.app_target {
                                crate::storage::log_line(&format!("app volume: {t} -> {} audio session(s) on {} output(s)", found.len(), kept.mgrs.len()));
                            }
                            kept.app_vols = found;
                            kept.app_target = t;
                            kept.app_at = Some(std::time::Instant::now());
                        }
                        let mut gone = false;
                        for v in &kept.app_vols {
                            if let Some(l) = app_cmd_l {
                                let _ = v.SetMasterVolume(l, none);
                            }
                            if let Some(m) = app_cmd_m {
                                let _ = v.SetMute(m, none);
                            }
                            match (v.GetMasterVolume(), v.GetMute()) {
                                (Ok(l), Ok(m)) => {
                                    if !app_matched || l > app_level {
                                        app_level = l;
                                    }
                                    if !app_matched {
                                        app_mute = m.as_bool();
                                    }
                                    app_matched = true;
                                }
                                _ => gone = true,
                            }
                        }
                        if gone {
                            kept.app_vols.clear();
                            kept.app_at = None; // look again at the next reading
                        }
                    }
                }
                match (vol.GetMasterVolumeLevelScalar(), vol.GetMute()) {
                    (Ok(level), Ok(muted)) => Some(Volume { level, muted: muted.as_bool(), device: kept.name.clone(), device_id: kept.id.clone(), devices: kept.devices.clone(), app_matched, app_level, app_mute }),
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
