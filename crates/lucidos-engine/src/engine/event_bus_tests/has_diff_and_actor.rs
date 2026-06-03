use super::super::*;
use super::*;

/// Per-commit ChangeProposed (empty change_id, populated commit_sha) must flip
/// coding_agent_has_diff to TRUE so the Diff button surfaces immediately after a
/// commit lands. This is the live signal — set in the same projection tx as
/// the event insert.
#[tokio::test]
async fn per_commit_change_proposed_does_not_change_has_diff() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "feat-branch", None).await;

    let before: bool = sqlx::query_scalar(
        "SELECT coding_agent_has_diff FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    emit_change_proposed_per_commit(&bus, thread_id, "feat-branch", "abc123", Some("commit subject")).await;

    let after: bool = sqlx::query_scalar(
        "SELECT coding_agent_has_diff FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        before, after,
        "per-commit ChangeProposed (empty change_id) must NOT change coding_agent_has_diff — \
         it's inert in the projection"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// ChangeApplied must clear coding_agent_has_diff — the merge resolved everything
/// to main, so `git diff main..branch` is now empty.
#[tokio::test]
async fn change_applied_clears_coding_agent_has_diff() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "feat-branch", None).await;

    force_has_diff(&pool, thread_id).await;

    let seeded: bool = sqlx::query_scalar(
        "SELECT coding_agent_has_diff FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        seeded,
        "precondition: direct seed must set coding_agent_has_diff=true before ChangeApplied can clear it"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
            change_id: String::new(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let v: bool = sqlx::query_scalar(
        "SELECT coding_agent_has_diff FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!v, "ChangeApplied must clear coding_agent_has_diff");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// ChangeDiscarded must clear coding_agent_has_diff — the branch is being thrown
/// away, so even if the diff is non-empty we don't want to surface a Diff
/// button for it.
#[tokio::test]
async fn change_discarded_clears_coding_agent_has_diff() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "feat-branch", None).await;

    force_has_diff(&pool, thread_id).await;

    let seeded: bool = sqlx::query_scalar(
        "SELECT coding_agent_has_diff FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        seeded,
        "precondition: direct seed must set coding_agent_has_diff=true before ChangeDiscarded can clear it"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeDiscarded {
            change_id: String::new(),
            actor: None,
            path: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let v: bool = sqlx::query_scalar(
        "SELECT coding_agent_has_diff FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!v, "ChangeDiscarded must clear coding_agent_has_diff");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// ThreadArchived must clear coding_agent_has_diff — the thread is leaving the
/// inbox, so the Diff button should not surface for it.
#[tokio::test]
async fn thread_archived_clears_coding_agent_has_diff() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    start_cc_session(&bus, thread_id, "feat-branch", None).await;

    force_has_diff(&pool, thread_id).await;

    let seeded: bool = sqlx::query_scalar(
        "SELECT coding_agent_has_diff FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        seeded,
        "precondition: direct seed must set coding_agent_has_diff=true before ThreadArchived can clear it"
    );

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let v: bool = sqlx::query_scalar(
        "SELECT coding_agent_has_diff FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!v, "ThreadArchived must clear coding_agent_has_diff");

    pool.close().await;
    teardown_test_db(&db_name).await;
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
        label: "My MacBook".to_string(),
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

    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events WHERE id = $1",
    )
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
    assert_eq!(payload_actor["label"], "My MacBook");

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

    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events WHERE id = $1",
    )
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
        label: "Wave 4 Test Device".to_string(),
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
        assert_eq!(payload_actor["label"], "Wave 4 Test Device");
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
        "SELECT status, coding_agent_proposed FROM thread_summaries WHERE thread_id = $1",
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
        "per-commit ChangeProposed (empty change_id) must NOT flip coding_agent_proposed — \
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

/// The `20260517144000_clear_orphan_coding_agent_proposed` migration must
/// clear orphan chips (set on `thread_summaries` but no pending row in
/// `changes`) without disturbing live chips backed by a real pending change.
#[tokio::test]
async fn startup_clears_orphan_proposed_chip_without_pending_change() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _cb_rx) = EventBus::new(pool.clone());

    // Thread A: orphan — coding_agent_proposed=TRUE but no row in `changes`.
    let thread_a = Uuid::new_v4();
    start_cc_session(&bus, thread_a, "claude-code/orphan-a", None).await;
    sqlx::query(
        "UPDATE thread_summaries SET status = 'waiting', \
         coding_agent_proposed = TRUE, coding_agent_requires_restart = TRUE \
         WHERE thread_id = $1",
    )
    .bind(thread_a)
    .execute(&pool)
    .await
    .unwrap();

    // Thread B: genuine pending change — full natural CC lifecycle (start →
    // idle → aggregate ChangeProposed) so the chip flips to 'waiting' and the
    // `changes` row is inserted by the projection. Use a real UUID for
    // change_id so `write_proposed_aggregate`'s `parse_change_id` accepts it
    // and the INSERT lands (the shared `emit_change_proposed` test helper
    // builds a non-UUID change_id, which short-circuits the row insert).
    let thread_b = Uuid::new_v4();
    start_cc_session(&bus, thread_b, "claude-code/real-b", None).await;
    bus.emit(BusEvent::Thread {
        thread_id: thread_b,
        event: ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    bus.emit(BusEvent::Thread {
        thread_id: thread_b,
        event: ThreadEvent::ChangeProposed {
            change_id: Uuid::new_v4().to_string(),
            description: Some("real change".into()),
            files: vec!["src/b.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: "claude-code/real-b".into(),
            repo_root: "/tmp".into(),
            hardened: false,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    // Mirrors the 20260517144000_clear_orphan_coding_agent_proposed migration.
    sqlx::query(
        "UPDATE thread_summaries ts \
         SET coding_agent_proposed = FALSE, \
             coding_agent_requires_restart = FALSE, \
             status = CASE WHEN ts.status = 'waiting' THEN 'idle' ELSE ts.status END \
         WHERE ts.coding_agent_proposed = TRUE \
           AND ts.coding_agent_is_external_repo = FALSE \
           AND ts.coding_agent_applying = FALSE \
           AND NOT EXISTS ( \
             SELECT 1 FROM changes c \
             WHERE c.thread_id = ts.thread_id AND c.status = 'pending' \
           )",
    )
    .execute(&pool)
    .await
    .unwrap();

    let (status_a, proposed_a): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_proposed FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_a)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status_a, "idle", "orphan thread A must drop to idle");
    assert!(!proposed_a, "orphan chip on thread A must clear");

    let (status_b, proposed_b): (String, bool) = sqlx::query_as(
        "SELECT status, coding_agent_proposed FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_b)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status_b, "idle", "genuine pending change settles to 'idle' under Option B");
    assert!(proposed_b, "genuine chip on thread B must stay set");

    pool.close().await;
    teardown_test_db(&db_name).await;
}
