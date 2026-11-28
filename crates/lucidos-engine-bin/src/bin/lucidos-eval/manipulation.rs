use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::arm::{missing_flag_message, Arm, FlagAvailability};

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub const MEMORY_SECTION: &str = "Long-term Memory";

pub const PANEL_SECTION: &str = "Context Panel";

pub const HISTORY_SECTION: &str = "Conversation History";

pub const EXCHANGE_STARTERS: [&str; 4] = [
    "MessageReceived",
    "UserQuestionAnswered",
    "ChildThreadCompleted",
    "EventWaitDelivered",
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapturedRound {
    pub sequence: i64,
    pub has_memory: bool,
    pub has_history: bool,
    #[serde(default)]
    pub has_panel: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlacedRound {
    pub round: usize,
    pub exchange: usize,
    pub captured: CapturedRound,
}

pub async fn captured_rounds(pool: &PgPool, thread_id: Uuid) -> Fallible<Vec<CapturedRound>> {
    let rows = sqlx::query(
        "SELECT sequence, \
                EXISTS (SELECT 1 FROM jsonb_array_elements(payload->'sections') s \
                         WHERE s->>'name' = $2) AS has_memory, \
                EXISTS (SELECT 1 FROM jsonb_array_elements(payload->'sections') s \
                         WHERE s->>'name' = $3) AS has_history, \
                EXISTS (SELECT 1 FROM jsonb_array_elements(payload->'sections') s \
                         WHERE s->>'name' = $4) AS has_panel \
           FROM events \
          WHERE event_type = 'ContextCaptured' AND thread_id = $1 \
            AND payload->>'producer' = 'main_llm' \
          ORDER BY sequence",
    )
    .bind(thread_id)
    .bind(MEMORY_SECTION)
    .bind(HISTORY_SECTION)
    .bind(PANEL_SECTION)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(CapturedRound {
                sequence: row.try_get("sequence")?,
                has_memory: row.try_get("has_memory")?,
                has_history: row.try_get("has_history")?,
                has_panel: row.try_get("has_panel")?,
            })
        })
        .collect()
}

pub async fn exchange_starts(pool: &PgPool, thread_id: Uuid) -> Fallible<Vec<i64>> {
    let starters: Vec<String> = EXCHANGE_STARTERS.iter().map(|s| s.to_string()).collect();
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT sequence FROM events \
          WHERE thread_id = $1 AND event_type = ANY($2) ORDER BY sequence",
    )
    .bind(thread_id)
    .bind(&starters)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

pub fn place_rounds(rounds: &[CapturedRound], exchange_starts: &[i64]) -> Vec<PlacedRound> {
    let mut placed = Vec::with_capacity(rounds.len());
    let mut current_exchange = usize::MAX;
    let mut round_in_exchange = 0usize;
    for captured in rounds {
        let exchange = exchange_starts
            .iter()
            .filter(|start| **start < captured.sequence)
            .count();
        if exchange != current_exchange {
            current_exchange = exchange;
            round_in_exchange = 0;
        }
        round_in_exchange += 1;
        placed.push(PlacedRound {
            round: round_in_exchange,
            exchange,
            captured: captured.clone(),
        });
    }
    placed
}

pub fn check(arm: Arm, placed: &[PlacedRound]) -> Fallible<()> {
    let expected = arm.expects_a_context_panel();
    let violations: Vec<String> = placed
        .iter()
        .filter(|round| round.captured.has_panel != expected)
        .map(|round| {
            format!(
                "round {} of exchange {} {} `{PANEL_SECTION}`",
                round.round,
                round.exchange,
                if round.captured.has_panel {
                    "carried"
                } else {
                    "was missing"
                }
            )
        })
        .collect();
    if violations.is_empty() {
        return Ok(());
    }
    Err(format!(
        "manipulation_check_failed: every round of the {arm} arm must have \
         `{PANEL_SECTION}` {}, and {} rounds disagree. This is a harness failure and never \
         a result. First disagreements: {}",
        if expected { "present" } else { "absent" },
        violations.len(),
        violations
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ")
    )
    .into())
}

