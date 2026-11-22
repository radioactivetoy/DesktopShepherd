//! Translations: English (the text in the code), Spanish and Catalan.
//!
//! Every text the person sees goes through `t("English text")`, which returns the text in the
//! chosen language (Settings > Language; "Automatic" follows Windows' display language). Texts
//! with values use `{}` placeholders filled in order by `tf`. A text missing from a table simply
//! shows in English. Month and day names come from `month_name` / `weekday_name`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    En,
    Es,
    Ca,
}

/// 0 = English, 1 = Spanish, 2 = Catalan (the language in use now).
static LANG: AtomicU8 = AtomicU8::new(0);

/// The choices in Settings, in order: (setting value, name shown in its own language).
pub const CHOICES: &[(u8, &str)] = &[(0, "Automatic (Windows)"), (1, "English"), (2, "Español"), (3, "Català")];

/// Windows' display language for this user, as one of ours (English if neither).
pub fn windows_lang() -> Lang {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Globalization::GetUserDefaultUILanguage;
        // The low 10 bits are the primary language: 0x0A Spanish, 0x03 Catalan.
        match GetUserDefaultUILanguage() & 0x3FF {
            0x0A => return Lang::Es,
            0x03 => return Lang::Ca,
            _ => {}
        }
    }
    Lang::En
}

/// Applies the setting (0 = automatic, 1 = English, 2 = Spanish, 3 = Catalan).
pub fn set_lang(setting: u8) {
    let l = match setting {
        1 => Lang::En,
        2 => Lang::Es,
        3 => Lang::Ca,
        _ => windows_lang(),
    };
    LANG.store(
        match l {
            Lang::En => 0,
            Lang::Es => 1,
            Lang::Ca => 2,
        },
        Ordering::Relaxed,
    );
}

pub fn lang() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        1 => Lang::Es,
        2 => Lang::Ca,
        _ => Lang::En,
    }
}

fn table(l: Lang) -> &'static HashMap<&'static str, &'static str> {
    static ES_MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    static CA_MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    static EMPTY: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    match l {
        Lang::Es => ES_MAP.get_or_init(|| ES.iter().copied().collect()),
        Lang::Ca => CA_MAP.get_or_init(|| CA.iter().copied().collect()),
        Lang::En => EMPTY.get_or_init(HashMap::new),
    }
}

/// The text in the language in use (the English text if it has no translation).
pub fn t(s: &'static str) -> &'static str {
    match lang() {
        Lang::En => s,
        l => table(l).get(s).copied().unwrap_or(s),
    }
}

/// `t` with `{}` placeholders filled in order: tf("{} items", &[&n]).
pub fn tf(s: &'static str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = String::new();
    let mut rest = t(s);
    for a in args {
        match rest.find("{}") {
            Some(i) => {
                out.push_str(&rest[..i]);
                out.push_str(&a.to_string());
                rest = &rest[i + 2..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// Full month name (1 = January).
pub fn month_name(m: u32) -> &'static str {
    const EN: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    const ES_M: [&str; 12] = ["enero", "febrero", "marzo", "abril", "mayo", "junio", "julio", "agosto", "septiembre", "octubre", "noviembre", "diciembre"];
    const CA_M: [&str; 12] = ["gener", "febrer", "març", "abril", "maig", "juny", "juliol", "agost", "setembre", "octubre", "novembre", "desembre"];
    let i = (m.clamp(1, 12) - 1) as usize;
    match lang() {
        Lang::En => EN[i],
        Lang::Es => ES_M[i],
        Lang::Ca => CA_M[i],
    }
}

/// Short month name (1 = Jan).
pub fn month_short(m: u32) -> &'static str {
    const EN: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    const ES_M: [&str; 12] = ["ene", "feb", "mar", "abr", "may", "jun", "jul", "ago", "sept", "oct", "nov", "dic"];
    const CA_M: [&str; 12] = ["gen", "febr", "març", "abr", "maig", "juny", "jul", "ag", "set", "oct", "nov", "des"];
    let i = (m.clamp(1, 12) - 1) as usize;
    match lang() {
        Lang::En => EN[i],
        Lang::Es => ES_M[i],
        Lang::Ca => CA_M[i],
    }
}

/// Full day name (0 = Monday).
pub fn weekday_name(d: u32) -> &'static str {
    const EN: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
    const ES_D: [&str; 7] = ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"];
    const CA_D: [&str; 7] = ["dilluns", "dimarts", "dimecres", "dijous", "divendres", "dissabte", "diumenge"];
    let i = (d % 7) as usize;
    match lang() {
        Lang::En => EN[i],
        Lang::Es => ES_D[i],
        Lang::Ca => CA_D[i],
    }
}

/// Two-letter day name (0 = Monday): Mo / Lu / Dl.
pub fn weekday_short(d: u32) -> &'static str {
    const EN: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
    const ES_D: [&str; 7] = ["Lu", "Ma", "Mi", "Ju", "Vi", "Sá", "Do"];
    const CA_D: [&str; 7] = ["Dl", "Dt", "Dc", "Dj", "Dv", "Ds", "Dg"];
    let i = (d % 7) as usize;
    match lang() {
        Lang::En => EN[i],
        Lang::Es => ES_D[i],
        Lang::Ca => CA_D[i],
    }
}

/// "Sunday, 22 November" / "domingo, 22 de noviembre" / "diumenge, 22 de novembre".
pub fn long_date(d: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    let wd = weekday_name(d.weekday().num_days_from_monday());
    let m = month_name(d.month());
    match lang() {
        Lang::En => format!("{wd}, {} {m}", d.day()),
        Lang::Es | Lang::Ca => {
            let de = if lang() == Lang::Ca && matches!(m.chars().next(), Some('a' | 'o')) { "d'" } else { "de " };
            format!("{wd}, {} {de}{m}", d.day())
        }
    }
}

/// "November 2026" / "noviembre de 2026" / "novembre de 2026".
pub fn month_year(d: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    let m = month_name(d.month());
    match lang() {
        Lang::En => format!("{m} {}", d.year()),
        _ => format!("{m} de {}", d.year()),
    }
}

/// "22 Nov" / "22 nov".
pub fn short_date(d: chrono::NaiveDate) -> String {
    use chrono::Datelike;
    format!("{} {}", d.day(), month_short(d.month()))
}

// ---- tables: (English text exactly as in the code, translation) -------------------------------
// Keep each table in the same order as the other; a missing entry shows in English.

pub static ES: &[(&str, &str)] = &[
    ("Automatic (Windows)", "Automático (Windows)"),
    ("Language", "Idioma"),
];

pub static CA: &[(&str, &str)] = &[
    ("Automatic (Windows)", "Automàtic (Windows)"),
    ("Language", "Idioma"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_and_dates() {
        LANG.store(1, Ordering::Relaxed);
        assert_eq!(t("Language"), "Idioma");
        assert_eq!(t("not translated"), "not translated");
        let d = chrono::NaiveDate::from_ymd_opt(2026, 11, 22).unwrap();
        assert_eq!(long_date(d), "domingo, 22 de noviembre");
        LANG.store(2, Ordering::Relaxed);
        assert_eq!(month_year(d), "novembre de 2026");
        let d = chrono::NaiveDate::from_ymd_opt(2026, 8, 3).unwrap();
        assert_eq!(long_date(d), "dilluns, 3 d'agost");
        LANG.store(0, Ordering::Relaxed);
        assert_eq!(tf("{} of {}", &[&1, &2]), "1 of 2");
    }
}
