use super::*;

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
