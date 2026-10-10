use super::*;

/// Every case the frontend twin is held to. Add a case here, then regenerate.
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

fn generate_title_match_fixture() -> String {
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

fn fixture_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("lucidos-app/src/generated/title-match-fixture.json")
}

#[test]
#[ignore]
fn generate_title_match_fixture_file() {
    std::fs::write(fixture_path(), generate_title_match_fixture()).expect("fixture writes");
}

#[test]
fn title_match_fixture_is_up_to_date() {
    let existing = std::fs::read_to_string(fixture_path()).unwrap_or_default();
    assert_eq!(
        existing,
        generate_title_match_fixture(),
        "Title-match fixture is stale. Run: cargo test -p lucidos-engine --lib generate_title_match_fixture_file -- --ignored"
    );
}

#[test]
fn title_match_ignores_case_and_spacing() {
    assert_eq!(
        title_match("Fix  Search", " fix search "),
        TitleMatch::Exact
    );
    assert_eq!(title_match("Anything", "   "), TitleMatch::None);
}

#[test]
fn a_query_that_starts_a_word_beats_one_inside_a_word() {
    assert_eq!(
        title_match("Open settings (⌘,)", "settings"),
        TitleMatch::WordStart
    );
    assert_eq!(
        title_match("test_capture_settings.py", "settings"),
        TitleMatch::WordStart
    );
    assert_eq!(
        title_match("TimeoutSettings.d.ts", "settings"),
        TitleMatch::Phrase
    );
    assert_eq!(title_match("Research notes", "search"), TitleMatch::Phrase);
}

#[test]
fn a_later_word_start_is_found_past_an_earlier_mid_word_hit() {
    assert_eq!(
        title_match("Research and search", "search"),
        TitleMatch::WordStart
    );
}

#[test]
fn tokens_out_of_order_are_no_match() {
    assert_eq!(
        title_match("Ranking for search", "search ranking"),
        TitleMatch::None
    );
}

#[test]
fn coverage_breaks_a_tie_within_one_level() {
    let shortcut = title_rank("Open settings (⌘,)", "settings");
    let file = title_rank("settings-system-v2.png", "settings");
    assert_eq!(shortcut.level, file.level);
    assert_eq!(TitleRank::best_first(&shortcut, &file), Ordering::Less);
}

#[test]
fn a_stronger_level_wins_whatever_the_coverage() {
    let word_start = title_rank("A very long title naming settings somewhere", "settings");
    let phrase = title_rank("xsettings", "settings");
    assert!(phrase.coverage > word_start.coverage);
    assert_eq!(TitleRank::best_first(&word_start, &phrase), Ordering::Less);
}

#[test]
fn no_title_match_has_no_coverage() {
    assert_eq!(title_rank("Habit Tracker", "settings").coverage, 0.0);
}
