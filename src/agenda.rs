//! The agenda widget's data: online calendars (iCal / .ics links, e.g. Google Calendar's
//! "secret address in iCal format" or Outlook's published calendar).
//!
//! This is the only part of DesktopShepherd that uses the network, and only for links the person
//! added to an agenda widget. Each link is downloaded on a short-lived background thread with
//! Windows' own HTTP library (WinHTTP: no extra crate, no web engine), at most every 30 minutes
//! (5 after a failure) and only while an agenda widget is on screen. The download is read into
//! the events of the next 60 days and kept in memory.
//!
//! Reading the iCal format covers what calendars commonly publish: single and all-day events,
//! times in UTC ("Z") or local, repeating events (daily / weekly with days / monthly by date or
//! "2nd Tuesday" / yearly, with interval, count, end date), cancelled dates (EXDATE), moved
//! instances (RECURRENCE-ID) and cancelled events. Times given with a named time zone (TZID) are
//! taken as local time (right when the calendar is in the person's own zone).

use chrono::{Datelike, Duration as CDuration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Weekday};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// One event on one day, ready to show.
#[derive(Clone, Debug, PartialEq)]
pub struct Occurrence {
    pub start: NaiveDateTime,
    pub end: NaiveDateTime,
    pub all_day: bool,
    pub title: String,
}

#[derive(Default)]
struct CalState {
    name: Option<String>,
    events: Vec<Occurrence>,
    fetched_at: Option<Instant>,
    error: Option<String>,
    loading: bool,
}

static STATE: Mutex<Option<HashMap<String, CalState>>> = Mutex::new(None);

/// "Refresh now" was pressed in a settings window: the next look downloads everything again.
static REFRESH_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn request_refresh() {
    REFRESH_ASKED.store(true, std::sync::atomic::Ordering::SeqCst);
    #[cfg(windows)]
    crate::runloop::wake_after(Duration::ZERO);
}

pub fn take_refresh() -> bool {
    REFRESH_ASKED.swap(false, std::sync::atomic::Ordering::SeqCst)
}

const REFRESH: Duration = Duration::from_secs(30 * 60);
const RETRY: Duration = Duration::from_secs(5 * 60);
/// How far ahead events are read (the widget shows up to 14 days).
const AHEAD_DAYS: i64 = 60;

/// What the widget shows for one link.
pub struct CalView {
    pub name: Option<String>,
    pub events: Vec<Occurrence>,
    pub error: Option<String>,
    pub loading: bool,
}

/// Starts downloads that are due (never twice at once) and returns what is known now, in the
/// order of `urls`. `force` downloads everything again (Refresh now).
pub fn view(urls: &[String], force: bool) -> Vec<CalView> {
    let Ok(mut g) = STATE.lock() else { return Vec::new() };
    let map = g.get_or_insert_with(HashMap::new);
    let now = Instant::now();
    let mut out = Vec::new();
    for url in urls {
        let st = map.entry(url.clone()).or_default();
        let wait = if st.error.is_some() { RETRY } else { REFRESH };
        let due = st.fetched_at.map(|t| now.duration_since(t) >= wait).unwrap_or(true);
        if !st.loading && (due || force) {
            st.loading = true;
            start_download(url.clone());
        }
        out.push(CalView { name: st.name.clone(), events: st.events.clone(), error: st.error.clone(), loading: st.loading });
    }
    // Forget links that were removed from every widget.
    let keep: HashSet<&String> = urls.iter().collect();
    if map.len() > 32 {
        map.retain(|k, v| keep.contains(k) || v.loading);
    }
    out
}

