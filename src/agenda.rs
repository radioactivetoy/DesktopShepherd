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
//! converted with the zone's rules the calendar itself includes (VTIMEZONE: Google and Outlook
//! send them); a zone without rules is taken as local time.

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
    /// Where and what (shared by every repeat of the same event).
    pub location: std::sync::Arc<str>,
    pub description: std::sync::Arc<str>,
}

#[derive(Default)]
struct CalState {
    name: Option<String>,
    events: std::sync::Arc<Vec<Occurrence>>,
    fetched_at: Option<Instant>,
    error: Option<String>,
    loading: bool,
    /// The calendar had more events than are read (see `MAX_EVENTS`).
    truncated: bool,
}

static STATE: Mutex<Option<HashMap<String, CalState>>> = Mutex::new(None);

/// "Refresh now" was pressed in a settings window: the next look downloads everything again.
static REFRESH_ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn request_refresh() {
    REFRESH_ASKED.store(true, std::sync::atomic::Ordering::SeqCst);
    // Every link is due again, whichever agenda widget looks next.
    if let Ok(mut g) = STATE.lock() {
        for st in g.get_or_insert_with(HashMap::new).values_mut() {
            st.fetched_at = None;
        }
    }
    #[cfg(windows)]
    crate::runloop::wake_after(Duration::ZERO);
}

pub fn take_refresh() -> bool {
    REFRESH_ASKED.swap(false, std::sync::atomic::Ordering::SeqCst)
}

const REFRESH: Duration = Duration::from_secs(30 * 60);
const RETRY: Duration = Duration::from_secs(5 * 60);
/// Which events are read: from about six weeks back (the month view can show last month) to a
/// year ahead.
const BACK_DAYS: i64 = 45;
const AHEAD_DAYS: i64 = 366;

