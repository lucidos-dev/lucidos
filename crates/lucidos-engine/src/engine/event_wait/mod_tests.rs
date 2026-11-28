use super::register::ARMING_LOOKBACK_SECS;
use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::MessageOrigin;
use crate::test_support::{refuse_event_type, seed_thread_event, setup_test_db, teardown_test_db};
use serde_json::json;

fn sub(event_type: &str, condition: Option<Value>) -> EventSubscription {
    EventSubscription {
        event_type: event_type.to_string(),
        condition,
    }
}

fn lookback_of(items: &[(&str, Value, i64)]) -> ArmingLookback {
    ArmingLookback {
        matches: items
            .iter()
            .map(|(event_type, payload, ago)| LookbackMatch {
                event_type: (*event_type).to_string(),
                payload: payload.clone(),
                age_secs: *ago,
            })
            .collect(),
        more: false,
    }
}

async fn max_sequence(pool: &sqlx::PgPool) -> i64 {
    sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM events")
        .fetch_one(pool)
        .await
        .unwrap()
}

fn wait_with(thread_id: Uuid, on: Vec<EventSubscription>, watermark: i64) -> LiveWait {
    LiveWait {
        background_timing: None,
        wait_id: Uuid::new_v4(),
        thread_id,
        tool_use_id: "toolu_park".into(),
        on,
        reason: "waiting for a change".into(),
        armed_at: Utc::now(),
        expires_at: Utc::now() + chrono::Duration::hours(1),
        watermark,
    }
}

#[test]
fn matched_index_names_the_entry_that_fired() {
    let w = wait_with(
        Uuid::new_v4(),
        vec![
            sub("ResponseGenerated", None),
            sub("ChangeProposed", Some(json!({"file_count": {"$gt": 0}}))),
        ],
        0,
    );
    assert_eq!(
        w.matched_index("ChangeProposed", &json!({"file_count": 3})),
        Some(1),
    );
    assert_eq!(w.matched_index("ResponseGenerated", &json!({})), Some(0));
    assert_eq!(
        w.matched_index("ChangeProposed", &json!({"file_count": 0})),
        None
    );
    assert_eq!(w.matched_index("ThreadArchived", &json!({})), None);
}

#[test]
fn waits_matching_returns_only_the_hits_in_registration_order() {
    let tid = Uuid::new_v4();
    let a = wait_with(tid, vec![sub("ChangeProposed", None)], 10);
    let b = wait_with(tid, vec![sub("ResponseGenerated", None)], 20);
    let c = wait_with(tid, vec![sub("ChangeProposed", None)], 30);
    let all = vec![a.clone(), b, c.clone()];

    let hits = waits_matching(&all, "ChangeProposed", &json!({}));
    assert_eq!(hits, vec![(a.wait_id, 0), (c.wait_id, 0)]);

    assert!(waits_matching(&all, "ThreadArchived", &json!({})).is_empty());
}

#[tokio::test]
async fn take_resolves_a_wait_exactly_once() {
    let waits = LiveWaits::new();
    let w = wait_with(Uuid::new_v4(), vec![sub("ChangeProposed", None)], 0);
    let id = w.wait_id;
    waits.insert(w).await;

    assert!(waits.take(id).await.is_some(), "first take wins");
    assert!(
        waits.take(id).await.is_none(),
        "a second matching event must find nothing: one tool call has one result"
    );
    assert!(waits.is_empty().await);
}

#[tokio::test]
async fn for_thread_scopes_to_one_thread_and_snapshot_is_watermark_ordered() {
    let waits = LiveWaits::new();
    let mine = Uuid::new_v4();
    let theirs = Uuid::new_v4();
    waits
        .insert(wait_with(mine, vec![sub("A", None)], 30))
        .await;
    waits
        .insert(wait_with(mine, vec![sub("B", None)], 10))
        .await;
    waits
        .insert(wait_with(theirs, vec![sub("C", None)], 20))
        .await;

    assert_eq!(waits.for_thread(mine).await.len(), 2);
    assert_eq!(waits.for_thread(theirs).await.len(), 1);
    assert_eq!(waits.snapshot().await.len(), 3);

    let marks: Vec<i64> = waits.snapshot().await.iter().map(|w| w.watermark).collect();
    assert_eq!(
        marks,
        vec![10, 20, 30],
        "a burst resolves in registration order"
    );
}

#[test]
fn the_firehose_never_reaches_the_wait_matcher() {
    assert!(!is_awaitable_event(&ThreadEvent::TextStreamed {
        text: "tok".into()
    }));
    assert!(!is_awaitable_event(&ThreadEvent::ThoughtStreamed {
        text: "tok".into()
    }));
}

#[test]
fn the_event_wait_family_never_reaches_the_wait_matcher() {
    let started = ThreadEvent::EventWaitStarted {
        background_timing: None,
        wait_id: Uuid::new_v4(),
        tool_use_id: "toolu".into(),
        on: vec![sub("ChangeProposed", None)],
        reason: "r".into(),
        armed_at: Utc::now(),
        expires_at: Utc::now(),
        watermark: 0,
    };
    assert!(!is_awaitable_event(&started));
    assert!(!is_awaitable_event(&ThreadEvent::EventWaitExpired {
        wait_id: Uuid::new_v4(),
    }));
    assert!(is_subscribable(&started));
}

#[test]
fn a_side_question_never_reaches_the_wait_matcher() {
    for event in crate::test_support::every_side_question_event() {
        assert!(!is_awaitable_event(&event), "{}", event.event_type());
    }
}

#[test]
fn ordinary_events_are_awaitable() {
    assert!(is_awaitable_event(&ThreadEvent::ThreadArchived));
    assert!(is_awaitable_event(&ThreadEvent::ToolCalled {
        name: "run_bash".into(),
        args: json!({}),
        description: String::new(),
    }));
}

#[test]
fn expired_waits_uses_an_inclusive_deadline() {
    let now = Utc::now();
    let tid = Uuid::new_v4();
    let mut past = wait_with(tid, vec![sub("A", None)], 0);
    past.expires_at = now - chrono::Duration::seconds(1);
    let mut exactly = wait_with(tid, vec![sub("A", None)], 0);
    exactly.expires_at = now;
    let mut future = wait_with(tid, vec![sub("A", None)], 0);
    future.expires_at = now + chrono::Duration::seconds(1);

    let expired = expired_waits(&[past.clone(), exactly.clone(), future], now);
    assert_eq!(expired, vec![past.wait_id, exactly.wait_id]);
}

#[test]
fn the_delivery_text_carries_the_event_and_the_reason() {
    let text = delivery_reentry_text(
        "ChangeProposed",
        &json!({"file_count": 2}),
        "waiting to apply",
    );
    assert!(text.contains("ChangeProposed"), "{text}");
    assert!(text.contains("file_count"), "{text}");
    assert!(text.contains("waiting to apply"), "{text}");
}

#[test]
fn the_delivery_text_keeps_only_the_tail_of_a_long_output() {
    let mut stdout = "compiling crate\n".repeat(70_000);
    stdout.push_str("test result: FAILED. 3 passed; 1 failed");
    let text = delivery_reentry_text(
        "BackgroundBashCompleted",
        &json!({"task_id": "t1", "exit_code": 101, "stdout": stdout, "stderr": ""}),
        "waiting for the build",
    );
    assert!(
        text.len() < 10_000,
        "a megabyte of stdout must not reach the prompt, got {} bytes",
        text.len()
    );
    assert!(
        text.contains("test result: FAILED"),
        "keeps the tail: {text}"
    );
    assert!(text.contains("earlier bytes cut"), "says it cut: {text}");
    assert!(
        text.contains("\"exit_code\": 101"),
        "keeps short fields: {text}"
    );
}

#[test]
fn the_expiry_text_says_it_timed_out_and_discourages_re_subscribing() {
    let w = wait_with(Uuid::new_v4(), vec![sub("ChangeProposed", None)], 0);
    let text = expiry_reentry_text(&w, &[]);
    assert!(text.contains("Timed out"), "{text}");
    assert!(text.contains("ChangeProposed"), "{text}");
    assert!(
        text.contains("waiting for a change"),
        "carries the reason: {text}"
    );
    assert!(text.contains("Report"), "{text}");
}

#[test]
fn the_registration_result_says_nothing_is_blocking() {
    let engine_side = super::register::registered_tool_result_text(
        &wait_with(Uuid::new_v4(), vec![sub("ChangeProposed", None)], 0),
        None,
        &[],
    );
    assert!(engine_side.contains("ChangeProposed"), "{engine_side}");
    assert!(
        engine_side.contains("Nothing is blocking"),
        "the model must not stall the turn: {engine_side}"
    );
    assert!(
        engine_side.contains("NEW turn"),
        "says where the re-entry lands: {engine_side}"
    );
    assert!(
        engine_side.contains("Do not call await_event again"),
        "the cheap half of the duplicate refusal: {engine_side}"
    );
}

#[test]
fn the_expected_wake_request_names_each_agents_call_and_the_history() {
    use super::keep_alive::WaitAgent;
    use super::register::expected_wake_request_text;
    use super::wait_durations::WaitHistory;
    let history = WaitHistory {
        samples: 4,
        median_secs: 540.0,
        high_secs: 1800.0,
    };
    let lucidos = expected_wake_request_text(WaitAgent::Lucidos, Some(&history));
    assert!(lucidos.contains("call expect_wake"), "{lucidos}");
    assert!(
        lucidos.contains("Half ended within about 9 min"),
        "{lucidos}"
    );
    let cc = expected_wake_request_text(WaitAgent::ClaudeCode, None);
    assert!(cc.contains("`lucidos expect-wake <duration>"), "{cc}");
    assert!(cc.contains("No past waits like this one"), "{cc}");
}

#[test]
fn the_delivery_text_says_the_subscription_is_spent_and_re_arming_is_a_call() {
    let text = delivery_reentry_text(
        "ChangeProposed",
        &json!({"file_count": 3}),
        "waiting to apply",
    );
    assert!(text.contains("now spent"), "{text}");
    assert!(
        text.contains("call await_event again before this turn ends"),
        "the re-subscribe has to name the call AND the deadline: {text}"
    );
    assert!(
        text.contains("Narrating it does not do it"),
        "the observed failure was prose written in place of the call: {text}"
    );
}