fn start_download(url: String) {
    let key = url.clone();
    let spawned = std::thread::Builder::new().name("agenda-download".into()).spawn(move || {
        let result = download(&url).and_then(|text| {
            if !text.contains("BEGIN:VCALENDAR") {
                return Err("that link isn't an iCal calendar (.ics)".to_string());
            }
            let today = chrono::Local::now().date_naive();
            Ok(parse(&text, today, today + CDuration::days(AHEAD_DAYS)))
        });
        if let Ok(mut g) = STATE.lock() {
            let st = g.get_or_insert_with(HashMap::new).entry(url.clone()).or_default();
            st.loading = false;
            st.fetched_at = Some(Instant::now());
            match result {
                Ok((name, events)) => {
                    st.name = name;
                    st.events = events;
                    st.error = None;
                }
                Err(e) => {
                    crate::storage::log_line(&format!("agenda: download failed: {e}"));
                    st.error = Some(e); // the last good events stay
                }
            }
        }
        #[cfg(windows)]
        crate::runloop::wake_after(Duration::ZERO);
    });
    if spawned.is_err() {
        if let Ok(mut g) = STATE.lock() {
            if let Some(st) = g.get_or_insert_with(HashMap::new).get_mut(&key) {
                st.loading = false;
            }
        }
    }
}

// ---- downloading ---------------------------------------------------------------------------

/// Downloads a link with WinHTTP (follows redirects, uses the system proxy settings, 20 s
/// timeouts, at most 8 MB).
#[cfg(windows)]
fn download(url: &str) -> Result<String, String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Networking::WinHttp::*;
    let url = url.trim();
    let (secure, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("webcal://").or_else(|| url.strip_prefix("webcals://")) {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return Err("the link must start with https://, http:// or webcal://".into());
    };
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => (h, p.parse::<u16>().unwrap_or(if secure { 443 } else { 80 })),
        _ => (hostport, if secure { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err("the link has no server name".into());
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
        let session = Handle(WinHttpOpen(&HSTRING::from("DesktopShepherd-Agenda/1.0"), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
        if session.0.is_null() {
            return Err("Windows' web access could not start".into());
        }
        let _ = WinHttpSetTimeouts(session.0, 20_000, 20_000, 20_000, 20_000);
        let connect = Handle(WinHttpConnect(session.0, &HSTRING::from(host), port, 0));
        if connect.0.is_null() {
            return Err(format!("could not reach {host}"));
        }
        let flags = if secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
        let request = Handle(WinHttpOpenRequest(connect.0, &HSTRING::from("GET"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), flags));
        if request.0.is_null() {
            return Err("could not prepare the request".into());
        }
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(|e| format!("could not reach {host} ({})", e.message()))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|e| format!("no answer from {host} ({})", e.message()))?;
        let mut status: u32 = 0;
        let mut len = std::mem::size_of::<u32>() as u32;
        let _ = WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut _),
            &mut len,
            std::ptr::null_mut(),
        );
        if status != 200 {
            return Err(match status {
                401 | 403 => format!("{host} refused the link (error {status}): is it the private/secret iCal address?"),
                404 => format!("{host} doesn't know that link (error 404)"),
                _ => format!("{host} answered with error {status}"),
            });
        }
        let mut body: Vec<u8> = Vec::new();
        loop {
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
            if body.len() > 8 * 1024 * 1024 {
                return Err("that calendar is too big (over 8 MB)".into());
            }
        }
        Ok(String::from_utf8_lossy(&body).into_owned())
    }
}

#[cfg(not(windows))]
fn download(_url: &str) -> Result<String, String> {
    Err("not available on this system".into())
}

// ---- reading iCal ----------------------------------------------------------------------------

struct Prop {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

impl Prop {
    fn param(&self, key: &str) -> Option<&str> {
        self.params.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }
}

/// "NAME;P1=a;P2="b:c":value" -> Prop (colons inside quotes belong to the parameters).
fn parse_line(line: &str) -> Option<Prop> {
    let mut in_q = false;
    let mut split = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_q = !in_q,
            ':' if !in_q => {
                split = Some(i);
                break;
            }
            _ => {}
        }
    }
    let i = split?;
    let (head, value) = (&line[..i], &line[i + 1..]);
    let mut parts = head.split(';');
    let name = parts.next()?.trim().to_ascii_uppercase();
    let params = parts.filter_map(|p| p.split_once('=')).map(|(k, v)| (k.trim().to_ascii_uppercase(), v.trim_matches('"').to_string())).collect();
    Some(Prop { name, params, value: value.to_string() })
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') | Some('N') => out.push(' '),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A date or date-time value -> (local date-time, all day).
fn parse_time(p: &Prop) -> Option<(NaiveDateTime, bool)> {
    let v = p.value.trim();
    let date_only = p.param("VALUE").map(|x| x.eq_ignore_ascii_case("DATE")).unwrap_or(false) || (v.len() == 8 && !v.contains('T'));
    parse_value(v, date_only)
}

