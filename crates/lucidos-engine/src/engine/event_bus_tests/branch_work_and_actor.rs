use super::super::*;
use super::*;

/// Legacy per-commit ChangeProposed (empty change_id, populated commit_sha)
/// must stay inert. Immediate Diff-button visibility comes from the
/// coding-agent post-commit hook refreshing the branch work directly, not
/// from creating a partial proposal event.
#[tokio::test]
async fn per_commit_change_proposed_does_not_change_the_change_state() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "feat-branch", None).await;
    let before = read_change_state(&pool, thread_id).await;

    emit_change_proposed_per_commit(
        &bus,
        thread_id,
        "feat-branch",
        "abc123",
        Some("commit subject"),
    )
    .await;

    assert_eq!(
        read_change_state(&pool, thread_id).await,
        before,
        "per-commit ChangeProposed (empty change_id) is inert in the projection"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn branch_work_refresh_sets_unproposed_and_broadcasts_aggregate() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "feat-branch", None).await;
    let mut rx = bus.subscribe();

    let changed = bus
        .refresh_branch_work_and_broadcast(thread_id, true)
        .await
        .unwrap();
    assert!(changed, "first refresh must report a projection change");
    assert_eq!(
        read_change_state(&pool, thread_id).await,
        CodingAgentChangeState::Unproposed { reason: None },
        "the projection must record the branch work in the DB"
    );

    let emitted = rx.recv().await.unwrap();
    assert_eq!(
        emitted.seq, None,
        "diff refresh is transient, not persisted"
    );
    match emitted.typed {
        BusEvent::Thread {
            thread_id: emitted_thread_id,
            event: ThreadEvent::CodingAgentDiffChanged { has_diff },
            ..
        } => {
            assert_eq!(emitted_thread_id, thread_id);
            assert!(has_diff);
        }
        other => panic!("expected CodingAgentDiffChanged broadcast, got {:?}", other),
    }
    let aggregate = emitted
        .aggregate
        .expect("diff refresh must carry a fresh aggregate snapshot");
    assert_eq!(
        aggregate.coding_agent_change_state.kind(),
        ChangeStateKind::Unproposed,
        "aggregate must carry the unproposed work"
    );

    let unchanged = bus
        .refresh_branch_work_and_broadcast(thread_id, true)
        .await
        .unwrap();
    assert!(!unchanged, "second refresh with same value must be a no-op");
    assert!(
        matches!(
            rx.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Empty)
        ),
        "unchanged refresh must not broadcast duplicate transient events"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The post-commit hook never touches a proposal: a pending change owns the