#[test]
fn every_subscription_text_says_where_the_subscription_stands() {
    let w = wait_with(Uuid::new_v4(), vec![sub("ChangeProposed", None)], 0);
    let payload = json!({"file_count": 1});

    let shapes: Vec<(&str, String, &str)> = vec![
        (
            "registration",
            super::register::registered_tool_result_text(&w, None, &[]),
            "Nothing is blocking",
        ),
        (
            "registration with an arming lookback",
            super::register::registered_tool_result_text(
                &w,
                Some(&lookback_of(&[("ChangeProposed", payload.clone(), 26)])),
                &[],
            ),
            "will NOT deliver anything below",
        ),
        (
            "delivery",
            delivery_reentry_text("ChangeProposed", &payload, "waiting to apply"),
            "now spent",
        ),
        (
            "expiry",
            expiry_reentry_text(&w, &[]),
            "rather than subscribing again",
        ),
    ];

    for (shape, text, must_say) in shapes {
        assert!(
            text.contains(must_say),
            "the {shape} text must say where the subscription stands \
             (looking for {must_say:?}):\n{text}"
        );
    }
}

async fn seed_completion_card(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    parent_id: Uuid,
    child_thread_id: Uuid,
    child_thread_title: &str,
) -> crate::core::EventRow {
    seed_thread(bus, parent_id).await;
    let card_id = seed_thread_event(
        bus,
        parent_id,
        ThreadEvent::ChildThreadCompleted {
            child_thread_id,
            child_thread_title: Some(child_thread_title.to_string()),
            status: crate::engine::thread_events::ChildCompletionStatus::Success,
            summary: "all green".into(),
            pending_change_ids: vec![],
            sub_thread_pending_changes: vec![],
            integrated_branch: None,
        },
    )
    .await;
    crate::core::store::EventStore::new(pool.clone())
        .get_event_by_id(card_id)
        .await
        .unwrap()
        .expect("the card row")
}