/// What the widget shows for one link.
pub struct CalView {
    pub name: Option<String>,
    pub events: std::sync::Arc<Vec<Occurrence>>,
    pub error: Option<String>,
    pub loading: bool,
    /// Too many events: only the first ones are shown.
    pub truncated: bool,
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
        let wait = if st.error.is_some() { RETRY } else if crate::leak_test("agenda") { Duration::from_secs(60) } else { REFRESH };
        let due = st.fetched_at.map(|t| now.duration_since(t) >= wait).unwrap_or(true);
        if !st.loading && (due || force) {
            st.loading = true;
            start_download(url.clone());
        }
        out.push(CalView { name: st.name.clone(), events: st.events.clone(), error: st.error.clone(), loading: st.loading, truncated: st.truncated });
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
        let result = std::panic::catch_unwind(|| {
            download(&url).and_then(|text| {
                if !text.contains("BEGIN:VCALENDAR") {
                    return Err(crate::i18n::t("that link isn't an iCal calendar (.ics)").to_string());
                }
                let today = chrono::Local::now().date_naive();
                Ok(parse_capped(&text, today - CDuration::days(BACK_DAYS), today + CDuration::days(AHEAD_DAYS)))
            })
        })
        .unwrap_or_else(|_| Err(crate::i18n::t("that calendar has something DesktopShepherd can't read").to_string()));
        if let Ok(mut g) = STATE.lock() {
            let st = g.get_or_insert_with(HashMap::new).entry(url.clone()).or_default();
            st.loading = false;
            st.fetched_at = Some(Instant::now());
            match result {
                Ok((name, events, truncated)) => {
                    st.name = name;
                    st.truncated = truncated;
                    st.events = std::sync::Arc::new(events);
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
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(crate::i18n::t("the link must start with https://, http:// or webcal://").into());
    };
    let secure = match scheme.to_ascii_lowercase().as_str() {
        "https" | "webcal" | "webcals" => true,
        "http" => false,
        _ => return Err(crate::i18n::t("the link must start with https://, http:// or webcal://").into()),
    };
    let rest = rest.split('#').next().unwrap_or("");
    // Server part: up to the first / or ?; the rest (path and query) is what is asked for.
    let cut = rest.find(['/', '?']).unwrap_or(rest.len());
    let (hostport, path) = (&rest[..cut], &rest[cut..]);
    let path = if path.is_empty() { "/".to_string() } else if path.starts_with('?') { format!("/{path}") } else { path.to_string() };
    let path = path.as_str();
    // "user:password@server": only the server is used.
    let hostport = hostport.rsplit('@').next().unwrap_or(hostport);
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => (h, p.parse::<u16>().unwrap_or(if secure { 443 } else { 80 })),
        _ => (hostport, if secure { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err(crate::i18n::t("the link has no server name").into());
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
            return Err(crate::i18n::t("Windows' web access could not start").into());
        }
        let _ = WinHttpSetTimeouts(session.0, 20_000, 20_000, 20_000, 20_000);
        let connect = Handle(WinHttpConnect(session.0, &HSTRING::from(host), port, 0));
        if connect.0.is_null() {
            return Err(crate::i18n::tf("could not reach {}", &[&host]));
        }
        let flags = if secure { WINHTTP_FLAG_SECURE } else { WINHTTP_OPEN_REQUEST_FLAGS(0) };
        let request = Handle(WinHttpOpenRequest(connect.0, &HSTRING::from("GET"), &HSTRING::from(path), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), flags));
        if request.0.is_null() {
            return Err(crate::i18n::t("could not prepare the request").into());
        }
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(|e| crate::i18n::tf("could not reach {} ({})", &[&host, &e.message()]))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|e| crate::i18n::tf("no answer from {} ({})", &[&host, &e.message()]))?;
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
                401 | 403 => crate::i18n::tf("{} refused the link (error {}): is it the private/secret iCal address?", &[&host, &status]),
                // Google answers 404 to a calendar's public address unless it is shared publicly.
                404 if path.contains("/public/") => crate::i18n::t("that is the calendar's public address, which only works for public calendars: use its \"Secret address in iCal format\" instead").to_string(),
                404 => crate::i18n::tf("{} doesn't know that link (error 404)", &[&host]),
                _ => crate::i18n::tf("{} answered with error {}", &[&host, &status]),
            });
        }
        let mut body: Vec<u8> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            // A server that trickles bytes can't hold this thread for ever.
            if Instant::now() > deadline {
                return Err(crate::i18n::t("the download was interrupted").into());
            }
            let mut avail: u32 = 0;
            if WinHttpQueryDataAvailable(request.0, &mut avail).is_err() || avail == 0 {
                break;
            }
            let start = body.len();
            body.resize(start + avail as usize, 0);
            let mut read: u32 = 0;
            if WinHttpReadData(request.0, body[start..].as_mut_ptr() as *mut _, avail, &mut read).is_err() {
                return Err(crate::i18n::t("the download was interrupted").into());
            }
            body.truncate(start + read as usize);
            if read == 0 {
                break;
            }
            if body.len() > 8 * 1024 * 1024 {
                return Err(crate::i18n::t("that calendar is too big (over 8 MB)").into());
            }
        }
        Ok(String::from_utf8_lossy(&body).into_owned())
    }
}

#[cfg(not(windows))]
fn download(_url: &str) -> Result<String, String> {
    Err(crate::i18n::t("not available on this system").into())
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
    unescape_text(s, false)
}

/// iCal text -> plain text; `lines` keeps line breaks (descriptions), else they become spaces.
fn unescape_text(s: &str, lines: bool) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') | Some('N') => out.push(if lines { '\n' } else { ' ' }),
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
                let unit: i64 = match (c, in_time) {
                    ('W', _) => 7 * 86400,
                    ('D', _) => 86400,
                    ('H', true) => 3600,
                    ('M', true) => 60,
                    ('S', true) => 1,
                    _ => return None,
                };
                total = total.checked_add(n.checked_mul(unit)?)?;
                if total > 10 * 366 * 86400 {
                    return None; // longer than 10 years: not a real event
                }
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

