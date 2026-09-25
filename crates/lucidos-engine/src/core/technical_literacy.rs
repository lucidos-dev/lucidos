//! **Technical literacy**: how technical the words in an answer are. It is the
//! second part of the *response style*, beside the style, the shape of an answer.
//!
//! One global preference, `technical_literacy`, holds one of three levels.
//! Unset adds nothing, like the Standard style. First-run setup asks for it,
//! and only a level the user stated is ever stored.
//!
//! Every agent that talks to the user reads it:
//!
//! - chat and trigger threads, inside the `RESPONSE STYLE:` section;
//! - a coding-agent session, as [`coding_agent_section`] in its system prompt;
//! - a voice call's talker, in `voice::instructions_for`.
//!
//! See `docs/plans/2026-09-24-technical-literacy.md` and
//! `docs/plans/2026-09-24-technical-literacy-three-levels.md`.

use sqlx::PgPool;

use crate::core::{PreferenceStore, PREF_TECHNICAL_LITERACY};

/// One of the three levels, from least to most technical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TechnicalLiteracy {
    NonTechnical,
    Technical,
    Developer,
}

/// The stored values, in [`TechnicalLiteracy::ALL`] order. A slice rather than
/// a derived list, because the preference catalog needs a `const`.
pub const IDS: &[&str] = &["non-technical", "technical", "developer"];

/// The value that clears the level. The agent's preference tool accepts only
/// catalog values, so clearing needs a value of its own.
pub const NOT_SET_ID: &str = "not-set";

/// Every value the catalog accepts: [`NOT_SET_ID`], then [`IDS`].
pub const SETTABLE_IDS: &[&str] = &[NOT_SET_ID, "non-technical", "technical", "developer"];

/// A retired fourth level, merged into [`TechnicalLiteracy::NonTechnical`]. A
/// row that still holds it reads as that level, so nobody is asked again.
const MERGED_EVERYDAY_ID: &str = "everyday";

/// Added to every level, outside the per-level text.
const LITERACY_FLOOR: &str = "- Never name this level to the user and never talk down to them. \
     When they use a term themselves, use it back.";

/// A non-technical user cannot answer a plumbing question, so none reaches
/// them. A git-branch question put to one is the case.
const RELEVANCE_RULE: &str = "- Never ask them a question they cannot answer, such as what \
     to do with a git branch, a commit, a config value or an id. Decide it yourself, then say \
     what happened in their terms: \"I saved your changes\", not \"I merged the branch\".";

/// Chat only: coding agents are an expert tool, so the Lucidos Agent keeps a
/// non-technical user away from them without hiding them.
const STEERING_RULE: &str = "- Do the work yourself. Never suggest or start a coding-agent \
     thread for them: coding agents are an expert tool.";

impl TechnicalLiteracy {
    pub const ALL: [Self; 3] = [Self::NonTechnical, Self::Technical, Self::Developer];

    pub fn id(self) -> &'static str {
        match self {
            Self::NonTechnical => IDS[0],
            Self::Technical => IDS[1],
            Self::Developer => IDS[2],
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        let raw = raw.trim();
        if raw == MERGED_EVERYDAY_ID {
            return Some(Self::NonTechnical);
        }
        Self::ALL.into_iter().find(|level| level.id() == raw)
    }

