//! The fixture the frontend's `title_rank` twin is held to.

use super::*;

/// Every case the frontend twin is held to.
const CASES: &[(&str, &str)] = &[
    ("Settings", "settings"),
    ("Fix  Search", " fix search "),
    ("Open settings (⌘,)", "settings"),
    ("settings-system-v2.png", "settings"),
    ("test_capture_settings.py", "settings"),
    ("TimeoutSettings.d.ts", "settings"),
    ("Thread search ranking", "SEARCH"),
    ("Research notes", "search"),
    ("Research and search", "search"),
    ("Ranking for search", "search ranking"),
    ("Habit Tracker", "settings"),
    ("Anything", "   "),
    ("Ümlaut über alles", "über"),
    ("naïveÜber", "über"),
    ("Café 2", "2"),
    // Matches never overlap, so the word start at 3 is never reached.
    ("xa-a-a", "a-a"),
    // A combining vowel sign is alphabetic, so it is no word boundary.
    ("राम", "म"),
    // Whitespace is the Unicode White_Space property on both sides.
    ("a\u{85}b", "a b"),
    ("a\u{feff}b", "a b"),
];

pub(crate) fn render_fixture() -> String {
    let cases: Vec<serde_json::Value> = CASES
        .iter()
        .map(|(title, query)| {
            serde_json::json!({
                "title": title,
                "query": query,
                "expected": title_rank(title, query),
            })
        })
        .collect();
    let mut out = serde_json::to_string_pretty(&cases).expect("fixture serializes");
    out.push('\n');
    out
}