/// Longest list read from one rule part (BYDAY, BYMONTHDAY): more is not a real calendar.
const MAX_RULE_LIST: usize = 64;
/// At most this many events are read from one calendar, and this many occurrences (every repeat
/// counts) in total: a hostile or huge calendar can't use up memory or time.
const MAX_EVENTS: usize = 5000;
const MAX_OCCURRENCES: usize = 20_000;
/// Total walk steps over all repeating events of one calendar.
const MAX_STEPS: usize = 3_000_000;

/// What is left to spend while reading one calendar.
struct Budget {
    occurrences: usize,
    steps: usize,
    /// Something was cut short.
    hit: bool,
}

struct Rule {
    freq: String,
    interval: i64,
    count: Option<u32>,
    until: Option<NaiveDateTime>,
    by_day: Vec<(i32, Weekday)>,
    by_month_day: Vec<i32>,
    by_month: Vec<u32>,
    /// A part of the rule can never match (e.g. BYMONTHDAY=40): the event does not repeat.
    impossible: bool,
}

fn parse_rule(v: &str) -> Option<Rule> {
    let mut r = Rule { freq: String::new(), interval: 1, count: None, until: None, by_day: Vec::new(), by_month_day: Vec::new(), by_month: Vec::new(), impossible: false };
    for part in v.split(';') {
        let Some((k, val)) = part.split_once('=') else { continue };
        match k.trim().to_ascii_uppercase().as_str() {
            "FREQ" => r.freq = val.trim().to_ascii_uppercase(),
            "INTERVAL" => r.interval = val.trim().parse::<i64>().unwrap_or(1).clamp(1, 1000),
            "COUNT" => r.count = val.trim().parse().ok(),
            "UNTIL" => r.until = parse_value(val, val.trim().len() == 8).map(|(t, all)| if all { t + CDuration::days(1) - CDuration::seconds(1) } else { t }),
            "BYDAY" => {
                for d in val.split(',') {
                    let d = d.trim().to_ascii_uppercase();
                    // The last two characters are the day ("MO"), any number before them the week.
                    let cut = d.char_indices().rev().nth(1).map(|(i, _)| i).unwrap_or(0);
                    let (num, code) = d.split_at(cut);
                    if let Some(wd) = weekday(code) {
                        if r.by_day.len() < MAX_RULE_LIST {
                            r.by_day.push((num.parse::<i32>().unwrap_or(0).clamp(-5, 5), wd));
                        }
                    }
                }
            }
            "BYMONTHDAY" => {
                let all: Vec<i32> = val.split(',').filter_map(|x| x.trim().parse().ok()).take(MAX_RULE_LIST).collect();
                r.by_month_day = all.iter().copied().filter(|d| (1..=31).contains(d) || (-31..=-1).contains(d)).collect();
                if r.by_month_day.is_empty() {
                    r.impossible = true;
                }
            }
            "BYMONTH" => {
                let mut ms: Vec<u32> = val.split(',').filter_map(|x| x.trim().parse().ok()).filter(|m| (1..=12).contains(m)).collect();
                ms.sort_unstable();
                ms.dedup();
                r.by_month = ms;
            }
            _ => {}
        }
    }
    (!r.freq.is_empty()).then_some(r)
}

/// The start times of a repeating event, from its first start up to `to` (both the window and
/// a safety limit end the walk).
fn expand(start: NaiveDateTime, rule: &Rule, to: NaiveDateTime) -> Vec<NaiveDateTime> {
    expand_with(start, rule, to, &mut Budget { occurrences: 20_000, steps: 20_000, hit: false })
}

