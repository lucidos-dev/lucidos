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
///   2. The consumer in `engine/mod.rs` actually references the constant —
///      catches a regression where a future edit reverts to a literal `""`
///      while the constant stays defined elsewhere.
#[test]
fn spawn_consumer_continue_must_send_non_empty_user_message() {
    use super::CONTINUE_RESUME_USER_MESSAGE;

    assert!(
            !CONTINUE_RESUME_USER_MESSAGE.trim().is_empty(),
            "CONTINUE_RESUME_USER_MESSAGE is empty — CC --print --resume would hang on stdin and zombie the thread"
        );

    let consumer_src = include_str!("../engine_impl/construction.rs");
    assert!(
        consumer_src.contains("CONTINUE_RESUME_USER_MESSAGE"),
        "engine/engine_impl/construction.rs no longer references CONTINUE_RESUME_USER_MESSAGE — \
             the SpawnConsumer's Continue path may have reverted to passing a \
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
/// `ChangeProposed` for it (branch name + real committed file), flipping
/// `coding_agent_proposed` TRUE.
#[tokio::test]
async fn startup_proposes_held_back_change_for_eligible_thread() {
    use super::{
        propose_held_back_changes_on_startup_with_roots, select_unproposed_idle_cc_threads,
    };
    use crate::test_support::{make_repo_and_worktree, start_cc_session};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let branch = "claude-code/held-back-propose-test";
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
    start_cc_session(&bus, thread_id, branch, None).await;

    // A clean Generated terminal: the thread FINISHED its turn (the per-idle
    // propose was held back), which is what makes it eligible for re-proposal —
    // `propose_one_held_back_change` only rescues threads whose last turn ended
    // cleanly.
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
         SET coding_agent_has_diff = TRUE, \
             coding_agent_proposed = FALSE, \
             archive_state = 'inbox' \
         WHERE thread_id = $1",
    )
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("stamp stuck shape");

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

    // The contract: coding_agent_proposed is TRUE — the ChangeProposed event
    // landed and its projection arm ran.
    let proposed_after: bool = sqlx::query_scalar(
        "SELECT coding_agent_proposed FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        proposed_after,
        "coding_agent_proposed must flip TRUE after the held-back propose runs — \
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
        "UPDATE thread_summaries \
         SET coding_agent_has_diff = TRUE, coding_agent_proposed = TRUE \
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
        "UPDATE thread_summaries \
         SET coding_agent_has_diff = FALSE, coding_agent_proposed = FALSE \
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
         SET coding_agent_has_diff = TRUE, coding_agent_proposed = FALSE, \
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
    // via Continue, so the helper must skip it even though it has a diff and
    // no proposal.
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
        "UPDATE thread_summaries \
         SET coding_agent_has_diff = TRUE, coding_agent_proposed = FALSE \
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
         SET coding_agent_has_diff = TRUE, \
             coding_agent_proposed = FALSE, \
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
        "SELECT coding_agent_proposed FROM thread_summaries WHERE thread_id = $1",
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
