//! Downloads for online features (weather, update check, agenda): Windows' own HTTP library
//! (WinHTTP), no extra crates, no web engine. Call only from a background thread: a request can
//! take seconds. Redirects are followed, the system proxy is used, 20 s timeouts.

/// Downloads `url` (https or http) and returns the body, at most `max` bytes, within 60 s.
pub fn get_bytes(url: &str, max: usize) -> Result<Vec<u8>, String> {
    get_bytes_within(url, max, std::time::Duration::from_secs(60))
}

/// `get_bytes` with a total time budget: a server that trickles bytes can't hold the thread for
/// ever (each single wait is also limited to 20 s).
#[cfg(windows)]
pub fn get_bytes_within(url: &str, max: usize, budget: std::time::Duration) -> Result<Vec<u8>, String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Networking::WinHttp::*;
    let url = url.trim();
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err("not a web address".into());
    };
    let secure = match scheme.to_ascii_lowercase().as_str() {
        "https" | "webcal" | "webcals" => true,
        "http" => false,
        _ => return Err("not a web address".into()),
    };
    let rest = rest.split('#').next().unwrap_or("");
    let cut = rest.find(['/', '?']).unwrap_or(rest.len());
    let (hostport, path) = (&rest[..cut], &rest[cut..]);
    let path = if path.is_empty() { "/".to_string() } else if path.starts_with('?') { format!("/{path}") } else { path.to_string() };
    let hostport = hostport.rsplit('@').next().unwrap_or(hostport);
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()) => (h, p.parse::<u16>().unwrap_or(if secure { 443 } else { 80 })),
        _ => (hostport, if secure { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err("the address has no server name".into());
    }
    struct Handle(*mut core::ffi::c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = WinHttpCloseHandle(self.0);
                }
            }
        }
    }
    unsafe {
        let agent = HSTRING::from(format!("DesktopShepherd/{}", env!("CARGO_PKG_VERSION")));
        let session = Handle(WinHttpOpen(&agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
        if session.0.is_null() {
            return Err("Windows' web access could not start".into());
        }
        let _ = WinHttpSetTimeouts(session.0, 20_000, 20_000, 20_000, 20_000);
        let connect = Handle(WinHttpConnect(session.0, &HSTRING::from(host), port, 0));
        if connect.0.is_null() {
            return Err(format!("could not reach {host}"));
        }
        let flags = if secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
        let request = Handle(WinHttpOpenRequest(connect.0, &HSTRING::from("GET"), &HSTRING::from(path.as_str()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), flags));
        if request.0.is_null() {
            return Err("could not prepare the request".into());
        }
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(|e| format!("could not reach {host} ({})", e.message()))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|e| format!("no answer from {host} ({})", e.message()))?;
        let mut status: u32 = 0;
        let mut len = std::mem::size_of::<u32>() as u32;
        let _ = WinHttpQueryHeaders(request.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some(&mut status as *mut u32 as *mut _), &mut len, std::ptr::null_mut());
        if status != 200 {
            return Err(format!("{host} answered with error {status}"));
        }
        let mut body: Vec<u8> = Vec::new();
        let deadline = std::time::Instant::now() + budget;
        loop {
            if std::time::Instant::now() > deadline {
                return Err("the download took too long".into());
            }
            let mut avail: u32 = 0;
            if WinHttpQueryDataAvailable(request.0, &mut avail).is_err() || avail == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + avail as usize, 0);
            let mut read: u32 = 0;
            if WinHttpReadData(request.0, body[start..].as_mut_ptr() as *mut _, avail, &mut read).is_err() {
                return Err("the download was interrupted".into());
            }
            body.truncate(start + read as usize);
            if read == 0 {
                break;
            }
            if body.len() > max {
                return Err("the download is too big".into());
            }
        }
        Ok(body)
    }
}

#[cfg(not(windows))]
pub fn get_bytes_within(_url: &str, _max: usize, _budget: std::time::Duration) -> Result<Vec<u8>, String> {
    Err("not available on this system".into())
}

/// `get_bytes` as text (at most 4 MB).
pub fn get_text(url: &str) -> Result<String, String> {
    get_bytes(url, 4 * 1024 * 1024).map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// Encodes text for a web address ("São Paulo" -> "S%C3%A3o%20Paulo").
pub fn url_encode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
