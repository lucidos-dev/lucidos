//! The section-presence gate (I2), and the preflight that refuses to run
//! without the flag it is gating.
//!
//! ADR 0087 decision 10, as its second amendment restates it: the lean arm
//! carries a context panel on every round and the control arm never does. A
//! silent no-op flag produces a null result that reads as a pass. A violation
//! therefore aborts the repeat, and is never recorded as a finding.
//!
//! The gate used to assert the two sections were ABSENT from round 2. They now
//! leave at the round 1 boundary unless the model keeps them. So their absence
//! is the default rather than evidence about the flag.

use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::arm::{missing_flag_message, Arm, FlagAvailability};

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The memory-recall section ADR 0085 drops.
pub const MEMORY_SECTION: &str = "Long-term Memory";

/// The panel the context mode appends to the tail of every round.
///
/// Present in the lean arm and absent in the control arm, which is what the
/// manipulation check reads. The engine spells the same name in
/// `context_panel::PANEL_SECTION`, and the name IS the contract.
pub const PANEL_SECTION: &str = "Context Panel";

/// The conversation-history section ADR 0085 reorders.
///
/// **Not `Conversation`.** The engine emits both names and they are different
/// things. `Conversation History` is the cross-turn history built by the chat
/// context, which is what the flag reorders into releasable bodies.
/// `Conversation` is the within-turn delta the agentic loop appends on its
/// second iteration onward, which the flag does not reorder at all. Neither is
/// the gate any more, and gating on the latter would have failed the lean arm
/// forever.
pub const HISTORY_SECTION: &str = "Conversation History";

/// Events that start a fresh exchange, and therefore a fresh round 1.
///
/// ADR 0085 decision 13: an answer-driven resume, a manual Continue and an
/// event-wait delivery each carry the payload again, because the prefix is
/// cold. A gate reading "absent from the thread's second round on" would abort
/// every repeat whose task asks a question. The gate is per exchange.
pub const EXCHANGE_STARTERS: [&str; 4] = [
    "MessageReceived",
    "UserQuestionAnswered",
    "ChildThreadCompleted",
    "EventWaitDelivered",
];

/// One captured round, reduced to what the gate reads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapturedRound {
    pub sequence: i64,
    pub has_memory: bool,
    pub has_history: bool,
    /// Whether this round carried a context panel. Defaulted, so a results
    /// file written before the amendment still parses.
    #[serde(default)]
    pub has_panel: bool,
}

/// A round placed inside its exchange.
#[derive(Debug, Clone, PartialEq)]
pub struct PlacedRound {
    pub round: usize,
    pub exchange: usize,
    pub captured: CapturedRound,
}

