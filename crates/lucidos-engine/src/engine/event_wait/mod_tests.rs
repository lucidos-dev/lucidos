use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::test_support::{setup_test_db, teardown_test_db};
use serde_json::json;

fn sub(event_type: &str, condition: Option<Value>) -> EventSubscription {
    EventSubscription {
        event_type: event_type.to_string(),
        condition,
    }
}

fn wait_with(thread_id: Uuid, on: Vec<EventSubscription>, watermark: i64) -> LiveWait {
    LiveWait {
        wait_id: Uuid::new_v4(),
        thread_id,
        tool_use_id: "toolu_park".into(),
        on,
        reason: "waiting for a change".into(),
        expires_at: Utc::now() + chrono::Duration::hours(1),
        watermark,
    }
}

// ── matching ────────────────────────────────────────────────────────

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
    // Second entry, so the model can tell which of its subscriptions woke it.
    assert_eq!(
        w.matched_index("ChangeProposed", &json!({"file_count": 3})),
        Some(1),
    );
    assert_eq!(w.matched_index("ResponseGenerated", &json!({})), Some(0));
    // Right name, condition false.
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

// ── the one-shot gate (I7) ──────────────────────────────────────────

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

// ── the awaitable gate (I9) ─────────────────────────────────────────

#[test]
fn the_firehose_never_reaches_the_wait_matcher() {
    assert!(!is_awaitable_event(&ThreadEvent::TextStreamed {
        text: "tok".into()
    }));
    assert!(!is_awaitable_event(&ThreadEvent::ThoughtStreamed {
        text: "tok".into()
    }));
}

