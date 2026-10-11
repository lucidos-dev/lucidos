//! The `proposed` change state and `coding_agent_requires_restart` are a cache
//! of the `changes` table: after every event they equal what the thread's
//! pending changes say. The unproposed reason never outlives its state.

use super::super::*;
use super::*;
use crate::core::changes_projection::ChangesProjection;
use crate::engine::thread_events::UnproposedReason;

/// The two cached facts, and what the thread's pending changes say they
/// should be.
async fn proposal_and_truth(pool: &PgPool, thread_id: Uuid) -> ((bool, bool), (bool, bool)) {
    let row: (bool, bool, bool, bool) = sqlx::query_as(
        "SELECT t.coding_agent_change_state = 'proposed', t.coding_agent_requires_restart, \
                EXISTS (SELECT 1 FROM changes c \
                        WHERE c.thread_id = t.thread_id AND c.status = 'pending'), \
                COALESCE((SELECT bool_or(c.requires_restart) FROM changes c \
                          WHERE c.thread_id = t.thread_id AND c.status = 'pending'), FALSE) \
         FROM thread_summaries t WHERE t.thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap();
    ((row.0, row.1), (row.2, row.3))
}

async fn assert_follows_changes(pool: &PgPool, thread_id: Uuid, context: &str) {
    let (columns, truth) = proposal_and_truth(pool, thread_id).await;
    assert_eq!(
        columns, truth,
        "(proposed, requires_restart) drifted from the changes table after {context}"
    );
}

fn propose(change_id: Uuid, branch: &str, requires_restart: bool) -> ThreadEvent {
    proposal(change_id.to_string(), None, branch, requires_restart, false)
}

/// What a Stop proposed before ADR 0400: the turn's work, marked incomplete.
/// The event stays readable history, so the cache must still follow it.
fn propose_legacy_incomplete(change_id: Uuid, branch: &str) -> ThreadEvent {
    proposal(change_id.to_string(), None, branch, false, true)
}

fn withheld(branch: &str, reason: UnproposedReason) -> ThreadEvent {
    ThreadEvent::ProposalWithheld {
        branch_name: branch.into(),
        files: vec!["a.rs".into()],
        reason,
    }
}

fn withdrawn(change_id: Uuid) -> ThreadEvent {
    ThreadEvent::ChangeWithdrawn {
        change_id: change_id.to_string(),
    }
}

fn idled(has_changes: bool) -> ThreadEvent {
    ThreadEvent::CodingAgentIdled {
        has_changes,
        is_external_repo: false,
        requires_restart: false,
        cc_session_id: None,
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        reason: None,
        worktree_path: None,
        worktree_head_sha: None,
        bg_bash_pending: false,
    }
}

/// A legacy per-commit proposal: no change id, a commit sha.
fn propose_per_commit(branch: &str, requires_restart: bool) -> ThreadEvent {
    proposal(
        String::new(),
        Some("abc123".into()),
        branch,
        requires_restart,
        false,
    )
}

fn proposal(
    change_id: String,
    commit_sha: Option<String>,
    branch: &str,
    requires_restart: bool,
    incomplete: bool,
) -> ThreadEvent {
    ThreadEvent::ChangeProposed {
        change_id,
        description: Some("work".into()),
        files: vec!["a.rs".into()],
        requires_restart,
        origin: None,
        commit_sha,
        branch_name: branch.into(),
        repo_root: "/tmp".into(),
        hardened: false,
        incomplete,
        set_aside: false,
        path: String::new(),
        diff: String::new(),
    }
}

fn applied(change_id: Uuid) -> ThreadEvent {
    ThreadEvent::ChangeApplied {
        change_id: change_id.to_string(),
        requires_restart: false,
        client_update: false,
        commits: vec![],
        thread_title: None,
        actor: None,
        pre_merge_sha: None,
        post_merge_sha: None,
        path: String::new(),
    }
}

fn discarded(change_id: Uuid) -> ThreadEvent {
    ThreadEvent::ChangeDiscarded {
        change_id: change_id.to_string(),
        actor: None,
        path: String::new(),
    }
}

/// A small deterministic generator, so a failing sequence replays exactly.
struct XorShift(u64);

impl XorShift {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n
    }
}