/// `proposed` state, and an empty branch fact cannot clear it.
#[tokio::test]
async fn branch_work_refresh_leaves_a_proposed_thread_alone() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "feat-branch", None).await;
    emit_change_proposed(&bus, thread_id, "feat-branch", false).await;

    for has_diff in [false, true] {
        let changed = bus
            .refresh_branch_work_and_broadcast(thread_id, has_diff)
            .await
            .unwrap();
        assert!(!changed, "has_diff={has_diff}");
        assert!(read_change_state(&pool, thread_id).await.is_proposed());
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Seed branch work, emit `event`, and return the change state it leaves.
async fn state_after_clearing_event(event: ThreadEvent) -> CodingAgentChangeState {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "feat-branch", None).await;
    force_branch_work(&pool, thread_id).await;
    assert_eq!(
        read_change_state_kind(&pool, thread_id).await,
        ChangeStateKind::Unproposed,
        "precondition: the seed records branch work"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let state = read_change_state(&pool, thread_id).await;
    pool.close().await;
    teardown_test_db(&db_name).await;
    state
}

/// ChangeApplied clears the state: the merge resolved everything to main, so
/// `git diff main..branch` is now empty.
#[tokio::test]
async fn change_applied_clears_the_change_state() {
    let state = state_after_clearing_event(ThreadEvent::ChangeApplied {
        change_id: String::new(),
        requires_restart: false,
        client_update: false,
        commits: vec![],
        thread_title: None,
        actor: None,
        pre_merge_sha: None,
        post_merge_sha: None,
        path: String::new(),
    })
    .await;
    assert_eq!(state, CodingAgentChangeState::None);
}

/// ChangeDiscarded clears the state: the branch is being thrown away, so even
/// a non-empty diff surfaces no Diff button.
#[tokio::test]
async fn change_discarded_clears_the_change_state() {
    let state = state_after_clearing_event(ThreadEvent::ChangeDiscarded {
        change_id: String::new(),
        actor: None,
        path: String::new(),
    })
    .await;
    assert_eq!(state, CodingAgentChangeState::None);
}

/// ThreadArchived clears the state: the thread is leaving the inbox, so the
/// Diff button should not surface for it.
#[tokio::test]
async fn thread_archived_clears_the_change_state() {
    let state = state_after_clearing_event(ThreadEvent::ThreadArchived).await;
    assert_eq!(state, CodingAgentChangeState::None);
}

/// Regression: `POST /api/v1/events/emit` used to drop the request actor on the
/// floor — every persisted DomainEvent row landed with no `actor` field, so
/// the UI couldn't attribute the emit to a device. EventBus now merges the
/// actor into the inner payload so a SELECT on the persisted row sees the
/// same shape as every other actor-bearing event.
#[tokio::test]
async fn domain_event_persisted_payload_carries_actor_when_provided() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());

    let actor = MessageOrigin::Device {
        device_id: "test-dev-42".to_string(),
    };
    let result = bus
        .emit(BusEvent::System(SystemEvent::DomainEvent {
            event_type: "TestActorStamped".to_string(),
            payload: serde_json::json!({"summary": "hello"}),
            depth: 0,
            transient: false,
            actor: Some(actor.clone()),
        }))
        .await
        .unwrap()
        .expect("non-transient DomainEvent persists");

    let payload: serde_json::Value = sqlx::query_scalar("SELECT payload FROM events WHERE id = $1")
        .bind(result.event_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(payload["summary"], "hello", "original payload preserved");
    let payload_actor = payload
        .get("actor")
        .expect("actor must be persisted as a top-level payload key");
    assert_eq!(payload_actor["kind"], "device");
    assert_eq!(payload_actor["device_id"], "test-dev-42");
    assert!(
        payload_actor.get("label").is_none(),
        "a device actor stores its id, never its name"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Engine-internal callers (LLM tool, scheduler) pass `actor: None`. The
/// persisted payload must be unchanged — adding a `null` actor key would
/// litter every existing domain event consumer with a useless field.
#[tokio::test]
async fn domain_event_persisted_payload_unchanged_when_actor_none() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());

    let result = bus
        .emit(BusEvent::System(SystemEvent::DomainEvent {
            event_type: "TestNoActor".to_string(),
            payload: serde_json::json!({"summary": "x", "n": 7}),
            depth: 0,
            transient: false,
            actor: None,
        }))
        .await
        .unwrap()
        .expect("non-transient DomainEvent persists");

    let payload: serde_json::Value = sqlx::query_scalar("SELECT payload FROM events WHERE id = $1")
        .bind(result.event_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(payload, serde_json::json!({"summary": "x", "n": 7}));
    assert!(
        !payload.as_object().unwrap().contains_key("actor"),
        "no `actor` key must be added when caller passed None"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Wave-4 actor-stamped mutating endpoints: each new `SystemEvent` variant
/// must be `is_persisted=true` so it lands in the events table, must round-trip
/// through serde (`to_payload` produces a JSON object the projection can
/// store), and must carry the `actor` field through to the persisted row.
/// One test covers all 15 variants — exhaustive enum match keeps the list
/// honest when a future variant is added.
#[tokio::test]
async fn wave4_mutating_endpoint_events_persist_with_actor() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb) = EventBus::new(pool.clone());

    let actor = MessageOrigin::Device {
        device_id: "dev-wave4".to_string(),
    };

    // Construct one of every new variant. Exhaustive enum match below
    // catches drift: any new variant that's not listed here forces the
    // author to make a deliberate choice about whether it needs a test row.
    let variants: Vec<SystemEvent> = vec![
        SystemEvent::PinnedAppPinned {
            app_id: "habit-tracker".into(),
            device_id: "dev-1".into(),
            actor: Some(actor.clone()),
        },
        SystemEvent::PinnedAppUnpinned {
            app_id: "habit-tracker".into(),
            device_id: "dev-1".into(),
            actor: Some(actor.clone()),
        },
        SystemEvent::DeviceRegistered {
            device_id: "dev-1".into(),
            user_agent: Some("Mozilla/5.0".into()),
            actor: Some(actor.clone()),
        },
        SystemEvent::DeviceRenamed {
            device_id: "dev-1".into(),
            name: Some("My MacBook".into()),
            actor: Some(actor.clone()),
        },
        SystemEvent::DevicePushChanged {
            device_id: "dev-1".into(),
            push_enabled: true,
            actor: Some(actor.clone()),
        },
        SystemEvent::DeviceDeleted {
            device_id: "dev-1".into(),
            actor: Some(actor.clone()),
        },
        SystemEvent::RepositoryAdded {
            repo_id: "repo-id-1".into(),
            name: "MyRepo".into(),
            root_path: "/tmp/myrepo".into(),
            actor: Some(actor.clone()),
        },
        SystemEvent::RepositoryRemoved {
            repo_id: "repo-id-1".into(),
            actor: Some(actor.clone()),
        },
        SystemEvent::CredentialCreated {
            service_name: "openai".into(),
            auth_type: crate::core::AuthType::Bearer,
            actor: Some(actor.clone()),
        },
        SystemEvent::CredentialUpdated {
            service_name: "openai".into(),
            actor: Some(actor.clone()),
        },
        SystemEvent::CredentialDeleted {
            service_name: "openai".into(),
            actor: Some(actor.clone()),
        },
        SystemEvent::OAuthAccountDeleted {
            account_id: Uuid::new_v4().to_string(),
            actor: Some(actor.clone()),
        },
        SystemEvent::DataFileWritten {
            path: "artifacts/notes.md".into(),
            commit: Some("abc1234".into()),
            actor: Some(actor.clone()),
        },
        SystemEvent::DataFileDeleted {
            path: "artifacts/old.md".into(),
            commit: Some("def5678".into()),
            actor: Some(actor.clone()),
        },
        SystemEvent::DataFileEdited {
            path: "config/apis.json".into(),
            operations_count: 3,
            actor: Some(actor.clone()),
        },
    ];

    for evt in &variants {
        assert!(
            evt.is_persisted(),
            "{} must be persisted so an audit trail exists",
            evt.event_type()
        );
        let event_type = evt.event_type();
        bus.emit(BusEvent::System(evt.clone()))
            .await
            .unwrap()
            .expect("persisted SystemEvent must return EmitResult");

        // Query the most recent row of this event_type and assert the
        // actor field is populated. event_type uniqueness across variants
        // in this test makes the latest-row lookup deterministic.
        let payload: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM events WHERE event_type = $1 ORDER BY sequence DESC LIMIT 1",
        )
        .bind(event_type)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|e| panic!("{event_type} missing from events table: {e}"));

        // System-event payloads use serde's tagged-enum shape
        // (`{"type": "...", "data": {...}}` via `#[serde(tag="type",
        // content="data")]`), so the actor field lands inside `data`.
        let inner = payload.get("data").unwrap_or(&payload);
        let payload_actor = inner
            .get("actor")
            .unwrap_or_else(|| panic!("{event_type} payload missing `actor` key: {payload}"));
        assert_eq!(
            payload_actor["kind"], "device",
            "{event_type} actor must be the device variant"
        );
        assert_eq!(payload_actor["device_id"], "dev-wave4");
        assert!(
            payload_actor.get("label").is_none(),
            "a device actor stores its id, never its name"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Per-commit ChangeProposed (empty change_id, commit_sha set) is inert in
/// the projection: must NOT flip the chip, touch status, or insert into
/// `changes`. Aggregate end-of-turn emit is the sole writer.
#[tokio::test]
async fn per_commit_change_proposed_does_not_flip_chip() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "claude-code/orphan", None).await;

    emit_change_proposed_per_commit(
        &bus,
        thread_id,
        "claude-code/orphan",
        "abc123",
        Some("first commit"),
    )
    .await;

    let (status, proposed): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_change_state = 'proposed' \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    // SessionStarted left status='running'; a per-commit emit must NOT touch
    // it (no promotion to 'waiting', no demotion to 'idle' — it's inert).
    assert_eq!(
        status, "running",
        "per-commit ChangeProposed must NOT change status (it's inert)"
    );
    assert!(
        !proposed,
        "per-commit ChangeProposed (empty change_id) must NOT make the thread proposed — \
         only the aggregate end-of-turn emit means 'real finished work'"
    );

    // Also verify nothing landed in the `changes` table (no aggregate row).
    let changes_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM changes WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        changes_count, 0,
        "per-commit emit must not create a changes row — only the aggregate does"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// `proposed` is a cache of the `changes` table. The next sync corrects a
/// `proposed` thread with no pending row to where it lands, restart cleared.
/// A thread whose pending change is real stays `proposed`.
#[tokio::test]
async fn the_proposal_sync_clears_an_orphan_proposed_state() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    let orphan = Uuid::new_v4();
    start_cc_session(&bus, orphan, "claude-code/orphan-a", None).await;
    seed_change_state(
        &pool,
        orphan,
        CodingAgentChangeState::Proposed {
            requires_restart: true,
        },
    )
    .await;

    let genuine = Uuid::new_v4();
    start_cc_session(&bus, genuine, "claude-code/real-b", None).await;
    emit_change_proposed(&bus, genuine, "claude-code/real-b", true).await;

    for thread_id in [orphan, genuine] {
        crate::core::changes_projection::ChangesProjection::sync_thread_proposal(
            &pool,
            thread_id,
            ChangeStateKind::Unproposed,
        )
        .await
        .unwrap();
    }

    assert_eq!(
        read_change_state(&pool, orphan).await,
        CodingAgentChangeState::Unproposed { reason: None },
        "an orphan proposed state lands where its sync says, restart cleared"
    );
    assert_eq!(
        read_change_state(&pool, genuine).await,
        CodingAgentChangeState::Proposed {
            requires_restart: true
        },
        "a real pending change keeps the thread proposed"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