/// Read every captured round on the thread, with the two section flags.
pub async fn captured_rounds(pool: &PgPool, thread_id: Uuid) -> Fallible<Vec<CapturedRound>> {
    // `producer = main_llm` is load-bearing, not tidiness. An `AuxCapture`
    // writes `ContextCaptured` on the same thread for the memory classifier,
    // the title and the summariser. Each carries one section named for its
    // purpose and never a panel. Counting those rounds fails the gate on every
    // lean repeat, and ADR 0087 makes that abort the repeat rather than record
    // a result. The ledger check never noticed: an auxiliary row landing first
    // set the per-exchange oracle to false and silently disabled itself.
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

/// Sequences at which a fresh exchange started on the thread.
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

/// Number each captured round within its exchange.
///
/// A round belongs to the newest exchange that started before it. Rounds
/// before any starter are exchange 0, which happens only on a malformed thread.
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

/// The gate. `Err` aborts the repeat with `manipulation_check_failed`.
///
/// The lean arm carries a `Context Panel` on every round of an exchange that
/// built one. The control arm carries one on no round at all. That is the
/// no-op tripwire: a flag that changed nothing renders no panel.
///
/// It deliberately does NOT assert on the two sections any more. Under persist
/// on demand they leave at the round 1 boundary unless the model keeps them,
/// so their absence says nothing about the flag.
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

/// Refuse to run when the engine does not implement the flag (I2).
///
/// The lean arm without ADR 0085 is the control arm wearing a label. Its
/// manipulation check would fail on every round, so the honest refusal is here,
/// before anything is seeded and before a single token is spent.
pub fn preflight(availability: FlagAvailability) -> Fallible<()> {
    match availability {
        FlagAvailability::Present => Ok(()),
        FlagAvailability::Missing => Err(missing_flag_message().into()),
    }
}

/// Where the engine declares every preference key it knows.
pub const CATALOG_SOURCE: &str = "crates/lucidos-engine/src/core/preference_catalog.rs";

/// Whether the engine this harness was built beside knows the flag.
///
/// Read from the checkout rather than over HTTP, because no endpoint exposes
/// the catalog and the write path accepts any key. The harness and the engine
/// are built from one checkout by `scripts/eval-context-mode.sh`, so this is
/// the build that will run.
///
/// An unreadable catalog is refused, not assumed present. An unknown here would
/// authorize a lean run that measures nothing.
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

/// Whether a catalog source declares the flag as a settable key.
///
/// Reads the declared keys out of the source and asks [`FlagAvailability`].
/// One rule then decides it here and anywhere else a key list turns up. A
/// mention in a comment is not a declaration.
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

    /// Captured rounds as `(sequence, has_panel)`. The two section flags stay
    /// on the row for the cost table and the census. The gate no longer reads
    /// them, so they are set alike here.
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
        check(Arm::Control, &placed).unwrap();
    }

    /// A control arm carrying a panel means the flag leaked into it, so the
    /// two arms are not one preference apart.
    #[test]
    fn the_control_arm_fails_when_a_panel_appears() {
        let placed = place_rounds(&rounds(&[(10, false), (20, true)]), &[1]);
        let err = check(Arm::Control, &placed).unwrap_err().to_string();
        assert!(err.contains("manipulation_check_failed"));
        assert!(err.contains("Context Panel"));
    }

    #[test]
    fn the_lean_arm_passes_when_every_round_carries_a_panel() {
        let placed = place_rounds(&rounds(&[(10, true), (20, true), (30, true)]), &[1]);
        check(Arm::Lean, &placed).unwrap();
    }

    /// The failure this whole check exists to catch: a flag that changes
    /// nothing. Every lean round carries a panel, so a missing one is the flag
    /// having stopped working mid-exchange.
    #[test]
    fn the_lean_arm_fails_when_the_flag_stops_working_mid_exchange() {
        let placed = place_rounds(&rounds(&[(10, true), (20, false)]), &[1]);
        let err = check(Arm::Lean, &placed).unwrap_err().to_string();
        assert!(err.contains("manipulation_check_failed"));
        assert!(err.contains("was missing"));
    }

    /// The two sections are no longer the gate. A lean round that still carries
    /// both is a model that released nothing, which is a result and not a
    /// harness failure.
    #[test]
    fn a_lean_round_still_carrying_both_sections_is_not_a_violation() {
        let placed = place_rounds(&rounds(&[(10, true), (20, true)]), &[1]);
        assert!(placed.iter().all(|r| r.captured.has_memory));
        check(Arm::Lean, &placed).unwrap();
    }

    /// ADR 0085 decision 13: a re-entry is a fresh round 1, in both arms. The
    /// panel is rendered on every round either way.
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
        check(Arm::Lean, &placed).unwrap();
    }

    #[test]
    fn a_single_round_thread_is_still_checked() {
        check(Arm::Lean, &place_rounds(&rounds(&[(10, true)]), &[1])).unwrap();
        check(Arm::Control, &place_rounds(&rounds(&[(10, false)]), &[1])).unwrap();
    }

    /// Every round is assertable, in every exchange. The ledger had an escape
    /// hatch, because a brand-new thread assembled no body region and so built
    /// no ledger. A panel always states the budget, so there is no round the
    /// flag can be silently inert on.
    #[test]
    fn every_round_of_every_exchange_is_assertable() {
        let placed = place_rounds(
            &rounds(&[(10, false), (20, true), (30, true), (40, false)]),
            &[1, 25],
        );
        let err = check(Arm::Lean, &placed).unwrap_err().to_string();
        assert!(err.contains("2 rounds disagree"), "{err}");
        assert!(err.contains("exchange 1"), "{err}");
        assert!(err.contains("exchange 2"), "{err}");
    }

    /// A lean thread's very first round carries one too. Under the ledger this
    /// was the unassertable case, and it is where a dead flag would hide.
    #[test]
    fn a_lean_round_one_with_nothing_addressable_still_needs_its_panel() {
        let err = check(Arm::Lean, &place_rounds(&rounds(&[(10, false)]), &[1]))
            .unwrap_err()
            .to_string();
        assert!(err.contains("was missing"), "{err}");
    }

    /// I2's fail-closed half. An engine under test that has no mode flag makes
    /// the preflight refuse, and name it.
    #[test]
    fn the_preflight_refuses_when_the_flag_is_missing() {
        let err = preflight(FlagAvailability::Missing)
            .unwrap_err()
            .to_string();
        assert!(err.contains("context_mode_flag_missing"));
        assert!(err.contains(crate::arm::CONTEXT_MODE_PREFERENCE_KEY));
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

    /// The mode's own key is not the whole contract. An engine carrying it
    /// without the two schedule keys ignores the numbers the arm seeds.
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

    /// The state of this repository right now, asserted rather than assumed.
    ///
    /// ADR 0085 landed, so the engine beside this crate declares the flag and
    /// the lean arm can be exercised. Until then this asserted the opposite,
    /// and its own failure was the reminder to come back here.
    ///
    /// It stays as the tripwire in the other direction. The preflight scans the
    /// catalog's `key: "…"` literals. A rename, a move, or a spelling as a
    /// constant all read as `Missing`, and the next lean run would refuse
    /// rather than measure.
    #[test]
    fn the_engine_in_this_checkout_declares_the_context_mode_flag() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("the crate sits two levels below the repository root")
            .to_path_buf();
        assert_eq!(
            flag_availability_in_checkout(&repo_root).unwrap(),
            FlagAvailability::Present,
            "the engine's catalog no longer declares ADR 0085's `{}` in the `key: \"…\"` \
             form this preflight scans for, so every lean run would refuse.",
            crate::arm::CONTEXT_MODE_PREFERENCE_KEY
        );
    }
}
