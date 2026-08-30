//! The resident context block: what a voice session can answer with no wait.
//!
//! A tool-less talker cannot look anything up mid-sentence (ADR 0149). So this
//! block is the whole of what voice answers instantly, which makes it a product
//! decision rather than a tuning detail.
//!
//! **Configurable and dynamic.** It is a registry of named sections, each with
//! a builder that runs at session open. The `voice_resident_sections`
//! preference names which are on, so adding a section is one entry here and
//! turning one off is a preference edit.
//!
//! **Snapshot at open, refreshed by appending.** The block enters the session as
//! its first history item and is never rewritten. Rewriting it would invalidate
//! the cached prefix behind it, which is the append-only invariant the plan
//! carries.

use std::collections::HashSet;

use crate::core::{PreferenceStore, PREF_VOICE_RESIDENT_SECTIONS};
use crate::engine::LucidosEngine;

use super::sections::{ResidentSection, SECTIONS};

/// The heading the block opens with, so the talker can tell it from a turn.
const BLOCK_HEADING: &str = "[WHAT YOU ALREADY KNOW]";

/// Which sections a stored preference names.
///
/// **A row that exists means exactly what it lists, and an EMPTY one means
/// none.** Only `None`, a row that was never written, falls back to the default
/// set. The two used to be one case, which made the last section impossible to
/// turn off: clearing it read as "never set" and brought all three back.
///
/// A named section nobody defines is dropped with a log line: a typo must not
/// cost the user the whole block.
///
/// Ordered by the registry, never by the preference. The block then reads the
/// same way whatever order the reader toggled them in.
fn sections_from(stored: Option<&str>) -> Vec<&'static ResidentSection> {
    let Some(stored) = stored else {
        return SECTIONS.iter().filter(|s| s.on_by_default).collect();
    };

    let wanted: HashSet<&str> = stored
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    for name in &wanted {
        if !SECTIONS.iter().any(|s| s.id == *name) {
            log!(
                "[Voice] {} names an unknown section '{}'. Ignoring it",
                PREF_VOICE_RESIDENT_SECTIONS,
                name
            );
        }
    }
    SECTIONS.iter().filter(|s| wanted.contains(s.id)).collect()
}

/// Which sections a session opens with, read from the preference store.
///
/// A read error opens with NO sections, not with the defaults. An unreadable
/// row is unknown rather than unset, and the two answers are no longer the
/// same: since an empty row means "none", guessing the defaults would hand the
/// talker the thread and the workspace shape that this reader deliberately
/// turned off. The call still goes up, knowing less (`.claude/rules/rust.md`).
pub async fn enabled_sections(engine: &LucidosEngine) -> Vec<&'static ResidentSection> {
    match PreferenceStore::get(engine.pool(), PREF_VOICE_RESIDENT_SECTIONS).await {
        Ok(stored) => sections_from(stored.as_deref()),
        Err(e) => {
            log!(
                "[Voice] Could not read {}: {}. Opening with no resident block",
                PREF_VOICE_RESIDENT_SECTIONS,
                e
            );
            vec![]
        }
    }
}

