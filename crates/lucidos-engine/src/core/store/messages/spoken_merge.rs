//! When two spoken rows are one thing said, and how their words join.
//!
//! A voice provider ends a speaker's turn after a fraction of a second of
//! silence, so it cuts a sentence wherever the speaker breathes. Every turn is
//! written down as it finishes, which is what lets the transcript read by the
//! clock alone (ADR 0201). Putting the pieces back together is a READING of
//! those rows, done in two places: here for the doer's conversation history,
//! and in `store/thread-events/spokenMerge.ts` for the transcript.
//!
//! **One rule, two languages, proven identical.** The cases below are
//! generated into a fixture the TypeScript side replays, so the two cannot
//! drift. Regenerate with:
//!
//! ```text
//! cargo test -p lucidos-engine --lib -- --ignored generate_spoken_merge_fixture_file
//! ```

/// How far apart two spoken rows may be and still be one thing said.
///
/// A reported call held gaps of 1.0s, 2.0s, 1.3s and 3.6s INSIDE single
/// sentences, so anything under about four seconds is ordinary speech. Five
/// clears that with margin.
///
/// A bound could not decide a turn's end, which is what ADR 0187 and ADR 0188
/// each proved the hard way: that question is asked live, with no idea whether
/// more words are coming. This one is asked afterwards, holding both
/// timestamps, and it only decides how words are grouped on screen.
pub(crate) const MERGE_GAP_SECS: f64 = 5.0;

/// Characters that attach to the word before them, with no space.
///
/// The seam is where the transcriber cut, so the next piece often opens with
/// the punctuation that ended the last one. `Status` and `, please` are one
/// sentence, and a space between them would be wrong.
const CLITIC_OPENERS: &[char] = &[
    '.', ',', '!', '?', ';', ':', '…', ')', ']', '}', '%', '\'', '\u{2019}', '"', '\u{201D}',
];

/// Should these two rows read as one thing said?
///
/// Both halves are required. Adjacency is decided by the caller, which walks
/// the rows in clock order: anything from another speaker between them means
/// they are two things, however close the clock says they are.
pub(crate) fn is_one_utterance(gap_secs: f64, same_speaker: bool) -> bool {
    same_speaker && (0.0..=MERGE_GAP_SECS).contains(&gap_secs)
}

/// Join one piece of speech onto another.
///
/// No space before a clitic, one space otherwise. Empty pieces contribute
/// nothing, so a merge over a blank row reads as though it were not there.
pub(crate) fn join_spoken(first: &str, second: &str) -> String {
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

/// One generated case: the inputs, and what both languages must answer.
#[cfg(test)]
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

/// Every case the fixture pins. Add one here and regenerate.
#[cfg(test)]
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

/// The fixture the TypeScript side replays, as JSON.
///
/// Pretty-printed with a trailing newline, matching every other generated file
/// in `crates/lucidos-app/src/generated/`.
#[cfg(test)]
fn generate_fixture() -> String {
    let mut json = serde_json::to_string_pretty(&serde_json::json!({
        "merge_gap_secs": MERGE_GAP_SECS,
        "cases": cases(),
    }))
    .expect("the merge cases serialize");
    json.push('\n');
    json
}

/// Where the fixture lives, beside the other generated contract files.
#[cfg(test)]
fn fixture_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate has a parent directory")
        .join("lucidos-app/src/generated/spoken-merge-fixture.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spoken_merge_fixture_is_up_to_date() {
        let generated = generate_fixture();
        let path = fixture_path();
        let existing = std::fs::read_to_string(&path).unwrap_or_else(|_| {
            panic!(
                "The spoken-merge fixture does not exist at {}. Run: cargo test -p lucidos-engine --lib -- --ignored generate_spoken_merge_fixture_file",
                path.display()
            )
        });
        assert_eq!(
            existing, generated,
            "The spoken-merge fixture is stale. Run: cargo test -p lucidos-engine --lib -- --ignored generate_spoken_merge_fixture_file"
        );
    }

    #[test]
    #[ignore]
    fn generate_spoken_merge_fixture_file() {
        let path = fixture_path();
        std::fs::create_dir_all(path.parent().expect("the fixture has a directory"))
            .expect("the generated directory is writable");
        std::fs::write(&path, generate_fixture()).expect("the fixture is writable");
        crate::log!("[ContractTest] Generated: {}", path.display());
    }

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

    /// A row that reads as written BEFORE the one in front of it is not a
    /// neighbour, it is a clock nobody can trust. Merging there would glue
    /// rows the reader sees in the other order.
    #[test]
    fn a_backwards_gap_never_merges() {
        assert!(!is_one_utterance(-0.5, true));
    }
}