fn parse_value(v: &str, date_only: bool) -> Option<(NaiveDateTime, bool)> {
    let v = v.trim();
    if date_only {
        let d = NaiveDate::parse_from_str(v.get(..8)?, "%Y%m%d").ok()?;
        return Some((d.and_time(NaiveTime::MIN), true));
    }
    let utc = v.ends_with('Z') || v.ends_with('z');
    let t = NaiveDateTime::parse_from_str(v.trim_end_matches(['Z', 'z']).get(..15)?, "%Y%m%dT%H%M%S").ok()?;
    if utc {
        Some((chrono::Utc.from_utc_datetime(&t).with_timezone(&chrono::Local).naive_local(), false))
    } else {
        Some((t, false)) // floating or named zone: taken as local time
    }
}

/// "PT1H30M", "P1D", "-PT15M" -> duration.
fn parse_duration(v: &str) -> Option<CDuration> {
    let v = v.trim();
    let (neg, v) = match v.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, v.strip_prefix('+').unwrap_or(v)),
    };
    let v = v.strip_prefix('P')?;
    let mut total = 0i64;
    let mut num = String::new();
    let mut in_time = false;
    for c in v.chars() {
        match c {
            'T' => in_time = true,
            '0'..='9' => num.push(c),
            _ => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += match (c, in_time) {
                    ('W', _) => n * 7 * 86400,
                    ('D', _) => n * 86400,
                    ('H', true) => n * 3600,
                    ('M', true) => n * 60,
                    ('S', true) => n,
                    _ => return None,
                };
            }
        }
    }
    Some(CDuration::seconds(if neg { -total } else { total }))
}

fn weekday(code: &str) -> Option<Weekday> {
    Some(match code {
        "MO" => Weekday::Mon,
        "TU" => Weekday::Tue,
        "WE" => Weekday::Wed,
        "TH" => Weekday::Thu,
        "FR" => Weekday::Fri,
        "SA" => Weekday::Sat,
        "SU" => Weekday::Sun,
        _ => return None,
    })
}

/// The n-th (1 = first, -1 = last) given weekday of a month.
fn nth_weekday(year: i32, month: u32, n: i32, wd: Weekday) -> Option<NaiveDate> {
    if n > 0 {
        let first = NaiveDate::from_ymd_opt(year, month, 1)?;
        let shift = (7 + wd.num_days_from_monday() as i64 - first.weekday().num_days_from_monday() as i64) % 7;
        let d = first + CDuration::days(shift + 7 * (n as i64 - 1));
        (d.month() == month).then_some(d)
    } else {
        let next = if month == 12 { NaiveDate::from_ymd_opt(year + 1, 1, 1)? } else { NaiveDate::from_ymd_opt(year, month + 1, 1)? };
        let last = next - CDuration::days(1);
        let shift = (7 + last.weekday().num_days_from_monday() as i64 - wd.num_days_from_monday() as i64) % 7;
        let d = last - CDuration::days(shift + 7 * ((-n) as i64 - 1));
        (d.month() == month).then_some(d)
    }
}

struct Rule {
    freq: String,
    interval: i64,
    count: Option<u32>,
    until: Option<NaiveDateTime>,
    by_day: Vec<(i32, Weekday)>,
    by_month_day: Vec<i32>,
}

