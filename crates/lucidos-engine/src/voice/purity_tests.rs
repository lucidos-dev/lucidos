//! The reasoner does not know voice exists.
//!
//! ADR 0149: the Lucidos Agent is shown a voice session by reading the talker's
//! turns, never told about one. Break that and the same question gets two
//! answers by input channel. That is the one thing a user can neither see nor
//! predict.
//!
//! A source scan rather than a prompt diff, because the prompt is assembled
//! from a live engine and the property is structural: a prompt file that names
//! nothing voice-shaped cannot say a session is live. It also catches the real
//! failure earlier, at the first `use`.

use crate::test_support::source_scan::{production_sources, src_root};

/// Every engine source that helps assemble what the Lucidos Agent reads.
const PROMPT_PATH_PREFIXES: &[&str] = &[
    "engine/chat/",
    "engine/agent_session/prompts.rs",
    "engine/context.rs",
    "engine/tools/capabilities.rs",
];

/// Prose about writing style, which reaches no model and is all over the tree.
const ALLOWED_PHRASES: &[&str] = &["active voice", "passive voice"];

fn prompt_path_sources() -> Vec<(String, String)> {
    let sources: Vec<(String, String)> = production_sources()
        .into_iter()
        .filter(|(rel, _)| PROMPT_PATH_PREFIXES.iter().any(|p| rel.starts_with(p)))
        .collect();
    assert!(
        sources.len() > 10,
        "the prompt-path prefixes matched almost nothing, so this guard is \
         scanning the wrong tree: {:?}",
        sources.iter().map(|(rel, _)| rel).collect::<Vec<_>>()
    );
    sources
}

#[test]
fn the_reasoners_prompt_path_never_mentions_voice() {
    let mut offenders = Vec::new();
    for (rel, text) in prompt_path_sources() {
        let mut haystack = text.to_lowercase();
        for phrase in ALLOWED_PHRASES {
            haystack = haystack.replace(phrase, "");
        }
        for (index, line) in haystack.lines().enumerate() {
            if line.contains("voice") {
                offenders.push(format!("{}:{}", rel, index + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "The reasoner must never be told a voice session is live (ADR 0149). \
         These prompt-path lines mention voice: {:?}",
        offenders
    );
}

/// The other half: the live-session registry must have no reader on that path.
///
/// A file could learn a call is up without writing the word in a sentence.
/// This is the symbol it would have to reach for.
#[test]
fn nothing_on_the_prompt_path_reads_the_live_session_registry() {
    for (rel, text) in prompt_path_sources() {
        assert!(
            !text.contains("voice_sessions"),
            "{} reads the live voice-session registry (ADR 0149)",
            rel
        );
    }
}

/// The guard is only worth anything if the prefixes still resolve. A renamed
/// directory would otherwise make it pass by scanning nothing.
#[test]
fn every_prompt_path_prefix_still_exists() {
    for prefix in PROMPT_PATH_PREFIXES {
        let path = src_root().join(prefix.trim_end_matches('/'));
        assert!(path.exists(), "{} no longer exists", prefix);
    }
}
