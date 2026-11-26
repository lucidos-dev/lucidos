pub(crate) const MERGE_GAP_SECS: f64 = 5.0;

const CLITIC_OPENERS: &[char] = &[
    '.', ',', '!', '?', ';', ':', '…', ')', ']', '}', '%', '\'', '\u{2019}', '"', '\u{201D}',
];

pub(crate) fn is_one_utterance(gap_secs: f64, same_speaker: bool) -> bool {
    same_speaker && (0.0..=MERGE_GAP_SECS).contains(&gap_secs)
}

pub fn join_spoken(first: &str, second: &str) -> String {
    let first = first.trim();
    let second = second.trim();
    if first.is_empty() {
        return second.to_string();
    }
    if second.is_empty() {
        return first.to_string();
    }
    let attaches = second
        .chars()
        .next()
        .is_some_and(|c| CLITIC_OPENERS.contains(&c));
    if attaches {
        format!("{}{}", first, second)
    } else {
        format!("{} {}", first, second)
    }
}

#[derive(serde::Serialize)]
struct MergeCase {
    pub name: &'static str,
    pub first: &'static str,
    pub second: &'static str,
    pub gap_secs: f64,
    pub same_speaker: bool,
    pub merges: bool,
    pub joined: String,
}

fn cases() -> Vec<MergeCase> {
    const INPUTS: &[(&str, &str, &str, f64, bool)] = &[
        ("a comma clause attaches", "Status", ", please", 0.4, true),
        (
            "a full stop attaches",
            "Nothing is waiting on you",
            ".",
            1.0,
            true,
        ),
        (
            "a word takes a space",
            "Still",
            "in it. I'm pulling the threads.",
            0.2,
            true,
        ),
        (
            "a long pause is two things",
            "Status",
            ", please",
            20.0,
            true,
        ),
        (
            "two speakers never merge",
            "What's the status",
            "Nothing is waiting.",
            0.3,
            false,
        ),
        (
            "a blank row adds nothing",
            "Built and notarized.",
            "   ",
            0.5,
            true,
        ),
        (
            "the bound itself merges",
            "Okay",
            "so what is next?",
            MERGE_GAP_SECS,
            true,
        ),
        (
            "a hair past the bound does not",
            "Okay",
            "so what is next?",
            MERGE_GAP_SECS + 0.001,
            true,
        ),
        (
            "a closing quote attaches",
            "He said \u{201C}no",
            "\u{201D} and left.",
            0.3,
            true,
        ),
    ];
    INPUTS
        .iter()
        .map(|(name, first, second, gap_secs, same_speaker)| MergeCase {
            name,
            first,
            second,
            gap_secs: *gap_secs,
            same_speaker: *same_speaker,
            merges: is_one_utterance(*gap_secs, *same_speaker),
            joined: join_spoken(first, second),
        })
        .collect()
}

pub fn render_fixture() -> String {
    let mut json = serde_json::to_string_pretty(&serde_json::json!({
        "merge_gap_secs": MERGE_GAP_SECS,
        "cases": cases(),
    }))
    .expect("the merge cases serialize");
    json.push('\n');
    json
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clitic_needs_no_space() {
        assert_eq!(join_spoken("Status", ", please"), "Status, please");
        assert_eq!(
            join_spoken("Nothing is waiting", "."),
            "Nothing is waiting."
        );
    }

    #[test]
    fn a_word_takes_one_space() {
        assert_eq!(join_spoken("Still", "in it."), "Still in it.");
        assert_eq!(join_spoken("Still ", " in it."), "Still in it.");
    }

    #[test]
    fn a_blank_piece_contributes_nothing() {
        assert_eq!(join_spoken("Done.", "   "), "Done.");
        assert_eq!(join_spoken("", "Done."), "Done.");
    }

    #[test]
    fn the_bound_is_inclusive_and_a_long_pause_is_two_things() {
        assert!(is_one_utterance(MERGE_GAP_SECS, true));
        assert!(!is_one_utterance(MERGE_GAP_SECS + 0.001, true));
        assert!(!is_one_utterance(20.0, true));
    }

    #[test]
    fn two_speakers_are_never_one_utterance() {
        assert!(!is_one_utterance(0.1, false));
    }

    #[test]
    fn a_backwards_gap_never_merges() {
        assert!(!is_one_utterance(-0.5, true));
    }
}
