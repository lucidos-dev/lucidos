use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};

/// Regression: the spawn consumer's Continue path must hand CC a non-empty
/// `user_message`. `claude --print --resume` parks indefinitely on stdin
/// when no input is sent (verified empirically against claude 2.1.123),
/// the engine's `events_rx` never resolves, and the thread sits "Running"
/// forever — the second-stage zombie observed on thread `ca025588-...`.
///
/// Two assertions:
///   1. The constant itself stays non-empty (and non-whitespace).
///   2. The consumer in `engine/mod.rs` actually routes through
///      `continue_input_for_reason`, the one function that guarantees a
///      non-empty input on every branch. This catches a regression where a
///      future edit reverts to a literal `""` while the constant stays defined
///      elsewhere. (The consumer stopped passing the constant directly on
///      2026-08-10, when an `answered_after_idle` continuation started carrying
///      the user's answer instead; every other reason still resolves to the
///      constant, inside that function.)
#[test]
fn spawn_consumer_continue_must_send_non_empty_user_message() {
    use super::CONTINUE_RESUME_USER_MESSAGE;

    assert!(
            !CONTINUE_RESUME_USER_MESSAGE.trim().is_empty(),
            "CONTINUE_RESUME_USER_MESSAGE is empty — CC --print --resume would hang on stdin and zombie the thread"
        );

    let consumer_src = include_str!("../engine_impl/construction.rs");
    assert!(
        consumer_src.contains("continue_input_for_reason"),
        "engine/engine_impl/construction.rs no longer routes its Continue input through \
             continue_input_for_reason, so the SpawnConsumer may have reverted to passing a \
             literal user_message and risks the empty-stdin zombie regression"
    );
}