fn expand_with(start: NaiveDateTime, rule: &Rule, to: NaiveDateTime, budget: &mut Budget) -> Vec<NaiveDateTime> {
    let mut out = Vec::new();
    if rule.impossible {
        out.push(start);
        return out;
    }
    let time = start.time();
    let mut emitted = 0u32;
    let mut push = |t: NaiveDateTime, out: &mut Vec<NaiveDateTime>, budget: &mut Budget| -> bool {
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
        if budget.occurrences == 0 {
            budget.hit = true;
            return false;
        }
        budget.occurrences -= 1;
        emitted += 1;
        out.push(t);
        true
    };
    let d0 = start.date();
    for step in 0..20_000i64 {
        if budget.steps == 0 {
            budget.hit = true;
            break;
        }
        budget.steps -= 1;
        let k = step * rule.interval;
        let mut dates: Vec<NaiveDate> = Vec::new();
        // The first day of the period being looked at: past the end, nothing more can match.
        let cursor: NaiveDate;
        match rule.freq.as_str() {
            "DAILY" => {
                let Some(d) = d0.checked_add_signed(CDuration::days(k)) else { break };
                cursor = d;
                // "Every weekday" = daily on MO..FR.
                if rule.by_day.is_empty() || rule.by_day.iter().any(|(_, wd)| *wd == d.weekday()) {
                    dates.push(d);
                }
            }
            "WEEKLY" => {
                let Some(week_start) = (d0 - CDuration::days(d0.weekday().num_days_from_monday() as i64)).checked_add_signed(CDuration::weeks(k)) else { break };
                cursor = week_start;
                if rule.by_day.is_empty() {
                    let Some(d) = d0.checked_add_signed(CDuration::weeks(k)) else { break };
                    dates.push(d);
                } else {
                    let mut ds: Vec<NaiveDate> = rule.by_day.iter().map(|(_, wd)| week_start + CDuration::days(wd.num_days_from_monday() as i64)).collect();
                    ds.sort();
                    dates.extend(ds);
                }
            }
            "MONTHLY" => {
                let m0 = d0.year() * 12 + d0.month0() as i32 + k as i32;
                let (y, m) = (m0.div_euclid(12), m0.rem_euclid(12) as u32 + 1);
                cursor = NaiveDate::from_ymd_opt(y, m, 1).unwrap_or(d0);
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
                        } else if let Some(d) = md.checked_neg().and_then(|n| nth_last_day(y, m, n)) {
                            dates.push(d);
                        }
                    }
                } else {
                    dates.extend(NaiveDate::from_ymd_opt(y, m, d0.day())); // the 31st skips short months
                }
                dates.sort();
            }
            "YEARLY" => {
                let Some(y) = i32::try_from(k).ok().and_then(|k| d0.year().checked_add(k)).filter(|y| *y < 9999) else { break };
                cursor = NaiveDate::from_ymd_opt(y, 1, 1).unwrap_or(d0);
                let months: Vec<u32> = if rule.by_month.is_empty() { vec![d0.month()] } else { rule.by_month.clone() };
                for m in months {
                    if rule.by_day.iter().any(|(n, _)| *n != 0) {
                        // e.g. "the 2nd Tuesday of November"
                        for (n, wd) in rule.by_day.iter().filter(|(n, _)| *n != 0) {
                            dates.extend(nth_weekday(y, m, *n, *wd));
                        }
                    } else {
                        dates.extend(NaiveDate::from_ymd_opt(y, m, d0.day()));
                    }
                }
                dates.sort();
            }
            _ => {
                out.push(start);
                return out;
            }
        }
        if dates.first().map(|d| d.and_time(time) > to).unwrap_or(false) || cursor.and_time(NaiveTime::MIN) > to {
            break;
        }
        for d in dates {
            if !push(d.and_time(time), &mut out, budget) {
                return out;
            }
        }
    }
    out
}

fn nth_last_day(y: i32, m: u32, n: i32) -> Option<NaiveDate> {
    let next = if m == 12 { NaiveDate::from_ymd_opt(y + 1, 1, 1)? } else { NaiveDate::from_ymd_opt(y, m + 1, 1)? };
    let d = next - CDuration::days(n as i64);
    (n >= 1 && d.month() == m).then_some(d)
}

/// A calendar's time zones: TZID -> (first change, yearly rule, offset from UTC in seconds).
type Zones = HashMap<String, Vec<(NaiveDateTime, Option<Rule>, i64)>>;

