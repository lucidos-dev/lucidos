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

/// Which sections a session opens with, resolved from the preference.
///
/// Unset means the built-in default set. A named section nobody defines is
/// dropped with a log line: a typo must not cost the user the whole block.
pub async fn enabled_sections(engine: &LucidosEngine) -> Vec<&'static ResidentSection> {
    let stored = match PreferenceStore::get(engine.pool(), PREF_VOICE_RESIDENT_SECTIONS).await {
        Ok(Some(v)) if !v.trim().is_empty() => Some(v),
        Ok(_) => None,
        Err(e) => {
            log!(
                "[Voice] Could not read {}: {}. Using the default sections",
                PREF_VOICE_RESIDENT_SECTIONS,
                e
            );
            None
        }
    };

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

/// Build the block this session opens with.
///
/// Every builder runs now, so a section reports the workspace as it is rather
/// than as it was. A builder that fails is skipped with a log line: a section
/// the talker cannot see is better than a session that cannot open.
pub async fn build_block(engine: &LucidosEngine, thread_id: uuid::Uuid) -> String {
    let mut out = String::from(BLOCK_HEADING);
    out.push_str(
        "\nEverything below was true when this call started. Nothing here is \
         live, and you cannot refresh it yourself.\n",
    );

    for section in enabled_sections(engine).await {
        match (section.build)(engine, thread_id).await {
            Ok(body) if body.trim().is_empty() => {}
            Ok(body) => {
                out.push_str("\n## ");
                out.push_str(section.title);
                out.push('\n');
                out.push_str(body.trim_end());
                out.push('\n');
            }
            Err(e) => log!(
                "[Voice] Section '{}' failed: {}. Skipping it",
                section.id,
                e
            ),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