    /// The option label the user picks, on every card and in Settings.
    pub fn card_label(self) -> &'static str {
        match self {
            Self::NonTechnical => "Keep it plain",
            Self::Technical => "Technical",
            Self::Developer => "I write software",
        }
    }

    /// The line under [`Self::card_label`]. It says what the user gets in
    /// their own words, never how much detail: that is the style's job.
    pub fn card_line(self) -> &'static str {
        match self {
            Self::NonTechnical => "Everyday words, no jargon.",
            Self::Technical => "Technical terms are fine.",
            Self::Developer => "Talk to me like a developer.",
        }
    }

    fn instruction(self) -> &'static str {
        match self {
            Self::NonTechnical => {
                "- The user wants plain words. Use everyday language.\n\
                 - No jargon, code, file paths, ids or command names unless they ask. \
                 When a technical term is unavoidable, explain it in a few plain words.\n\
                 - Say what something does for them, not how it works inside."
            }
            Self::Technical => {
                "- The user is technical. Technical terms need no explanation.\n\
                 - This sets which words you may use, not how much to say. Add paths, \
                 commands or configuration only when the answer needs them."
            }
            Self::Developer => {
                "- The user is a software developer. Engineering terms, code and ids need no explanation.\n\
                 - This sets which words you may use, not how much to say. Add code, \
                 paths or logs only when the answer needs them."
            }
        }
    }

    /// Whether this level only gets questions about goals and outcomes.
    fn asks_only_what_they_can_answer(self) -> bool {
        self == Self::NonTechnical
    }

    /// The lines this level adds to any agent's prompt, floor included.
    pub fn rules(self) -> String {
        self.compose(false)
    }

    /// [`Self::rules`] plus the steering away from coding agents, for the
    /// Lucidos Agent alone.
    pub fn chat_rules(self) -> String {
        self.compose(true)
    }

    fn compose(self, steer: bool) -> String {
        let mut lines = vec![self.instruction()];
        if self.asks_only_what_they_can_answer() {
            lines.push(RELEVANCE_RULE);
            if steer {
                lines.push(STEERING_RULE);
            }
        }
        lines.push(LITERACY_FLOOR);
        lines.join("\n")
    }
}

/// The level this workspace stored, or `None`.
///
/// A missing row, an unknown value and a failed read all resolve to `None`. A
/// turn that fails on a bad row is worse than one that runs without the level.
pub async fn read(pool: &PgPool) -> Option<TechnicalLiteracy> {
    let raw = match PreferenceStore::get(pool, PREF_TECHNICAL_LITERACY).await {
        Ok(raw) => raw?,
        Err(e) => {
            log!(
                "[TechnicalLiteracy] failed to read '{}': {}. Leaving it unset",
                PREF_TECHNICAL_LITERACY,
                e
            );
            return None;
        }
    };
    if raw.trim().is_empty() || raw.trim() == NOT_SET_ID {
        return None;
    }
    let level = TechnicalLiteracy::parse(&raw);
    if level.is_none() {
        log!(
            "[TechnicalLiteracy] '{}' holds unknown level '{}'. Leaving it unset",
            PREF_TECHNICAL_LITERACY,
            raw
        );
    }
    level
}