/// "+0100", "-0500", "+053000" -> seconds east of UTC.
fn parse_offset(v: &str) -> Option<i64> {
    let v = v.trim();
    let (sign, digits) = match v.chars().next()? {
        '+' => (1, &v[1..]),
        '-' => (-1, &v[1..]),
        _ => (1, v),
    };
    if digits.len() < 4 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let h: i64 = digits[0..2].parse().ok()?;
    let m: i64 = digits[2..4].parse().ok()?;
    let s: i64 = digits.get(4..6).and_then(|x| x.parse().ok()).unwrap_or(0);
    Some(sign * (h * 3600 + m * 60 + s))
}

/// A time on a zone's clock -> the same moment on this PC's clock. Uses the zone's latest
/// change at or before that time (winter / summer time); a zone the calendar doesn't describe
/// is taken as local time.
fn zone_to_local(t: NaiveDateTime, tzid: Option<&str>, zones: &Zones) -> NaiveDateTime {
    let Some(parts) = tzid.and_then(|z| zones.get(z)) else { return t };
    let mut best: Option<(NaiveDateTime, i64)> = None;
    for (start, rule, offset) in parts {
        let onset = match rule {
            Some(r) => {
                // Only the last two years of a yearly rule matter for this time.
                let mut s = *start;
                if r.freq == "YEARLY" && r.count.is_none() && t.year() - 1 > s.year() {
                    if let Some(d) = NaiveDate::from_ymd_opt(t.year() - 1, s.month(), s.day().min(28)) {
                        s = d.and_time(s.time());
                    }
                }
                expand(s, r, t).into_iter().filter(|o| *o <= t).last()
            }
            None => (*start <= t).then_some(*start),
        };
        if let Some(o) = onset {
            if best.map(|(b, _)| o > b).unwrap_or(true) {
                best = Some((o, *offset));
            }
        }
    }
    let Some((_, offset)) = best else { return t };
    let Some(utc) = t.checked_sub_signed(CDuration::seconds(offset)) else { return t };
    chrono::Utc.from_utc_datetime(&utc).with_timezone(&chrono::Local).naive_local()
}

/// Reads an iCal text into the calendar's name and the events touching [from, to], sorted.
#[cfg(test)]
pub fn parse(text: &str, from: NaiveDate, to: NaiveDate) -> (Option<String>, Vec<Occurrence>) {
    let (name, out, _) = parse_capped(text, from, to);
    (name, out)
}