pub fn check_tree_path(arm: Arm, tree_ready: bool) -> Fallible<()> {
    if tree_ready == arm.expects_tree_memory() {
        return Ok(());
    }
    Err(format!(
        "manipulation_check_failed: the {arm} arm {} the Tree memory path when its task \
         started. This is a harness failure and never a result.",
        if tree_ready { "was on" } else { "was not on" }
    )
    .into())
}

pub fn check_no_recall_on_tree(arm: Arm, memory_recalled: bool) -> Fallible<()> {
    if !(arm.expects_tree_memory() && memory_recalled) {
        return Ok(());
    }
    Err(format!(
        "manipulation_check_failed: a thread of the {arm} arm recalled memory, which only a \
         Classic turn does. This is a harness failure and never a result."
    )
    .into())
}

pub fn preflight(availability: FlagAvailability) -> Fallible<()> {
    match availability {
        FlagAvailability::Present => Ok(()),
        FlagAvailability::Missing => Err(missing_flag_message().into()),
    }
}

pub const CATALOG_SOURCE: &str = "crates/lucidos-core/src/core/preference_catalog.rs";

pub fn flag_availability_in_checkout(repo_root: &std::path::Path) -> Fallible<FlagAvailability> {
    let path = repo_root.join(CATALOG_SOURCE);
    let source = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "cannot read the engine's preference catalog at {}: {e}. Without it there is no \
             way to tell whether ADR 0085's flag exists, and an unknown must not authorize \
             a lean run.",
            path.display()
        )
    })?;
    Ok(catalog_declares(&source))
}

fn catalog_declares(source: &str) -> FlagAvailability {
    FlagAvailability::from_catalog_keys(&declared_keys(source))
}

