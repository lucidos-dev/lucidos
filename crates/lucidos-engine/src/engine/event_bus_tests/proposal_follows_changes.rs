//! `coding_agent_proposed`, `coding_agent_requires_restart` and
//! `coding_agent_incomplete` are a cache of the `changes` table: after every
//! event they equal what the thread's pending changes say.

use super::super::*;
use super::*;
use crate::core::changes_projection::ChangesProjection;

/// The two columns, and what the thread's pending changes say they should be.
async fn proposal_and_truth(pool: &PgPool, thread_id: Uuid) -> ((bool, bool), (bool, bool)) {
    let row: (bool, bool, bool, bool) = sqlx::query_as(
        "SELECT t.coding_agent_proposed, t.coding_agent_requires_restart, \
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

/// `coding_agent_incomplete`, and whether any pending change is incomplete.
async fn incomplete_and_truth(pool: &PgPool, thread_id: Uuid) -> (bool, bool) {
    sqlx::query_as(
        "SELECT t.coding_agent_incomplete, \
                COALESCE((SELECT bool_or(c.incomplete) FROM changes c \
                          WHERE c.thread_id = t.thread_id AND c.status = 'pending'), FALSE) \
         FROM thread_summaries t WHERE t.thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn assert_follows_changes(pool: &PgPool, thread_id: Uuid, context: &str) {
    let (columns, truth) = proposal_and_truth(pool, thread_id).await;
    assert_eq!(
        columns, truth,
        "(proposed, requires_restart) drifted from the changes table after {context}"
    );
    let (column, truth) = incomplete_and_truth(pool, thread_id).await;
    assert_eq!(
        column, truth,
        "incomplete drifted from the changes table after {context}"
    );
}

fn propose(change_id: Uuid, branch: &str, requires_restart: bool) -> ThreadEvent {
    proposal(change_id.to_string(), None, branch, requires_restart, false)
}

/// What a user Stop proposes: the work the turn left, marked incomplete.
fn propose_stopped(change_id: Uuid, branch: &str) -> ThreadEvent {
    proposal(change_id.to_string(), None, branch, false, true)
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
/// idle or an archive, on one of two changes. After each, the columns must
/// equal the `changes` query. A refused event (an archived thread refusing a
/// proposal, say) still has to leave the two in step.
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
            let (label, event) = match rng.below(6) {
                0 if rng.below(2) == 0 => ("stopped propose", propose_stopped(*change_id, branch)),
                0 => ("propose", propose(*change_id, branch, restart)),
                1 => ("per-commit propose", propose_per_commit(branch, restart)),
                2 => ("apply", applied(*change_id)),
                3 => ("discard", discarded(*change_id)),
                4 => (
                    "idle",
                    ThreadEvent::CodingAgentIdled {
                        has_changes: true,
                        is_external_repo: false,
                        requires_restart: restart,
                        cc_session_id: None,
                        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                        reason: None,
                        worktree_path: None,
                        worktree_head_sha: None,
                        bg_bash_pending: false,
                    },
                ),
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

/// I3 of ADR 0346: a stopped turn's change keeps the proposal up, so it still
/// blocks Archive, and marks it incomplete. A clean re-proposal of the same
/// change clears the mark. Setting it aside clears both, and bringing it back
/// restores what the row says.
#[tokio::test]
async fn an_incomplete_change_keeps_the_proposal_and_marks_it_incomplete() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    let branch = format!("claude-code/stop-{thread_id}");
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
    let state = || async {
        (
            proposal_and_truth(&pool, thread_id).await.0 .0,
            incomplete_and_truth(&pool, thread_id).await.0,
        )
    };

    emit(propose_stopped(change_id, &branch)).await.unwrap();
    assert_eq!(
        state().await,
        (true, true),
        "a Stop proposes incomplete work"
    );

    emit(propose(change_id, &branch, false)).await.unwrap();
    assert_eq!(
        state().await,
        (true, false),
        "a clean finish clears the mark"
    );

    emit(propose_stopped(change_id, &branch)).await.unwrap();
    emit(ThreadEvent::ChangeSetAside {
        change_id: change_id.to_string(),
    })
    .await
    .unwrap();
    assert_eq!(
        state().await,
        (false, false),
        "a set-aside change is not pending"
    );

    emit(ThreadEvent::ChangeBroughtBack {
        change_id: change_id.to_string(),
    })
    .await
    .unwrap();
    assert_eq!(
        state().await,
        (true, true),
        "bringing it back restores the mark"
    );

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
    ChangesProjection::sync_thread_proposal(&pool, thread_id)
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

/// I3 of ADR 0346, on the rebuild path: an incomplete change rebuilt from its
/// events marks the thread incomplete again.
#[tokio::test]
async fn an_incomplete_change_rebuilt_from_events_marks_the_thread_again() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    let branch = format!("claude-code/rebuilt-stop-{thread_id}");
    start_cc_session(&bus, thread_id, &branch, None).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: propose_stopped(change_id, &branch),
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    sqlx::query("DELETE FROM changes WHERE id = $1")
        .bind(change_id)
        .execute(&pool)
        .await
        .unwrap();
    ChangesProjection::sync_thread_proposal(&pool, thread_id)
        .await
        .unwrap();
    assert!(!incomplete_and_truth(&pool, thread_id).await.0);

    ChangesProjection::new(pool.clone())
        .rebuild_missing_from_events()
        .await
        .unwrap();
    assert_eq!(incomplete_and_truth(&pool, thread_id).await, (true, true));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Rows that drifted before the columns became a cache are healed once, by
/// the resync migration: a flag left up with no pending row, and a pending row
/// whose flag was cleared.
#[tokio::test]
async fn the_resync_migration_heals_rows_that_drifted_before() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let (orphan_flag, orphan_row) = (Uuid::new_v4(), Uuid::new_v4());
    for thread_id in [orphan_flag, orphan_row] {
        start_cc_session(
            &bus,
            thread_id,
            &format!("claude-code/drift-{thread_id}"),
            None,
        )
        .await;
    }
    bus.emit(BusEvent::Thread {
        thread_id: orphan_row,
        event: propose(
            Uuid::new_v4(),
            &format!("claude-code/drift-{orphan_row}"),
            true,
        ),
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    sqlx::query(
        "UPDATE thread_summaries SET coding_agent_proposed = (thread_id = $1), \
                coding_agent_requires_restart = (thread_id = $1) \
         WHERE thread_id IN ($1, $2)",
    )
    .bind(orphan_flag)
    .bind(orphan_row)
    .execute(&pool)
    .await
    .unwrap();

    sqlx::raw_sql(include_str!(
        "../../../migrations/20260926174928_resync_coding_agent_proposal_from_changes.sql"
    ))
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(
        proposal_and_truth(&pool, orphan_flag).await.0,
        (false, false)
    );
    assert_eq!(proposal_and_truth(&pool, orphan_row).await.0, (true, true));

    pool.close().await;
    teardown_test_db(&db_name).await;
}
