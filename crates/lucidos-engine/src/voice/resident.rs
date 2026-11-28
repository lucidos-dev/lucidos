use std::collections::HashSet;

use crate::core::prefs;
use crate::engine::LucidosEngine;

use super::sections::{ResidentSection, SECTIONS};

const BLOCK_HEADING: &str = "[WHAT YOU ALREADY KNOW]";

/// INVARIANT: Which sections a stored preference names.
///
/// **A row that exists means exactly what it lists, and an EMPTY one means
/// none.** Only `None`, a row that was never written, falls back to the
/// catalog default. The two used to be one case, which made the last section
/// impossible to turn off: clearing it read as "never set" and brought all
/// three back.
///
/// A named section nobody defines is dropped with a log line: a typo must not
/// cost the user the whole block.
///
/// Ordered by the registry, never by the preference. The block then reads the
/// same way whatever order the reader toggled them in.
pub(super) fn sections_from(stored: Option<&str>) -> Vec<&'static ResidentSection> {
    let listed = stored.unwrap_or_else(|| prefs::VOICE_RESIDENT_SECTIONS.default_text());
    let wanted: HashSet<&str> = listed
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    for name in &wanted {
        if !SECTIONS.iter().any(|s| s.id == *name) {
            log_legacy!(
                "[Voice] {} names an unknown section '{}'. Ignoring it",
                prefs::VOICE_RESIDENT_SECTIONS.key(),
                name
            );
        }
    }
    SECTIONS.iter().filter(|s| wanted.contains(s.id)).collect()
}

/// INVARIANT: Which sections a session opens with, read from the preference store.
///
/// A read error opens with NO sections, not with the defaults. An unreadable
/// row is unknown rather than unset, and the two answers are no longer the
/// same: since an empty row means "none", guessing the defaults would hand the
/// talker the thread and the workspace shape that this reader deliberately
/// turned off. The call still goes up, knowing less (`.claude/rules/rust.md`).
pub async fn enabled_sections(engine: &LucidosEngine) -> Vec<&'static ResidentSection> {
    match prefs::VOICE_RESIDENT_SECTIONS
        .try_stored(engine.pool())
        .await
    {
        Ok(stored) => sections_from(stored.as_deref()),
        Err(e) => {
            log_legacy!(
                "[Voice] Could not read {}: {}. Opening with no resident block",
                prefs::VOICE_RESIDENT_SECTIONS.key(),
                e
            );
            vec![]
        }
    }
}

pub async fn build_block(engine: &LucidosEngine, thread_id: uuid::Uuid) -> String {
    let mut built: Vec<(&'static str, String)> = Vec::new();

    for section in enabled_sections(engine).await {
        match (section.build)(engine, thread_id).await {
            Ok(body) if body.trim().is_empty() => {}
            Ok(body) => built.push((section.title, body)),
            Err(e) => log_legacy!(
                "[Voice] Section '{}' failed: {}. Skipping it",
                section.id,
                e
            ),
        }
    }
    assemble_block(&built)
}

/// INVARIANT: Lay the built sections out under the heading.
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

    #[test]
    fn no_row_at_all_means_the_default_set() {
        assert_eq!(
            ids(None),
            ids(Some(prefs::VOICE_RESIDENT_SECTIONS.default_text()))
        );
        assert!(!ids(None).is_empty());
    }

    #[test]
    fn an_empty_row_means_no_sections_rather_than_all_of_them() {
        assert!(ids(Some("")).is_empty());
        assert!(ids(Some("  ")).is_empty());
    }

    #[test]
    fn a_row_that_lists_two_gives_exactly_those_two() {
        assert_eq!(
            ids(Some("workspace-shape,who-and-where")),
            vec!["who-and-where", "workspace-shape"]
        );
    }

    #[test]
    fn an_unknown_id_is_dropped_and_the_rest_still_load() {
        assert_eq!(ids(Some("this-thread,who-and-there")), vec!["this-thread"]);
    }

    #[test]
    fn a_block_with_no_sections_says_so_rather_than_going_missing() {
        let block = assemble_block(&[]);
        assert!(block.starts_with(BLOCK_HEADING), "{}", block);
        assert!(block.contains("no context block"), "{}", block);
        assert!(block.contains("look everything up"), "{}", block);
    }

    #[test]
    fn the_empty_block_makes_no_promise_about_what_follows() {
        assert!(!assemble_block(&[]).contains("Everything below"));
    }

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

    #[test]
    fn every_default_section_exists() {
        for id in prefs::VOICE_RESIDENT_SECTIONS.default_text().split(',') {
            assert!(
                SECTIONS.iter().any(|s| s.id == id.trim()),
                "the default names '{}', which no section defines",
                id
            );
        }
    }

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