/// The tightest possible loop, closed structurally rather than by validation:
/// a wait on `EventWaitStarted` would satisfy itself the instant any thread in
/// the workspace registers one, including its own.
#[test]
fn the_event_wait_family_never_reaches_the_wait_matcher() {
    let started = ThreadEvent::EventWaitStarted {
        wait_id: Uuid::new_v4(),
        tool_use_id: "toolu".into(),
        on: vec![sub("ChangeProposed", None)],
        reason: "r".into(),
        expires_at: Utc::now(),
        watermark: 0,
    };
    assert!(!is_awaitable_event(&started));
    assert!(!is_awaitable_event(&ThreadEvent::EventWaitExpired {
        wait_id: Uuid::new_v4(),
    }));
    // But the gate the TRIGGER matcher uses still admits them: they stay
    // triggerable, so "notify me when a thread's wait timed out" works.
    assert!(is_subscribable(&started));
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

// ── the deadline boundary ───────────────────────────────────────────

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

// ── the wake payloads ───────────────────────────────────────────────

#[test]
fn the_delivery_wake_carries_the_event_and_the_reason() {
    let text = delivery_wake_text(
        "ChangeProposed",
        &json!({"file_count": 2}),
        "waiting to apply",
    );
    assert!(text.contains("ChangeProposed"), "{text}");
    assert!(text.contains("file_count"), "{text}");
    assert!(text.contains("waiting to apply"), "{text}");
}

/// An expiry wakes the thread rather than dropping it, and the text has to
/// steer the model away from subscribing again to the same thing: a silent
/// re-subscribe loop would be the polling this feature replaces, with extra
/// steps.
#[test]
fn the_expiry_wake_says_it_timed_out_and_discourages_re_subscribing() {
    let w = wait_with(Uuid::new_v4(), vec![sub("ChangeProposed", None)], 0);
    let text = expiry_wake_text(&w, &[]);
    assert!(text.contains("Timed out"), "{text}");
    assert!(text.contains("ChangeProposed"), "{text}");
    assert!(
        text.contains("waiting for a change"),
        "carries the reason: {text}"
    );
    assert!(text.contains("Report"), "{text}");
}

/// Registration confirms the subscription AND tells the model it is free to
/// finish. Without the second half the model reads "subscribed" as "blocked"
/// and stalls the turn waiting for something that arrives as a new turn.
#[test]
fn the_registration_result_says_nothing_is_blocking() {
    let engine_side = super::register::registered_tool_result_text(&wait_with(
        Uuid::new_v4(),
        vec![sub("ChangeProposed", None)],
        0,
    ));
    assert!(engine_side.contains("ChangeProposed"), "{engine_side}");
    assert!(
        engine_side.contains("Nothing is blocking"),
        "the model must not stall the turn: {engine_side}"
    );
    assert!(
        engine_side.contains("NEW turn"),
        "says where the wake lands: {engine_side}"
    );
    assert!(
        engine_side.contains("Do not call await_event again"),
        "the cheap half of the duplicate refusal: {engine_side}"
    );
}

/// Delivery is the only resolution that CONSUMES the subscription, and it used
/// to be the only one whose text said nothing about that. Silence read as
/// agreement with its neighbours, which say "do not register again": on
/// 2026-08-06 a live thread woke, reported the event, closed with "Re-arming
/// the watch now" and ended the turn with no second call, leaving the user
/// looking at an idle thread that had just promised to keep watching.
#[test]
fn the_delivery_wake_says_the_subscription_is_spent_and_re_arming_is_a_call() {
    let text = delivery_wake_text(
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

/// Every text the model reads off a subscription has to say what state it is
/// in, because the right next move differs for each: finish the turn after
/// registering, subscribe again after a delivery, report back after an expiry.
/// A shape that says nothing gets read as whichever neighbouring instruction
/// the model remembers, which is how the delivery silence above turned into a
/// watch that stopped without saying so.
///
/// **Add a row when you add a shape.** One that ships silent reproduces exactly
/// that bug. A cancel is deliberately absent: the user ended it, and it writes
/// no wake at all.
#[test]
fn every_subscription_text_says_where_the_subscription_stands() {
    let w = wait_with(Uuid::new_v4(), vec![sub("ChangeProposed", None)], 0);
    let payload = json!({"file_count": 1});

    let shapes: Vec<(&str, String, &str)> = vec![
        (
            "registration",
            super::register::registered_tool_result_text(&w),
            "Nothing is blocking",
        ),
        (
            "delivery",
            delivery_wake_text("ChangeProposed", &payload, "waiting to apply"),
            "now spent",
        ),
        (
            "expiry",
            expiry_wake_text(&w, &[]),
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

/// The fan-in's dedupe gate (`child_completion_has_an_event_wait`) hands the
/// matcher the `ChildThreadCompleted`'s **persisted payload column**, so that
/// column has to be a shape the matcher understands. Both halves are pinned
/// here: the bare subscription, and one carrying a `condition` on a real field.
///
/// This is the half the gate's first version got wrong. It probed only for a
/// persisted `EventWaitDelivered`, but the fan-in and the dispatcher are woken
/// by the same broadcast on separate tasks, so that row is almost never written
/// yet when the fan-in looks: the gate essentially never fired and both wakes
/// ran. Matching the live cache against this payload is what closes it.
#[tokio::test]
async fn a_child_completion_card_matches_a_wait_watching_for_one() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let parent_id = Uuid::new_v4();
    seed_thread(&bus, parent_id).await;
    let emitted = bus
        .emit(BusEvent::Thread {
            thread_id: parent_id,
            event: ThreadEvent::ChildThreadCompleted {
                child_thread_id: Uuid::new_v4(),
                child_thread_title: Some("Nightly E2E".into()),
                status: crate::engine::thread_events::ChildCompletionStatus::Success,
                summary: "all green".into(),
                pending_change_ids: vec![],
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap()
        .expect("the card must persist");

    let row = crate::core::store::EventStore::new(pool.clone())
        .get_event_by_id(emitted.event_id)
        .await
        .unwrap()
        .expect("the card row");

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

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// ── durability: rebuild + catch-up (I3, S7) ─────────────────────────

async fn emit_subscribe(bus: &EventBus, thread_id: Uuid, on: Vec<EventSubscription>) -> Uuid {
    let wait_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitStarted {
            wait_id,
            tool_use_id: format!("toolu_{}", wait_id.simple()),
            on,
            reason: "waiting for a change".into(),
            expires_at: Utc::now() + chrono::Duration::hours(1),
            watermark: 0,
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("EventWaitStarted emit")
    .expect("EventWaitStarted persisted");
    wait_id
}

#[tokio::test]
async fn rebuild_recovers_a_live_wait_and_skips_resolved_ones() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let live_thread = Uuid::new_v4();
    let live_id = emit_subscribe(&bus, live_thread, vec![sub("ChangeProposed", None)]).await;

    // A wait that already resolved must NOT come back on boot.
    let done_thread = Uuid::new_v4();
    let done_id = emit_subscribe(&bus, done_thread, vec![sub("ChangeProposed", None)]).await;
    bus.emit(BusEvent::Thread {
        thread_id: done_thread,
        event: ThreadEvent::EventWaitCanceled {
            wait_id: done_id,
            cause: crate::engine::thread_events::EventWaitCancelCause::UserStop,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

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

/// A wait whose deadline passed while the engine was down is re-armed rather
/// than dropped, so the deadline sweep can wake its thread with an expiry.
/// Dropping it here is the silent-stall the whole design refuses (I3).
#[tokio::test]
async fn rebuild_re_arms_a_wait_that_expired_while_the_engine_was_down() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wait_id = Uuid::new_v4();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitStarted {
            wait_id,
            tool_use_id: "toolu_stale".into(),
            on: vec![sub("ChangeProposed", None)],
            reason: "waiting across a restart".into(),
            expires_at: Utc::now() - chrono::Duration::hours(2),
            watermark: 0,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

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

/// The catch-up scan closes both the restart gap and the live registration
/// race with one mechanism: events after the watermark are replayed against the
/// wait, oldest first.
#[tokio::test]
async fn catch_up_finds_matches_after_the_watermark_and_ignores_earlier_ones() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let other_thread = Uuid::new_v4();
    // An event BEFORE the wait registers. It must not satisfy the wait.
    bus.emit(BusEvent::Thread {
        thread_id: other_thread,
        event: ThreadEvent::ThreadArchived,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let watermark: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM events")
        .fetch_one(&pool)
        .await
        .unwrap();

    // Two matching events after the watermark, plus a non-matching one.
    for _ in 0..2 {
        bus.emit(BusEvent::Thread {
            thread_id: other_thread,
            event: ThreadEvent::ThreadArchived,
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
    }
    bus.emit(BusEvent::Thread {
        thread_id: other_thread,
        event: ThreadEvent::ThreadSaved,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    let wait = wait_with(Uuid::new_v4(), vec![sub("ThreadArchived", None)], watermark);
    // Only the FIRST match is returned: a wait is a rendezvous, and the scan
    // stops paging the moment it finds one.
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
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ToolCalled {
                name: name.into(),
                args: json!({}),
                description: String::new(),
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
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

// ── attachment, derived (S5b) ───────────────────────────────────────

// ── emission order ──────────────────────────────────────────────────

/// `EventWaitDelivered` must persist BEFORE its wake anchor, so a crash
/// between the two leaves a resolved wait rather than an anchor for a wait the
/// boot rebuild would re-arm.
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
           AND event_type IN ('EventWaitDelivered','UserPromptInjected') ORDER BY sequence",
    )
    .bind(thread_id.to_string())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(order, vec!["EventWaitDelivered", "UserPromptInjected"]);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A delivery writes NO `ToolResult`. `await_event` paired its own call at
/// registration, so a result here would be a second one for that call, which is
/// a provider 400 on the thread's next turn.
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
           AND event_type IN ('EventWaitExpired','UserPromptInjected') ORDER BY sequence",
    )
    .bind(thread_id.to_string())
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(order, vec!["EventWaitExpired", "UserPromptInjected"]);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// After a resolution the boot rebuild must not re-arm the wait, which is what
/// makes delivery genuinely one-shot across a restart and not just in memory.
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

/// Seed a chat thread so `thread_summaries` has a row. The `EventWaitStarted`
/// projection is an UPDATE, not an upsert, so a status assertion needs a real
/// turn to have started first (in production one always has).
async fn seed_thread(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "start the work".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
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
    .unwrap();
}

// ── no resolution moves the status ──────────────────────────────────

/// **The rule that replaced the attached/detached status split.** A
/// subscription never holds its thread's turn, so no `EventWait*` event may
/// write a status. The sharpest case is a resolution landing on a thread that
/// is running something unrelated: a status write there reports a live turn as
/// freshly revived, or settles it outright.
#[tokio::test]
async fn no_event_wait_resolution_touches_the_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    // `MessageReceived` leaves the thread Running, i.e. mid-turn.
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
        },
    ] {
        let label = event.event_type();
        bus.emit(BusEvent::Thread {
            thread_id,
            event,
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
        assert_eq!(
            status(pool.clone()).await,
            "running",
            "{label} must leave the running turn alone"
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The other half of the same rule: a thread whose turn genuinely ended stays
/// `idle` while it holds a subscription. That is the whole point of the
/// 2026-08-06 change, so it is pinned rather than left implied.
#[tokio::test]
async fn a_thread_holding_a_subscription_is_idle() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "subscribed, I will report back".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

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

// ── the wake anchor and the lost-wake sweep (I3b) ────────────────────

/// A detached resolution has no tool-result slot, so `emit_resolution` writes a
/// `UserPromptInjected` instead: the frontend's exchange-starter, which is what
/// makes the wake read as the new turn it genuinely is. The anchor id must be
/// that event, since the re-entry hangs its whole exchange off it.
#[tokio::test]
async fn a_delivery_anchors_on_a_user_prompt_injected() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    let waits = LiveWaits::new();
    rebuild_live_waits(&pool, &waits).await.unwrap();
    let wait = waits.take(wait_id).await.unwrap();

    let wake = emit_delivery(
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
            .bind(wake.anchor_event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(anchor_type, "UserPromptInjected");
    assert_eq!(
        anchor_text.as_deref(),
        Some(wake.text.as_str()),
        "the anchor carries the same words the re-entry prompts with"
    );
    assert!(wake.text.contains("ChangeProposed"), "{}", wake.text);

    // No ToolResult: there was no open slot to fill.
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

/// The anchor also points BACK at the delivery, so a client can render the
/// matched event by name with its payload folded away instead of showing the
/// pretty-printed JSON that `text` has to carry for the model. The link is an
/// id rather than a copy: the `EventWaitDelivered` it names already holds
/// `event_type` and `payload` as fields.
#[tokio::test]
async fn a_delivery_anchor_points_back_at_its_resolution() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    let waits = LiveWaits::new();
    rebuild_live_waits(&pool, &waits).await.unwrap();
    let wait = waits.take(wait_id).await.unwrap();

    let wake = emit_delivery(
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
    .bind(wake.anchor_event_id)
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
    // What the link buys: the same facts the prose spells out, as fields.
    assert_eq!(delivered_type, "ChangeProposed");
    assert_eq!(delivered_payload, json!({"file_count": 3}));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An expiry's anchor leaves the link unset. There is no payload to point at,
/// so a client following it would land on a row that says only what the prose
/// already said.
#[tokio::test]
async fn an_expiry_anchor_carries_no_delivery_link() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    let waits = LiveWaits::new();
    rebuild_live_waits(&pool, &waits).await.unwrap();
    let wait = waits.take(wait_id).await.unwrap();

    let wake = emit_expiry(&bus, &wait, &[]).await.unwrap();

    let linked: Option<Uuid> = sqlx::query_scalar(
        "SELECT (payload->>'delivered_event_id')::uuid FROM events WHERE id = $1",
    )
    .bind(wake.anchor_event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(linked, None);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Seed a `thread_summaries` row, then subscribe + resolve, leaving the anchor
/// as the thread's last word. That is the crash shape
/// `refire_unresolved_event_wakes` exists for: the resolution is persisted (so
/// the rebuild will not re-arm the wait) and the turn never ran.
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
async fn the_lost_wake_sweep_finds_a_resolution_whose_turn_never_ran() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    subscribe_and_resolve_without_waking(&bus, &pool, thread_id).await;

    let lost = lost_event_wakes(&pool).await.unwrap();
    assert_eq!(lost.len(), 1, "{lost:?}");
    assert_eq!(lost[0].thread_id, thread_id);
    assert!(
        lost[0].wake.text.contains("ChangeProposed"),
        "the re-entry re-uses the persisted prompt: {}",
        lost[0].wake.text
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The other half of the rule, and the one that matters: a wake that DID run
/// must never be re-driven, or a restart would double-run the turn.
#[tokio::test]
async fn the_lost_wake_sweep_skips_a_wake_that_already_ran() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    subscribe_and_resolve_without_waking(&bus, &pool, thread_id).await;
    assert_eq!(lost_event_wakes(&pool).await.unwrap().len(), 1);

    // One event from the woken turn is enough to prove it ran.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "the change landed".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();

    assert!(
        lost_event_wakes(&pool).await.unwrap().is_empty(),
        "a consumed wake is not lost"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_lost_wake_sweep_skips_a_discarded_thread() {
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
        lost_event_wakes(&pool).await.unwrap().is_empty(),
        "reviving a thread the user threw away is the archive-curtain problem again"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// ── the consecutive-park cap (I13) ───────────────────────────────────

#[tokio::test]
async fn consecutive_subscriptions_counts_only_since_the_last_human_message() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    use super::register::consecutive_subscriptions;

    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await; // a human message
    assert_eq!(
        consecutive_subscriptions(&pool, thread_id).await.unwrap(),
        0
    );

    emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    emit_subscribe(&bus, thread_id, vec![sub("ResponseGenerated", None)]).await;
    assert_eq!(
        consecutive_subscriptions(&pool, thread_id).await.unwrap(),
        2
    );

    // An AGENT message must NOT reset the counter: cross-thread ping-pong is
    // made of exactly those, so counting it would disarm the cap it should trip.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "[CHILD THREAD COMPLETED]".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: crate::engine::thread_events::ActorMode::Agent,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    assert_eq!(
        consecutive_subscriptions(&pool, thread_id).await.unwrap(),
        2,
        "an agent message is not a human in the loop"
    );

    seed_thread(&bus, thread_id).await; // a real human follow-up
    assert_eq!(
        consecutive_subscriptions(&pool, thread_id).await.unwrap(),
        0,
        "a human message resets the streak"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_never_emitted_event_type_is_flagged_on_expiry_only() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    use super::register::event_type_ever_emitted;

    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    assert!(event_type_ever_emitted(&pool, "MessageReceived").await);
    assert!(!event_type_ever_emitted(&pool, "ReleaseFinnished").await);

    // The note rides on the expiry, which is the first moment the model can
    // learn it: registration accepts an unknown name on purpose, so a typo is
    // invisible until the deadline.
    let w = wait_with(thread_id, vec![sub("ReleaseFinnished", None)], 0);
    let text = expiry_wake_text(&w, &["ReleaseFinnished".to_string()]);
    assert!(text.contains("never emitted"), "{text}");
    assert!(text.contains("ReleaseFinnished"), "{text}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// ── cancel ───────────────────────────────────────────────────────────

/// A canceled wait must not come back at the next boot, exactly like a
/// delivered one: the user ended it, and re-arming it would resurrect a
/// subscription they explicitly stopped.
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

// ── a failed emit must not strand the wait ───────────────────────────

/// The failure mode a bare `Err` hides. `take` has already removed the wait, so
/// dropping the error would leave it gone from the live set (nothing can match
/// or expire it) while its `EventWaitStarted` is still unresolved in the store.
/// The two arms exist so the caller can tell "nothing was written, re-arm it"
/// from "the resolution landed, do NOT re-arm or it delivers twice".
#[tokio::test]
async fn a_resolution_that_never_persisted_is_reported_as_still_live() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;
    let waits = LiveWaits::new();
    rebuild_live_waits(&pool, &waits).await.unwrap();
    let wait = waits.take(wait_id).await.unwrap();

    // Close the pool under the bus: every emit now fails, starting with the
    // resolution itself.
    pool.close().await;
    let err = emit_delivery(&bus, &wait, Uuid::new_v4(), "ChangeProposed", &json!({}), 0)
        .await
        .expect_err("a closed pool cannot persist the resolution");
    assert!(
        matches!(err, ResolutionEmitError::Unresolved(_)),
        "nothing was written, so the wait is still live: {err:?}"
    );
    // The wording is what a maintainer reads in the log; keep it honest.
    assert!(err.to_string().contains("resolution was not persisted"));

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
    let err = emit_cancel(&bus, &wait, EventWaitCancelCause::UserStop, None)
        .await
        .expect_err("a closed pool cannot persist the cancel");
    assert!(
        matches!(err, ResolutionEmitError::Unresolved(_)),
        "a cancel that did not land leaves the wait live too: {err:?}"
    );

    teardown_test_db(&db_name).await;
}

/// The boot-ordering trap, pinned. `rebuild_event_waits` delivers inline, so
/// the pair it writes is momentarily indistinguishable from a stranded one: the
/// wake turn runs in a separate task and writes nothing for hundreds of
/// milliseconds. Running the lost-wake sweep AFTER the rebuild therefore
/// re-drives every wake the rebuild just queued, and each recovered thread
/// wakes twice. `main.rs` runs the sweep FIRST for exactly this reason; this
/// test is what makes that ordering a decision rather than an accident.
#[tokio::test]
async fn a_freshly_delivered_wait_looks_exactly_like_a_lost_one() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    let wait_id = emit_subscribe(&bus, thread_id, vec![sub("ChangeProposed", None)]).await;

    // Exactly what the boot rebuild's catch-up scan does: resolve the wait and
    // hand the turn to the wake channel, which has not run yet.
    let waits = LiveWaits::new();
    rebuild_live_waits(&pool, &waits).await.unwrap();
    let wait = waits.take(wait_id).await.unwrap();
    emit_delivery(&bus, &wait, Uuid::new_v4(), "ChangeProposed", &json!({}), 0)
        .await
        .unwrap();

    assert_eq!(
        lost_event_wakes(&pool).await.unwrap().len(),
        1,
        "a just-delivered pair IS a lost-wake candidate until its turn writes \
         something, which is why the sweep must run before the rebuild",
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The paging loop's termination edge: a full page with no match must page on,
/// and an exactly-full LAST page must still terminate rather than spin.
#[tokio::test]
async fn the_catch_up_scan_pages_past_non_matching_rows_and_terminates() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    let thread_id = Uuid::new_v4();
    // More rows of the subscribed TYPE than one page holds, none of which pass
    // the condition, so the scan cannot stop at the first page. Conditions are
    // evaluated in Rust, which is exactly why a bare `LIMIT 1` would be wrong.
    for i in 0..(super::CATCH_UP_PAGE + 5) {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ToolCalled {
                name: "run_bash".into(),
                description: format!("call {i}"),
                args: json!({}),
            },
            meta: EventMeta::NONE,
        })
        .await
        .unwrap();
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

    // Now put a match beyond the first page and prove paging reaches it.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ToolCalled {
            name: "the_one_that_matches".into(),
            description: "found".into(),
            args: json!({}),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    let hit = catch_up_from_watermark(&pool, &wait)
        .await
        .unwrap()
        .expect("the match sits past the first page");
    assert_eq!(hit.1, "ToolCalled");
    assert_eq!(hit.2["name"], "the_one_that_matches");

    pool.close().await;
    teardown_test_db(&db_name).await;
}