/// `parse`, plus whether the calendar was too large and cut short (see `MAX_EVENTS`).
pub fn parse_capped(text: &str, from: NaiveDate, to: NaiveDate) -> (Option<String>, Vec<Occurrence>, bool) {
    let mut budget = Budget { occurrences: MAX_OCCURRENCES, steps: MAX_STEPS, hit: false };
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
    // Time zones the calendar describes: TZID -> its changes (first start, yearly rule, offset).
    let mut zones: Zones = HashMap::new();
    let mut zone_id: Option<String> = None;
    let mut part: Option<Vec<Prop>> = None; // a STANDARD or DAYLIGHT block being read
    for l in &lines {
        let upper = l.to_ascii_uppercase();
        if cur.is_none() {
            match upper.as_str() {
                "BEGIN:VTIMEZONE" => {
                    zone_id = None;
                    continue;
                }
                "END:VTIMEZONE" => {
                    zone_id = None;
                    continue;
                }
                "BEGIN:STANDARD" | "BEGIN:DAYLIGHT" => {
                    part = Some(Vec::new());
                    continue;
                }
                "END:STANDARD" | "END:DAYLIGHT" => {
                    if let (Some(id), Some(ps)) = (zone_id.as_ref(), part.take()) {
                        let get = |n: &str| ps.iter().find(|p| p.name == n);
                        let start = get("DTSTART").and_then(|p| parse_value(&p.value, false)).map(|t| t.0);
                        let offset = get("TZOFFSETTO").and_then(|p| parse_offset(&p.value));
                        if let (Some(start), Some(offset)) = (start, offset) {
                            let rule = get("RRULE").and_then(|p| parse_rule(&p.value));
                            zones.entry(id.clone()).or_default().push((start, rule, offset));
                        }
                    }
                    continue;
                }
                _ => {}
            }
            if let Some(ps) = part.as_mut() {
                if let Some(p) = parse_line(l) {
                    ps.push(p);
                }
                continue;
            }
            if upper.starts_with("TZID") {
                zone_id = parse_line(l).map(|p| p.value.trim().to_string());
                continue;
            }
        }
        if upper == "BEGIN:VEVENT" {
            cur = Some(Vec::new());
            depth_other = 0;
            continue;
        }
        if upper == "END:VEVENT" {
            if let Some(e) = cur.take() {
                if events.len() < MAX_EVENTS {
                    events.push(e);
                } else {
                    budget.hit = true;
                }
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
        let location: std::sync::Arc<str> = get("LOCATION").map(|p| unescape(&p.value)).unwrap_or_default().trim().into();
        let description: std::sync::Arc<str> = get("DESCRIPTION").map(|p| unescape_text(&p.value, true)).unwrap_or_default().trim().into();
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
                expand_with(start, &rule, to_t + CDuration::days(1), &mut budget).into_iter().filter(|t| !ex.contains(t) && !moved.contains(&(uid.clone(), *t))).collect()
            }
            _ => {
                if budget.occurrences == 0 {
                    budget.hit = true;
                    break;
                }
                budget.occurrences -= 1;
                vec![start]
            }
        };
        // Times with a named zone: worked out in that zone (above), shown in local time.
        let tzid = get("DTSTART").and_then(|p| p.param("TZID")).map(|z| z.to_string());
        for s in starts {
            let s = if all_day { s } else { zone_to_local(s, tzid.as_deref(), &zones) };
            let Some(end) = s.checked_add_signed(length) else { continue };
            // Touches the window (an event of zero length counts at its start).
            if s < to_t && (end > from_t || (end == s && s >= from_t)) {
                out.push(Occurrence { start: s, end, all_day, title: title.clone(), location: location.clone(), description: description.clone() });
            }
        }
    }
    out.sort_by(|a, b| a.start.cmp(&b.start).then(b.all_day.cmp(&a.all_day)).then(a.title.cmp(&b.title)));
    (name, out, budget.hit)
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
    fn weekdays_yearly_nth_and_bad_input() {
        let cal = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nDTSTART:20261120T080000\nRRULE:FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR;COUNT=4\nSUMMARY:Work\nEND:VEVENT\nBEGIN:VEVENT\nDTSTART;VALUE=DATE:20251111\nRRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=2TU\nSUMMARY:Second Tuesday\nEND:VEVENT\nBEGIN:VEVENT\nDTSTART:20261123T080000\nDURATION:P99999999999D\nRRULE:FREQ=DAILY;INTERVAL=99999999999;BYDAY=a\u{e9}\nSUMMARY:Broken\nEND:VEVENT\nEND:VCALENDAR\n";
        let (_, ev) = parse(cal, d(2026, 11, 1), d(2026, 11, 30));
        let v: Vec<String> = ev.iter().map(|e| format!("{} {}", e.start.format("%a %d"), e.title)).collect();
        // Fri 20, (weekend skipped) Mon 23, Tue 24, Wed 25; 2nd Tuesday of Nov 2026 = 10th; the
        // broken event is read without crashing (its huge duration is ignored).
        assert!(v.contains(&"Tue 10 Second Tuesday".to_string()), "{v:?}");
        let work: Vec<&String> = v.iter().filter(|s| s.ends_with("Work")).collect();
        assert_eq!(work, ["Fri 20 Work", "Mon 23 Work", "Tue 24 Work", "Wed 25 Work"]);
    }

    #[test]
    fn named_time_zone_is_converted() {
        // New York: UTC-5 in winter, UTC-4 in summer (second Sunday of March to first Sunday of November).
        let cal = "BEGIN:VCALENDAR\nBEGIN:VTIMEZONE\nTZID:America/New_York\nBEGIN:DAYLIGHT\nTZOFFSETFROM:-0500\nTZOFFSETTO:-0400\nDTSTART:19700308T020000\nRRULE:FREQ=YEARLY;BYMONTH=3;BYDAY=2SU\nEND:DAYLIGHT\nBEGIN:STANDARD\nTZOFFSETFROM:-0400\nTZOFFSETTO:-0500\nDTSTART:19701101T020000\nRRULE:FREQ=YEARLY;BYMONTH=11;BYDAY=1SU\nEND:STANDARD\nEND:VTIMEZONE\nBEGIN:VEVENT\nDTSTART;TZID=America/New_York:20261124T090000\nDTEND;TZID=America/New_York:20261124T100000\nSUMMARY:Winter call\nEND:VEVENT\nBEGIN:VEVENT\nDTSTART;TZID=America/New_York:20260715T090000\nSUMMARY:Summer call\nEND:VEVENT\nEND:VCALENDAR\n";
        let local = |y, m, dd, h| chrono::Utc.from_utc_datetime(&d(y, m, dd).and_hms_opt(h, 0, 0).unwrap()).with_timezone(&chrono::Local).naive_local();
        let (_, ev) = parse(cal, d(2026, 7, 1), d(2026, 11, 30));
        let winter = ev.iter().find(|e| e.title == "Winter call").unwrap();
        assert_eq!(winter.start, local(2026, 11, 24, 14)); // 09:00 New York = 14:00 UTC
        assert_eq!(winter.end - winter.start, CDuration::hours(1));
        let summer = ev.iter().find(|e| e.title == "Summer call").unwrap();
        assert_eq!(summer.start, local(2026, 7, 15, 13)); // 09:00 New York (summer) = 13:00 UTC
        assert_eq!(parse_offset("+0530"), Some(19800));
    }

    #[test]
    fn place_and_description() {
        let cal = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nDTSTART:20261124T090000\nSUMMARY:Dentist\nLOCATION:Calle Mayor 5\\, Madrid\nDESCRIPTION:Bring the card.\\nAsk about the bill.\nEND:VEVENT\nEND:VCALENDAR\n";
        let (_, ev) = parse(cal, d(2026, 11, 1), d(2026, 11, 30));
        assert_eq!(&*ev[0].location, "Calle Mayor 5, Madrid");
        assert_eq!(&*ev[0].description, "Bring the card.\nAsk about the bill.");
    }

    #[test]
    fn hostile_rules_are_cut_short() {
        // A bad day number: no repeats, no crash (including the most negative number).
        let cal = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nDTSTART:20261123T080000\nRRULE:FREQ=MONTHLY;BYMONTHDAY=-2147483648,99\nSUMMARY:Bad\nEND:VEVENT\nEND:VCALENDAR\n";
        let (_, ev, cut) = parse_capped(cal, d(2026, 11, 1), d(2027, 11, 30));
        assert_eq!(ev.len(), 1);
        assert!(!cut);
        // Too many events: capped, and flagged.
        let mut big = String::from("BEGIN:VCALENDAR\n");
        for i in 0..(MAX_EVENTS + 10) {
            big.push_str(&format!("BEGIN:VEVENT\nDTSTART:20261123T080000\nSUMMARY:E{i}\nEND:VEVENT\n"));
        }
        let (_, ev, cut) = parse_capped(&big, d(2026, 11, 1), d(2026, 11, 30));
        assert_eq!(ev.len(), MAX_EVENTS);
        assert!(cut);
        // Endless repeating events stop at the occurrence budget.
        let mut rep = String::from("BEGIN:VCALENDAR\n");
        for i in 0..200 {
            rep.push_str(&format!("BEGIN:VEVENT\nDTSTART:20000101T080000\nRRULE:FREQ=DAILY\nSUMMARY:R{i}\nEND:VEVENT\n"));
        }
        let (_, ev, cut) = parse_capped(&rep, d(1999, 1, 1), d(2030, 1, 1));
        assert!(cut);
        assert!(ev.len() <= MAX_OCCURRENCES);
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("PT1H30M"), Some(CDuration::minutes(90)));
        assert_eq!(parse_duration("P1D"), Some(CDuration::days(1)));
        assert_eq!(parse_duration("P1W"), Some(CDuration::weeks(1)));
    }
}
