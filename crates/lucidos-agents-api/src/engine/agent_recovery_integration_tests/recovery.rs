use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};

#[tokio::test]
async fn restored_worktree_recovery_requires_a_new_request_not_an_automatic_resume() {
    use crate::engine::agent_recovery::restored_recovery_branches;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    crate::test_support::start_cc_session(&bus, thread, "claude-code/archived", None).await;
    let sibling = Uuid::new_v4();
    crate::test_support::start_cc_session(&bus, sibling, "claude-code/archived", None).await;
    assert!(restored_recovery_branches(&pool).await.unwrap().is_none());
    sqlx::query("INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) SELECT $1, 'snapshot', now(), MAX(sequence) FROM events")
        .bind(Uuid::new_v4()).execute(&pool).await.unwrap();
    for _ in 0..2 {
        bus.emit(BusEvent::Thread {
            thread_id: thread,
            event: ThreadEvent::ContinuationStarted {
                branch: "claude-code/archived".into(),
                origin: None,
                reason: Some(crate::engine::agent_recovery::AUTO_RESUME_AFTER_SWITCH_REASON.into()),
                resumed_abort_id: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
        assert!(restored_recovery_branches(&pool)
            .await
            .unwrap()
            .unwrap()
            .is_empty());
        assert_eq!(
            crate::engine::branch_recovery_queries::branch_turn_in_flight(
                &pool,
                "claude-code/archived"
            )
            .await
            .unwrap(),
            Some(false)
        );
    }
    bus.emit(BusEvent::Thread {
        thread_id: thread,
        event: ThreadEvent::CodingAgentUserMessageSent {
            text: "continue my work".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    for _ in 0..2 {
        assert_eq!(
            restored_recovery_branches(&pool)
                .await
                .unwrap()
                .unwrap()
                .get("claude-code/archived"),
            Some(&thread)
        );
        assert_eq!(
            crate::engine::branch_recovery_queries::branch_turn_in_flight(
                &pool,
                "claude-code/archived"
            )
            .await
            .unwrap(),
            Some(true)
        );
    }
    sqlx::query("ALTER TABLE snapshot_restore_boundary RENAME TO hidden_restore_boundary")
        .execute(&pool)
        .await
        .unwrap();
    assert!(restored_recovery_branches(&pool).await.is_err());
    sqlx::query("ALTER TABLE hidden_restore_boundary RENAME TO snapshot_restore_boundary")
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn boot_withdraws_a_pending_incomplete_change_once() {
    use crate::core::changes::ChangeStatus;
    use crate::engine::agent_recovery::withdraw_incomplete_pending_changes_on_startup;
    use crate::engine::thread_events::UnproposedReason;
    use crate::engine::thread_lifecycle::CodingAgentChangeState;
    use crate::test_support::{read_change_state, start_cc_session};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    let propose = |branch: &str, incomplete: bool| ThreadEvent::ChangeProposed {
        content: Default::default(),
        change_id: Uuid::new_v4().to_string(),
        description: Some("work".into()),
        files: vec!["a.rs".into()],
        requires_restart: false,
        origin: None,
        commit_sha: None,
        branch_name: branch.into(),
        repo_root: "/tmp/repo".into(),
        hardened: false,
        incomplete,
        set_aside: false,
        path: String::new(),
        diff: String::new(),
    };

    let stopped = Uuid::new_v4();
    let clean = Uuid::new_v4();
    start_cc_session(&bus, stopped, "claude-code/stopped", None).await;
    start_cc_session(&bus, clean, "claude-code/clean", None).await;
    for (thread_id, branch, incomplete) in [
        (stopped, "claude-code/stopped", true),
        (clean, "claude-code/clean", false),
    ] {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: propose(branch, incomplete),
            meta: cc_meta.clone(),
        })
        .await
        .expect("change proposed emits");
    }

    withdraw_incomplete_pending_changes_on_startup(&pool, &bus).await;
    withdraw_incomplete_pending_changes_on_startup(&pool, &bus).await;

    let status_of = |branch: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, ChangeStatus>(
                "SELECT status FROM changes WHERE branch_name = $1",
            )
            .bind(branch)
            .fetch_one(&pool)
            .await
            .expect("row exists")
        }
    };
    assert_eq!(
        status_of("claude-code/stopped").await,
        ChangeStatus::Withdrawn
    );
    assert_eq!(status_of("claude-code/clean").await, ChangeStatus::Pending);
    assert_eq!(
        read_change_state(&pool, stopped).await,
        CodingAgentChangeState::Unproposed {
            reason: Some(UnproposedReason::TurnIncomplete)
        }
    );
    assert!(read_change_state(&pool, clean).await.is_proposed());

    let count = |event_type: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = $2",
            )
            .bind(stopped)
            .bind(event_type)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    assert_eq!(
        count("ChangeWithdrawn").await,
        1,
        "a second boot adds nothing"
    );
    assert_eq!(
        count("ProposalWithheld").await,
        1,
        "a second boot adds nothing"
    );

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn apply_now_no_live_session_fast_path_preserves_clean_pending_change() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let branch = "claude-code/apply-now-fast-path-test";
    let change_id = Uuid::new_v4();
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-fastpath".into(),
            branch: branch.into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("session start emits")
    .expect("event persisted");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            content: Default::default(),
            change_id: change_id.to_string(),
            description: Some("clean prior turn".into()),
            files: vec!["src/lib.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: branch.into(),
            repo_root: "/tmp/repo".into(),
            hardened: true,
            incomplete: false,
            set_aside: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: cc_meta,
    })
    .await
    .expect("change proposed emits")
    .expect("event persisted");

    let pending = bus
        .changes_projection()
        .pending_for_thread(thread_id)
        .await
        .expect("pending_for_thread query");
    assert_eq!(
        pending.len(),
        1,
        "apply_now's fast path queries pending_for_thread — must return the clean change"
    );

    let apply_now_src = include_str!("../agent_session/apply_now.rs");
    let read = apply_now_src
        .find("let pending = self.changes().pending_for_thread(thread_id).await?;")
        .expect("the no-live-session branch reads what is pending");
    let apply = apply_now_src
        .find("engine.apply_pending_now(thread_id, pending, actor)")
        .expect("the no-live-session branch applies what is pending");
    assert!(read < apply);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn last_turn_ended_cleanly_distinguishes_terminal_kinds() {
    use crate::engine::agent_recovery::last_turn_ended_cleanly;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };

    let no_terminal = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: no_terminal,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-noterm".into(),
            branch: "claude-code/noterm".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    assert!(
        !last_turn_ended_cleanly(&pool, no_terminal).await,
        "no terminal event yet — must NOT be classified as clean (the work is withheld)"
    );

    let clean = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: clean,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-clean".into(),
            branch: "claude-code/clean".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    bus.emit(BusEvent::Thread {
        thread_id: clean,
        event: ThreadEvent::ResponseGenerated {
            text: "Done.".into(),
            images: Vec::new(),
            model: Some("test-model".into()),
            reasoning_effort: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    bus.emit(BusEvent::Thread {
        thread_id: clean,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some("sid-clean".into()),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
            turn_end: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    assert!(
        last_turn_ended_cleanly(&pool, clean).await,
        "ResponseGenerated as the latest terminal MUST classify as clean — \
         the stale-session recovery would otherwise withhold a finished turn's work"
    );

    let aborted = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: aborted,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-abort".into(),
            branch: "claude-code/abort".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    bus.emit(BusEvent::Thread {
        thread_id: aborted,
        event: ThreadEvent::ResponseAborted {
            text: String::new(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::EngineShutdown,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    assert!(
        !last_turn_ended_cleanly(&pool, aborted).await,
        "ResponseAborted as latest terminal MUST classify as not-clean — \
         the user-visible branch state may be mid-edit"
    );

    let canceled = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: canceled,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-cancel".into(),
            branch: "claude-code/cancel".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    bus.emit(BusEvent::Thread {
        thread_id: canceled,
        event: ThreadEvent::ResponseCanceled {
            text: String::new(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::CancelCause::UserStop,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    assert!(
        !last_turn_ended_cleanly(&pool, canceled).await,
        "ResponseCanceled as latest terminal MUST classify as not-clean"
    );

    let failed = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: failed,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-fail".into(),
            branch: "claude-code/fail".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    bus.emit(BusEvent::Thread {
        thread_id: failed,
        event: ThreadEvent::ResponseFailed {
            error: "test error".into(),
            spend_cap: None,
            agent_exit: None,
            provider_error: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    assert!(
        !last_turn_ended_cleanly(&pool, failed).await,
        "ResponseFailed as latest terminal MUST classify as not-clean"
    );

    let clean_then_aborted = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: clean_then_aborted,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-mixed".into(),
            branch: "claude-code/mixed".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    bus.emit(BusEvent::Thread {
        thread_id: clean_then_aborted,
        event: ThreadEvent::ResponseGenerated {
            text: "Turn 1 done.".into(),
            images: Vec::new(),
            model: Some("test-model".into()),
            reasoning_effort: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    bus.emit(BusEvent::Thread {
        thread_id: clean_then_aborted,
        event: ThreadEvent::ResponseAborted {
            text: String::new(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::EngineShutdown,
        },
        meta: cc_meta,
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    assert!(
        !last_turn_ended_cleanly(&pool, clean_then_aborted).await,
        "a later Aborted MUST shadow an earlier clean Generated — the LATEST \
         terminal is what reflects the branch's current trustworthiness"
    );

    let chat_meta = EventMeta {
        channel: Some(EventChannel::Chat),
        ..EventMeta::NONE
    };
    let cross_channel = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id: cross_channel,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-xchan".into(),
            branch: "claude-code/xchan".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    bus.emit(BusEvent::Thread {
        thread_id: cross_channel,
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
    bus.emit(BusEvent::Thread {
        thread_id: cross_channel,
        event: ThreadEvent::ResponseGenerated {
            text: "chat answer".into(),
            images: Vec::new(),
            model: Some("test-model".into()),
            reasoning_effort: None,
        },
        meta: chat_meta,
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    assert!(
        !last_turn_ended_cleanly(&pool, cross_channel).await,
        "chat-channel ResponseGenerated MUST NOT shadow a CC-channel \
         ResponseAborted — stale-session recovery is CC-only and the CC \
         branch is genuinely mid-edit"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn last_turn_end_mirrors_the_idle_gate() {
    use crate::engine::agent_recovery::{last_turn_end, TurnEnd};
    use crate::engine::thread_events::{AbortCause, CancelCause};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    let canceled = |cause| ThreadEvent::ResponseCanceled {
        text: String::new(),
        images: Vec::new(),
        model: None,
        reasoning_effort: None,
        cause,
    };
    let cases = [
        (
            Some(ThreadEvent::ResponseGenerated {
                text: "Done.".into(),
                images: Vec::new(),
                model: None,
                reasoning_effort: None,
            }),
            Some(TurnEnd::Finished),
        ),
        (
            Some(canceled(CancelCause::UserStop)),
            Some(TurnEnd::Unfinished),
        ),
        (Some(canceled(CancelCause::SupersededByFollowup)), None),
        (
            Some(ThreadEvent::ResponseFailed {
                error: "stream interrupted".into(),
                spend_cap: None,
                agent_exit: None,
                provider_error: None,
            }),
            Some(TurnEnd::Unfinished),
        ),
        (
            Some(ThreadEvent::ResponseAborted {
                text: String::new(),
                images: Vec::new(),
                model: None,
                reasoning_effort: None,
                cause: AbortCause::EngineShutdown,
            }),
            Some(TurnEnd::Unfinished),
        ),
        (None, None),
    ];
    for (terminal, expected) in cases {
        let thread_id = Uuid::new_v4();
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SessionStarted {
                sandbox: None,
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                session_id: format!("sid-{thread_id}"),
                branch: format!("claude-code/{thread_id}"),
                repo_id: None,
                coding_agent_kind: Default::default(),
                coding_agent_folder: String::new(),
                app_id: None,
                integrated: false,
                auto_compact_window: None,
                auto_compact_arm: None,
                continuity: None,
            },
            meta: cc_meta.clone(),
        })
        .await
        .expect("emit succeeds");
        let name = terminal.as_ref().map(|t| t.event_type());
        if let Some(event) = terminal {
            bus.emit(BusEvent::Thread {
                thread_id,
                event,
                meta: cc_meta.clone(),
            })
            .await
            .expect("emit succeeds");
        }
        assert_eq!(
            last_turn_end(&pool, thread_id).await,
            expected,
            "last terminal {name:?}"
        );
    }

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn canceled_turn_classifies_idle_so_restart_does_not_reopen_it() {
    use crate::engine::agent_recovery::BRANCH_CLASSIFICATION_SQL;
    use crate::engine::thread_events::{AbortCause, CancelCause, SessionEndReason};
    use std::collections::HashMap;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };

    let start = |branch: &str| {
        let branch = branch.to_string();
        let bus = bus.clone();
        let meta = cc_meta.clone();
        async move {
            let thread_id = Uuid::new_v4();
            bus.emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::SessionStarted {
                    sandbox: None,
                    coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                    session_id: format!("sid-{branch}"),
                    branch,
                    repo_id: None,
                    coding_agent_kind: Default::default(),
                    coding_agent_folder: String::new(),
                    app_id: None,
                    integrated: false,
                    auto_compact_window: None,
                    auto_compact_arm: None,
                    continuity: None,
                },
                meta,
            })
            .await
            .expect("emit succeeds")
            .expect("event persisted");
            thread_id
        }
    };
    let emit = |thread_id: Uuid, event: ThreadEvent| {
        let bus = bus.clone();
        let meta = cc_meta.clone();
        async move {
            bus.emit(BusEvent::Thread {
                thread_id,
                event,
                meta,
            })
            .await
            .expect("emit succeeds")
            .expect("event persisted");
        }
    };
    let canceled_by_user = ThreadEvent::ResponseCanceled {
        text: String::new(),
        images: Vec::new(),
        model: None,
        reasoning_effort: None,
        cause: CancelCause::UserStop,
    };

    let user_stopped = start("claude-code/user-stopped").await;
    emit(user_stopped, canceled_by_user.clone()).await;

    let failed = start("claude-code/failed").await;
    emit(
        failed,
        ThreadEvent::ResponseFailed {
            error: "stream ended".into(),
            spend_cap: None,
            agent_exit: None,
            provider_error: None,
        },
    )
    .await;

    let panicked = start("claude-code/panicked").await;
    emit(
        panicked,
        ThreadEvent::SessionEnded {
            reason: SessionEndReason::Panic,
        },
    )
    .await;

    let legacy_no_reason = start("claude-code/legacy-no-reason").await;
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
         VALUES ($1, 'SessionEnded', '{}'::jsonb, $2, 'thread', $2::text)",
    )
    .bind(Uuid::new_v4())
    .bind(legacy_no_reason)
    .execute(&pool)
    .await
    .expect("legacy reason-less SessionEnded row inserts");

    start("claude-code/mid-turn").await;

    let switched = start("claude-code/switched").await;
    emit(
        switched,
        ThreadEvent::ResponseAborted {
            text: String::new(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
            cause: AbortCause::EngineShutdown,
        },
    )
    .await;

    let stale_resume = start("claude-code/stale-resume").await;
    emit(
        stale_resume,
        ThreadEvent::SessionEnded {
            reason: SessionEndReason::StaleResume,
        },
    )
    .await;

    let switched_then_session_ended = start("claude-code/switched-then-session-ended").await;
    emit(
        switched_then_session_ended,
        ThreadEvent::ResponseAborted {
            text: String::new(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
            cause: AbortCause::EngineShutdown,
        },
    )
    .await;
    emit(
        switched_then_session_ended,
        ThreadEvent::SessionEnded {
            reason: SessionEndReason::Shutdown,
        },
    )
    .await;

    let canceled_then_resumed = start("claude-code/canceled-then-resumed").await;
    emit(canceled_then_resumed, canceled_by_user).await;
    emit(
        canceled_then_resumed,
        ThreadEvent::CodingAgentUserMessageSent {
            text: "actually, carry on".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
    )
    .await;

    let rows: Vec<(Option<String>, Option<String>)> = sqlx::query_as(&BRANCH_CLASSIFICATION_SQL)
        .fetch_all(&pool)
        .await
        .expect("branch classification query runs");
    let status: HashMap<String, String> = rows
        .into_iter()
        .filter_map(|(branch, status)| Some((branch?, status?)))
        .collect();

    for (branch, expected, why) in [
        (
            "claude-code/user-stopped",
            "idle",
            "the user already ended this turn with Stop; a restart must not \
             re-open it as \"Response interrupted\" with a Continue button",
        ),
        (
            "claude-code/failed",
            "idle",
            "the turn ended on an error the user already saw; there is nothing \
             to continue",
        ),
        (
            "claude-code/panicked",
            "idle",
            "SessionEnded is terminal; landing in neither set made the caller \
             treat it as in-flight",
        ),
        (
            "claude-code/legacy-no-reason",
            "idle",
            "a reason-less SessionEnded is LegacyNonTerminal, so the reason \
             filter must keep it rather than NULL it out of the scan",
        ),
        (
            "claude-code/mid-turn",
            "running",
            "no terminal at all: this is the genuine mid-turn crash the \
             interrupt panel exists for",
        ),
        (
            "claude-code/switched",
            "running",
            "the switch teardown boundary must stay resumable, or \
             Switch to new version silently stops resuming work",
        ),
        (
            "claude-code/stale-resume",
            "running",
            "a stale --resume is retried against a fresh session; the \
             SessionEnded is transient, not a turn boundary",
        ),
        (
            "claude-code/switched-then-session-ended",
            "running",
            "a shutdown-reason SessionEnded is the engine going away mid-turn, \
             so it must not cancel out the switch boundary that precedes it",
        ),
        (
            "claude-code/canceled-then-resumed",
            "running",
            "the follow-up turn was in flight; a terminal only ends the turn \
             it terminates",
        ),
    ] {
        assert_eq!(
            status.get(branch).map(String::as_str),
            Some(expected),
            "branch {branch} must classify as {expected}: {why}"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn archived_question_park_classifies_idle_so_restart_does_not_revive_it() {
    use crate::engine::agent_question::aq_test_helpers::emit_user_question;
    use crate::engine::agent_recovery::BRANCH_CLASSIFICATION_SQL;
    use crate::engine::thread_events::AnswerKind;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    let emit = |thread_id: Uuid, event: ThreadEvent| {
        let bus = bus.clone();
        let meta = cc_meta.clone();
        async move {
            let label = event.event_type();
            bus.emit(BusEvent::Thread {
                thread_id,
                event,
                meta,
            })
            .await
            .unwrap_or_else(|e| panic!("{label} emit: {e}"))
            .unwrap_or_else(|| panic!("{label} persisted"));
        }
    };
    let session_started = |branch: &str| ThreadEvent::SessionStarted {
        sandbox: None,
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        session_id: "sid-test".into(),
        branch: branch.into(),
        repo_id: None,
        coding_agent_kind: Default::default(),
        coding_agent_folder: String::new(),
        app_id: None,
        integrated: false,
        auto_compact_window: None,
        auto_compact_arm: None,
        continuity: None,
    };
    let archive_parked = |branch: &'static str| {
        let emit = &emit;
        let bus = &bus;
        let session_started = &session_started;
        async move {
            let thread_id = Uuid::new_v4();
            emit(thread_id, session_started(branch)).await;
            emit_user_question(bus, thread_id, "toolu_park").await;
            emit(
                thread_id,
                ThreadEvent::UserQuestionAnswered {
                    compose: None,
                    tool_use_id: "toolu_park".into(),
                    quiet: false,
                    answer: AnswerKind::Canceled,
                },
            )
            .await;
            emit(thread_id, ThreadEvent::ThreadArchived).await;
            thread_id
        }
    };

    archive_parked("claude-code/archived-parked").await;

    let revived = archive_parked("claude-code/unarchived-then-new-turn").await;
    emit(revived, ThreadEvent::ThreadUnarchived).await;
    emit(
        revived,
        ThreadEvent::CodingAgentUserMessageSent {
            text: "pick it back up".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
    )
    .await;

    let rows: Vec<(String, String)> = sqlx::query_as(&BRANCH_CLASSIFICATION_SQL)
        .fetch_all(&pool)
        .await
        .expect("branch classification query runs");
    let status: std::collections::HashMap<String, String> = rows.into_iter().collect();

    assert_eq!(
        status
            .get("claude-code/archived-parked")
            .map(String::as_str),
        Some("idle"),
        "the archive ended the parked turn, so a restart must not reopen it"
    );
    assert_eq!(
        status
            .get("claude-code/unarchived-then-new-turn")
            .map(String::as_str),
        Some("running"),
        "a turn started after unarchiving was in flight; the archive ends only \
         the turn before it"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn boot_floor_withdraws_only_the_switch_promises_it_did_not_keep() {
    use crate::engine::agent_recovery::settle_unresumed_switch_threads;
    use crate::engine::thread_events::{AbortCause, MessageOrigin};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let device = MessageOrigin::Device {
        device_id: "d1".into(),
    };

    let resumed = Uuid::new_v4();
    let declined = Uuid::new_v4();
    let archived = Uuid::new_v4();
    let with_change = Uuid::new_v4();
    for thread_id in [resumed, declined, archived, with_change] {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode: None,
                text: "do the thing".into(),
                user_image_hashes: vec![],
                device_id: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: crate::engine::thread_events::ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: EventMeta::NONE,
        })
        .await
        .expect("emit succeeds")
        .expect("event persisted");

        if thread_id == with_change {
            let cc_meta = EventMeta {
                channel: Some(crate::engine::thread_events::EventChannel::ClaudeCode),
                ..EventMeta::NONE
            };
            bus.emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::SessionStarted {
                    sandbox: None,
                    coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                    session_id: "cc-session".into(),
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
                meta: cc_meta.clone(),
            })
            .await
            .expect("emit succeeds")
            .expect("event persisted");
            bus.emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ChangeProposed {
                    content: Default::default(),
                    change_id: "c-1".into(),
                    description: Some("Fix".into()),
                    files: vec!["src/main.rs".into()],
                    requires_restart: false,
                    origin: None,
                    commit_sha: None,
                    branch_name: "claude-code/fix".into(),
                    repo_root: "/tmp".into(),
                    hardened: false,
                    incomplete: false,
                    set_aside: false,
                    path: String::new(),
                    diff: String::new(),
                },
                meta: cc_meta,
            })
            .await
            .expect("emit succeeds")
            .expect("event persisted");
        }

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseAborted {
                text: "interrupted by an engine restart".into(),
                images: Vec::new(),
                model: None,
                reasoning_effort: None,
                cause: AbortCause::EngineShutdown,
            },
            meta: EventMeta {
                actor: Some(device.clone()),
                ..EventMeta::NONE
            },
        })
        .await
        .expect("emit succeeds")
        .expect("event persisted");
    }

    sqlx::query("UPDATE thread_summaries SET archive_state = 'archived' WHERE thread_id = $1")
        .bind(archived)
        .execute(&pool)
        .await
        .expect("archive the thread");

    let status_of = |thread_id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .expect("summary row exists")
        }
    };

    for thread_id in [resumed, declined, archived, with_change] {
        assert_eq!(
            status_of(thread_id).await,
            "paused",
            "a switch teardown must settle the thread at paused, not failed"
        );
    }

    let withdrawals = |thread_id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 \
                   AND event_type = 'ResponseAborted' \
                   AND payload->>'cause' = 'recovery_after_restart'",
            )
            .bind(thread_id.to_string())
            .fetch_one(&pool)
            .await
            .expect("count query")
        }
    };

    settle_unresumed_switch_threads(&pool, &bus, &std::collections::HashSet::from([resumed])).await;

    assert_eq!(
        withdrawals(resumed).await,
        0,
        "the thread this boot IS resuming must keep its promise: withdrawing it \
         would hand the user a Continue button for a turn already back in flight"
    );
    assert_eq!(
        withdrawals(declined).await,
        1,
        "the thread this boot declined must have its promise withdrawn, which is \
         what re-arms its Continue button"
    );
    assert_eq!(
        withdrawals(archived).await,
        1,
        "an archived thread is selected by NEITHER resume drain, so the floor is \
         its only chance: excluding it here is a permanent dead end"
    );
    assert_eq!(
        withdrawals(with_change).await,
        1,
        "a pending change does not exempt a thread from the floor. It used to, \
         by keeping the row off 'paused', which is the one status this selects."
    );

    for thread_id in [declined, archived, with_change] {
        assert_eq!(
            status_of(thread_id).await,
            "failed",
            "a withdrawn resume promise must stop reading as paused"
        );
    }

    for thread_id in [declined, archived, with_change] {
        let actors: Vec<Option<String>> = sqlx::query_scalar(
            "SELECT payload->'actor'->>'kind' FROM events \
             WHERE aggregate_id = $1 AND event_type = 'ResponseAborted' \
             ORDER BY sequence",
        )
        .bind(thread_id.to_string())
        .fetch_all(&pool)
        .await
        .expect("actor query");
        assert_eq!(
            actors,
            vec![Some("device".to_string()), Some("device".to_string())],
            "both boundaries about ONE user-initiated teardown must name the \
             human who caused it: the switch abort and the withdrawal of its \
             promise are two statements about the same restart"
        );
    }
    assert_eq!(
        status_of(resumed).await,
        "paused",
        "the thread whose promise is being KEPT must stay paused: it is on its \
         way back, and nothing is being asked of the user"
    );

    bus.emit(BusEvent::Thread {
        thread_id: resumed,
        event: ThreadEvent::ContinuationStarted {
            branch: String::new(),
            origin: None,
            reason: Some(crate::engine::agent_recovery::AUTO_RESUME_AFTER_SWITCH_REASON.into()),
            resumed_abort_id: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    settle_unresumed_switch_threads(&pool, &bus, &std::collections::HashSet::new()).await;
    assert_eq!(
        withdrawals(declined).await,
        1,
        "a second boot must not stack another withdrawal on the same abort"
    );
    assert_eq!(
        withdrawals(resumed).await,
        0,
        "a thread resumed on an earlier boot is superseded by its own \
         ContinuationStarted and must never be swept"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_stray_cancel_during_teardown_costs_the_switch_its_auto_resume() {
    use crate::engine::agent_recovery::{
        settle_unresumed_switch_threads, switch_was_user_initiated, BRANCH_CLASSIFICATION_SQL,
    };
    use crate::engine::thread_events::{AbortCause, CancelCause, MessageOrigin};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let device = MessageOrigin::Device {
        device_id: "d1".into(),
    };
    let fixed = Uuid::new_v4();
    let buggy = Uuid::new_v4();
    let branch_of = |thread_id: Uuid| format!("claude-code/{thread_id}");

    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };

    for thread_id in [fixed, buggy] {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode: None,
                text: "so go?".into(),
                user_image_hashes: vec![],
                device_id: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: crate::engine::thread_events::ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
            meta: cc_meta.clone(),
        })
        .await
        .expect("emit succeeds")
        .expect("event persisted");

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ResponseAborted {
                text: "This response was interrupted by an engine restart.".into(),
                images: Vec::new(),
                model: None,
                reasoning_effort: None,
                cause: AbortCause::EngineShutdown,
            },
            meta: EventMeta {
                actor: Some(device.clone()),
                ..cc_meta.clone()
            },
        })
        .await
        .expect("emit succeeds")
        .expect("event persisted");

        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SessionStarted {
                sandbox: None,
                session_id: String::new(),
                branch: branch_of(thread_id),
                repo_id: None,
                coding_agent_kind: crate::engine::thread_events::CodingAgentKind::Lucidos,
                coding_agent_folder: String::new(),
                app_id: None,
                coding_agent: crate::runtime::CodingAgent::ClaudeCode,
                integrated: false,
                auto_compact_window: None,
                auto_compact_arm: None,
                continuity: None,
            },
            meta: cc_meta.clone(),
        })
        .await
        .expect("emit succeeds")
        .expect("event persisted");
    }

    bus.emit(BusEvent::Thread {
        thread_id: buggy,
        event: ThreadEvent::ResponseCanceled {
            text: String::new(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
            cause: CancelCause::UserStop,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    let branch_status = |branch: String| {
        let pool = pool.clone();
        async move {
            let rows: Vec<(String, String)> = sqlx::query_as(&BRANCH_CLASSIFICATION_SQL)
                .fetch_all(&pool)
                .await
                .expect("branch classification query");
            rows.into_iter()
                .find(|(b, _)| *b == branch)
                .map(|(_, status)| status)
                .expect("the seeded branch is classified")
        }
    };
    assert_eq!(
        branch_status(branch_of(fixed)).await,
        "running",
        "with no phantom cancel the newest lifecycle event is SessionStarted, so \
         the turn is in flight and the resume gate can pick it up"
    );
    assert_eq!(
        branch_status(branch_of(buggy)).await,
        "idle",
        "the phantom cancel is a turn-ended event, so the classifier reads the \
         turn as finished and no resume is even attempted"
    );

    for thread_id in [fixed, buggy] {
        assert!(
            switch_was_user_initiated(&pool, thread_id).await,
            "the device-attributed teardown abort is still the newest thing on \
             the thread in the START-event sense"
        );
    }

    let status_of = |thread_id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>(
                "SELECT status FROM thread_summaries WHERE thread_id = $1",
            )
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .expect("summary row exists")
        }
    };
    for thread_id in [fixed, buggy] {
        assert_eq!(
            status_of(thread_id).await,
            "paused",
            "a switch teardown settles at paused, and the phantom cancel does \
             not disturb that: `ResponseCanceled` is a verdict-preserving arm"
        );
    }

    settle_unresumed_switch_threads(&pool, &bus, &std::collections::HashSet::from([fixed])).await;

    assert_eq!(
        status_of(fixed).await,
        "paused",
        "the resumed thread keeps its promise and stays behind the pause glyph \
         with no Continue button"
    );
    assert_eq!(
        status_of(buggy).await,
        "failed",
        "the reported end state: a transcript that opens 'Paused by restart' and \
         a thread that ends red, in the attention count, asking the user to \
         Continue work the engine had promised to resume itself"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_applied_change_on_the_branch_does_not_cost_a_later_turn_its_resume() {
    use crate::engine::agent_recovery::{
        branch_awaits_recovery, switch_was_user_initiated, BRANCH_CLASSIFICATION_SQL,
    };
    use crate::engine::thread_events::{AbortCause, MessageOrigin};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let branch = format!("claude-code/{thread_id}");
    let change_id = Uuid::new_v4();
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };
    let device = MessageOrigin::Device {
        device_id: "d1".into(),
    };

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            session_id: "sid-turn-1".into(),
            branch: branch.clone(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            content: Default::default(),
            change_id: change_id.to_string(),
            description: Some("turn 1".into()),
            files: vec!["a.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: branch.clone(),
            repo_root: "/tmp/repo".into(),
            hardened: false,
            incomplete: false,
            set_aside: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
            content: Default::default(),
            change_id: change_id.to_string(),
            requires_restart: false,
            client_update: false,
            commits: vec!["feat: turn 1".into()],
            thread_title: None,
            actor: Some(device.clone()),
            pre_merge_sha: None,
            post_merge_sha: None,
            landing_merge_sha: None,
            path: String::new(),
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentUserMessageSent {
            text: "follow-up".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            session_id: "sid-turn-2".into(),
            branch: branch.clone(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseAborted {
            text: String::new(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
            cause: AbortCause::EngineShutdown,
        },
        meta: EventMeta {
            actor: Some(device),
            ..cc_meta.clone()
        },
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    let completed: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM changes WHERE branch_name = $1 AND status = 'applied'",
    )
    .bind(&branch)
    .fetch_one(&pool)
    .await
    .expect("changes row query");
    assert_eq!(
        completed, 1,
        "precondition: turn 1's change is applied and the branch is in the \
         completed set the retired veto read"
    );

    let rows: Vec<(String, String)> = sqlx::query_as(&BRANCH_CLASSIFICATION_SQL)
        .fetch_all(&pool)
        .await
        .expect("branch classification query");
    let status = rows
        .into_iter()
        .find(|(b, _)| *b == branch)
        .map(|(_, s)| s)
        .expect("the seeded branch is classified");
    assert_eq!(
        status, "running",
        "an applied change from turn 1 must not read as turn 2 having ended"
    );

    assert!(
        switch_was_user_initiated(&pool, thread_id).await,
        "the teardown abort carries a device actor and EngineShutdown, so this \
         is a user switch and the boot owes it an auto-resume"
    );

    let pending = bus
        .changes_projection()
        .get_pending_by_branch(&branch)
        .await
        .expect("pending lookup succeeds");
    assert!(
        pending.is_none(),
        "precondition: the applied change leaves nothing pending, so the gate \
         cannot be rescued by the pending-change arm"
    );
    assert!(
        branch_awaits_recovery(&branch, &std::collections::HashSet::new(), false),
        "the reported failure: recovery must not skip this branch, or no resume \
         is actuated and the boot floor withdraws the promise the user was shown"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_user_switch_auto_resumes_an_app_coding_agent_thread() {
    use crate::engine::agent_recovery::{
        lookup_app_spawn_id, switch_was_user_initiated, BRANCH_CLASSIFICATION_SQL,
    };
    use crate::engine::thread_events::{AbortCause, MessageOrigin};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let app_id = "habit-tracker";
    let branch = format!("lucidos-claude-code-app-{app_id}-add-streaks-{thread_id}");
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: String::new(),
            branch: branch.clone(),
            repo_id: None,
            coding_agent_kind: crate::engine::thread_events::CodingAgentKind::App,
            coding_agent_folder: format!("/home/u/workspaces/dev/data/apps/{app_id}"),
            app_id: Some(app_id.to_string()),
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseAborted {
            text: String::new(),
            images: Vec::new(),
            model: None,
            reasoning_effort: None,
            cause: AbortCause::EngineShutdown,
        },
        meta: EventMeta {
            actor: Some(MessageOrigin::Device {
                device_id: "d1".into(),
            }),
            ..cc_meta.clone()
        },
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentToolResult {
            name: String::new(),
            result: "The user doesn't want to proceed with this tool use.".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            tool_use_id: "toolu_1".into(),
            parent_tool_use_id: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentTextStreamed {
            text: "\n\n".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            parent_tool_use_id: None,
            progress_note: false,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    let rows: Vec<(String, String)> = sqlx::query_as(&BRANCH_CLASSIFICATION_SQL)
        .fetch_all(&pool)
        .await
        .expect("branch classification query");
    let status = rows
        .into_iter()
        .find(|(b, _)| *b == branch)
        .map(|(_, s)| s)
        .expect("the app branch is classified");
    assert_eq!(
        status, "running",
        "neither trailing event is a turn terminal, so the app thread's turn is \
         still in flight when the next engine boots"
    );

    assert!(
        switch_was_user_initiated(&pool, thread_id).await,
        "the teardown abort carries a device actor and EngineShutdown, so this \
         app thread is owed an auto-resume rather than a Continue button"
    );

    assert_eq!(
        lookup_app_spawn_id(&pool, thread_id).await.as_deref(),
        Some(app_id),
        "an app thread's lost worktree must rebuild as a sparse cone over its \
         own app folder, never as the whole workspace tree"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_app_id_lookup_answers_none_for_a_non_app_thread() {
    use crate::engine::agent_recovery::lookup_app_spawn_id;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: String::new(),
            branch: format!("lucidos-claude-code-repo-fix-oauth-urls-{thread_id}"),
            repo_id: None,
            coding_agent_kind: crate::engine::thread_events::CodingAgentKind::Lucidos,
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    assert_eq!(
        lookup_app_spawn_id(&pool, thread_id).await,
        None,
        "a Lucidos-source thread has no app folder to narrow a rebuild to"
    );
    assert_eq!(
        lookup_app_spawn_id(&pool, Uuid::new_v4()).await,
        None,
        "a thread with no SessionStarted at all answers None, not a panic"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_crash_still_offers_continue_on_an_app_coding_agent_thread() {
    use crate::engine::agent_recovery::switch_was_user_initiated;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: String::new(),
            branch: format!("lucidos-claude-code-app-habit-tracker-add-streaks-{thread_id}"),
            repo_id: None,
            coding_agent_kind: crate::engine::thread_events::CodingAgentKind::App,
            coding_agent_folder: "/home/u/workspaces/dev/data/apps/habit-tracker".into(),
            app_id: Some("habit-tracker".into()),
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentTextStreamed {
            text: "mid-turn when the engine died".into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            parent_tool_use_id: None,
            progress_note: false,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    assert!(
        !switch_was_user_initiated(&pool, thread_id).await,
        "no teardown boundary means no user switch, so an app thread that \
         crashed the engine gets Continue rather than an auto-resume"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_parked_question_survives_a_restart_on_an_app_coding_agent_thread() {
    use crate::engine::question_park_sql::thread_has_unanswered_question;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "sid-app-park".into(),
            branch: format!("lucidos-claude-code-app-habit-tracker-add-streaks-{thread_id}"),
            repo_id: None,
            coding_agent_kind: crate::engine::thread_events::CodingAgentKind::App,
            coding_agent_folder: "/home/u/workspaces/dev/data/apps/habit-tracker".into(),
            app_id: Some("habit-tracker".into()),
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAsked {
            tool_use_id: "toolu_q1".into(),
            cc_session_id: "sid-app-park".into(),
            question: "Border or background?".into(),
            options: Vec::new(),
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            permission_request: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .expect("emit succeeds")
    .expect("event persisted");

    assert!(
        thread_has_unanswered_question(&pool, thread_id).await,
        "the question is still the newest thing on the thread, so recovery must \
         preserve the app thread rather than abort or idle it"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

mod unengaged_switch_resume {
    use uuid::Uuid;

    use crate::engine::agent_recovery::unengaged_switch_resume_candidates;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{
        AbortCause, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
    };
    use crate::test_support::{
        message_received, setup_test_db, start_cc_session, teardown_test_db,
    };

    fn cc_meta(actor: Option<MessageOrigin>) -> EventMeta {
        EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            actor,
            ..EventMeta::NONE
        }
    }

    async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent, meta: EventMeta) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta,
        })
        .await
        .expect("emit succeeds");
    }

    async fn emit_switch_abort(bus: &EventBus, thread_id: Uuid) {
        emit_teardown_abort(bus, thread_id, device()).await;
    }

    async fn emit_teardown_abort(bus: &EventBus, thread_id: Uuid, actor: Option<MessageOrigin>) {
        crate::engine::thread_events::emit_response_aborted(
            bus,
            thread_id,
            AbortCause::EngineShutdown,
            String::new(),
            vec![],
            None,
            None,
            cc_meta(actor),
            "[test] teardown abort",
        )
        .await;
    }

    async fn seed_prompt_and_abort(bus: &EventBus, thread_id: Uuid, actor: Option<MessageOrigin>) {
        emit(bus, thread_id, message_received("fix it"), cc_meta(None)).await;
        emit_teardown_abort(bus, thread_id, actor).await;
    }

    fn device() -> Option<MessageOrigin> {
        Some(MessageOrigin::Device {
            device_id: "d1".into(),
        })
    }

    #[tokio::test]
    async fn a_switch_interrupted_thread_with_no_session_is_a_candidate() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_prompt_and_abort(&bus, thread_id, device()).await;

        assert_eq!(
            unengaged_switch_resume_candidates(&pool).await,
            vec![thread_id]
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_crash_interrupted_thread_with_no_session_is_not_a_candidate() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        seed_prompt_and_abort(&bus, Uuid::new_v4(), None).await;

        assert!(unengaged_switch_resume_candidates(&pool).await.is_empty());

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_started_thread_whose_agent_read_nothing_is_a_candidate() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        emit(&bus, thread_id, message_received("fix it"), cc_meta(None)).await;
        start_cc_session(&bus, thread_id, "claude-code/started", None).await;
        emit_switch_abort(&bus, thread_id).await;

        assert_eq!(
            unengaged_switch_resume_candidates(&pool).await,
            vec![thread_id]
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_thread_whose_agent_read_its_prompt_is_not_a_candidate() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        emit(&bus, thread_id, message_received("fix it"), cc_meta(None)).await;
        start_cc_session(&bus, thread_id, "claude-code/read", None).await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::CodingAgentInputRead {
                input_event_id: Uuid::new_v4(),
                started_turn: true,
                verbatim: false,
            },
            cc_meta(None),
        )
        .await;
        emit_switch_abort(&bus, thread_id).await;

        assert!(unengaged_switch_resume_candidates(&pool).await.is_empty());

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_thread_whose_resume_already_began_is_not_a_candidate() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        seed_prompt_and_abort(&bus, thread_id, device()).await;
        emit(
            &bus,
            thread_id,
            ThreadEvent::ContinuationStarted {
                branch: String::new(),
                origin: None,
                reason: Some(crate::engine::agent_recovery::AUTO_RESUME_AFTER_SWITCH_REASON.into()),
                resumed_abort_id: None,
            },
            cc_meta(None),
        )
        .await;

        assert!(unengaged_switch_resume_candidates(&pool).await.is_empty());

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_chat_thread_is_not_a_candidate() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let chat_meta = EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        };
        emit(&bus, thread_id, message_received("hi"), chat_meta.clone()).await;
        crate::engine::thread_events::emit_response_aborted(
            &bus,
            thread_id,
            AbortCause::EngineShutdown,
            String::new(),
            vec![],
            None,
            None,
            EventMeta {
                actor: device(),
                ..chat_meta
            },
            "[test] teardown abort",
        )
        .await;

        assert!(unengaged_switch_resume_candidates(&pool).await.is_empty());

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