fn declared_keys(source: &str) -> Vec<String> {
    source
        .match_indices("key: \"")
        .filter_map(|(at, marker)| {
            let rest = &source[at + marker.len()..];
            rest.find('"').map(|end| rest[..end].to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rounds(flags: &[(i64, bool)]) -> Vec<CapturedRound> {
        flags
            .iter()
            .map(|(sequence, has_panel)| CapturedRound {
                sequence: *sequence,
                has_memory: true,
                has_history: true,
                has_panel: *has_panel,
            })
            .collect()
    }

    #[test]
    fn the_control_arm_passes_when_no_round_carries_a_panel() {
        let placed = place_rounds(&rounds(&[(10, false), (20, false), (30, false)]), &[1]);
        check(Arm::CONTROL, &placed).unwrap();
    }

    #[test]
    fn the_control_arm_fails_when_a_panel_appears() {
        let placed = place_rounds(&rounds(&[(10, false), (20, true)]), &[1]);
        let err = check(Arm::CONTROL, &placed).unwrap_err().to_string();
        assert!(err.contains("manipulation_check_failed"));
        assert!(err.contains("Context Panel"));
    }

    #[test]
    fn the_lean_arm_passes_when_every_round_carries_a_panel() {
        let placed = place_rounds(&rounds(&[(10, true), (20, true), (30, true)]), &[1]);
        check(Arm::LEAN, &placed).unwrap();
    }

    #[test]
    fn the_lean_arm_fails_when_the_flag_stops_working_mid_exchange() {
        let placed = place_rounds(&rounds(&[(10, true), (20, false)]), &[1]);
        let err = check(Arm::LEAN, &placed).unwrap_err().to_string();
        assert!(err.contains("manipulation_check_failed"));
        assert!(err.contains("was missing"));
    }

    #[test]
    fn a_lean_round_still_carrying_both_sections_is_not_a_violation() {
        let placed = place_rounds(&rounds(&[(10, true), (20, true)]), &[1]);
        assert!(placed.iter().all(|r| r.captured.has_memory));
        check(Arm::LEAN, &placed).unwrap();
    }

    #[test]
    fn a_re_entry_restarts_the_round_count_for_the_lean_arm() {
        let placed = place_rounds(
            &rounds(&[(10, true), (20, true), (40, true), (50, true)]),
            &[1, 30],
        );
        assert_eq!(
            placed.iter().map(|r| r.round).collect::<Vec<_>>(),
            vec![1, 2, 1, 2]
        );
        check(Arm::LEAN, &placed).unwrap();
    }

    #[test]
    fn a_single_round_thread_is_still_checked() {
        check(Arm::LEAN, &place_rounds(&rounds(&[(10, true)]), &[1])).unwrap();
        check(Arm::CONTROL, &place_rounds(&rounds(&[(10, false)]), &[1])).unwrap();
    }

    #[test]
    fn every_round_of_every_exchange_is_assertable() {
        let placed = place_rounds(
            &rounds(&[(10, false), (20, true), (30, true), (40, false)]),
            &[1, 25],
        );
        let err = check(Arm::LEAN, &placed).unwrap_err().to_string();
        assert!(err.contains("2 rounds disagree"), "{err}");
        assert!(err.contains("exchange 1"), "{err}");
        assert!(err.contains("exchange 2"), "{err}");
    }

    #[test]
    fn a_lean_round_one_with_nothing_addressable_still_needs_its_panel() {
        let err = check(Arm::LEAN, &place_rounds(&rounds(&[(10, false)]), &[1]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("was missing"), "{err}");
    }

    #[test]
    fn the_preflight_refuses_when_the_flag_is_missing() {
        let err = preflight(FlagAvailability::Missing)
            .unwrap_err()
            .to_string();
        assert!(err.contains("arm_flag_missing"));
        assert!(err.contains(crate::arm::CONTEXT_MODE_PREFERENCE_KEY));
        assert!(err.contains(crate::arm::MEMORY_MODULE_PREFERENCE_KEY));
    }

    #[test]
    fn a_tree_arm_must_be_on_the_tree_path_and_a_classic_arm_must_not() {
        check_tree_path(Arm::LEAN_TREE, true).unwrap();
        check_tree_path(Arm::LEAN, false).unwrap();
        let err = check_tree_path(Arm::LEAN_TREE, false)
            .unwrap_err()
            .to_string();
        assert!(err.contains("manipulation_check_failed"), "{err}");
        assert!(err.contains("lean-tree"), "{err}");
        assert!(check_tree_path(Arm::CONTROL, true).is_err());
    }

    #[test]
    fn only_a_tree_thread_that_recalled_fails_the_recall_gate() {
        check_no_recall_on_tree(Arm::LEAN, true).unwrap();
        check_no_recall_on_tree(Arm::LEAN_TREE, false).unwrap();
        let err = check_no_recall_on_tree(Arm::CONTROL_TREE, true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("manipulation_check_failed"), "{err}");
    }

    #[test]
    fn the_preflight_passes_once_the_flag_exists() {
        preflight(FlagAvailability::Present).unwrap();
    }

    #[test]
    fn a_catalog_declaring_every_key_reads_as_present() {
        let source: String = crate::arm::ARM_PREFERENCE_KEYS
            .iter()
            .map(|key| format!("PrefSpec {{ key: \"{key}\", scope: Global }}\n"))
            .collect();
        assert_eq!(catalog_declares(&source), FlagAvailability::Present);
    }

    #[test]
    fn a_catalog_declaring_the_mode_alone_reads_as_missing() {
        let source = "PrefSpec { key: \"self_curated_context_mode\", scope: Global }";
        assert_eq!(catalog_declares(source), FlagAvailability::Missing);
    }

    #[test]
    fn a_catalog_only_mentioning_the_key_in_prose_reads_as_missing() {
        let source = "// self_curated_context_mode is not implemented yet";
        assert_eq!(catalog_declares(source), FlagAvailability::Missing);
    }

    #[test]
    fn the_engine_in_this_checkout_declares_the_context_mode_flag() {
        let repo_root = crate::repository_root();
        assert_eq!(
            flag_availability_in_checkout(repo_root).unwrap(),
            FlagAvailability::Present,
            "the engine's catalog no longer declares ADR 0085's `{}` in the `key: \"…\"` \
             form this preflight scans for, so every lean run would refuse.",
            crate::arm::CONTEXT_MODE_PREFERENCE_KEY
        );
    }
}