fn parse_rule(v: &str) -> Option<Rule> {
    let mut r = Rule { freq: String::new(), interval: 1, count: None, until: None, by_day: Vec::new(), by_month_day: Vec::new() };
    for part in v.split(';') {
        let Some((k, val)) = part.split_once('=') else { continue };
        match k.trim().to_ascii_uppercase().as_str() {
            "FREQ" => r.freq = val.trim().to_ascii_uppercase(),
            "INTERVAL" => r.interval = val.trim().parse::<i64>().unwrap_or(1).max(1),
            "COUNT" => r.count = val.trim().parse().ok(),
            "UNTIL" => r.until = parse_value(val, val.trim().len() == 8).map(|(t, all)| if all { t + CDuration::days(1) - CDuration::seconds(1) } else { t }),
            "BYDAY" => {
                for d in val.split(',') {
                    let d = d.trim().to_ascii_uppercase();
                    let (num, code) = d.split_at(d.len().saturating_sub(2));
                    if let Some(wd) = weekday(code) {
                        r.by_day.push((num.parse::<i32>().unwrap_or(0), wd));
                    }
                }
            }
            "BYMONTHDAY" => r.by_month_day = val.split(',').filter_map(|x| x.trim().parse().ok()).collect(),
            _ => {}
        }
    }
    (!r.freq.is_empty()).then_some(r)
}

/// The start times of a repeating event, from its first start up to `to` (both the window and
/// a safety limit end the walk).
fn expand(start: NaiveDateTime, rule: &Rule, to: NaiveDateTime) -> Vec<NaiveDateTime> {
    let mut out = Vec::new();
    let time = start.time();
    let mut emitted = 0u32;
    let mut push = |t: NaiveDateTime, out: &mut Vec<NaiveDateTime>| -> bool {
        // false = stop
        if t < start {
            return true;
        }
        if rule.until.map(|u| t > u).unwrap_or(false) || t > to {
            return false;
        }
        if let Some(c) = rule.count {
            if emitted >= c {
                return false;
            }
        }
        emitted += 1;
        out.push(t);
        true
    };
    let d0 = start.date();
    for step in 0..20_000i64 {
        let k = step * rule.interval;
        let mut dates: Vec<NaiveDate> = Vec::new();
        match rule.freq.as_str() {
            "DAILY" => dates.push(d0 + CDuration::days(k)),
            "WEEKLY" => {
                let week_start = d0 - CDuration::days(d0.weekday().num_days_from_monday() as i64) + CDuration::weeks(k);
                if rule.by_day.is_empty() {
                    dates.push(d0 + CDuration::weeks(k));
                } else {
                    let mut ds: Vec<NaiveDate> = rule.by_day.iter().map(|(_, wd)| week_start + CDuration::days(wd.num_days_from_monday() as i64)).collect();
                    ds.sort();
                    dates.extend(ds);
                }
            }
            "MONTHLY" => {
                let m0 = d0.year() * 12 + d0.month0() as i32 + k as i32;
                let (y, m) = (m0.div_euclid(12), m0.rem_euclid(12) as u32 + 1);
                if !rule.by_day.is_empty() {
                    for (n, wd) in &rule.by_day {
                        if *n != 0 {
                            dates.extend(nth_weekday(y, m, *n, *wd));
                        } else {
                            // every such weekday of the month
                            for i in 1..=5 {
                                dates.extend(nth_weekday(y, m, i, *wd));
                            }
                        }
                    }
                } else if !rule.by_month_day.is_empty() {
                    for md in &rule.by_month_day {
                        if *md > 0 {
                            dates.extend(NaiveDate::from_ymd_opt(y, m, *md as u32));
                        } else if let Some(d) = nth_last_day(y, m, -*md) {
                            dates.push(d);
                        }
                    }
                } else {
                    dates.extend(NaiveDate::from_ymd_opt(y, m, d0.day())); // the 31st skips short months
                }
                dates.sort();
            }
            "YEARLY" => dates.extend(NaiveDate::from_ymd_opt(d0.year() + k as i32, d0.month(), d0.day())),
            _ => {
                out.push(start);
                return out;
            }
        }
        if dates.first().map(|d| d.and_time(time) > to).unwrap_or(false) {
            break;
        }
        for d in dates {
            if !push(d.and_time(time), &mut out) {
                return out;
            }
        }
    }
    out
}

fn nth_last_day(y: i32, m: u32, n: i32) -> Option<NaiveDate> {
    let next = if m == 12 { NaiveDate::from_ymd_opt(y + 1, 1, 1)? } else { NaiveDate::from_ymd_opt(y, m + 1, 1)? };
    Some(next - CDuration::days(n as i64))
}