/// Engine-startup recovery: an idle coding-agent thread with a committed
/// branch diff but no proposed change (e.g. one wedged by the now-removed
/// bg-bash propose-gate, whose only escape was a 5-min nudge or a manual
/// seed-change POST) must get its `ChangeProposed` re-emitted so the Apply
/// button reappears without the user nudging it. Per dev threads
/// `c1cec485-b1d0-483d-b31d-e2ba21dd76fb` / `7f971704-75cf-4a9e-8280-973eb2bea45d`.
///
/// Contract: `select_unproposed_idle_cc_threads` picks the thread, and
/// `propose_held_back_changes_on_startup_with_roots` emits exactly one
/// `ChangeProposed` for it (branch name + real committed file), moving the
/// thread to `proposed`.
#[tokio::test]
async fn startup_proposes_held_back_change_for_eligible_thread() {
    use super::{
        propose_held_back_changes_on_startup_with_roots, select_unproposed_idle_cc_threads,
    };

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let branch = "claude-code/held-back-propose-test";
    let (_tmp, repo_root, thread_id) = stuck_thread_with_held_back_work(&pool, &bus, branch).await;
    crate::engine::git_ops::record_planned(
        &pool,
        &repo_root,
        branch,
        crate::engine::git_ops::PlanMarkerKind::AcknowledgedSimple,
        None,
        Some("test fixture"),
        &[],
        "abc",
    )
    .await
    .expect("record a satisfying plan marker");

    // Selection picks exactly this thread.
    let selected = select_unproposed_idle_cc_threads(&pool).await;
    assert_eq!(
        selected,
        vec![thread_id],
        "selection must return the idle CC thread with a diff and no proposal"
    );

    // Lucidos-kind thread → lucidos_repo_root is the test repo. workspace_path
    // is irrelevant for this thread (only routed for App-kind); pass the same
    // tempdir to keep the test self-contained.
    propose_held_back_changes_on_startup_with_roots(&pool, &bus, &repo_root, &repo_root, &selected)
        .await;

    // The contract: the thread is proposed, so the ChangeProposed event landed
    // and its projection arm ran.
    let proposed_after: bool = sqlx::query_scalar(
        "SELECT coding_agent_change_state = 'proposed' FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        proposed_after,
        "the thread must be proposed after the held-back propose runs — \
         without this the user has a worktree with a real diff but no Apply button. \
         See threads c1cec485-… / 7f971704-… (2026-05-28/29)."
    );

    // Exactly one ChangeProposed event lives in the timeline for the thread,
    // carrying the branch name + the real committed file.
    let cp_rows: Vec<(serde_json::Value,)> = sqlx::query_as(
        "SELECT payload FROM events \
         WHERE thread_id = $1 AND event_type = 'ChangeProposed' \
         ORDER BY sequence",
    )
    .bind(thread_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        cp_rows.len(),
        1,
        "exactly one ChangeProposed must be emitted by the recovery helper"
    );
    let payload = &cp_rows[0].0;
    assert_eq!(
        payload["branch_name"],
        serde_json::Value::String(branch.into())
    );
    let files = payload["files"].as_array().expect("files is array");
    assert!(
        files.iter().any(|f| f.as_str() == Some("a.txt")),
        "ChangeProposed must list the real committed file; got {:?}",
        files
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An idle Lucidos-kind thread whose turn ended cleanly with committed work on
/// `branch` that no change carries: the shape the held-back sweep rescues.
async fn stuck_thread_with_held_back_work(
    pool: &sqlx::PgPool,
    bus: &EventBus,
    branch: &str,
) -> (tempfile::TempDir, std::path::PathBuf, Uuid) {
    use crate::test_support::{make_repo_and_worktree, start_cc_session};

    let (_tmp, repo_root, wt) = make_repo_and_worktree(branch).await;

    // Real committed work on the branch — without this,
    // `proposal_files_for_branch` returns None and the propose path skips.
    std::fs::write(wt.join("a.txt"), "held-back content").unwrap();
    use crate::engine::git_ops::git_cmd;
    git_cmd(&["add", "."], &wt).await.unwrap();
    git_cmd(&["commit", "-m", "held-back commit"], &wt)
        .await
        .unwrap();

    let thread_id = Uuid::new_v4();

    // SessionStarted seeds `is_coding_agent=true, state='active'` and the
    // branch lookup the held-back-propose helper needs.
    start_cc_session(bus, thread_id, branch, None).await;

    // A clean Generated terminal: the thread FINISHED its turn, and the
    // per-idle propose was held back. `propose_one_held_back_change` rescues
    // only threads whose last turn ended cleanly.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "Done.".into(),
            images: Vec::new(),
            model: Some("test-model".into()),
            reasoning_effort: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    // Stuck-after-restart shape: a real committed diff, never proposed.
    sqlx::query(
        "UPDATE thread_summaries \
         SET coding_agent_change_state = 'unproposed', \
             coding_agent_unproposed_reason = NULL, \
             archive_state = 'inbox' \
         WHERE thread_id = $1",
    )
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("stamp stuck shape");
    (_tmp, repo_root, thread_id)
}

/// The payloads of a thread's `ProposalWithheld` events, oldest first.
async fn withheld_payloads(pool: &sqlx::PgPool, thread_id: Uuid) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT payload FROM events \
         WHERE thread_id = $1 AND event_type = 'ProposalWithheld' ORDER BY sequence",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// The sweep applies the same plan floor as `propose_change`. Work on a
/// Lucidos-source branch with no marker stays unproposed, because Apply would
/// refuse it. I3: the hold is announced once with branch, files and reason, and
/// a second boot over the same branch announces nothing.
#[tokio::test]
async fn startup_holds_a_held_back_change_whose_branch_has_no_plan_marker() {
    use super::propose_held_back_changes_on_startup_with_roots;
    use crate::engine::thread_events::UnproposedReason;
    use crate::engine::thread_lifecycle::CodingAgentChangeState;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let branch = "claude-code/held-back-no-plan";
    let (_tmp, repo_root, thread_id) = stuck_thread_with_held_back_work(&pool, &bus, branch).await;

    for _boot in 0..2 {
        propose_held_back_changes_on_startup_with_roots(
            &pool,
            &bus,
            &repo_root,
            &repo_root,
            &[thread_id],
        )
        .await;
    }

    let proposals: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = 'ChangeProposed'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(proposals, 0, "a marker-less branch must not be proposed");
    let withheld = withheld_payloads(&pool, thread_id).await;
    assert_eq!(
        withheld.len(),
        1,
        "two boots, one announcement: {withheld:?}"
    );
    assert_eq!(withheld[0]["branch_name"], branch);
    assert_eq!(withheld[0]["files"], serde_json::json!(["a.txt"]));
    assert_eq!(withheld[0]["reason"], "plan_missing");
    assert_eq!(
        crate::test_support::read_change_state(&pool, thread_id).await,
        CodingAgentChangeState::Unproposed {
            reason: Some(UnproposedReason::PlanMissing)
        }
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A thread still holding an event wait after a clean turn parked mid-work, so
/// its idle held the proposal (ADR 0395). A restart must not undo that hold:
/// the wait survives it, and its delivery, expiry or cancel proposes later. A
/// user Stop never proposes, wait or no wait: its work is withheld (ADR 0400).
#[tokio::test]
async fn startup_propose_helper_holds_only_a_clean_turn_holding_an_event_wait() {
    use super::propose_held_back_changes_on_startup_with_roots;
    use crate::engine::thread_events::CancelCause;
    use crate::test_support::{make_repo_and_worktree, start_cc_session};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    let cases = [
        (
            "clean",
            ThreadEvent::ResponseGenerated {
                text: "Not done yet.".into(),
                images: Vec::new(),
                model: Some("test-model".into()),
                reasoning_effort: None,
            },
            0,
        ),
        (
            "stopped",
            ThreadEvent::ResponseCanceled {
                text: String::new(),
                images: Vec::new(),
                model: None,
                reasoning_effort: None,
                cause: CancelCause::UserStop,
            },
            1,
        ),
    ];
    for (name, terminal, expected_withheld) in cases {
        let branch = format!("claude-code/held-at-boot-{name}");
        let (_tmp, repo_root, wt) = make_repo_and_worktree(&branch).await;
        std::fs::write(wt.join("a.txt"), "waiting work").unwrap();
        use crate::engine::git_ops::git_cmd;
        git_cmd(&["add", "."], &wt).await.unwrap();
        git_cmd(&["commit", "-m", "waiting work"], &wt)
            .await
            .unwrap();

        // A satisfying plan marker, so the plan floor stays out of a test
        // about the event-wait hold.
        crate::engine::git_ops::record_planned(
            &pool,
            &repo_root,
            &branch,
            crate::engine::git_ops::PlanMarkerKind::AcknowledgedSimple,
            None,
            Some("test fixture"),
            &[],
            "abc",
        )
        .await
        .expect("record a satisfying plan marker");

        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, &branch, None).await;
        let wait_id = Uuid::new_v4();
        for event in [crate::test_support::event_wait_started(wait_id), terminal] {
            bus.emit(BusEvent::Thread {
                thread_id,
                event,
                meta: cc_meta.clone(),
            })
            .await
            .expect("emit succeeds");
        }
        sqlx::query(
            "UPDATE thread_summaries SET coding_agent_change_state = 'unproposed' \
             WHERE thread_id = $1",
        )
        .bind(thread_id)
        .execute(&pool)
        .await
        .expect("stamp the held shape");

        propose_held_back_changes_on_startup_with_roots(
            &pool,
            &bus,
            &repo_root,
            &repo_root,
            &[thread_id],
        )
        .await;

        let proposals: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'ChangeProposed'",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(proposals, 0, "{name} turn holding a live event wait");
        assert_eq!(
            withheld_payloads(&pool, thread_id).await.len(),
            expected_withheld,
            "{name}: only the unfinished turn is announced as withheld"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Companion: the held-back propose helper must skip threads that don't need
/// rescuing. Three cases — already-proposed, no diff, external repo — must
/// emit nothing. Without this, the recovery could double-emit `ChangeProposed`
/// (the proposal projection has dedup, but the timeline would carry a noise
/// event) or re-emit on external-repo threads (the engine doesn't own
/// proposals for those branches at all — the user pushes/PRs from CC).
#[tokio::test]
async fn startup_propose_helper_skips_ineligible_threads() {
    use super::propose_held_back_changes_on_startup_with_roots;
    use crate::test_support::{make_repo_and_worktree, start_cc_session};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let branch = "claude-code/skip-ineligible-test";
    let (_tmp, repo_root, wt) = make_repo_and_worktree(branch).await;
    std::fs::write(wt.join("a.txt"), "x").unwrap();
    use crate::engine::git_ops::git_cmd;
    git_cmd(&["add", "."], &wt).await.unwrap();
    git_cmd(&["commit", "-m", "x"], &wt).await.unwrap();

    // Case A: already-proposed. The held-back path must skip — the user
    // already has an Apply button from the existing ChangeProposed.
    let already_proposed = Uuid::new_v4();
    start_cc_session(&bus, already_proposed, branch, None).await;
    sqlx::query(
        "UPDATE thread_summaries SET coding_agent_change_state = 'proposed' \
         WHERE thread_id = $1",
    )
    .bind(already_proposed)
    .execute(&pool)
    .await
    .unwrap();

    // Case B: no diff. The held-back path has nothing to propose — emitting
    // an empty ChangeProposed would mislabel the timeline.
    let no_diff = Uuid::new_v4();
    let branch_b = "claude-code/no-diff-test";
    start_cc_session(&bus, no_diff, branch_b, None).await;
    sqlx::query(
        "UPDATE thread_summaries SET coding_agent_change_state = 'none' \
         WHERE thread_id = $1",
    )
    .bind(no_diff)
    .execute(&pool)
    .await
    .unwrap();

    // Case C: external repo. The engine doesn't own proposals for external
    // repos at all — CC pushes/PRs from the session itself.
    let external = Uuid::new_v4();
    let branch_c = "claude-code/external-test";
    start_cc_session(&bus, external, branch_c, Some("ext-repo".into())).await;
    sqlx::query(
        "UPDATE thread_summaries \
         SET coding_agent_change_state = 'unproposed', \
             coding_agent_is_external_repo = TRUE \
         WHERE thread_id = $1",
    )
    .bind(external)
    .execute(&pool)
    .await
    .unwrap();

    // Case D: a real committed diff (reuses `branch`'s commit) but the last
    // turn ended NOT-clean (ResponseAborted — engine killed it mid-turn). The
    // sweep only re-proposes FINISHED threads; an interrupted thread recovers
    // via Continue, so its work is withheld, never proposed (ADR 0400).
    let interrupted = Uuid::new_v4();
    start_cc_session(&bus, interrupted, branch, None).await;
    bus.emit(BusEvent::Thread {
        thread_id: interrupted,
        event: ThreadEvent::ResponseAborted {
            text: String::new(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::EngineShutdown,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    sqlx::query(
        "UPDATE thread_summaries SET coding_agent_change_state = 'unproposed' \
         WHERE thread_id = $1",
    )
    .bind(interrupted)
    .execute(&pool)
    .await
    .unwrap();

    propose_held_back_changes_on_startup_with_roots(
        &pool,
        &bus,
        &repo_root,
        &repo_root,
        &[already_proposed, no_diff, external, interrupted],
    )
    .await;

    // No NEW ChangeProposed event from this helper run, on any of the four.
    for tid in [already_proposed, no_diff, external, interrupted] {
        let n: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events \
             WHERE thread_id = $1 AND event_type = 'ChangeProposed'",
        )
        .bind(tid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            n, 0,
            "thread {} is ineligible (already-proposed / no-diff / external / interrupted-not-clean) — the helper must emit zero ChangeProposed",
            tid
        );
    }
    for (tid, expected) in [
        (already_proposed, 0),
        (no_diff, 0),
        (external, 0),
        (interrupted, 1),
    ] {
        assert_eq!(
            withheld_payloads(&pool, tid).await.len(),
            expected,
            "only the interrupted thread's work is announced as withheld"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// App-kind regression: an App coding-agent thread's branch lives in the
/// workspace's own git (`<workspace>/data/apps/<id>/`), not the Lucidos main
/// worktree. If the helper hardcoded `lucidos_repo_root` for every thread,
/// App-kind stuck threads would silently fall out — `proposal_files_for_branch`
/// would return `None` against the wrong repo and the thread would stay
/// stuck with a worktree-with-diff and no Apply button, exactly the shape
/// this whole helper exists to fix.
///
/// Contract: when `coding_agent_kind = 'app'`, the helper must inspect the
/// `workspace_path` git for the branch (not `lucidos_repo_root`), and emit
/// `ChangeProposed` against that repo. The test uses two distinct
/// tempdir-backed repos to make the routing observable: if the code reached
/// for the wrong root, the branch wouldn't exist there and no event would
/// land.
#[tokio::test]
async fn startup_proposes_held_back_change_for_app_kind_thread() {
    use super::{
        propose_held_back_changes_on_startup_with_roots, select_unproposed_idle_cc_threads,
    };
    use crate::test_support::{make_repo_and_worktree, start_cc_session};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let branch = "claude-code/app-held-back-test";
    let (_ws_tmp, workspace_path, app_wt) = make_repo_and_worktree(branch).await;

    // Real committed work on the App branch in the WORKSPACE repo. The
    // Lucidos repo (created below) deliberately does NOT carry this branch
    // so the test can distinguish "routed to workspace" from "routed to
    // lucidos".
    std::fs::write(app_wt.join("a.txt"), "app held-back content").unwrap();
    use crate::engine::git_ops::git_cmd;
    git_cmd(&["add", "."], &app_wt).await.unwrap();
    git_cmd(&["commit", "-m", "app commit"], &app_wt)
        .await
        .unwrap();

    // Separate Lucidos main repo. The App branch is intentionally absent
    // here — if the helper mis-routes to lucidos_repo_root, the branch
    // lookup returns None and no event lands.
    use crate::test_support::make_repo_and_worktree as make_repo;
    let lucidos_branch = "claude-code/unrelated-lucidos-branch";
    let (_luc_tmp, lucidos_repo_root, _luc_wt) = make_repo(lucidos_branch).await;

    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, branch, None).await;

    // Clean Generated terminal — only finished threads are eligible for
    // re-proposal (`propose_one_held_back_change` skips non-clean turns).
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "Done.".into(),
            images: Vec::new(),
            model: Some("test-model".into()),
            reasoning_effort: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    // Stamp the stuck-after-restart shape AND the App kind. start_cc_session
    // defaults to Lucidos kind, so override it explicitly here.
    sqlx::query(
        "UPDATE thread_summaries \
         SET coding_agent_change_state = 'unproposed', \
             coding_agent_kind = 'app', \
             archive_state = 'inbox' \
         WHERE thread_id = $1",
    )
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("stamp App-kind stuck shape");

    let selected = select_unproposed_idle_cc_threads(&pool).await;
    assert_eq!(
        selected,
        vec![thread_id],
        "selection must include App-kind too"
    );

    propose_held_back_changes_on_startup_with_roots(
        &pool,
        &bus,
        &lucidos_repo_root,
        &workspace_path,
        &selected,
    )
    .await;

    let proposed_after: bool = sqlx::query_scalar(
        "SELECT coding_agent_change_state = 'proposed' FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        proposed_after,
        "App-kind thread must also get rescued — the helper must read \
         coding_agent_kind='app' and look up the branch in workspace_path, \
         not lucidos_repo_root. Hardcoded lucidos_repo_root would silently \
         skip every App-kind stuck thread."
    );

    let cp_rows: Vec<(serde_json::Value,)> = sqlx::query_as(
        "SELECT payload FROM events \
         WHERE thread_id = $1 AND event_type = 'ChangeProposed' \
         ORDER BY sequence",
    )
    .bind(thread_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        cp_rows.len(),
        1,
        "exactly one ChangeProposed for the App thread"
    );
    let payload = &cp_rows[0].0;
    assert_eq!(
        payload["branch_name"],
        serde_json::Value::String(branch.into())
    );
    assert_eq!(
        payload["repo_root"],
        serde_json::Value::String(workspace_path.to_string_lossy().to_string()),
        "repo_root on the emitted event must be workspace_path, not lucidos_repo_root"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
