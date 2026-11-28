use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};

#[test]
fn spawn_consumer_continue_must_send_non_empty_user_message() {
    use super::CONTINUE_RESUME_USER_MESSAGE;

    assert!(
            !CONTINUE_RESUME_USER_MESSAGE.trim().is_empty(),
            "CONTINUE_RESUME_USER_MESSAGE is empty — CC --print --resume would hang on stdin and zombie the thread"
        );

    let consumer_src =
        include_str!("../../../../lucidos-engine/src/engine/engine_impl/construction.rs");
    assert!(
        consumer_src.contains("continue_input_for_reason"),
        "engine/engine_impl/construction.rs no longer routes its Continue input through \
             continue_input_for_reason, so the SpawnConsumer may have reverted to passing a \
             literal user_message and risks the empty-stdin zombie regression"
    );
}

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
    crate::engine::git_ops::record_hardened(
        &pool,
        &repo_root,
        branch,
        "abc",
        crate::engine::git_ops::HardenTier::Full,
    )
    .await
    .expect("record the harden marker");

    let selected = select_unproposed_idle_cc_threads(&pool).await;
    assert_eq!(
        selected,
        vec![thread_id],
        "selection must return the idle CC thread with a diff and no proposal"
    );

    propose_held_back_changes_on_startup_with_roots(&pool, &bus, &repo_root, &repo_root, &selected)
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
        "the thread must be proposed after the held-back propose runs — \
         without this the user has a worktree with a real diff but no Apply button. \
         See threads c1cec485-… / 7f971704-… (2026-05-28/29)."
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

async fn stuck_thread_with_held_back_work(
    pool: &sqlx::PgPool,
    bus: &EventBus,
    branch: &str,
) -> (tempfile::TempDir, std::path::PathBuf, Uuid) {
    use crate::test_support::{make_repo_and_worktree, start_cc_session};

    let (_tmp, repo_root, wt) = make_repo_and_worktree(branch).await;

    std::fs::write(wt.join("a.txt"), "held-back content").unwrap();
    use crate::engine::git_ops::git_cmd;
    git_cmd(&["add", "."], &wt).await.unwrap();
    git_cmd(&["commit", "-m", "held-back commit"], &wt)
        .await
        .unwrap();

    let thread_id = Uuid::new_v4();

    start_cc_session(bus, thread_id, branch, None).await;

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

#[tokio::test]
async fn startup_withholds_a_held_back_change_a_hook_refuses() {
    use super::propose_held_back_changes_on_startup_with_roots;
    use crate::test_support::{hook_refusals, install_refusing_hook};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let branch = "claude-code/held-back-refused";
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
    crate::engine::git_ops::record_hardened(
        &pool,
        &repo_root,
        branch,
        "abc",
        crate::engine::git_ops::HardenTier::Full,
    )
    .await
    .expect("record the harden marker");
    install_refusing_hook(&bus, "require-a-ticket", "ChangeProposed", "No ticket.").await;

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
    assert_eq!(proposals, 0);
    let withheld = withheld_payloads(&pool, thread_id).await;
    assert_eq!(withheld.len(), 1, "{withheld:?}");
    assert_eq!(withheld[0]["reason"], "hook_refused");
    assert_eq!(hook_refusals(&pool, "require-a-ticket").await, 2);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn startup_skips_a_lane_threads_integration_branch() {
    use super::propose_held_back_changes_on_startup_with_roots;
    use crate::engine::thread_lifecycle::CodingAgentChangeState;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    for marked in [false, true] {
        let branch = crate::engine::git_ops::lane_branch(Uuid::new_v4());
        let (_tmp, repo_root, thread_id) =
            stuck_thread_with_held_back_work(&pool, &bus, &branch).await;
        if marked {
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
            crate::engine::git_ops::record_hardened(
                &pool,
                &repo_root,
                &branch,
                "abc",
                crate::engine::git_ops::HardenTier::Full,
            )
            .await
            .expect("record the harden marker");
        }

        propose_held_back_changes_on_startup_with_roots(
            &pool,
            &bus,
            &repo_root,
            &repo_root,
            &[thread_id],
        )
        .await;

        let proposals: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = 'ChangeProposed'",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            proposals, 0,
            "marked={marked}: a lane branch is never proposed"
        );
        assert!(
            withheld_payloads(&pool, thread_id).await.is_empty(),
            "marked={marked}: a lane branch is never withheld either"
        );
        assert_eq!(
            crate::test_support::read_change_state(&pool, thread_id).await,
            CodingAgentChangeState::Unproposed { reason: None },
            "marked={marked}"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

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

    std::fs::write(app_wt.join("a.txt"), "app held-back content").unwrap();
    use crate::engine::git_ops::git_cmd;
    git_cmd(&["add", "."], &app_wt).await.unwrap();
    git_cmd(&["commit", "-m", "app commit"], &app_wt)
        .await
        .unwrap();

    use crate::test_support::make_repo_and_worktree as make_repo;
    let lucidos_branch = "claude-code/unrelated-lucidos-branch";
    let (_luc_tmp, lucidos_repo_root, _luc_wt) = make_repo(lucidos_branch).await;

    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, branch, None).await;

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

#[tokio::test]
async fn startup_never_reproposes_historical_closed_lane_work() {
    use crate::engine::event_bus::SystemEvent;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_tmp, repo, thread) =
        stuck_thread_with_held_back_work(&pool, &bus, "historical-lane-work").await;
    let batch = Uuid::new_v4();
    for event in [
        SystemEvent::LaneBatchStarted {
            batch_id: batch,
            members: vec![],
            base_sha: "base".into(),
            lane_thread_id: thread,
        },
        SystemEvent::LaneBatchFailed {
            batch_id: batch,
            reason: "test failure".into(),
            actor: None,
        },
    ] {
        bus.emit(BusEvent::System(event)).await.unwrap();
    }
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE thread_id = $1")
        .bind(thread)
        .fetch_one(&pool)
        .await
        .unwrap();
    for _ in 0..2 {
        super::propose_held_back_changes_on_startup_with_roots(
            &pool,
            &bus,
            &repo,
            &repo,
            &[thread],
        )
        .await;
    }
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE thread_id = $1")
        .bind(thread)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        after, before,
        "startup neither proposes nor repeats a hold for a closed lane"
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn proposal_details_read_the_hardened_marker() {
    use crate::engine::agent_recovery::recovery::branch_proposal_details;
    use crate::engine::git_ops::{record_hardened, HardenTier};

    let (pool, db_name) = setup_test_db().await;
    let branch = "claude-code/details-hardened";
    let (_tmp, repo_root, _wt) = crate::test_support::make_repo_and_worktree(branch).await;
    let changes = crate::core::changes_projection::ChangesProjection::new(pool.clone());
    let files: Vec<String> = Vec::new();

    let before =
        branch_proposal_details(&pool, &changes, Uuid::new_v4(), branch, &repo_root, &files).await;
    assert!(
        !before.hardened,
        "no marker and no change row: not hardened"
    );

    record_hardened(&pool, &repo_root, branch, "abc", HardenTier::Full)
        .await
        .expect("record the harden marker");
    let after =
        branch_proposal_details(&pool, &changes, Uuid::new_v4(), branch, &repo_root, &files).await;
    assert!(
        after.hardened,
        "the marker survives the worktree, so the details must report hardened"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