/// Reads an iCal text into the calendar's name and the events touching [from, to], sorted.
pub fn parse(text: &str, from: NaiveDate, to: NaiveDate) -> (Option<String>, Vec<Occurrence>) {
    // Unfold continuation lines (a line starting with a space or tab belongs to the one before).
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let l = raw.trim_end_matches('\r');
        if (l.starts_with(' ') || l.starts_with('\t')) && !lines.is_empty() {
            if let Some(last) = lines.last_mut() {
                last.push_str(&l[1..]);
            }
        } else {
            lines.push(l.to_string());
        }
    }
    let from_t = from.and_time(NaiveTime::MIN);
    let to_t = to.and_time(NaiveTime::MIN) + CDuration::days(1);
    let mut name = None;
    let mut events: Vec<Vec<Prop>> = Vec::new();
    let mut cur: Option<Vec<Prop>> = None;
    let mut depth_other = 0; // inside VALARM etc. within an event
    for l in &lines {
        let upper = l.to_ascii_uppercase();
        if upper == "BEGIN:VEVENT" {
            cur = Some(Vec::new());
            depth_other = 0;
            continue;
        }
        if upper == "END:VEVENT" {
            if let Some(e) = cur.take() {
                events.push(e);
            }
            continue;
        }
        if let Some(e) = cur.as_mut() {
            if upper.starts_with("BEGIN:") {
                depth_other += 1;
            } else if upper.starts_with("END:") {
                depth_other -= 1;
            } else if depth_other == 0 {
                if let Some(p) = parse_line(l) {
                    e.push(p);
                }
            }
        } else if upper.starts_with("X-WR-CALNAME") {
            name = parse_line(l).map(|p| unescape(&p.value)).filter(|s| !s.trim().is_empty());
        }
    }
    // Moved or cancelled single instances of repeating events: (UID, original start).
    let mut moved: HashSet<(String, NaiveDateTime)> = HashSet::new();
    for e in &events {
        let uid = e.iter().find(|p| p.name == "UID").map(|p| p.value.clone()).unwrap_or_default();
        if let Some(rid) = e.iter().find(|p| p.name == "RECURRENCE-ID").and_then(parse_time) {
            moved.insert((uid, rid.0));
        }
    }
    let mut out = Vec::new();
    for e in &events {
        let get = |n: &str| e.iter().find(|p| p.name == n);
        if get("STATUS").map(|p| p.value.trim().eq_ignore_ascii_case("CANCELLED")).unwrap_or(false) {
            continue;
        }
        let Some((start, all_day)) = get("DTSTART").and_then(parse_time) else { continue };
        let length = match get("DTEND").and_then(parse_time) {
            Some((end, _)) if end > start => end - start,
            _ => match get("DURATION").and_then(|p| parse_duration(&p.value)) {
                Some(d) if d > CDuration::zero() => d,
                _ => {
                    if all_day {
                        CDuration::days(1)
                    } else {
                        CDuration::zero()
                    }
                }
            },
        };
        let title = get("SUMMARY").map(|p| unescape(&p.value)).filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "(no title)".into());
        let uid = get("UID").map(|p| p.value.clone()).unwrap_or_default();
        let starts = match (get("RRULE").and_then(|p| parse_rule(&p.value)), get("RECURRENCE-ID")) {
            (Some(rule), None) => {
                let mut ex: HashSet<NaiveDateTime> = HashSet::new();
                for p in e.iter().filter(|p| p.name == "EXDATE") {
                    let date_only = p.param("VALUE").map(|x| x.eq_ignore_ascii_case("DATE")).unwrap_or(false);
                    for v in p.value.split(',') {
                        if let Some((t, all)) = parse_value(v, date_only || v.trim().len() == 8) {
                            ex.insert(if all && !all_day { t.date().and_time(start.time()) } else { t });
                        }
                    }
                }
                expand(start, &rule, to_t).into_iter().filter(|t| !ex.contains(t) && !moved.contains(&(uid.clone(), *t))).collect()
            }
            _ => vec![start],
        };
        for s in starts {
            let end = s + length;
            // Touches the window (an event of zero length counts at its start).
            if s < to_t && (end > from_t || (end == s && s >= from_t)) {
                out.push(Occurrence { start: s, end, all_day, title: title.clone() });
            }
        }
    }
    out.sort_by(|a, b| a.start.cmp(&b.start).then(b.all_day.cmp(&a.all_day)).then(a.title.cmp(&b.title)));
    (name, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    const CAL: &str = "BEGIN:VCALENDAR\r\nX-WR-CALNAME:Work\r\nBEGIN:VEVENT\r\nUID:a\r\nDTSTART:20261123T090000\r\nDTEND:20261123T100000\r\nSUMMARY:Stand-up\\, team\r\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE;COUNT=4\r\nEXDATE:20261125T090000\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:b\r\nDTSTART;VALUE=DATE:20261124\r\nDTEND;VALUE=DATE:20261126\r\nSUMMARY:Trip to\r\n  Madrid\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:c\r\nDTSTART:20261127T120000\r\nSUMMARY:Cancelled\r\nSTATUS:CANCELLED\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";

    #[test]
    fn reads_events_rules_and_exceptions() {
        let (name, ev) = parse(CAL, d(2026, 11, 22), d(2026, 12, 31));
        assert_eq!(name.as_deref(), Some("Work"));
        let titles: Vec<(String, String)> = ev.iter().map(|e| (e.start.format("%d %H:%M").to_string(), e.title.clone())).collect();
        // Mon 23, (Wed 25 excluded), Mon 30, Wed 2 Dec = COUNT 4 includes the excluded one.
        assert_eq!(
            titles,
            vec![
                ("23 09:00".into(), "Stand-up, team".into()),
                ("24 00:00".into(), "Trip to Madrid".into()),
                ("30 09:00".into(), "Stand-up, team".into()),
                ("02 09:00".into(), "Stand-up, team".into()),
            ]
        );
        assert!(ev[1].all_day);
        assert_eq!(ev[1].end.date(), d(2026, 11, 26));
    }

    #[test]
    fn monthly_nth_weekday_and_yearly() {
        let cal = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nDTSTART:20260101T180000\nRRULE:FREQ=MONTHLY;BYDAY=-1FR\nSUMMARY:Last Friday\nEND:VEVENT\nBEGIN:VEVENT\nDTSTART;VALUE=DATE:20200229\nRRULE:FREQ=YEARLY\nSUMMARY:Leap\nEND:VEVENT\nEND:VCALENDAR\n";
        let (_, ev) = parse(cal, d(2026, 11, 1), d(2027, 1, 31));
        let days: Vec<String> = ev.iter().map(|e| format!("{} {}", e.start.format("%Y-%m-%d"), e.title)).collect();
        assert_eq!(days, vec!["2026-11-27 Last Friday", "2026-12-25 Last Friday", "2027-01-29 Last Friday"]);
    }

    #[test]
    fn moved_instance_replaces_the_original() {
        let cal = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:x\nDTSTART:20261123T100000\nRRULE:FREQ=DAILY;UNTIL=20261125T235959\nSUMMARY:Daily\nEND:VEVENT\nBEGIN:VEVENT\nUID:x\nRECURRENCE-ID:20261124T100000\nDTSTART:20261124T150000\nSUMMARY:Daily (moved)\nEND:VEVENT\nEND:VCALENDAR\n";
        let (_, ev) = parse(cal, d(2026, 11, 22), d(2026, 11, 30));
        let v: Vec<String> = ev.iter().map(|e| format!("{} {}", e.start.format("%d %H"), e.title)).collect();
        assert_eq!(v, vec!["23 10 Daily", "24 15 Daily (moved)", "25 10 Daily"]);
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("PT1H30M"), Some(CDuration::minutes(90)));
        assert_eq!(parse_duration("P1D"), Some(CDuration::days(1)));
        assert_eq!(parse_duration("P1W"), Some(CDuration::weeks(1)));
    }
}
