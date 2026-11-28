use sqlx::PgPool;

use crate::core::prefs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TechnicalLiteracy {
    NonTechnical,
    Technical,
    Developer,
}

/// INVARIANT: The stored values, in [`TechnicalLiteracy::ALL`] order. A slice rather than
/// a derived list, because the preference catalog needs a `const`.
pub const IDS: &[&str] = &["non-technical", "technical", "developer"];

pub const NOT_SET_ID: &str = "not-set";

pub const SETTABLE_IDS: &[&str] = &[NOT_SET_ID, "non-technical", "technical", "developer"];

/// INVARIANT: A retired fourth level, merged into [`TechnicalLiteracy::NonTechnical`]. A
/// row that still holds it reads as that level, so nobody is asked again.
const MERGED_EVERYDAY_ID: &str = "everyday";

const LITERACY_FLOOR: &str = "- Never name this level to the user and never talk down to them. \
     When they use a term themselves, use it back.";

const RELEVANCE_RULE: &str = "- Never ask them a question they cannot answer, such as what \
     to do with a git branch, a commit, a config value or an id. Decide it yourself, then say \
     what happened in their terms: \"I saved your changes\", not \"I merged the branch\".";

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

    pub fn card_label(self) -> &'static str {
        match self {
            Self::NonTechnical => "Keep it plain",
            Self::Technical => "Technical",
            Self::Developer => "I write software",
        }
    }

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

    fn asks_only_what_they_can_answer(self) -> bool {
        self == Self::NonTechnical
    }

    pub fn rules(self) -> String {
        self.compose(false)
    }

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

/// INVARIANT: The level this workspace stored, or `None`.
///
/// A missing row, an unknown value and a failed read all resolve to `None`. A
/// turn that fails on a bad row is worse than one that runs without the level.
///
/// Reads the raw row rather than the handle's value, because the retired
/// [`MERGED_EVERYDAY_ID`] is no catalog value and still has to parse.
pub async fn read(pool: &PgPool) -> Option<TechnicalLiteracy> {
    let raw = prefs::TECHNICAL_LITERACY.stored(pool).await?;
    if raw.trim().is_empty() || raw.trim() == NOT_SET_ID {
        return None;
    }
    let level = TechnicalLiteracy::parse(&raw);
    if level.is_none() {
        log_legacy!(
            "[TechnicalLiteracy] '{}' holds unknown level '{}'. Leaving it unset",
            prefs::TECHNICAL_LITERACY.key(),
            raw
        );
    }
    level
}

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

    #[test]
    fn the_merged_everyday_level_reads_as_non_technical() {
        assert_eq!(
            TechnicalLiteracy::parse(MERGED_EVERYDAY_ID),
            Some(TechnicalLiteracy::NonTechnical)
        );
        assert!(!SETTABLE_IDS.contains(&MERGED_EVERYDAY_ID));
    }

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

    #[tokio::test]
    async fn read_resolves_levels_and_degrades_every_bad_row_to_unset() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let seed = |value: &'static str| {
            let pool = pool.clone();
            async move {
                let key = prefs::TECHNICAL_LITERACY.key();
                crate::test_support::seed_preference(&pool, key, value)
                    .await
                    .unwrap()
            }
        };

        assert_eq!(read(&pool).await, None);

        seed("technical").await;
        assert_eq!(read(&pool).await, Some(TechnicalLiteracy::Technical));

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

    mod guard_tests {
        use super::*;

        #[test]
        fn the_setup_interview_offers_exactly_the_levels_the_engine_reads() {
            let repo = crate::paths::repo_root().expect("repo root resolves under cargo test");
            let knowhow = std::fs::read_to_string(repo.join("system-knowhow/setup-interview.md"))
                .expect("the setup interview knowhow ships");
            assert!(knowhow.contains(prefs::TECHNICAL_LITERACY.key()));
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
    }
}