/// The invariant from the plan, as a property over event sequences: every
/// step is a proposal, a legacy per-commit proposal, an apply, a discard, an
/// idle, a withholding, a withdrawal or an archive, on one of two changes.
/// After each, the columns must equal the `changes` query. A refused event (an
/// archived thread refusing a proposal, say) still has to leave the two in
/// step.
#[tokio::test]
async fn the_proposal_columns_equal_the_pending_changes_after_every_event() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    for seed in 1..=24u64 {
        let mut rng = XorShift(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, &format!("claude-code/a-{thread_id}"), None).await;
        let changes = [
            (Uuid::new_v4(), format!("claude-code/a-{thread_id}")),
            (Uuid::new_v4(), format!("claude-code/b-{thread_id}")),
        ];
        let mut steps = Vec::new();
        for _ in 0..10 {
            let (change_id, branch) = &changes[rng.below(2) as usize];
            let restart = rng.below(2) == 0;
            let (label, event) = match rng.below(8) {
                0 if rng.below(2) == 0 => (
                    "legacy incomplete propose",
                    propose_legacy_incomplete(*change_id, branch),
                ),
                0 => ("propose", propose(*change_id, branch, restart)),
                1 => ("per-commit propose", propose_per_commit(branch, restart)),
                2 => ("apply", applied(*change_id)),
                3 => ("discard", discarded(*change_id)),
                4 => ("idle", idled(rng.below(2) == 0)),
                5 => (
                    "withhold",
                    withheld(branch, UnproposedReason::TurnIncomplete),
                ),
                6 => ("withdraw", withdrawn(*change_id)),
                _ => ("archive", ThreadEvent::ThreadArchived),
            };
            steps.push(label);
            let _ = bus
                .emit(BusEvent::Thread {
                    thread_id,
                    event,
                    meta: EventMeta {
                        channel: Some(EventChannel::ClaudeCode),
                        ..EventMeta::NONE
                    },
                })
                .await;
            assert_follows_changes(&pool, thread_id, &format!("seed {seed}, {steps:?}")).await;
        }
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// I6 and I8: each event moves the change state as the plan's table says, and
/// the reason never outlives `unproposed`. Only `ProposalWithheld` sets it.
#[tokio::test]
async fn each_event_moves_the_change_state_and_the_reason_follows_it() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    let branch = format!("claude-code/transitions-{thread_id}");
    start_cc_session(&bus, thread_id, &branch, None).await;

    let emit = |event: ThreadEvent| {
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        })
    };
    let unproposed = |reason| CodingAgentChangeState::Unproposed { reason };
    let proposed = CodingAgentChangeState::Proposed {
        requires_restart: false,
    };
    let incomplete = Some(UnproposedReason::TurnIncomplete);

    let steps: Vec<(&str, ThreadEvent, CodingAgentChangeState)> = vec![
        (
            "withheld on none",
            withheld(&branch, UnproposedReason::TurnIncomplete),
            unproposed(incomplete),
        ),
        (
            "withheld again, a new reason",
            withheld(&branch, UnproposedReason::PlanMissing),
            unproposed(Some(UnproposedReason::PlanMissing)),
        ),
        (
            "an idle with work keeps the reason",
            idled(true),
            unproposed(Some(UnproposedReason::PlanMissing)),
        ),
        (
            "a proposal clears it",
            propose(change_id, &branch, false),
            proposed,
        ),
        (
            "withheld on proposed stays proposed",
            withheld(&branch, UnproposedReason::PlanAwaitingApproval),
            proposed,
        ),
        (
            "an empty idle keeps a pending change proposed",
            idled(false),
            proposed,
        ),
        (
            "set aside lands unproposed",
            ThreadEvent::ChangeSetAside {
                change_id: change_id.to_string(),
            },
            unproposed(None),
        ),
        (
            "brought back",
            ThreadEvent::ChangeBroughtBack {
                change_id: change_id.to_string(),
            },
            proposed,
        ),
        (
            "withdrawn lands unproposed",
            withdrawn(change_id),
            unproposed(None),
        ),
        (
            "withheld after the withdrawal",
            withheld(&branch, UnproposedReason::TurnIncomplete),
            unproposed(incomplete),
        ),
        (
            "an idle with no work clears state and reason",
            idled(false),
            CodingAgentChangeState::None,
        ),
    ];
    for (label, event, expected) in steps {
        emit(event).await.unwrap();
        assert_eq!(
            read_change_state(&pool, thread_id).await,
            expected,
            "{label}"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Apply and Discard clear withheld work along with its reason.
#[tokio::test]
async fn apply_and_discard_clear_withheld_work_and_its_reason() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    for resolve in [applied, discarded] {
        let thread_id = Uuid::new_v4();
        let branch = format!("claude-code/resolve-{thread_id}");
        start_cc_session(&bus, thread_id, &branch, None).await;
        for event in [
            withheld(&branch, UnproposedReason::OutsideBound),
            resolve(Uuid::new_v4()),
        ] {
            bus.emit(BusEvent::Thread {
                thread_id,
                event,
                meta: EventMeta::NONE,
            })
            .await
            .unwrap();
        }
        assert_eq!(
            read_change_state(&pool, thread_id).await,
            CodingAgentChangeState::None
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Two pending changes on one thread: resolving one leaves the proposal up,
/// carrying the survivor's restart flag. `ChangeApplied` used to clear both
/// columns outright.
#[tokio::test]
async fn resolving_one_of_two_pending_changes_keeps_the_proposal() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let (first, second) = (Uuid::new_v4(), Uuid::new_v4());
    start_cc_session(&bus, thread_id, "claude-code/one", None).await;

    for event in [
        propose(first, &format!("claude-code/one-{thread_id}"), false),
        propose(second, &format!("claude-code/two-{thread_id}"), true),
    ] {
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    }
    assert_eq!(proposal_and_truth(&pool, thread_id).await.0, (true, true));

    bus.emit(BusEvent::Thread {
        thread_id,
        event: applied(first),
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_eq!(
        proposal_and_truth(&pool, thread_id).await.0,
        (true, true),
        "the second change is still pending and still needs a restart"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: discarded(second),
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_eq!(proposal_and_truth(&pool, thread_id).await.0, (false, false));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A legacy per-commit `ChangeProposed` updates the pending row by branch. It
/// used to leave the thread's restart flag behind the row's.
#[tokio::test]
async fn a_legacy_per_commit_proposal_moves_the_restart_flag_with_the_row() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let branch = format!("claude-code/legacy-{thread_id}");
    start_cc_session(&bus, thread_id, &branch, None).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: propose(Uuid::new_v4(), &branch, false),
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_eq!(proposal_and_truth(&pool, thread_id).await.0, (true, false));

    bus.emit(BusEvent::Thread {
        thread_id,
        event: propose_per_commit(&branch, true),
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_eq!(proposal_and_truth(&pool, thread_id).await.0, (true, true));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A `changes` row lost and then rebuilt from its events brings the thread's
/// proposal back with it. `rebuild_missing_from_events` used to recreate the
/// row and leave the flag down.
#[tokio::test]
async fn a_change_rebuilt_from_events_raises_the_proposal_again() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    let branch = format!("claude-code/rebuilt-{thread_id}");
    start_cc_session(&bus, thread_id, &branch, None).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: propose(change_id, &branch, true),
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // Lose the row, and let the cache follow the loss.
    sqlx::query("DELETE FROM changes WHERE id = $1")
        .bind(change_id)
        .execute(&pool)
        .await
        .unwrap();
    ChangesProjection::sync_thread_proposal(&pool, thread_id, ChangeStateKind::Unproposed)
        .await
        .unwrap();
    assert_eq!(proposal_and_truth(&pool, thread_id).await.0, (false, false));

    let recovered = ChangesProjection::new(pool.clone())
        .rebuild_missing_from_events()
        .await
        .unwrap();
    assert_eq!(recovered, 1);
    assert_eq!(proposal_and_truth(&pool, thread_id).await.0, (true, true));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// I12: the migration maps every combination of the three old booleans and
/// the restart flag onto the one state. It runs here in a scratch schema
/// holding the old columns, since the test database is already migrated.
#[tokio::test]
async fn the_change_state_migration_backfills_every_old_combination() {
    let (pool, db_name) = setup_test_db().await;
    let mut conn = pool.acquire().await.unwrap();
    sqlx::raw_sql(
        "CREATE SCHEMA pre_change_state; \
         SET search_path TO pre_change_state; \
         CREATE TABLE thread_summaries ( \
             thread_id UUID PRIMARY KEY, \
             coding_agent_has_diff BOOLEAN NOT NULL, \
             coding_agent_proposed BOOLEAN NOT NULL, \
             coding_agent_incomplete BOOLEAN NOT NULL, \
             coding_agent_requires_restart BOOLEAN NOT NULL);",
    )
    .execute(&mut *conn)
    .await
    .unwrap();

    let mut expected = Vec::new();
    for bits in 0..16u8 {
        let [has_diff, proposed, incomplete, restart] = [0, 1, 2, 3].map(|i| bits & (1 << i) != 0);
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO thread_summaries VALUES ($1, $2, $3, $4, $5)")
            .bind(id)
            .bind(has_diff)
            .bind(proposed)
            .bind(incomplete)
            .bind(restart)
            .execute(&mut *conn)
            .await
            .unwrap();
        let state = if proposed {
            "proposed"
        } else if has_diff {
            "unproposed"
        } else {
            "none"
        };
        expected.push((id, state.to_string(), restart && proposed));
    }

    sqlx::raw_sql(include_str!(
        "../../../migrations/20261009125328_coding_agent_change_state.sql"
    ))
    .execute(&mut *conn)
    .await
    .unwrap();

    for (id, state, restart) in expected {
        let row: (String, Option<String>, bool) = sqlx::query_as(
            "SELECT coding_agent_change_state, coding_agent_unproposed_reason, \
                    coding_agent_requires_restart \
             FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(row, (state, None, restart));
    }
    let old_columns: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.columns \
         WHERE table_schema = 'pre_change_state' AND column_name IN \
               ('coding_agent_has_diff', 'coding_agent_proposed', 'coding_agent_incomplete')",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(old_columns, 0, "the three booleans are dropped");

    drop(conn);
    pool.close().await;
    teardown_test_db(&db_name).await;
}