/// The section a coding-agent system prompt ends with, or `""` when unset.
///
/// It scopes the level to what the agent tells the user. Code, commits and
/// docs keep the repository's own conventions whatever the user's level.
pub fn coding_agent_section(level: Option<TechnicalLiteracy>) -> String {
    let Some(level) = level else {
        return String::new();
    };
    format!(
        "\n\nUSER'S TECHNICAL LITERACY: this governs what you write TO the user \
         (messages, questions, summaries). Code, commit messages and docs follow \
         the repository's own conventions.\n{}",
        level.rules()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_match_the_variants_in_order() {
        let ids: Vec<&str> = TechnicalLiteracy::ALL.into_iter().map(|l| l.id()).collect();
        assert_eq!(ids, IDS);
    }

    #[test]
    fn the_settable_values_are_not_set_then_every_level() {
        assert_eq!(SETTABLE_IDS[0], NOT_SET_ID);
        assert_eq!(&SETTABLE_IDS[1..], IDS);
        assert_eq!(TechnicalLiteracy::parse(NOT_SET_ID), None);
    }

    #[test]
    fn every_id_round_trips_and_nothing_else_parses() {
        for level in TechnicalLiteracy::ALL {
            assert_eq!(TechnicalLiteracy::parse(level.id()), Some(level));
            assert_eq!(
                TechnicalLiteracy::parse(&format!(" {} ", level.id())),
                Some(level)
            );
        }
        for bad in ["", "Developer", "non_technical", "expert"] {
            assert_eq!(TechnicalLiteracy::parse(bad), None, "{bad} must not parse");
        }
    }

    /// The retired fourth level reads as Keep it plain, so a user who picked
    /// it is never asked again. It is not offered for new writes.
    #[test]
    fn the_merged_everyday_level_reads_as_non_technical() {
        assert_eq!(
            TechnicalLiteracy::parse(MERGED_EVERYDAY_ID),
            Some(TechnicalLiteracy::NonTechnical)
        );
        assert!(!SETTABLE_IDS.contains(&MERGED_EVERYDAY_ID));
    }

    /// Each card option has its own label and line, and no line talks about
    /// how much detail comes back.
    #[test]
    fn every_level_has_distinct_card_copy_about_words_not_amount() {
        let mut labels = std::collections::HashSet::new();
        for level in TechnicalLiteracy::ALL {
            assert!(
                labels.insert(level.card_label()),
                "{level:?} repeats a label"
            );
            let line = level.card_line().to_lowercase();
            for amount in ["depth", "detail", "more", "less"] {
                assert!(!line.contains(amount), "{level:?} line speaks of {amount}");
            }
        }
    }

    #[test]
    fn every_level_ends_with_the_floor_and_says_something_different() {
        let mut seen = std::collections::HashSet::new();
        for level in TechnicalLiteracy::ALL {
            let rules = level.rules();
            assert!(rules.ends_with(LITERACY_FLOOR), "{level:?} lost the floor");
            assert!(
                seen.insert(level.instruction()),
                "{level:?} repeats a level"
            );
        }
    }

    /// Only Keep it plain is shielded from plumbing questions and steered
    /// away from coding agents. A developer may well be asked about a branch,
    /// and may well want a coding agent.
    #[test]
    fn only_the_plain_level_gets_relevance_and_steering() {
        for level in TechnicalLiteracy::ALL {
            let plain = level == TechnicalLiteracy::NonTechnical;
            assert_eq!(level.rules().contains(RELEVANCE_RULE), plain, "{level:?}");
            assert_eq!(
                level.chat_rules().contains(RELEVANCE_RULE),
                plain,
                "{level:?}"
            );
            assert_eq!(
                level.chat_rules().contains(STEERING_RULE),
                plain,
                "{level:?}"
            );
            assert!(
                !level.rules().contains(STEERING_RULE),
                "{level:?} steers outside chat"
            );
            assert!(level.chat_rules().ends_with(LITERACY_FLOOR));
        }
    }

    #[test]
    fn the_coding_agent_section_is_empty_when_unset() {
        assert_eq!(coding_agent_section(None), "");
    }

    #[test]
    fn the_coding_agent_section_carries_the_level_and_its_scope() {
        for level in TechnicalLiteracy::ALL {
            let section = coding_agent_section(Some(level));
            assert!(section.starts_with("\n\nUSER'S TECHNICAL LITERACY:"));
            assert!(section.contains("commit messages"));
            assert!(section.ends_with(&level.rules()));
        }
    }

    /// The setup interview stores what its literacy card returns. A value the
    /// knowhow names but this list lacks is refused by `set_preference`, and
    /// the interview then runs on with nothing stored.
    #[test]
    fn the_setup_interview_offers_exactly_the_levels_the_engine_reads() {
        let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
        let knowhow = std::fs::read_to_string(repo.join("system-knowhow/setup-interview.md"))
            .expect("the setup interview knowhow ships");
        assert!(knowhow.contains(crate::core::PREF_TECHNICAL_LITERACY));
        for level in TechnicalLiteracy::ALL {
            let row = format!(
                "| {} | {} | `{}` |",
                level.card_label(),
                level.card_line(),
                level.id()
            );
            assert!(
                knowhow.contains(&row),
                "the literacy card lacks the row {row}"
            );
        }
        assert!(!knowhow.contains(&format!("`{MERGED_EVERYDAY_ID}`")));
    }

    #[tokio::test]
    async fn read_resolves_levels_and_degrades_every_bad_row_to_unset() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let seed = |value: &'static str| {
            let pool = pool.clone();
            async move {
                crate::test_support::seed_preference(&pool, PREF_TECHNICAL_LITERACY, value)
                    .await
                    .unwrap()
            }
        };

        assert_eq!(read(&pool).await, None);

        seed("technical").await;
        assert_eq!(read(&pool).await, Some(TechnicalLiteracy::Technical));

        // The merged level still counts as answered, as Keep it plain.
        seed(MERGED_EVERYDAY_ID).await;
        assert_eq!(read(&pool).await, Some(TechnicalLiteracy::NonTechnical));

        seed("   ").await;
        assert_eq!(read(&pool).await, None);

        seed(NOT_SET_ID).await;
        assert_eq!(read(&pool).await, None);

        seed("wizard").await;
        assert_eq!(read(&pool).await, None);

        pool.close().await;
        crate::test_support::teardown_test_db(&db_name).await;
    }
}