#[tokio::test]
async fn a_child_completion_card_matches_a_wait_watching_for_one() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let row = seed_completion_card(&bus, &pool, parent_id, Uuid::new_v4(), "Nightly E2E").await;

    let bare = wait_with(parent_id, vec![sub("ChildThreadCompleted", None)], 0);
    assert_eq!(
        waits_matching(&[bare], &row.event_type, &row.payload).len(),
        1,
        "the persisted payload must match a bare subscription: {:?}",
        row.payload
    );

    let filtered = wait_with(
        parent_id,
        vec![sub(
            "ChildThreadCompleted",
            Some(json!({"status": "success"})),
        )],
        0,
    );
    assert_eq!(
        waits_matching(&[filtered], &row.event_type, &row.payload).len(),
        1,
        "a condition on the card's own field must match the persisted column too: {:?}",
        row.payload
    );

    let other = wait_with(parent_id, vec![sub("ChangeProposed", None)], 0);
    assert!(
        waits_matching(&[other], &row.event_type, &row.payload).is_empty(),
        "a wait watching something else must NOT suppress the fan-in"
    );

    let scoped = wait_with(
        parent_id,
        vec![sub(
            "ChildThreadCompleted",
            Some(json!({"thread_id": parent_id.to_string()})),
        )],
        0,
    );
    assert!(
        waits_matching(std::slice::from_ref(&scoped), &row.event_type, &row.payload).is_empty(),
        "precondition: the stored column carries no thread_id, which is why the \
         raw question is the wrong one: {:?}",
        row.payload
    );
    assert_eq!(
        waits_matching_row(std::slice::from_ref(&scoped), &row).len(),
        1,
        "the gate must see the thread-scoped wait and stand the fan-in down, or \
         the parent gets two turns for one completion: {:?}",
        row.payload
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_delivery_naming_the_childs_terminal_stands_the_fan_in_down() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    seed_thread(&bus, parent_id).await;

    let child_terminal_id = Uuid::new_v4();
    let completion_card_id = Uuid::new_v4();
    seed_thread_event(
        &bus,
        parent_id,
        ThreadEvent::EventWaitDelivered {
            wait_id: Uuid::new_v4(),
            event_id: child_terminal_id,
            event_type: "CodingAgentIdled".into(),
            payload: json!({"thread_id": child_id.to_string(), "has_changes": true}),
            matched_index: 0,
        },
    )
    .await;

    assert!(
        !wait_delivery_names_any(&pool, parent_id, &[completion_card_id.to_string()]).await,
        "precondition: asking about the card alone is what missed, and still does"
    );
    assert!(
        wait_delivery_names_any(
            &pool,
            parent_id,
            &[
                completion_card_id.to_string(),
                child_terminal_id.to_string()
            ],
        )
        .await,
        "the wait already carried this terminal to the parent, so the fan-in stands down"
    );

    let second_completion = [Uuid::new_v4().to_string(), Uuid::new_v4().to_string()];
    assert!(
        !wait_delivery_names_any(&pool, parent_id, &second_completion).await,
        "a later completion of the same child has its own terminal, and must still wake"
    );

    let other_parent = Uuid::new_v4();
    assert!(
        !wait_delivery_names_any(&pool, other_parent, &[child_terminal_id.to_string()]).await,
        "a delivery on another thread says nothing about this parent"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_wait_matches_a_child_completion_belonging_to_another_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let other_child_id = Uuid::new_v4();
    let row = seed_completion_card(
        &bus,
        &pool,
        Uuid::new_v4(),
        other_child_id,
        "Someone else's coding agent",
    )
    .await;

    let watcher_id = Uuid::new_v4();
    let watching = wait_with(
        watcher_id,
        vec![sub(
            "ChildThreadCompleted",
            Some(json!({"child_thread_id": other_child_id.to_string()})),
        )],
        0,
    );
    let watching_id = watching.wait_id;
    let wrong_child = wait_with(
        watcher_id,
        vec![sub(
            "ChildThreadCompleted",
            Some(json!({"child_thread_id": Uuid::new_v4().to_string()})),
        )],
        0,
    );

    let live = LiveWaits::new();
    live.insert(watching).await;
    live.insert(wrong_child).await;
    let hits = waits_matching(&live.snapshot().await, &row.event_type, &row.payload);

    assert_eq!(
        hits,
        vec![(watching_id, 0)],
        "a cross-thread ChildThreadCompleted must resolve the wait whose \
         child_thread_id it carries, and only that one: {:?}",
        row.payload
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn an_idle() -> ThreadEvent {
    ThreadEvent::CodingAgentIdled {
        has_changes: true,
        is_external_repo: false,
        requires_restart: false,
        cc_session_id: None,
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        reason: None,
        worktree_path: None,
        worktree_head_sha: None,
        bg_bash_pending: false,
        turn_end: None,
    }
}

async fn seed_coding_agent_idle(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> crate::core::EventRow {
    seed_thread_event(
        bus,
        thread_id,
        ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: format!("session-{}", thread_id.simple()),
            branch: format!("claude-code/{}", thread_id.simple()),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
    )
    .await;
    let idle_id = seed_thread_event(bus, thread_id, an_idle()).await;
    crate::core::store::EventStore::new(pool.clone())
        .get_event_by_id(idle_id)
        .await
        .unwrap()
        .expect("the idle row")
}

#[tokio::test]
async fn a_wait_scopes_to_one_coding_agent_session_on_every_path() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let watched = Uuid::new_v4();
    let other = Uuid::new_v4();
    let before = max_sequence(&pool).await;

    let other_row = seed_coding_agent_idle(&bus, &pool, other).await;
    let watched_row = seed_coding_agent_idle(&bus, &pool, watched).await;

    let scoped = |thread: Uuid| {
        sub(
            "CodingAgentIdled",
            Some(json!({ "thread_id": thread.to_string() })),
        )
    };

    let live_watched = crate::core::event_subscription::matchable_thread_payload(
        &an_idle(),
        &EventMeta::NONE,
        watched,
    );
    let waiter = Uuid::new_v4();
    let wait = wait_with(waiter, vec![scoped(watched)], before);
    let wait_id = wait.wait_id;
    assert_eq!(
        waits_matching(&[wait], "CodingAgentIdled", &live_watched),
        vec![(wait_id, 0)],
        "a live idle must resolve the wait scoped to its thread: {live_watched:?}"
    );
    let wrong = wait_with(waiter, vec![scoped(other)], before);
    assert!(
        waits_matching(&[wrong], "CodingAgentIdled", &live_watched).is_empty(),
        "and must not resolve one scoped to a different session"
    );

    let scanning = wait_with(waiter, vec![scoped(watched)], before);
    let hit = catch_up_from_watermark(&pool, &scanning)
        .await
        .unwrap()
        .expect("the scan finds the watched thread's idle");
    assert_eq!(
        hit.0, watched_row.id,
        "the scan must return the watched thread's idle, not {}'s",
        other_row.id
    );
    assert_eq!(hit.3, 0, "matched by the only entry in the list");
    assert_eq!(
        hit.2["thread_id"],
        json!(watched.to_string()),
        "and the payload it hands the re-entry carries the thread it matched on"
    );

    let after = max_sequence(&pool).await;
    let report = arming_lookback_matches(
        &pool,
        &[scoped(watched)],
        after,
        ARMING_LOOKBACK_SECS,
        &std::collections::HashSet::new(),
        3,
    )
    .await
    .unwrap();
    assert_eq!(
        report.matches.len(),
        1,
        "exactly the watched thread's idle is reported: {:?}",
        report.matches
    );
    assert_eq!(
        report.matches[0].payload["thread_id"],
        json!(watched.to_string()),
        "the reported payload names the thread, which is what lets the model \
         scope the wait it arms next"
    );

    assert!(
        watched_row.payload.get("thread_id").is_none(),
        "the stored payload must stay lean: {:?}",
        watched_row.payload
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn emit_subscribe(bus: &EventBus, thread_id: Uuid, on: Vec<EventSubscription>) -> Uuid {
    let wait_id = Uuid::new_v4();
    seed_thread_event(
        bus,
        thread_id,
        ThreadEvent::EventWaitStarted {
            background_timing: None,
            wait_id,
            tool_use_id: format!("toolu_{}", wait_id.simple()),
            on,
            reason: "waiting for a change".into(),
            armed_at: Utc::now(),
            expires_at: Utc::now() + chrono::Duration::hours(1),
            watermark: 0,
        },
    )
    .await;
    wait_id
}

#[tokio::test]
async fn rebuild_recovers_a_live_wait_and_skips_resolved_ones() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let live_thread = Uuid::new_v4();
    let live_id = emit_subscribe(&bus, live_thread, vec![sub("ChangeProposed", None)]).await;

    let done_thread = Uuid::new_v4();
    let done_id = emit_subscribe(&bus, done_thread, vec![sub("ChangeProposed", None)]).await;
    seed_thread_event(
        &bus,
        done_thread,
        ThreadEvent::EventWaitCanceled {
            wait_id: done_id,
            cause: crate::engine::thread_events::EventWaitCancelCause::UserStop,
            on: vec![],
            reason: String::new(),
        },
    )
    .await;

    let waits = LiveWaits::new();
    let loaded = rebuild_live_waits(&pool, &waits).await.unwrap();

    assert_eq!(loaded, 1, "only the unresolved wait is re-armed");
    let recovered = waits.take(live_id).await.expect("the live wait came back");
    assert_eq!(recovered.thread_id, live_thread);
    assert_eq!(recovered.on.len(), 1);
    assert_eq!(recovered.on[0].event_type, "ChangeProposed");
    assert_eq!(recovered.reason, "waiting for a change");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn rebuild_keeps_armed_at_and_falls_back_to_the_row_for_a_legacy_payload() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let armed_at = Utc::now() - chrono::Duration::minutes(7);
    let recorded_thread = Uuid::new_v4();
    let recorded_id = Uuid::new_v4();
    seed_thread_event(
        &bus,
        recorded_thread,
        ThreadEvent::EventWaitStarted {
            background_timing: None,
            wait_id: recorded_id,
            tool_use_id: "toolu_armed".into(),
            on: vec![sub("ChangeProposed", None)],
            reason: "waiting for a change".into(),
            armed_at,
            expires_at: Utc::now() + chrono::Duration::hours(1),
            watermark: 0,
        },
    )
    .await;

    let legacy_thread = Uuid::new_v4();
    let legacy_id = emit_subscribe(&bus, legacy_thread, vec![sub("ChangeProposed", None)]).await;
    sqlx::query(
        "UPDATE events SET payload = payload - 'armed_at' \
         WHERE aggregate = 'thread' AND aggregate_id = $1 AND event_type = 'EventWaitStarted'",
    )
    .bind(legacy_thread.to_string())
    .execute(&pool)
    .await
    .unwrap();
    let legacy_created: DateTime<Utc> = sqlx::query_scalar(
        "SELECT created FROM events WHERE aggregate = 'thread' AND aggregate_id = $1 \
         AND event_type = 'EventWaitStarted'",
    )
    .bind(legacy_thread.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();

    let waits = LiveWaits::new();
    rebuild_live_waits(&pool, &waits).await.unwrap();

    let recorded = waits.take(recorded_id).await.expect("recorded wait");
    assert_eq!(
        recorded.armed_at.timestamp_millis(),
        armed_at.timestamp_millis(),
        "a recorded arming time comes back as itself"
    );
    let legacy = waits.take(legacy_id).await.expect("legacy wait");
    assert_eq!(
        legacy.armed_at.timestamp_millis(),
        legacy_created.timestamp_millis(),
        "with no field, the row's own `created` is the arming time"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn rebuild_re_arms_a_wait_that_expired_while_the_engine_was_down() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wait_id = Uuid::new_v4();
    seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::EventWaitStarted {
            background_timing: None,
            wait_id,
            tool_use_id: "toolu_stale".into(),
            on: vec![sub("ChangeProposed", None)],
            reason: "waiting across a restart".into(),
            armed_at: Utc::now(),
            expires_at: Utc::now() - chrono::Duration::hours(2),
            watermark: 0,
        },
    )
    .await;

    let waits = LiveWaits::new();
    assert_eq!(rebuild_live_waits(&pool, &waits).await.unwrap(), 1);
    let snapshot = waits.snapshot().await;
    assert_eq!(
        expired_waits(&snapshot, Utc::now()),
        vec![wait_id],
        "the sweep must see it as due, not miss it"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn rebuild_reads_the_latest_timing_without_rearming_the_wait() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let wait_id = Uuid::new_v4();
    let armed_at = Utc::now();
    let expires_at = armed_at + chrono::Duration::minutes(20);
    seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::EventWaitStarted {
            background_timing: Some(BackgroundWaitTiming::Queued),
            wait_id,
            tool_use_id: "engine:bg-task-wait:fixture".into(),
            on: vec![sub("BackgroundBashCompleted", None)],
            reason: "a build slot".into(),
            armed_at,
            expires_at: armed_at + chrono::Duration::hours(24),
            watermark: 17,
        },
    )
    .await;
    seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::EventWaitTimingUpdated {
            wait_id,
            reason: "the build to finish".into(),
            expires_at,
            background_timing: BackgroundWaitTiming::Running {
                deadline: expires_at,
            },
        },
    )
    .await;

    let waits = LiveWaits::new();
    assert_eq!(rebuild_live_waits(&pool, &waits).await.unwrap(), 1);
    let wait = waits.take(wait_id).await.unwrap();
    assert_eq!(wait.watermark, 17);
    assert_eq!(
        wait.expires_at.timestamp_millis(),
        expires_at.timestamp_millis()
    );
    assert_eq!(wait.reason, "the build to finish");
    assert!(matches!(
        wait.background_timing,
        Some(BackgroundWaitTiming::Running { .. })
    ));
    assert_eq!(wait.on.len(), 1);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn catch_up_finds_matches_after_the_watermark_and_ignores_earlier_ones() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let other_thread = Uuid::new_v4();
    seed_thread_event(&bus, other_thread, ThreadEvent::ThreadArchived).await;

    let watermark: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();

    for _ in 0..2 {
        seed_thread_event(&bus, other_thread, ThreadEvent::ThreadArchived).await;
    }
    seed_thread_event(&bus, other_thread, ThreadEvent::ThreadSaved).await;

    let wait = wait_with(Uuid::new_v4(), vec![sub("ThreadArchived", None)], watermark);
    let (_, event_type, _, idx) = catch_up_from_watermark(&pool, &wait)
        .await
        .unwrap()
        .expect("a match after the watermark");
    assert_eq!(event_type, "ThreadArchived");
    assert_eq!(idx, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn catch_up_applies_the_condition_not_just_the_event_name() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let watermark: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();

    for name in ["run_bash", "run_python"] {
        seed_thread_event(
            &bus,
            thread_id,
            ThreadEvent::ToolCalled {
                name: name.into(),
                args: json!({}),
                description: String::new(),
            },
        )
        .await;
    }

    let wait = wait_with(
        thread_id,
        vec![sub("ToolCalled", Some(json!({"name": "run_python"})))],
        watermark,
    );
    let hit = catch_up_from_watermark(&pool, &wait)
        .await
        .unwrap()
        .expect("the conditioned entry matches exactly one row");
    assert_eq!(hit.2["name"], "run_python");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn catch_up_on_an_empty_subscription_list_queries_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let wait = wait_with(Uuid::new_v4(), vec![], 0);
    assert!(catch_up_from_watermark(&pool, &wait)
        .await
        .unwrap()
        .is_none());
    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn a_backup_completion() -> crate::engine::event_bus::SystemEvent {
    let started = Utc::now();
    crate::engine::event_bus::SystemEvent::BackupCompleted {
        filename: "lucidos-backup-2026.tar.zst".into(),
        size_bytes: 4096,
        started_at: started,
        finished_at: started,
    }
}

#[tokio::test]
async fn a_persisted_system_event_resolves_a_wait_live_and_on_replay() {
    use crate::core::event_subscription::matchable_system_payload;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let watermark = max_sequence(&pool).await;
    let event = a_backup_completion();
    let emitted = bus
        .emit(BusEvent::System(event.clone()))
        .await
        .unwrap()
        .expect("a persisted frame writes a row");

    let live = matchable_system_payload(&event);
    let wait = wait_with(
        Uuid::new_v4(),
        vec![sub(
            "BackupCompleted",
            Some(json!({"filename": "lucidos-backup-2026.tar.zst"})),
        )],
        watermark,
    );
    assert_eq!(
        waits_matching(
            std::slice::from_ref(&wait),
            event.stored_event_type(),
            &live
        )
        .len(),
        1,
        "the live frame must resolve the wait: {live:?}",
    );

    let row = crate::core::store::EventStore::new(pool.clone())
        .get_event_by_id(emitted.event_id)
        .await
        .unwrap()
        .expect("the backup row");
    assert_eq!(row.event_type, "BackupCompleted");
    assert!(
        row.thread_id.is_none(),
        "a system frame belongs to no thread: {:?}",
        row.thread_id
    );
    assert_eq!(
        matchable_payload(&row.event_type, row.payload.clone(), row.thread_id),
        live,
        "the row's view and the live view must be the same bytes: {:?}",
        row.payload
    );
    assert_eq!(
        waits_matching_row(std::slice::from_ref(&wait), &row).len(),
        1
    );

    let (id, event_type, payload, idx) = catch_up_from_watermark(&pool, &wait)
        .await
        .unwrap()
        .expect("the row is after the watermark");
    assert_eq!(id, emitted.event_id);
    assert_eq!(event_type, "BackupCompleted");
    assert_eq!(idx, 0);
    assert_eq!(payload, live);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_transient_system_frame_writes_no_row_and_resolves_nothing() {
    use crate::core::event_subscription::is_subscribable_system_event;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let watermark = max_sequence(&pool).await;
    let progress = crate::engine::event_bus::SystemEvent::BackupProgress {
        phase: "dumping".into(),
        progress: 2,
        total: 5,
    };
    assert!(
        !is_subscribable_system_event(&progress),
        "the live gate must drop it before any wait sees it"
    );
    assert!(bus
        .emit(BusEvent::System(progress))
        .await
        .unwrap()
        .is_none());

    let wait = wait_with(Uuid::new_v4(), vec![sub("BackupProgress", None)], watermark);
    assert!(
        catch_up_from_watermark(&pool, &wait)
            .await
            .unwrap()
            .is_none(),
        "nothing was written, so the scan has nothing to find"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn rebuild_survives_a_system_event_row_in_the_store() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("BackupCompleted", None)]).await;
    bus.emit(BusEvent::System(a_backup_completion()))
        .await
        .unwrap()
        .expect("a persisted frame writes a row");
    let aggregate_id: String =
        sqlx::query_scalar("SELECT aggregate_id FROM events WHERE event_type = 'BackupCompleted'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        aggregate_id, "global",
        "precondition: the row's aggregate_id is not a uuid"
    );

    let waits = LiveWaits::new();
    assert_eq!(rebuild_live_waits(&pool, &waits).await.unwrap(), 1);
    let recovered = waits.take(wait_id).await.expect("the wait came back");
    assert_eq!(recovered.on[0].event_type, "BackupCompleted");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn the_arming_lookback_leads_the_result_and_names_the_trap() {
    let w = wait_with(Uuid::new_v4(), vec![sub("ChangeProposed", None)], 0);
    let text = super::register::registered_tool_result_text(
        &w,
        Some(&lookback_of(&[(
            "ChangeProposed",
            json!({"file_count": 11}),
            26,
        )])),
        &[],
    );

    let report_at = text
        .find("ALREADY HAPPENED")
        .expect("the report is present");
    let subscribed_at = text.find("Subscribed to").expect("still confirms the wait");
    assert!(
        report_at < subscribed_at,
        "the actionable half leads:\n{text}"
    );
    assert!(
        text.contains("will NOT deliver anything below"),
        "a forward-only watch is the trap, and it has to be stated: {text}"
    );
    assert!(
        text.contains("in this turn"),
        "acting on it later is not an option the model has: {text}"
    );
    assert!(
        text.contains("26s ago"),
        "the age is the whole basis: {text}"
    );
    assert!(
        text.contains("\"file_count\": 11"),
        "carries the payload: {text}"
    );
}

#[test]
fn an_empty_lookback_leaves_the_registration_result_untouched() {
    let w = wait_with(Uuid::new_v4(), vec![sub("ChangeProposed", None)], 0);
    let plain = super::register::registered_tool_result_text(&w, None, &[]);
    let empty =
        super::register::registered_tool_result_text(&w, Some(&ArmingLookback::default()), &[]);
    assert_eq!(plain, empty);
    assert!(!plain.contains("ALREADY HAPPENED"), "{plain}");
}

#[test]
fn a_warning_leads_the_registration_result() {
    let w = wait_with(Uuid::new_v4(), vec![sub("ReleaseFinnished", None)], 0);
    let warning = crate::core::event_subscription::never_emitted_warning("ReleaseFinnished");
    let text = super::register::registered_tool_result_text(
        &w,
        Some(&lookback_of(&[("ReleaseFinnished", json!({}), 5)])),
        &[warning],
    );
    assert!(text.starts_with("WARNING: "), "{text}");
    assert!(text.contains("ReleaseFinnished"), "{text}");
    assert!(text.contains("Subscribed to"), "{text}");
}

#[test]
fn the_report_renders_an_age_the_model_can_act_on() {
    let w = wait_with(Uuid::new_v4(), vec![sub("ChangeProposed", None)], 0);
    let text = super::register::registered_tool_result_text(
        &w,
        Some(&lookback_of(&[
            ("ChangeProposed", json!({}), 5),
            ("ChangeProposed", json!({}), 134),
        ])),
        &[],
    );
    assert!(text.contains("5s ago"), "{text}");
    assert!(text.contains("2m 14s ago"), "{text}");
}

#[test]
fn a_fat_payload_is_truncated_but_still_identifies_its_event() {
    let w = wait_with(Uuid::new_v4(), vec![sub("ChangeProposed", None)], 0);
    let huge = json!({ "files": vec!["crates/some/long/path.rs"; 500] });
    let text = super::register::registered_tool_result_text(
        &w,
        Some(&lookback_of(&[("ChangeProposed", huge, 12)])),
        &[],
    );

    assert!(
        text.contains("payload truncated"),
        "the cut says it happened: {text}"
    );
    assert!(
        text.len() < 4_000,
        "a registration result must stay small, was {} bytes",
        text.len()
    );
    assert!(text.contains("ChangeProposed"), "identity survives: {text}");
    assert!(text.contains("12s ago"), "age survives: {text}");
    assert!(
        text.contains("Nothing is blocking"),
        "and the cut never eats the rest of the result: {text}"
    );
}

#[test]
fn truncating_a_payload_never_splits_a_character() {
    let w = wait_with(Uuid::new_v4(), vec![sub("ChangeProposed", None)], 0);
    let emoji = json!({ "note": "🎉".repeat(2_000) });
    let text = super::register::registered_tool_result_text(
        &w,
        Some(&lookback_of(&[("ChangeProposed", emoji, 1)])),
        &[],
    );
    assert!(text.contains("payload truncated"), "{text}");
    assert!(text.is_char_boundary(text.len()));
}

#[test]
fn a_truncated_report_says_so_and_says_what_to_do() {
    let w = wait_with(Uuid::new_v4(), vec![sub("ToolCalled", None)], 0);
    let mut found = lookback_of(&[("ToolCalled", json!({"name": "run_bash"}), 3)]);
    found.more = true;
    let text = super::register::registered_tool_result_text(&w, Some(&found), &[]);
    assert!(text.contains("More matched than are shown"), "{text}");
    assert!(text.contains("condition"), "{text}");
    assert!(text.contains("trigger"), "{text}");
}

#[tokio::test]
async fn the_lookback_finds_a_match_that_landed_before_the_wait_was_armed() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    seed_thread_event(&bus, Uuid::new_v4(), ThreadEvent::ThreadArchived).await;
    let watermark = max_sequence(&pool).await;

    let found = arming_lookback_matches(
        &pool,
        &[sub("ThreadArchived", None)],
        watermark,
        ARMING_LOOKBACK_SECS,
        &std::collections::HashSet::new(),
        3,
    )
    .await
    .unwrap();

    assert_eq!(found.matches.len(), 1, "the match is below the watermark");
    assert_eq!(found.matches[0].event_type, "ThreadArchived");
    assert!(!found.more);

    let wait = wait_with(Uuid::new_v4(), vec![sub("ThreadArchived", None)], watermark);
    assert!(
        catch_up_from_watermark(&pool, &wait)
            .await
            .unwrap()
            .is_none(),
        "the lookback is additive: it must not change what the forward scan sees"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_lookback_ignores_a_match_older_than_the_window() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    seed_thread_event(&bus, Uuid::new_v4(), ThreadEvent::ThreadArchived).await;
    let watermark = max_sequence(&pool).await;
    sqlx::query("UPDATE events SET created = now() - interval '1 hour'")
        .execute(&pool)
        .await
        .unwrap();

    let found = arming_lookback_matches(
        &pool,
        &[sub("ThreadArchived", None)],
        watermark,
        ARMING_LOOKBACK_SECS,
        &std::collections::HashSet::new(),
        3,
    )
    .await
    .unwrap();
    assert!(found.is_empty(), "older than the window: {found:?}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_lookback_window_is_measured_by_the_database_clock() {
    const MARGIN_SECS: i64 = 60;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let other = Uuid::new_v4();

    let mut ids = Vec::new();
    for name in ["inside", "outside"] {
        ids.push(
            seed_thread_event(
                &bus,
                other,
                ThreadEvent::ToolCalled {
                    name: name.into(),
                    args: json!({}),
                    description: String::new(),
                },
            )
            .await,
        );
    }
    let watermark = max_sequence(&pool).await;

    for (id, age) in [
        (ids[0], ARMING_LOOKBACK_SECS - MARGIN_SECS),
        (ids[1], ARMING_LOOKBACK_SECS + MARGIN_SECS),
    ] {
        sqlx::query("UPDATE events SET created = now() - make_interval(secs => $2) WHERE id = $1")
            .bind(id)
            .bind(age)
            .execute(&pool)
            .await
            .unwrap();
    }

    let found = arming_lookback_matches(
        &pool,
        &[sub("ToolCalled", None)],
        watermark,
        ARMING_LOOKBACK_SECS,
        &std::collections::HashSet::new(),
        3,
    )
    .await
    .unwrap();

    let names: Vec<&str> = found
        .matches
        .iter()
        .map(|m| m.payload["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["inside"],
        "one row each side of the boundary, both aged by the server"
    );
    let age = found.matches[0].age_secs;
    assert!(
        (ARMING_LOOKBACK_SECS - MARGIN_SECS..ARMING_LOOKBACK_SECS).contains(&age),
        "the reported age is the database's own subtraction, was {age}s"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_lookback_skips_an_event_the_thread_was_already_handed() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let other = Uuid::new_v4();

    let mut ids = Vec::new();
    for _ in 0..2 {
        ids.push(seed_thread_event(&bus, other, ThreadEvent::ThreadArchived).await);
    }
    let watermark = max_sequence(&pool).await;

    let handed: std::collections::HashSet<Uuid> = [ids[0]].into_iter().collect();
    let found = arming_lookback_matches(
        &pool,
        &[sub("ThreadArchived", None)],
        watermark,
        ARMING_LOOKBACK_SECS,
        &handed,
        3,
    )
    .await
    .unwrap();
    assert_eq!(found.matches.len(), 1, "the handed one is dropped");

    let nothing_handed = arming_lookback_matches(
        &pool,
        &[sub("ThreadArchived", None)],
        watermark,
        ARMING_LOOKBACK_SECS,
        &std::collections::HashSet::new(),
        3,
    )
    .await
    .unwrap();
    assert_eq!(
        nothing_handed.matches.len(),
        2,
        "decoy: both are inside the window, so the exclusion is what dropped one"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_lookback_reports_the_newest_matches_up_to_the_limit() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let other = Uuid::new_v4();

    for name in ["a", "b", "c", "d", "e"] {
        seed_thread_event(
            &bus,
            other,
            ThreadEvent::ToolCalled {
                name: name.into(),
                args: json!({}),
                description: String::new(),
            },
        )
        .await;
    }
    let watermark = max_sequence(&pool).await;

    let found = arming_lookback_matches(
        &pool,
        &[sub("ToolCalled", None)],
        watermark,
        ARMING_LOOKBACK_SECS,
        &std::collections::HashSet::new(),
        3,
    )
    .await
    .unwrap();

    assert_eq!(found.matches.len(), 3);
    assert!(found.more, "two more exist and the report says so");
    let names: Vec<&str> = found
        .matches
        .iter()
        .map(|m| m.payload["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["e", "d", "c"], "newest first");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_lookback_applies_the_condition_not_just_the_event_name() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let other = Uuid::new_v4();

    for name in ["run_bash", "run_python"] {
        seed_thread_event(
            &bus,
            other,
            ThreadEvent::ToolCalled {
                name: name.into(),
                args: json!({}),
                description: String::new(),
            },
        )
        .await;
    }
    let watermark = max_sequence(&pool).await;

    let found = arming_lookback_matches(
        &pool,
        &[sub("ToolCalled", Some(json!({"name": "run_bash"})))],
        watermark,
        ARMING_LOOKBACK_SECS,
        &std::collections::HashSet::new(),
        3,
    )
    .await
    .unwrap();
    assert_eq!(found.matches.len(), 1);
    assert_eq!(found.matches[0].payload["name"], "run_bash");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_lookback_stops_at_the_watermark() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let other = Uuid::new_v4();

    let watermark = max_sequence(&pool).await;
    seed_thread_event(&bus, other, ThreadEvent::ThreadArchived).await;

    let found = arming_lookback_matches(
        &pool,
        &[sub("ThreadArchived", None)],
        watermark,
        ARMING_LOOKBACK_SECS,
        &std::collections::HashSet::new(),
        3,
    )
    .await
    .unwrap();
    assert!(
        found.is_empty(),
        "after the watermark is the delivery's job"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_lookback_gives_up_rather_than_paging_a_busy_window() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let other = Uuid::new_v4();

    let seeded = ARMING_LOOKBACK_MAX_PAGES * (CATCH_UP_PAGE as usize) + 50;
    for i in 0..seeded {
        seed_thread_event(
            &bus,
            other,
            ThreadEvent::ToolCalled {
                name: format!("noise_{i}"),
                args: json!({}),
                description: String::new(),
            },
        )
        .await;
    }
    let watermark = max_sequence(&pool).await;

    let found = arming_lookback_matches(
        &pool,
        &[sub(
            "ToolCalled",
            Some(json!({"name": "nothing_matches_this"})),
        )],
        watermark,
        ARMING_LOOKBACK_SECS,
        &std::collections::HashSet::new(),
        3,
    )
    .await
    .unwrap();
    assert!(found.is_empty(), "the condition matches nothing: {found:?}");

    let unreachable = seeded - ARMING_LOOKBACK_MAX_PAGES * (CATCH_UP_PAGE as usize);
    assert!(
        unreachable > 0,
        "the fixture must exceed the budget or this asserts nothing"
    );

    seed_thread_event(
        &bus,
        other,
        ThreadEvent::ToolCalled {
            name: "findable".into(),
            args: json!({}),
            description: String::new(),
        },
    )
    .await;
    let near = arming_lookback_matches(
        &pool,
        &[sub("ToolCalled", Some(json!({"name": "findable"})))],
        max_sequence(&pool).await,
        ARMING_LOOKBACK_SECS,
        &std::collections::HashSet::new(),
        3,
    )
    .await
    .unwrap();
    assert_eq!(near.matches.len(), 1, "the newest match is still reported");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_lookback_on_an_empty_subscription_list_queries_nothing() {
    let (pool, db_name) = setup_test_db().await;
    let found = arming_lookback_matches(
        &pool,
        &[],
        i64::MAX,
        ARMING_LOOKBACK_SECS,
        &std::collections::HashSet::new(),
        3,
    )
    .await
    .unwrap();
    assert!(found.is_empty());
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn delivery_emits_the_resolution_before_its_anchor() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("ThreadArchived", None)]).await;
    let waits = LiveWaits::new();
    rebuild_live_waits(&pool, &waits).await.unwrap();
    let wait = waits.take(wait_id).await.unwrap();

    emit_delivery(&bus, &wait, Uuid::new_v4(), "ThreadArchived", &json!({}), 0)
        .await
        .unwrap();

    let order: Vec<String> = sqlx::query_scalar(
        "SELECT event_type FROM events WHERE aggregate_id = $1 \
           AND event_type IN ('EventWaitDelivered','PromptInjected') ORDER BY sequence",
    )
    .bind(thread_id.to_string())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(order, vec!["EventWaitDelivered", "PromptInjected"]);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_delivery_writes_no_tool_result() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("ThreadArchived", None)]).await;
    let waits = LiveWaits::new();
    rebuild_live_waits(&pool, &waits).await.unwrap();
    let wait = waits.take(wait_id).await.unwrap();

    emit_delivery(&bus, &wait, Uuid::new_v4(), "ThreadArchived", &json!({}), 0)
        .await
        .unwrap();

    let results: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ToolResult'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(results, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn expiry_emits_the_resolution_and_then_its_anchor() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("ThreadArchived", None)]).await;
    let waits = LiveWaits::new();
    rebuild_live_waits(&pool, &waits).await.unwrap();
    let wait = waits.take(wait_id).await.unwrap();

    emit_expiry(&bus, &wait, &[]).await.unwrap();

    let order: Vec<String> = sqlx::query_scalar(
        "SELECT event_type FROM events WHERE aggregate_id = $1 \
           AND event_type IN ('EventWaitExpired','PromptInjected') ORDER BY sequence",
    )
    .bind(thread_id.to_string())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(order, vec!["EventWaitExpired", "PromptInjected"]);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_delivered_wait_does_not_come_back_on_the_next_boot() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("ThreadArchived", None)]).await;
    let waits = LiveWaits::new();
    rebuild_live_waits(&pool, &waits).await.unwrap();
    let wait = waits.take(wait_id).await.unwrap();
    emit_delivery(&bus, &wait, Uuid::new_v4(), "ThreadArchived", &json!({}), 0)
        .await
        .unwrap();

    let after_restart = LiveWaits::new();
    assert_eq!(rebuild_live_waits(&pool, &after_restart).await.unwrap(), 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_wait_the_teardown_declined_is_delivered_by_the_next_engine() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("ThreadArchived", None)]).await;

    bus.emit(BusEvent::Thread {
        thread_id: Uuid::new_v4(),
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let after_restart = LiveWaits::new();
    assert_eq!(
        rebuild_live_waits(&pool, &after_restart).await.unwrap(),
        1,
        "an undelivered wait must come back armed: the declined resolution left \
         no `EventWaitDelivered` to retire it"
    );
    let rebuilt = after_restart
        .take(wait_id)
        .await
        .expect("the wait is live again");

    let (_, event_type, _, idx) = catch_up_from_watermark(&pool, &rebuilt)
        .await
        .unwrap()
        .expect("the match the teardown declined");
    assert_eq!(event_type, "ThreadArchived");
    assert_eq!(idx, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn seed_thread(bus: &EventBus, thread_id: Uuid) {
    seed_thread_event(
        bus,
        thread_id,
        ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "start the work".into(),
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
    )
    .await;
}

#[tokio::test]
async fn no_event_wait_resolution_touches_the_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;

    let status = |pool: sqlx::PgPool| async move {
        sqlx::query_scalar::<_, String>("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    assert_eq!(
        status(pool.clone()).await,
        "running",
        "registering a subscription happens mid-turn and must not disturb it"
    );

    for event in [
        ThreadEvent::EventWaitDelivered {
            wait_id,
            event_id: Uuid::new_v4(),
            event_type: "ChangeProposed".into(),
            payload: json!({}),
            matched_index: 0,
        },
        ThreadEvent::EventWaitExpired { wait_id },
        ThreadEvent::EventWaitCanceled {
            wait_id,
            cause: crate::engine::thread_events::EventWaitCancelCause::UserStop,
            on: vec![],
            reason: String::new(),
        },
    ] {
        let label = event.event_type();
        seed_thread_event(&bus, thread_id, event).await;
        assert_eq!(
            status(pool.clone()).await,
            "running",
            "{label} must leave the running turn alone"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_thread_holding_a_subscription_is_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::ResponseGenerated {
            text: "subscribed, I will report back".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
    )
    .await;

    let status: String =
        sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        status, "idle",
        "a subscription is not a park: the turn ended, so the thread is idle"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_delivery_anchors_on_a_prompt_injected() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    let waits = LiveWaits::new();
    rebuild_live_waits(&pool, &waits).await.unwrap();
    let wait = waits.take(wait_id).await.unwrap();

    let reentry = emit_delivery(
        &bus,
        &wait,
        Uuid::new_v4(),
        "ChangeProposed",
        &json!({"file_count": 3}),
        0,
    )
    .await
    .unwrap();

    let (anchor_type, anchor_text): (String, Option<String>) =
        sqlx::query_as("SELECT event_type, payload->>'text' FROM events WHERE id = $1")
            .bind(reentry.anchor_event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(anchor_type, "PromptInjected");
    assert_eq!(
        anchor_text.as_deref(),
        Some(reentry.text.as_str()),
        "the anchor carries the same words the re-entry prompts with"
    );
    assert!(reentry.text.contains("ChangeProposed"), "{}", reentry.text);

    let tool_results: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ToolResult'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(tool_results, 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_delivery_anchor_points_back_at_its_resolution() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    let waits = LiveWaits::new();
    rebuild_live_waits(&pool, &waits).await.unwrap();
    let wait = waits.take(wait_id).await.unwrap();

    let reentry = emit_delivery(
        &bus,
        &wait,
        Uuid::new_v4(),
        "ChangeProposed",
        &json!({"file_count": 3}),
        0,
    )
    .await
    .unwrap();

    let linked: Option<Uuid> = sqlx::query_scalar(
        "SELECT (payload->>'delivered_event_id')::uuid FROM events WHERE id = $1",
    )
    .bind(reentry.anchor_event_id)
    .fetch_one(&pool)
    .await
    .unwrap();

    let (delivered_id, delivered_type, delivered_payload): (Uuid, String, serde_json::Value) =
        sqlx::query_as(
            "SELECT id, payload->>'event_type', payload->'payload' FROM events \
             WHERE aggregate_id = $1 AND event_type = 'EventWaitDelivered'",
        )
        .bind(thread_id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(linked, Some(delivered_id));
    assert_eq!(delivered_type, "ChangeProposed");
    assert_eq!(delivered_payload, json!({"file_count": 3}));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_expiry_anchor_carries_no_delivery_link() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    let waits = LiveWaits::new();
    rebuild_live_waits(&pool, &waits).await.unwrap();
    let wait = waits.take(wait_id).await.unwrap();

    let reentry = emit_expiry(&bus, &wait, &[]).await.unwrap();

    let linked: Option<Uuid> = sqlx::query_scalar(
        "SELECT (payload->>'delivered_event_id')::uuid FROM events WHERE id = $1",
    )
    .bind(reentry.anchor_event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(linked, None);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_reentry_anchor_names_the_engine_and_the_wait() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let waits = LiveWaits::new();

    let anchor_of = |id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_as::<_, (Value, String, String)>(
                "SELECT payload->'origin', payload->>'mode', payload->>'text' \
                 FROM events WHERE id = $1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };

    let thread_id = Uuid::new_v4();
    let on = vec![
        sub("CodingAgentIdled", Some(json!({"session": "a"}))),
        sub("CodingAgentIdled", Some(json!({"session": "b"}))),
        sub("ChangeProposed", None),
    ];
    let expiring = emit_subscribe(&bus, thread_id, on.clone()).await;
    let delivering = emit_subscribe(&bus, thread_id, on).await;
    rebuild_live_waits(&pool, &waits).await.unwrap();

    let wait = waits.take(expiring).await.unwrap();
    let reentry = emit_expiry(&bus, &wait, &[]).await.unwrap();
    let (origin, mode, text) = anchor_of(reentry.anchor_event_id).await;
    assert_eq!(
        origin,
        json!({"kind": "engine", "reason": {
            "kind": "event_wait",
            "outcome": "expired",
            "watched": ["CodingAgentIdled", "ChangeProposed"],
            "wait_reason": "waiting for a change",
        }})
    );
    assert_eq!(mode, "agent");
    assert_eq!(text, expiry_reentry_text(&wait, &[]));

    let wait = waits.take(delivering).await.unwrap();
    let reentry = emit_delivery(&bus, &wait, Uuid::new_v4(), "ChangeProposed", &json!({}), 2)
        .await
        .unwrap();
    let (origin, _, text) = anchor_of(reentry.anchor_event_id).await;
    assert_eq!(origin["reason"]["outcome"], "delivered");
    assert_eq!(
        text,
        delivery_reentry_text("ChangeProposed", &json!({}), &wait.reason)
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn subscribe_and_resolve_without_waking(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) {
    seed_thread(bus, thread_id).await;
    let wait_id = emit_subscribe(bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    let waits = LiveWaits::new();
    rebuild_live_waits(pool, &waits).await.unwrap();
    let wait = waits.take(wait_id).await.unwrap();
    emit_delivery(
        bus,
        &wait,
        Uuid::new_v4(),
        "ChangeProposed",
        &json!({"file_count": 1}),
        0,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn the_lost_reentry_sweep_finds_a_resolution_whose_turn_never_ran() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    subscribe_and_resolve_without_waking(&bus, &pool, thread_id).await;

    let lost = lost_wait_reentries(&pool).await.unwrap();
    assert_eq!(lost.len(), 1, "{lost:?}");
    assert_eq!(lost[0].thread_id, thread_id);
    assert!(
        lost[0].reentry.text.contains("ChangeProposed"),
        "the re-entry re-uses the persisted prompt: {}",
        lost[0].reentry.text
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_lost_reentry_sweep_skips_a_reentry_that_already_ran() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    subscribe_and_resolve_without_waking(&bus, &pool, thread_id).await;
    assert_eq!(lost_wait_reentries(&pool).await.unwrap().len(), 1);

    seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::ResponseGenerated {
            text: "the change landed".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
    )
    .await;

    assert!(
        lost_wait_reentries(&pool).await.unwrap().is_empty(),
        "a consumed re-entry is not lost"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_lost_reentry_sweep_looks_past_side_questions() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    subscribe_and_resolve_without_waking(&bus, &pool, thread_id).await;
    for event in crate::test_support::every_side_question_event() {
        seed_thread_event(&bus, thread_id, event).await;
    }

    assert_eq!(
        lost_wait_reentries(&pool).await.unwrap().len(),
        1,
        "a side question does not consume the re-entry"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_lost_reentry_sweep_skips_a_discarded_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    subscribe_and_resolve_without_waking(&bus, &pool, thread_id).await;
    sqlx::query("UPDATE thread_summaries SET state = 'discarded' WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .unwrap();

    assert!(
        lost_wait_reentries(&pool).await.unwrap().is_empty(),
        "reviving a thread the user threw away is the archive-curtain problem again"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn an_agent_message() -> ThreadEvent {
    ThreadEvent::MessageReceived {
        provider: None,
        voice_session_id: None,
        wake_mode: None,
        text: "[CHILD THREAD COMPLETED]".into(),
        user_image_hashes: vec![],
        device_id: None,
        image_description: None,
        parent_thread_id: None,
        spawning_event_id: None,
        mode: crate::engine::thread_events::ActorMode::Agent,
        model: None,
        reasoning_effort: None,
        origin: None,
    }
}

#[tokio::test]
async fn recent_subscriptions_counts_only_since_the_last_human_message() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    use super::register::recent_subscriptions;

    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    assert_eq!(recent_subscriptions(&pool, thread_id).await.unwrap(), 0);

    emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    emit_subscribe(&bus, thread_id, vec![sub("ResponseGenerated", None)]).await;
    assert_eq!(recent_subscriptions(&pool, thread_id).await.unwrap(), 2);

    seed_thread_event(&bus, thread_id, an_agent_message()).await;
    assert_eq!(
        recent_subscriptions(&pool, thread_id).await.unwrap(),
        2,
        "an agent message is not a human in the loop"
    );

    seed_thread(&bus, thread_id).await;
    assert_eq!(
        recent_subscriptions(&pool, thread_id).await.unwrap(),
        0,
        "a human message resets the streak"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn recent_subscriptions_forgets_waits_older_than_the_window() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    use super::register::{recent_subscriptions, RECENT_SUBSCRIPTION_WINDOW_SECS};

    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    emit_subscribe(&bus, thread_id, vec![sub("ResponseGenerated", None)]).await;

    sqlx::query(
        "UPDATE events SET created = now() - make_interval(secs => $2 + 60) \
         WHERE id = (SELECT id FROM events \
                     WHERE aggregate = 'thread' AND aggregate_id = $1 \
                       AND event_type = 'EventWaitStarted' \
                     ORDER BY sequence LIMIT 1)",
    )
    .bind(thread_id.to_string())
    .bind(RECENT_SUBSCRIPTION_WINDOW_SECS)
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(
        recent_subscriptions(&pool, thread_id).await.unwrap(),
        1,
        "a wait armed before the window is serial progress, not a loop"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn subscribe_and_deliver(bus: &EventBus, thread_id: Uuid, event_id: Uuid) {
    let wait_id = emit_subscribe(bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    seed_thread_event(
        bus,
        thread_id,
        ThreadEvent::EventWaitDelivered {
            wait_id,
            event_id,
            event_type: "ChangeProposed".into(),
            payload: json!({}),
            matched_index: 0,
        },
    )
    .await;
}

async fn emit_domain_event(bus: &EventBus, actor: Option<MessageOrigin>) -> Uuid {
    bus.emit(BusEvent::System(
        crate::engine::event_bus::SystemEvent::DomainEvent {
            event_type: "BuildFinished".to_string(),
            payload: json!({"summary": "the build finished"}),
            depth: 0,
            transient: false,
            actor,
        },
    ))
    .await
    .unwrap()
    .expect("a domain event writes a row")
    .event_id
}

#[tokio::test]
async fn recent_subscriptions_skips_waits_another_thread_ended() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    use super::register::recent_subscriptions;

    let waiter = Uuid::new_v4();
    let card = seed_completion_card(&bus, &pool, waiter, Uuid::new_v4(), "A child").await;

    let session = seed_coding_agent_idle(&bus, &pool, Uuid::new_v4()).await;
    subscribe_and_deliver(&bus, waiter, session.id).await;
    subscribe_and_deliver(&bus, waiter, card.id).await;
    let released = emit_domain_event(
        &bus,
        Some(MessageOrigin::Api {
            user_agent: None,
            mode: crate::engine::thread_events::ActorMode::Agent,
            source_thread_id: Some(Uuid::new_v4()),
            emitting_trigger_id: None,
        }),
    )
    .await;
    subscribe_and_deliver(&bus, waiter, released).await;

    assert_eq!(
        recent_subscriptions(&pool, waiter).await.unwrap(),
        0,
        "every wait here was ended by another thread's work"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn recent_subscriptions_counts_self_wakes_unattributed_wakes_and_timeouts() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    use super::register::recent_subscriptions;

    let waiter = Uuid::new_v4();
    seed_thread(&bus, waiter).await;

    let own = seed_thread_event(&bus, waiter, an_agent_message()).await;
    subscribe_and_deliver(&bus, waiter, own).await;

    let anonymous = emit_domain_event(
        &bus,
        Some(MessageOrigin::Agent {
            agent: crate::engine::thread_events::AgentParticipant::LucidosAgent,
        }),
    )
    .await;
    subscribe_and_deliver(&bus, waiter, anonymous).await;

    let own_script = emit_domain_event(
        &bus,
        Some(MessageOrigin::Api {
            user_agent: None,
            mode: crate::engine::thread_events::ActorMode::Agent,
            source_thread_id: Some(waiter),
            emitting_trigger_id: None,
        }),
    )
    .await;
    subscribe_and_deliver(&bus, waiter, own_script).await;

    let expired = emit_subscribe(&bus, waiter, vec![sub("ChangeProposed", None)]).await;
    seed_thread_event(
        &bus,
        waiter,
        ThreadEvent::EventWaitExpired { wait_id: expired },
    )
    .await;

    emit_subscribe(&bus, waiter, vec![sub("ResponseGenerated", None)]).await;

    assert_eq!(recent_subscriptions(&pool, waiter).await.unwrap(), 5);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn recent_subscriptions_resets_on_a_human_answer_only() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    use super::register::recent_subscriptions;

    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    let answer = |tool_use_id: &str, actor: MessageOrigin| BusEvent::Thread {
        thread_id,
        event: ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: tool_use_id.into(),
            quiet: false,
            answer: crate::engine::thread_events::AnswerKind::Canceled,
        },
        meta: crate::engine::thread_events::EventMeta::with_actor(Some(actor)),
    };

    emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    bus.emit(answer(
        "toolu_agent",
        MessageOrigin::ThreadLink {
            thread_id: Uuid::new_v4(),
            title: None,
            spawning_event_id: None,
            mode: crate::engine::thread_events::ActorMode::Agent,
            direction: crate::engine::thread_events::ThreadDirection::Parent,
        },
    ))
    .await
    .unwrap();
    assert_eq!(
        recent_subscriptions(&pool, thread_id).await.unwrap(),
        1,
        "an agent answering is not a human in the loop"
    );

    bus.emit(answer(
        "toolu_human",
        MessageOrigin::Device {
            device_id: "test-device".into(),
        },
    ))
    .await
    .unwrap();
    assert_eq!(
        recent_subscriptions(&pool, thread_id).await.unwrap(),
        0,
        "a tapped answer resets the count"
    );

    emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    bus.emit(answer(
        "toolu_api",
        MessageOrigin::Api {
            user_agent: None,
            mode: crate::engine::thread_events::ActorMode::Human,
            source_thread_id: None,
            emitting_trigger_id: None,
        },
    ))
    .await
    .unwrap();
    assert_eq!(
        recent_subscriptions(&pool, thread_id).await.unwrap(),
        0,
        "a person answering over the API resets the count too"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_never_emitted_event_type_is_flagged_at_registration() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    use crate::core::event_subscription::{
        check_subscriptions, event_type_ever_emitted, SubscriptionSurface,
    };

    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    assert!(event_type_ever_emitted(&pool, "MessageReceived").await);
    assert!(!event_type_ever_emitted(&pool, "ReleaseFinnished").await);

    let warnings = check_subscriptions(
        &pool,
        &[sub("ReleaseFinnished", None)],
        SubscriptionSurface::Wait,
    )
    .await
    .expect("a domain event nobody has emitted yet is accepted");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("never been emitted"), "{warnings:?}");
    assert!(warnings[0].contains("ReleaseFinnished"), "{warnings:?}");

    let quiet = check_subscriptions(
        &pool,
        &[sub("ChangeProposed", None)],
        SubscriptionSurface::Wait,
    )
    .await
    .expect("a known engine name is accepted");
    assert!(
        quiet.is_empty(),
        "a known name warns about nothing: {quiet:?}"
    );

    let w = wait_with(thread_id, vec![sub("ReleaseFinnished", None)], 0);
    let text = expiry_reentry_text(&w, &["ReleaseFinnished".to_string()]);
    assert!(text.contains("never emitted"), "{text}");
    assert!(text.contains("ReleaseFinnished"), "{text}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_name_this_workspace_has_emitted_is_never_refused() {
    use crate::core::event_subscription::{check_subscriptions, SubscriptionSurface};
    use crate::engine::event_bus::SystemEvent;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let err = check_subscriptions(
        &pool,
        &[sub("CredentialStored", None)],
        SubscriptionSurface::Wait,
    )
    .await
    .unwrap_err();
    assert!(err.contains("CredentialCreated"), "{err}");

    bus.emit(BusEvent::System(SystemEvent::DomainEvent {
        event_type: "CredentialStored".to_string(),
        payload: json!({"summary": "the vault took a write"}),
        depth: 0,
        transient: false,
        actor: None,
    }))
    .await
    .unwrap()
    .expect("a domain event writes a row");

    let warnings = check_subscriptions(
        &pool,
        &[sub("CredentialStored", None)],
        SubscriptionSurface::Wait,
    )
    .await
    .expect("the store proves the name is real");
    assert!(warnings.is_empty(), "{warnings:?}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn seed_plugin_install(bus: &EventBus, id: &str, version: &str) {
    use crate::engine::event_bus::SystemEvent;
    bus.emit(BusEvent::System(SystemEvent::PluginInstalled {
        id: id.to_string(),
        manifest: json!({
            "summary": format!("Installed {id} v{version}"),
            "manifest": {"id": id, "version": version, "name": id},
            "files": [format!("apps/{id}/index.html")],
            "source_type": "git",
        }),
        files: vec![format!("apps/{id}/index.html")],
        files_removed: Vec::new(),
        affected_triggers: Vec::new(),
        installed_at: "2026-08-25T15:12:00+00:00".to_string(),
        source_type: "git".to_string(),
        actor: None,
    }))
    .await
    .unwrap()
    .expect("a plugin install writes a row");
}

#[tokio::test]
async fn a_condition_path_no_payload_carries_is_flagged_at_registration() {
    use crate::core::event_subscription::{check_subscriptions, SubscriptionSurface};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    seed_plugin_install(&bus, "pull-requests", "0.1.0").await;

    let dead = sub("PluginInstalled", Some(json!({"version": "0.1.0"})));
    let warnings = check_subscriptions(&pool, &[dead], SubscriptionSurface::Wait)
        .await
        .expect("a path is never a refusal, only a warning");
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(warnings[0].contains("'version'"), "{warnings:?}");
    assert!(
        warnings[0].contains("manifest.manifest.version"),
        "the real path is named: {warnings:?}"
    );

    let live = sub(
        "PluginInstalled",
        Some(json!({"manifest.manifest.version": "0.1.0"})),
    );
    let quiet = check_subscriptions(&pool, &[live], SubscriptionSurface::Wait)
        .await
        .unwrap();
    assert!(
        quiet.is_empty(),
        "the corroborated path is silent: {quiet:?}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_plugin_install_is_filterable_by_its_id() {
    use crate::core::event_subscription::{check_subscriptions, SubscriptionSurface};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    seed_plugin_install(&bus, "pull-requests", "0.1.0").await;

    let on = vec![sub("PluginInstalled", Some(json!({"id": "pull-requests"})))];
    let warnings = check_subscriptions(&pool, &on, SubscriptionSurface::Wait)
        .await
        .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");

    let lookback = arming_lookback_matches(
        &pool,
        &on,
        i64::MAX,
        3600,
        &std::collections::HashSet::new(),
        5,
    )
    .await
    .unwrap();
    assert_eq!(lookback.matches.len(), 1, "the condition matches the row");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_path_probe_stays_quiet_when_it_cannot_know() {
    use crate::core::event_subscription::{check_subscriptions, SubscriptionSurface};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;

    let scoped = sub(
        "MessageReceived",
        Some(json!({"thread_id": thread_id.to_string()})),
    );
    let no_rows = sub("BackupCompleted", Some(json!({"filename": "x.tar.gz"})));
    let optional = sub("MessageReceived", Some(json!({"actor": {"$ne": null}})));

    for entry in [scoped, no_rows] {
        let quiet = check_subscriptions(
            &pool,
            std::slice::from_ref(&entry),
            SubscriptionSurface::Wait,
        )
        .await
        .unwrap();
        assert!(quiet.is_empty(), "{entry:?} warned: {quiet:?}");
    }

    let warned = check_subscriptions(&pool, &[optional], SubscriptionSurface::Wait)
        .await
        .expect("still accepted");
    assert_eq!(warned.len(), 1, "{warned:?}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_dead_name_after_a_live_one_refuses_the_whole_call() {
    use crate::core::event_subscription::{check_subscriptions, SubscriptionSurface};

    let (pool, db_name) = setup_test_db().await;
    let subs = vec![
        sub("ResponseGenerated", None),
        sub("CredentialRequestResolved", None),
    ];
    let err = check_subscriptions(&pool, &subs, SubscriptionSurface::Wait)
        .await
        .unwrap_err();
    assert!(err.contains("CredentialRequestResolved"), "{err}");
    assert!(err.contains("CredentialRequested"), "{err}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_canceled_wait_does_not_come_back_on_the_next_boot() {
    use crate::engine::thread_events::EventWaitCancelCause;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    let waits = LiveWaits::new();
    rebuild_live_waits(&pool, &waits).await.unwrap();
    let wait = waits.take(wait_id).await.unwrap();

    emit_cancel(&bus, &wait, EventWaitCancelCause::UserStop, None)
        .await
        .unwrap();

    let order: Vec<String> = sqlx::query_scalar(
        "SELECT event_type FROM events WHERE aggregate_id = $1 \
           AND event_type IN ('EventWaitCanceled','ToolResult') ORDER BY sequence",
    )
    .bind(thread_id.to_string())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        order,
        vec!["EventWaitCanceled"],
        "a cancel writes one event and nothing else: `await_event` paired its \
         own call at registration, so there is no dangling call to close"
    );

    let after_restart = LiveWaits::new();
    assert_eq!(rebuild_live_waits(&pool, &after_restart).await.unwrap(), 0);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_refused_anchor_leaves_the_wait_live() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    let waits = LiveWaits::new();
    rebuild_live_waits(&pool, &waits).await.unwrap();
    let wait = waits.take(wait_id).await.unwrap();
    refuse_event_type(&pool, "PromptInjected").await;

    emit_delivery(&bus, &wait, Uuid::new_v4(), "ChangeProposed", &json!({}), 0)
        .await
        .expect_err("a refused anchor fails the delivery");

    let resolutions: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 \
           AND event_type = 'EventWaitDelivered'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        resolutions, 0,
        "a resolution must not land without its anchor"
    );
    let rebuilt = LiveWaits::new();
    rebuild_live_waits(&pool, &rebuilt).await.unwrap();
    assert!(
        rebuilt.take(wait_id).await.is_some(),
        "the wait is still live"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_cancel_that_never_persisted_is_reported_as_still_live() {
    use crate::engine::thread_events::EventWaitCancelCause;
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    let waits = LiveWaits::new();
    rebuild_live_waits(&pool, &waits).await.unwrap();
    let wait = waits.take(wait_id).await.unwrap();

    pool.close().await;
    emit_cancel(&bus, &wait, EventWaitCancelCause::UserStop, None)
        .await
        .expect_err("a closed pool cannot persist the cancel");

    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_freshly_delivered_wait_looks_exactly_like_a_lost_one() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;

    let waits = LiveWaits::new();
    rebuild_live_waits(&pool, &waits).await.unwrap();
    let wait = waits.take(wait_id).await.unwrap();
    emit_delivery(&bus, &wait, Uuid::new_v4(), "ChangeProposed", &json!({}), 0)
        .await
        .unwrap();

    assert_eq!(
        lost_wait_reentries(&pool).await.unwrap().len(),
        1,
        "a just-delivered pair IS a lost-re-entry candidate until its turn writes \
         something, which is why the sweep must run before the rebuild",
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_catch_up_scan_pages_past_non_matching_rows_and_terminates() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    for i in 0..(super::CATCH_UP_PAGE + 5) {
        seed_thread_event(
            &bus,
            thread_id,
            ThreadEvent::ToolCalled {
                name: "run_bash".into(),
                description: format!("call {i}"),
                args: json!({}),
            },
        )
        .await;
    }

    let wait = wait_with(
        thread_id,
        vec![sub(
            "ToolCalled",
            Some(json!({"name": "the_one_that_matches"})),
        )],
        0,
    );
    assert!(
        catch_up_from_watermark(&pool, &wait)
            .await
            .unwrap()
            .is_none(),
        "no row passes the condition, and the scan must end rather than loop"
    );

    seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::ToolCalled {
            name: "the_one_that_matches".into(),
            description: "found".into(),
            args: json!({}),
        },
    )
    .await;
    let hit = catch_up_from_watermark(&pool, &wait)
        .await
        .unwrap()
        .expect("the match sits past the first page");
    assert_eq!(hit.1, "ToolCalled");
    assert_eq!(hit.2["name"], "the_one_that_matches");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn restore_floor_prevents_archived_waits_rearming_on_every_boot() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let old = emit_subscribe(&bus, Uuid::new_v4(), vec![sub("ChangeProposed", None)]).await;
    sqlx::query("INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) SELECT $1, $2, now(), MAX(sequence) FROM events")
        .bind(Uuid::new_v4())
        .bind(crate::core::snapshot::handle::Handle::generate().as_str())
        .execute(&pool).await.unwrap();
    let new = emit_subscribe(&bus, Uuid::new_v4(), vec![sub("ChangeProposed", None)]).await;
    for _ in 0..2 {
        let waits = LiveWaits::new();
        assert_eq!(rebuild_live_waits(&pool, &waits).await.unwrap(), 1);
        assert!(waits.take(old).await.is_none());
        assert!(waits.take(new).await.is_some());
    }
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn restore_floor_excludes_old_resolution_reentries_but_keeps_new_ones() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let old = Uuid::new_v4();
    subscribe_and_resolve_without_waking(&bus, &pool, old).await;
    assert_eq!(lost_wait_reentries(&pool).await.unwrap().len(), 1);
    sqlx::query("INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) SELECT $1, $2, now(), MAX(sequence) FROM events")
        .bind(Uuid::new_v4())
        .bind(crate::core::snapshot::handle::Handle::generate().as_str())
        .execute(&pool).await.unwrap();
    let new = Uuid::new_v4();
    subscribe_and_resolve_without_waking(&bus, &pool, new).await;
    for _ in 0..2 {
        let lost = lost_wait_reentries(&pool).await.unwrap();
        assert_eq!(lost.len(), 1);
        assert_eq!(lost[0].thread_id, new);
    }
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn restore_settle_cancels_archived_waits_once_and_keeps_new_ones() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let old = emit_subscribe(&bus, thread, vec![sub("ChangeProposed", None)]).await;
    settle_restored_waits(&pool, &bus).await.unwrap();
    let canceled = || async {
        sqlx::query_scalar::<_, String>(
            "SELECT payload->>'wait_id' FROM events WHERE event_type = 'EventWaitCanceled' \
             AND payload->>'cause' = 'snapshot_restored'",
        )
        .fetch_all(&pool)
        .await
        .unwrap()
    };
    assert!(canceled().await.is_empty(), "a normal boot cancels nothing");
    sqlx::query("INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) SELECT $1, $2, now(), MAX(sequence) FROM events")
        .bind(Uuid::new_v4())
        .bind(crate::core::snapshot::handle::Handle::generate().as_str())
        .execute(&pool).await.unwrap();
    let new = emit_subscribe(&bus, thread, vec![sub("ChangeProposed", None)]).await;
    for _ in 0..2 {
        settle_restored_waits(&pool, &bus).await.unwrap();
        assert_eq!(canceled().await, [old.to_string()]);
    }
    let waits = LiveWaits::new();
    assert_eq!(rebuild_live_waits(&pool, &waits).await.unwrap(), 1);
    assert!(waits.take(new).await.is_some());
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn sub_thread_card_relay_is_addressed_and_forward_only_after_rebuild() {
    use crate::engine::event_bus::SubThreadCardKind;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let [ancestor, child, source_event_id, unrelated] = std::array::from_fn(|_| Uuid::new_v4());
    let event = SystemEvent::SubThreadCardOpened {
        ancestor_thread_id: ancestor,
        child_thread_id: child,
        source_thread_id: child,
        source_event_id,
        card_kind: SubThreadCardKind::Question,
    };
    let name = event.event_type();
    let payload = crate::core::event_subscription::matchable_system_payload(&event);
    let waits = LiveWaits::new();
    let addressed = wait_with(
        ancestor,
        vec![
            sub("ChildThreadCompleted", None),
            sub(name, Some(json!({"card_kind": "question"}))),
        ],
        max_sequence(&pool).await,
    );
    let other = wait_with(unrelated, vec![sub(name, None)], addressed.watermark);
    for wait in [&addressed, &other] {
        super::register::persist_wait(&bus, &waits, wait)
            .await
            .unwrap();
    }

    let mut live = bus.subscribe();
    let opening = bus.emit(BusEvent::System(event)).await.unwrap().unwrap();
    let frame = live.recv().await.unwrap();
    assert_eq!(frame.seq, Some(opening.seq));
    assert!(crate::scheduler::trigger_dispatch(&frame).is_none());
    assert_eq!(
        waits_matching_at(&waits.snapshot().await, name, &payload, frame.seq),
        vec![(addressed.wait_id, 1)]
    );
    let late_arm = wait_with(ancestor, vec![sub(name, None)], opening.seq);
    assert_eq!(late_arm.matched_index_at(name, &payload, frame.seq), None);
    assert!(catch_up_from_watermark(&pool, &late_arm)
        .await
        .unwrap()
        .is_none());

    let restored = LiveWaits::new();
    assert_eq!(rebuild_live_waits(&pool, &restored).await.unwrap(), 2);
    for wait in restored.snapshot().await {
        let hit = catch_up_from_watermark(&pool, &wait).await.unwrap();
        if wait.thread_id == ancestor {
            assert_eq!(
                hit,
                Some((opening.event_id, name.into(), payload.clone(), 1))
            );
        } else {
            assert!(hit.is_none());
        }
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn sub_thread_card_report_pages_current_open_cards_below_the_relay_watermark() {
    use crate::engine::event_bus_test_support::emit_thread_message;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let [ancestor, parent, detached, unrelated] = std::array::from_fn(|_| Uuid::new_v4());
    for (thread, link) in [
        (ancestor, None),
        (parent, Some(ancestor)),
        (detached, Some(ancestor)),
        (unrelated, None),
    ] {
        emit_thread_message(&bus, thread, link, "start").await;
    }
    let name = SystemEvent::SUB_THREAD_CARD_OPENED;
    let insert_legacy = |thread: Uuid, kind: &'static str| {
        let pool = pool.clone();
        async move {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created)
                 VALUES ($1, 'thread', $2, $3, $4, now() - interval '7 days')",
            )
            .bind(id)
            .bind(thread.to_string())
            .bind(kind)
            .bind(json!({"tool_use_id": id, "request_id": id}))
            .execute(&pool)
            .await
            .unwrap();
            id
        }
    };

    insert_legacy(ancestor, "UserQuestionAsked").await;
    insert_legacy(unrelated, "UserQuestionAsked").await;
    insert_legacy(detached, "UserQuestionAsked").await;
    let mut ids = Vec::new();
    for _ in 0..10 {
        let source = Uuid::new_v4();
        emit_thread_message(&bus, source, Some(parent), "child").await;
        ids.push(insert_legacy(source, "UserQuestionAsked").await);
    }
    insert_legacy(parent, "UserQuestionQueued").await;
    insert_legacy(parent, "CommandPermissionRequested").await;
    sqlx::query("UPDATE thread_summaries SET parent_thread_id = NULL WHERE thread_id = $1")
        .bind(detached)
        .execute(&pool)
        .await
        .unwrap();
    let delivered_later = insert_legacy(parent, "UserQuestionAsked").await;

    // The horizon is between this card's source row and its addressed relay.
    let watermark = max_sequence(&pool).await;
    bus.emit(BusEvent::System(SystemEvent::SubThreadCardOpened {
        ancestor_thread_id: ancestor,
        child_thread_id: parent,
        source_thread_id: parent,
        source_event_id: delivered_later,
        card_kind: crate::engine::event_bus::SubThreadCardKind::Question,
    }))
    .await
    .unwrap();
    let wait = wait_with(
        ancestor,
        vec![sub(name, Some(json!({"card_kind": "question"})))],
        watermark,
    );
    let first = super::pending_cards::pending_cards(&pool, &wait, 0)
        .await
        .unwrap();
    let after = first.next_after.unwrap();
    assert_eq!(first.cards.len(), 8);
    assert_eq!(first.cards[0]["source_event_id"], ids[0].to_string());
    assert_eq!(first.cards[0]["child_thread_id"], parent.to_string());

    // An answer before continuation removes this card from the remaining page.
    let source: String = sqlx::query_scalar("SELECT aggregate_id FROM events WHERE id = $1")
        .bind(ids[8])
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload)
         VALUES ($1, 'thread', $2, 'UserQuestionAnswered', $3)",
    )
    .bind(Uuid::new_v4())
    .bind(source)
    .bind(json!({"tool_use_id": ids[8]}))
    .execute(&pool)
    .await
    .unwrap();
    let second = super::pending_cards::pending_cards(&pool, &wait, after)
        .await
        .unwrap();
    assert!(second.next_after.is_none());
    assert_eq!(second.cards.len(), 1);
    assert_eq!(second.cards[0]["source_event_id"], ids[9].to_string());
    let hit = catch_up_from_watermark(&pool, &wait)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(hit.2["source_event_id"], delivered_later.to_string());

    pool.close().await;
    teardown_test_db(&db_name).await;
}
