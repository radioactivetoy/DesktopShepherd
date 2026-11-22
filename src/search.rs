//! Quick search: which desktop items match what was typed, best first. Pure logic.
//!
//! A name that starts with the text comes first, then one where a word starts with it
//! ("re" finds "Resident Evil" before "Firefox"), then one that merely contains it, then a
//! match on the words' first letters ("vsc" finds "Visual Studio Code"), and last a loose match whose
//! letters appear in order. Several words ("studio code") match when each starts a word. The
//! file type also counts ("pdf" finds every PDF). Ties are sorted by name.

use crate::model::DeskItem;

/// The best matches for `query`, at most `limit`, best first. Empty query = nothing.
pub fn rank<'a>(items: impl IntoIterator<Item = &'a DeskItem>, query: &str, limit: usize) -> Vec<&'a DeskItem> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let mut scored: Vec<(u8, String, &DeskItem)> = items
        .into_iter()
        .filter_map(|it| {
            let name = it.name.to_lowercase();
            score(&name, &it.ext, &q).map(|s| (s, name, it))
        })
        .collect();
    scored.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    scored.dedup_by(|a, b| a.2.path.eq_ignore_ascii_case(&b.2.path));
    scored.truncate(limit);
    scored.into_iter().map(|(_, _, it)| it).collect()
}

/// True if `q` starts a word of `name` (at the start, or after a non-letter).
fn starts_word(name: &str, q: &str) -> bool {
    name.match_indices(q).any(|(i, _)| name[..i].chars().last().map(|c| !c.is_alphanumeric()).unwrap_or(true))
}

/// 0 = starts with, 1 = a word starts with (every word of a several-word search), 2 = contains
/// (or file type), 3 = the words' first letters, 4 = letters in order.
fn score(name: &str, ext: &str, q: &str) -> Option<u8> {
    if name.starts_with(q) {
        return Some(0);
    }
    if starts_word(name, q) {
        return Some(1);
    }
    let words: Vec<&str> = q.split_whitespace().collect();
    if words.len() > 1 && words.iter().all(|w| starts_word(name, w)) {
        return Some(1);
    }
    if name.contains(q) || ext == q {
        return Some(2);
    }
    let initials: String = name.split(|c: char| !c.is_alphanumeric()).filter_map(|w| w.chars().next()).collect();
    if q.chars().count() >= 2 && initials.starts_with(q) {
        return Some(3);
    }
    if q.chars().count() >= 2 && in_order(name, q) {
        return Some(4);
    }
    None
}

/// Every letter of `q` appears in `name`, in the same order.
fn in_order(name: &str, q: &str) -> bool {
    let mut it = name.chars();
    q.chars().filter(|c| !c.is_whitespace()).all(|c| it.any(|n| n == c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Category;

    fn item(name: &str, ext: &str) -> DeskItem {
        DeskItem {
            path: format!("C:\\Desktop\\{name}.{ext}"),
            name: name.to_string(),
            ext: ext.to_string(),
            category: Category::Other,
            is_dir: false,
            on_desktop: true,
            size: 0,
            modified: 0,
        }
    }

    #[test]
    fn best_matches_first() {
        let items = [item("Firefox", "lnk"), item("Resident Evil", "lnk"), item("Reaper", "lnk"), item("Paint", "lnk")];
        let names: Vec<&str> = rank(&items, "re", 10).iter().map(|i| i.name.as_str()).collect();
        // starts-with (sorted by name), then contains
        assert_eq!(names, ["Reaper", "Resident Evil", "Firefox"]);
    }

    #[test]
    fn word_starts_beat_contains() {
        let items = [item("Ace Player", "lnk"), item("Stardew Valley", "lnk")];
        let names: Vec<&str> = rank(&items, "pla", 10).iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Ace Player"]);
        let names: Vec<&str> = rank(&items, "val", 10).iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Stardew Valley"]);
    }

    #[test]
    fn letters_in_order_and_file_type() {
        let items = [item("Visual Studio Code", "lnk"), item("report", "pdf"), item("notes", "txt")];
        assert_eq!(rank(&items, "vsc", 10)[0].name, "Visual Studio Code");
        assert_eq!(rank(&items, "pdf", 10)[0].name, "report");
        assert!(rank(&items, "", 10).is_empty());
        assert!(rank(&items, "zzz", 10).is_empty());
    }

    #[test]
    fn initials_and_several_words() {
        let items = [item("Visual Studio Code", "lnk"), item("Vast Sky Clouds", "lnk"), item("Viscosity", "lnk")];
        // "vsc": both initials matches first (by name), then the loose match.
        let names: Vec<&str> = rank(&items, "vsc", 10).iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Vast Sky Clouds", "Visual Studio Code", "Viscosity"]);
        let names: Vec<&str> = rank(&items, "studio code", 10).iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Visual Studio Code"]);
        // Initials beat loose letters-in-order.
        let items = [item("Sound Tester", "lnk"), item("Crucial Storage Executive", "lnk")];
        let names: Vec<&str> = rank(&items, "cse", 10).iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["Crucial Storage Executive"]);
    }

    #[test]
    fn limit_is_respected() {
        let items: Vec<DeskItem> = (0..30).map(|i| item(&format!("file {i}"), "txt")).collect();
        assert_eq!(rank(&items, "file", 12).len(), 12);
    }
}
