use super::*;
use crate::core::changes::ChangeStatus;
use crate::engine::thread_lifecycle::ThreadStatus;

#[tokio::test]
async fn coding_agent_idled_does_not_set_proposed_until_change_proposed() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/split", None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: true,
            cc_session_id: Some("sid-split".into()),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
            turn_end: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (status, is_external): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_is_external_repo FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        status, "idle",
        "CodingAgentIdled alone must NOT promote to 'waiting'"
    );
    assert_eq!(
        read_change_state(&pool, thread_id).await,
        CodingAgentChangeState::Unproposed { reason: None },
        "CodingAgentIdled records work, never a proposal or its restart flag"
    );
    assert!(!is_external, "is_external_repo=false in payload");

    emit_change_proposed(&bus, thread_id, "claude-code/split", true).await;

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "idle",
        "ChangeProposed keeps status='idle' (review is an artifact, not a parked loop)"
    );
    assert_eq!(
        read_change_state(&pool, thread_id).await,
        CodingAgentChangeState::Proposed {
            requires_restart: true
        },
        "the restart flag comes from the ChangeProposed payload"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn sse_thread_event_carries_aggregate() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let mut rx = bus.subscribe();

    start_cc_session(&bus, thread_id, "claude-code/agg-test", None).await;

    while rx.try_recv().is_ok() {}

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseCanceled {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::CancelCause::UserStop,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let evt = rx.recv().await.expect("ResponseCanceled broadcast");
    let agg = evt
        .aggregate
        .as_ref()
        .expect("aggregate must be present on persisted thread events");
    assert_eq!(
        agg.section, "inbox",
        "ResponseCanceled puts CC thread in inbox"
    );
    assert_eq!(
        agg.status,
        ThreadStatus::Idle,
        "after cancel, status returns to idle"
    );
    assert_eq!(agg.thread_id, thread_id.to_string());

    let sse = evt.to_sse_json();
    let v: serde_json::Value = serde_json::from_str(&sse).unwrap();
    assert_eq!(v["data"]["aggregate"]["section"], "inbox");
    assert_eq!(v["data"]["aggregate"]["status"], "idle");
    assert!(v["data"]["aggregate"].get("composeText").is_none());
    assert!(v["data"]["aggregate"].get("composeImages").is_none());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn claude_code_idled_with_changes_sets_proposed_keeps_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/test", None).await;
    emit_cc_idle(&bus, thread_id, true, None).await;

    let (status, has_changes): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_change_state = 'proposed' FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        status, "idle",
        "CodingAgentIdled(has_changes=true) + ChangeProposed settle to 'idle' under Option B"
    );
    assert!(has_changes);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn external_repo_idle_with_changes_never_shows_apply_discard() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/external", None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: true,
            requires_restart: false,
            cc_session_id: Some("sid-ext".into()),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
            turn_end: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let (status, has_changes, is_external): (String, bool, bool) = sqlx::query_as(
            "SELECT status, coding_agent_change_state = 'proposed', coding_agent_is_external_repo FROM thread_summaries WHERE thread_id = $1"
        ).bind(thread_id).fetch_one(&pool).await.unwrap();
    assert_eq!(
        status, "idle",
        "no proposal → idle (Apply/Discard never appear)"
    );
    assert!(
        !has_changes,
        "no ChangeProposed → the thread stays unproposed"
    );
    assert!(
        is_external,
        "coding_agent_is_external_repo reflects the payload"
    );

    let change_proposed_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = 'ChangeProposed'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        change_proposed_count, 0,
        "External repo threads must never have ChangeProposed events — \
             the runtime must skip propose_change for external repos"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn full_apply_cycle_ends_idle_not_waiting() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let events = vec![
        (
            ThreadEvent::SessionStarted {
                sandbox: None,
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                session_id: "s1".into(),
                branch: "claude-code/fix".into(),
                repo_id: None,
                coding_agent_kind: Default::default(),
                coding_agent_folder: String::new(),
                app_id: None,
                integrated: false,
                auto_compact_window: None,
                auto_compact_arm: None,
                continuity: None,
            },
            EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..EventMeta::NONE
            },
        ),
        (
            ThreadEvent::CodingAgentIdled {
                has_changes: true,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: Some("sid".into()),
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                reason: None,
                worktree_path: None,
                worktree_head_sha: None,
                bg_bash_pending: false,
                turn_end: None,
            },
            EventMeta::NONE,
        ),
        (
            ThreadEvent::ChangeProposed {
                content: Default::default(),
                change_id: "c1".into(),
                description: Some("Fix".into()),
                files: vec!["f.rs".into()],
                requires_restart: false,
                origin: None,
                commit_sha: None,
                branch_name: String::new(),
                repo_root: String::new(),
                hardened: false,
                incomplete: false,
                set_aside: false,
                path: String::new(),
                diff: String::new(),
            },
            EventMeta::NONE,
        ),
        (
            ThreadEvent::ChangeApplied {
                content: Default::default(),
                change_id: "c1".into(),
                requires_restart: false,
                client_update: false,
                commits: vec![],
                thread_title: None,
                actor: None,
                pre_merge_sha: None,
                post_merge_sha: None,
                landing_merge_sha: None,
                path: String::new(),
            },
            EventMeta::NONE,
        ),
        (
            ThreadEvent::CodingAgentIdled {
                has_changes: false,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: Some("sid".into()),
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                reason: None,
                worktree_path: None,
                worktree_head_sha: None,
                bg_bash_pending: false,
                turn_end: None,
            },
            EventMeta::NONE,
        ),
    ];

    for (event, meta) in events {
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta,
        })
        .await
        .unwrap();
    }

    let (status, has_changes, requires_restart, is_external): (String, bool, bool, bool) =
        sqlx::query_as(
            "SELECT status, coding_agent_change_state = 'proposed', coding_agent_requires_restart, coding_agent_is_external_repo \
             FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        status, "idle",
        "After full apply cycle, thread must be idle"
    );
    assert!(!has_changes, "the thread must not be proposed after apply");
    assert!(!requires_restart);
    assert!(!is_external);

    let sid: Option<String> = sqlx::query_scalar(
        "SELECT payload->>'cc_session_id' FROM events \
             WHERE thread_id = $1 AND event_type = 'CodingAgentIdled' \
             ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        sid.as_deref(),
        Some("sid"),
        "cc_session_id must survive for resume"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn change_applied_persists_device_actor_in_payload() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let device = MessageOrigin::Device {
        device_id: "dev-actor-test".into(),
    };

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
            content: Default::default(),
            change_id: "change-actor-test".into(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: Some(device.clone()),
            pre_merge_sha: None,
            post_merge_sha: None,
            landing_merge_sha: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("emit ChangeApplied");

    let actor_json: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'actor' FROM events \
         WHERE thread_id = $1 AND event_type = 'ChangeApplied' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .expect("query persisted ChangeApplied actor");

    let actor_json = actor_json.expect(
        "ChangeApplied payload must carry a non-null `actor` field — \
         a missing/null actor renders as 'Lucidos Engine' in the UI",
    );
    assert_eq!(
        actor_json.get("kind").and_then(|v| v.as_str()),
        Some("device"),
        "actor.kind must be 'device' (not engine/agent), got: {actor_json:?}"
    );
    assert_eq!(
        actor_json.get("device_id").and_then(|v| v.as_str()),
        Some("dev-actor-test"),
        "actor.device_id must round-trip, got: {actor_json:?}"
    );
    assert!(
        actor_json.get("label").is_none(),
        "a device actor stores its id, never its name, got: {actor_json:?}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_database_refuses_a_second_change_applied_for_one_change() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let change_id = seed_pending_change(&bus, thread_id, "claude-code/applied-once").await;
    let applied = || BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
            content: Default::default(),
            change_id: change_id.to_string(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: None,
            landing_merge_sha: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    };

    bus.emit(applied()).await.expect("the first apply records");
    let Err(second) = bus.emit(applied()).await else {
        panic!("a second ChangeApplied for one change is refused");
    };
    assert!(
        second
            .to_string()
            .contains(crate::engine::change_ops::CHANGE_APPLIED_ONCE_INDEX),
        "refused by the index, not some other failure: {second}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn slow_path_change_applied_carries_stashed_apply_actor() {
    use crate::engine::pending_apply_actors::PendingApplyActors;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let stash = PendingApplyActors::default();
    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();

    let device = MessageOrigin::Device {
        device_id: "iphone-slow-path".into(),
    };

    stash.stash(change_id, device.clone());

    let recovered = stash.take(change_id);
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
            content: Default::default(),
            change_id: change_id.to_string(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: recovered,
            pre_merge_sha: None,
            post_merge_sha: None,
            landing_merge_sha: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("emit ChangeApplied");

    let actor_json: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'actor' FROM events \
         WHERE thread_id = $1 AND event_type = 'ChangeApplied' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .expect("query persisted ChangeApplied actor");

    let actor_json = actor_json.expect(
        "slow-path ChangeApplied must carry the stashed actor — \
         a missing actor means the stash → take wiring regressed and the chip falls back to 'Lucidos Engine'",
    );
    assert_eq!(
        actor_json.get("kind").and_then(|v| v.as_str()),
        Some("device"),
    );
    assert_eq!(
        actor_json.get("device_id").and_then(|v| v.as_str()),
        Some("iphone-slow-path"),
    );

    assert!(
        stash.take(change_id).is_none(),
        "stash entry must be consumed after first take"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn propose_time_reconcile_keeps_single_pending_and_proposed_flag() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    let change_a = seed_pending_change(&bus, thread_id, "claude-code/old-A").await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeDiscarded {
            change_id: change_a.to_string(),
            actor: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    let change_b = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            content: Default::default(),
            change_id: change_b.to_string(),
            description: Some("Fix v2".into()),
            files: vec!["src/main.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: "claude-code/new-B".into(),
            repo_root: "/tmp".into(),
            hardened: false,
            incomplete: false,
            set_aside: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let pending: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, branch_name FROM changes WHERE thread_id = $1 AND status = 'pending' \
         ORDER BY created_at",
    )
    .bind(thread_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        pending.len(),
        1,
        "only the new-branch change may remain pending — the orphan blocks Archive; got {:?}",
        pending
    );
    assert_eq!(
        pending[0].0, change_b,
        "the surviving pending change is branch B's"
    );
    assert_eq!(pending[0].1, "claude-code/new-B");

    let a_status: ChangeStatus = sqlx::query_scalar("SELECT status FROM changes WHERE id = $1")
        .bind(change_a)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        a_status,
        ChangeStatus::Discarded,
        "the stale branch-A change must be discarded"
    );

    let proposed: bool = sqlx::query_scalar(
        "SELECT coding_agent_change_state = 'proposed' FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        proposed,
        "the thread must be proposed after a cross-branch propose: the new \
         change is still pending"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn propose_change_reconciles_stale_branches_before_change_proposed_emit() {
    let src = include_str!("../change_ops/propose.rs");
    let discard_pos = src
        .find("discard_open_changes_for_thread_except")
        .expect("propose_change must reconcile stale other-branch pending changes");
    let proposed_pos = src
        .find("ThreadEvent::ChangeProposed")
        .expect("propose_change must still emit ChangeProposed");
    assert!(
        discard_pos < proposed_pos,
        "the sibling reconcile must run BEFORE the ChangeProposed emit: otherwise \
         ChangeDiscarded's ClearAll wipes the change state this proposal sets. \
         See docs/plans/2026-07-01-orphaned-pending-change-blocks-archive.md"
    );
}

#[test]
fn apply_time_reconcile_lives_in_apply_change_gated_not_in_handler_or_emitter() {
    let apply = include_str!("../change_ops/apply.rs");
    assert!(
        apply.contains("discard_orphaned_pending_siblings"),
        "apply_change must reconcile orphaned sibling pending changes on a successful apply"
    );
    assert!(
        apply.contains("ApplyStatus::Applied"),
        "the apply-time reconcile must be gated on ApplyStatus::Applied — reconciling on \
         Noop/Hardening/Conflict discards newer sibling work (data loss)"
    );
    let apply_now =
        include_str!("../../../../lucidos-agents-api/src/engine/agent_session/apply_now.rs");
    assert!(
        apply_now.contains("discard_orphaned_pending_siblings"),
        "apply_now_success (live in-place merge) must reconcile orphaned siblings — it bypasses apply_change"
    );
    let http = include_str!("../../../../lucidos-agents-api/src/api/changes.rs");
    assert!(
        !http.contains("discard_orphaned_pending_siblings"),
        "the HTTP apply_change handler must NOT reconcile directly — apply_change does it, gated on Applied"
    );
    let emitters = include_str!("../change_ops_emitters.rs");
    assert!(
        !emitters.contains("discard_orphaned_pending_siblings"),
        "emit_change_applied must NOT reconcile siblings — it is shared with the external-repo \
         archive loop, which applies multiple pending changes per thread in sequence"
    );
}

#[test]
fn apply_time_dev_refresh_lives_only_on_the_shared_change_applied_emit() {
    let emitters = include_str!("../change_ops_emitters.rs");
    assert!(
        emitters.contains("post_apply_dev_refresh"),
        "emit_change_applied must drive the post-apply dev refresh — it is the one emit \
         every merge path performs exactly once"
    );
    for (label, src) in [
        (
            "change_ops/apply.rs",
            include_str!("../change_ops/apply.rs"),
        ),
        (
            "agent_session/apply_now.rs",
            include_str!("../../../../lucidos-agents-api/src/engine/agent_session/apply_now.rs"),
        ),
        (
            "agent_session/run_session/completion.rs",
            include_str!(
                "../../../../lucidos-agents-api/src/engine/agent_session/run_session/completion.rs"
            ),
        ),
        (
            "api/changes.rs",
            include_str!("../../../../lucidos-agents-api/src/api/changes.rs"),
        ),
    ] {
        assert!(
            !src.contains("join_or_start_background_rebuild()")
                && !src.contains("restart_background_rebuild()")
                && !src.contains("refresh_served_frontend_after_rebuild()"),
            "{label} must NOT trigger the rebuild / frontend re-snapshot directly. \
             emit_change_applied owns it, as the one emit every merge path performs."
        );
    }
}

#[test]
fn post_apply_refresh_runs_only_for_the_accepted_change_applied() {
    let emitters = include_str!("../change_ops_emitters.rs");
    let emit_pos = emitters
        .find("ThreadEvent::ChangeApplied {")
        .expect("emit_change_applied must emit ChangeApplied");
    let tail = &emitters[emit_pos..];
    assert!(
        tail.contains("Ok(Some(_)) => Ok(true)"),
        "emit_change_applied must emit via `emit` (not `emit_or_log`) and read the \
         outcome, so a failed emit fires no refresh"
    );
    let accepted_pos = tail
        .find("if accepted {")
        .expect("the post-apply refresh must be gated on the emit being accepted");
    let refresh_pos = tail
        .find("post_apply_dev_refresh(")
        .expect("emit_change_applied must call the post-apply refresh");
    assert!(
        accepted_pos < refresh_pos,
        "the `accepted` gate must wrap the post_apply_dev_refresh call, not follow it"
    );
}

#[test]
fn every_apply_clears_the_branch_gate_markers_from_the_shared_emit() {
    let emitters = include_str!("../change_ops_emitters.rs");
    let emit_pos = emitters
        .find("ThreadEvent::ChangeApplied {")
        .expect("emit_change_applied must emit ChangeApplied");
    let tail = &emitters[emit_pos..];
    let accepted_pos = tail
        .find("if accepted {")
        .expect("the marker clear must be gated on the emit being accepted");
    let clear_pos = tail
        .find("clear_gate_markers_of_applied_change(change_id)")
        .expect("emit_change_applied must clear the applied branch's markers");
    assert!(
        accepted_pos < clear_pos,
        "the `accepted` gate must wrap the clear"
    );
    for fn_name in ["consume_harden_marker(", "consume_plan_marker("] {
        assert!(tail.contains(fn_name), "the clear must call {fn_name}");
    }
    for (label, src) in [
        (
            "change_ops/apply.rs",
            include_str!("../change_ops/apply.rs"),
        ),
        (
            "agent_session/apply_now.rs",
            include_str!("../../../../lucidos-agents-api/src/engine/agent_session/apply_now.rs"),
        ),
    ] {
        assert!(
            !src.contains("consume_plan_marker("),
            "{label} must not clear the plan marker itself: emit_change_applied owns it"
        );
    }
}

#[test]
fn discard_change_notifies_apply_all_driver_so_batch_advances() {
    let discard = include_str!("../change_ops/discard.rs");
    assert!(
        discard.contains("notify_apply_all") && discard.contains("WITHDRAWN_MEMBER_REASON"),
        "discard_change must notify the Apply-All driver (Failed / WITHDRAWN_MEMBER_REASON) so a \
         batch member discarded mid-batch advances the batch instead of stalling it"
    );
}

mod guard_tests {
    #[test]
    fn change_applied_is_built_only_by_the_claim_holding_emitter() {
        let builders: Vec<String> = crate::test_support::source_scan::production_sources()
            .into_iter()
            .filter(|(path, src)| {
                !path.ends_with("event.rs") && src.contains("event: ThreadEvent::ChangeApplied {")
            })
            .map(|(path, _)| path)
            .collect();
        assert_eq!(builders, vec!["engine/change_ops_emitters.rs".to_string()]);
    }
}
