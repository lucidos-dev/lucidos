//! The doer does not know voice exists.
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
//!
//! **Two rings, because a file can be on the turn's path without being on the
//! model's.** `engine/chat/` orchestrates a turn AND assembles its prompt. The
//! orchestration half legitimately records that a message was spoken, so there
//! the word is allowed in exactly one shape: passing `voice_session_id` along.
//! The assembly half builds prompt text, and there it is banned outright.
//!
//! The outer ring's two arms are what the single word scan used to be. One
//! stops voice reaching a model as TEXT, the other stops it reaching one as a
//! DECISION. A turn that branched on a live call would answer the same question
//! two ways, which is the failure ADR 0149 names.

use crate::test_support::source_scan::{production_sources, src_root};

/// Every engine source that helps run the Lucidos Agent's turn.
const PROMPT_PATH_PREFIXES: &[&str] = &[
    "engine/chat/",
    "engine/agent_session/prompts.rs",
    "engine/context.rs",
    "engine/tools/capabilities.rs",
];

/// The inner ring: sources that turn thread state into what the model reads.
///
/// Nothing here may name voice at all, not even in a comment. A file that
/// cannot spell the concept cannot render it into a prompt, and this is the
/// list where that bite is worth its cost.
const PROMPT_ASSEMBLY_FILES: &[&str] = &[
    "engine/chat/process/context_build.rs",
    "engine/chat/process/context_mode.rs",
    "engine/chat/process/context_sections.rs",
    "engine/chat/process/history.rs",
    "engine/chat/process/system_prompt.rs",
    "engine/chat/process/working_understanding.rs",
    "engine/chat/process/workspace_payload.rs",
    "engine/agent_session/prompts.rs",
    "engine/context.rs",
    "engine/tools/capabilities.rs",
];

/// Prose about writing style, which reaches no model and is all over the tree.
const ALLOWED_PHRASES: &[&str] = &["active voice", "passive voice"];

fn scrubbed(text: &str) -> String {
    let mut haystack = text.to_lowercase();
    for phrase in ALLOWED_PHRASES {
        haystack = haystack.replace(phrase, "");
    }
    haystack
}

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

/// Every double-quoted literal in `text`, with escapes left as written.
///
/// Good enough for a word scan. A raw or byte string still yields its body,
/// and a miss costs a false pass on a shape no prompt uses.
fn string_literals(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut body = String::new();
        for inner in chars.by_ref() {
            match inner {
                '"' => break,
                _ => body.push(inner),
            }
        }
        out.push(body);
    }
    out
}

/// The outer ring: no text a model could read may say a session is live.
///
/// A prompt is built out of string literals. The word may appear in an
/// identifier being plumbed (`voice_session_id` rides `MessageReceived`) and in
/// a comment explaining it. Neither reaches a model.
#[test]
fn nothing_the_doer_reads_mentions_voice() {
    let mut offenders = Vec::new();
    for (rel, text) in prompt_path_sources() {
        for literal in string_literals(&scrubbed(&text)) {
            if literal.contains("voice") {
                offenders.push(format!("{}: {:?}", rel, literal));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "The doer must never be told a voice session is live (ADR 0149). \
         These prompt-path literals mention voice: {:?}",
        offenders
    );
}

/// The one shape the outer ring allows: handing the message's own field on.
///
/// A parameter, or an argument passed by name. Anything else, most of all a
/// condition, is a turn deciding something from the fact that a call is up.
fn passes_the_field_along(code: &str) -> bool {
    let line = code.trim();
    line == "voice_session_id," || line.starts_with("voice_session_id: Option<Uuid>")
}

/// The outer ring's second arm: nothing there may DECIDE anything from voice.
///
/// Comments are cut before the check, so a field can still be explained. That
/// also truncates a line at a `//` inside a literal, which costs nothing: the
/// arm above reads literals, and this one reads code.
#[test]
fn the_prompt_path_only_passes_the_field_along() {
    let mut offenders = Vec::new();
    for (rel, text) in prompt_path_sources() {
        for (index, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            if !scrubbed(code).contains("voice") {
                continue;
            }
            if passes_the_field_along(code) {
                continue;
            }
            offenders.push(format!("{}:{}", rel, index + 1));
        }
    }
    assert!(
        offenders.is_empty(),
        "A turn may pass `voice_session_id` along and nothing else (ADR 0149). \
         These lines do something else with voice: {:?}",
        offenders
    );
}

/// The inner ring: prompt assembly cannot name voice at all.
#[test]
fn the_doers_prompt_assembly_never_mentions_voice() {
    let mut offenders = Vec::new();
    for (rel, text) in prompt_path_sources() {
        if !PROMPT_ASSEMBLY_FILES.contains(&rel.as_str()) {
            continue;
        }
        for (index, line) in scrubbed(&text).lines().enumerate() {
            if line.contains("voice") {
                offenders.push(format!("{}:{}", rel, index + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "Prompt assembly must not know voice exists (ADR 0149). These lines \
         mention it: {:?}",
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

/// The guard is only worth anything if its paths still resolve. A renamed file
/// would otherwise make it pass by scanning nothing.
#[test]
fn every_prompt_path_still_exists() {
    for prefix in PROMPT_PATH_PREFIXES {
        let path = src_root().join(prefix.trim_end_matches('/'));
        assert!(path.exists(), "{} no longer exists", prefix);
    }
    for file in PROMPT_ASSEMBLY_FILES {
        let path = src_root().join(file);
        assert!(path.exists(), "{} no longer exists", file);
    }
}