/// Build the block this session opens with.
///
/// Every builder runs now, so a section reports the workspace as it is rather
/// than as it was. A builder that fails is skipped with a log line: a section
/// the talker cannot see is better than a session that cannot open.
///
/// Never empty: with no sections on, [`assemble_block`] says that in words.
pub async fn build_block(engine: &LucidosEngine, thread_id: uuid::Uuid) -> String {
    let mut built: Vec<(&'static str, String)> = Vec::new();

    for section in enabled_sections(engine).await {
        match (section.build)(engine, thread_id).await {
            Ok(body) if body.trim().is_empty() => {}
            Ok(body) => built.push((section.title, body)),
            Err(e) => log!(
                "[Voice] Section '{}' failed: {}. Skipping it",
                section.id,
                e
            ),
        }
    }
    assemble_block(&built)
}

/// Lay the built sections out under the heading.
///
/// Split from [`build_block`] so the layout is a test rather than something
/// only a live engine can exercise. Every builder needs one; this needs none.
///
/// **With no sections it says so, rather than returning nothing.** The talker's
/// instructions name a context block, and they are the cached prefix, so they
/// cannot vary. An absent block would leave that sentence pointing at nothing,
/// and the cheapest reading is that a block arrived and covered the question.
/// Under-calling the doer is the expensive mistake (`voice::mod`).
fn assemble_block(sections: &[(&str, String)]) -> String {
    if sections.is_empty() {
        return format!(
            "{}\nNothing. This call opened with no context block, so look \
             everything up rather than answering from memory.\n",
            BLOCK_HEADING
        );
    }
    let mut out = format!(
        "{}\nEverything below was true when this call started. Nothing here is \
         live, and you cannot refresh it yourself.\n",
        BLOCK_HEADING
    );
    for (title, body) in sections {
        out.push_str("\n## ");
        out.push_str(title);
        out.push('\n');
        out.push_str(body.trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(stored: Option<&str>) -> Vec<&'static str> {
        sections_from(stored).iter().map(|s| s.id).collect()
    }

    /// A workspace that never touched the preference opens with the built-in
    /// set. It is the only case that falls back.
    #[test]
    fn no_row_at_all_means_the_default_set() {
        assert_eq!(
            ids(None),
            vec!["who-and-where", "this-thread", "workspace-shape"]
        );
    }

    /// The bug the toggles would otherwise have. Clearing every section used to
    /// read as "never set", so the last one turned itself back on.
    #[test]
    fn an_empty_row_means_no_sections_rather_than_all_of_them() {
        assert!(ids(Some("")).is_empty());
        assert!(ids(Some("  ")).is_empty());
    }

    /// The registry's order, not the preference's.
    #[test]
    fn a_row_that_lists_two_gives_exactly_those_two() {
        assert_eq!(
            ids(Some("workspace-shape,who-and-where")),
            vec!["who-and-where", "workspace-shape"]
        );
    }

    /// A typo must cost the reader one section, never the whole block.
    #[test]
    fn an_unknown_id_is_dropped_and_the_rest_still_load() {
        assert_eq!(ids(Some("this-thread,who-and-there")), vec!["this-thread"]);
    }

    /// The instructions name a context block and cannot vary, being the cached
    /// prefix. So an absent block leaves that sentence pointing at nothing, and
    /// the cheapest reading is that one arrived and covered the question.
    #[test]
    fn a_block_with_no_sections_says_so_rather_than_going_missing() {
        let block = assemble_block(&[]);
        assert!(block.starts_with(BLOCK_HEADING), "{}", block);
        assert!(block.contains("no context block"), "{}", block);
        assert!(block.contains("look everything up"), "{}", block);
    }

    /// It promises nothing it is not carrying. The heading over an ordinary
    /// block says everything below was true when the call started, and there
    /// is no below.
    #[test]
    fn the_empty_block_makes_no_promise_about_what_follows() {
        assert!(!assemble_block(&[]).contains("Everything below"));
    }

    /// The other side of it: a block with a body still carries the heading and
    /// the not-live warning the talker reads it under.
    #[test]
    fn a_block_with_a_section_carries_the_heading() {
        let built = [("Who you are talking to, and when", "Workspace: dev".into())];
        let block = assemble_block(&built);
        assert!(block.starts_with(BLOCK_HEADING), "{}", block);
        assert!(block.contains("## Who you are talking to, and when"));
        assert!(block.contains("Workspace: dev"));
    }

    #[test]
    fn every_section_id_is_unique() {
        let mut ids: Vec<&str> = SECTIONS.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "two sections share an id");
    }

    /// The default set is what a workspace that never edited the preference
    /// gets, so it is a product decision worth pinning.
    #[test]
    fn the_default_set_is_the_workspace_shape_one() {
        let on: Vec<&str> = SECTIONS
            .iter()
            .filter(|s| s.on_by_default)
            .map(|s| s.id)
            .collect();
        assert_eq!(on, vec!["who-and-where", "this-thread", "workspace-shape"]);
    }

    /// A section id is a preference value the user types, so it must stay
    /// kebab-case (the public-API convention) and stay stable.
    #[test]
    fn every_section_id_is_kebab_case() {
        for section in SECTIONS {
            assert!(
                section
                    .id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "section id '{}' is not kebab-case",
                section.id
            );
        }
    }
}
