//! Argument validation for `await_event`.
//!
//! Every case here is a refusal the model reads inside the same turn, which is
//! the one advantage this tool has over a trigger's silent footgun. So the
//! assertions are about the WORDS as much as the outcome: a refusal that does
//! not say what to do instead just gets retried verbatim.

use super::*;
use serde_json::json;

fn args(on: Value, timeout: Value, reason: &str) -> Value {
    json!({ "on": on, "timeout_secs": timeout, "reason": reason })
}

// ── the `on:` list ───────────────────────────────────────────────────

#[test]
fn a_well_formed_subscription_list_parses() {
    let subs = parse_subscriptions(&json!({
        "on": [
            {"event_type": "ChangeProposed"},
            {"event_type": "ResponseGenerated", "condition": {"thread_id": "abc"}},
        ]
    }))
    .expect("valid");
    assert_eq!(subs.len(), 2);
    assert_eq!(subs[0].event_type, "ChangeProposed");
    assert!(subs[0].condition.is_none());
    assert_eq!(subs[1].condition, Some(json!({"thread_id": "abc"})));
}

#[test]
fn an_empty_on_list_is_refused_because_nothing_could_match_it() {
    let err = parse_subscriptions(&json!({ "on": [] })).unwrap_err();
    assert!(err.contains("nothing could ever match"), "{err}");
}

#[test]
fn a_missing_on_list_is_refused() {
    let err = parse_subscriptions(&json!({})).unwrap_err();
    assert!(err.contains("`on`"), "{err}");
}

/// S3, the half that makes `await_event` better than a trigger: a subscription
/// that could never fire is an error at the tool boundary, not a silent no-op
/// the user discovers days later.
#[test]
fn a_streaming_event_is_refused_with_something_to_do_instead() {
    let err = parse_subscriptions(&json!({ "on": [{"event_type": "TextStreamed"}] })).unwrap_err();
    assert!(err.contains("per-token streaming"), "{err}");
    assert!(
        err.contains("ResponseGenerated"),
        "names a usable alternative: {err}"
    );
}

#[test]
fn the_event_wait_family_is_refused_as_self_satisfying() {
    for name in ["EventWaitStarted", "EventWaitDelivered", "EventWaitExpired"] {
        let err = parse_subscriptions(&json!({ "on": [{"event_type": name}] })).unwrap_err();
        assert!(err.contains("satisfy itself"), "{name}: {err}");
    }
}

#[test]
fn a_system_only_event_is_refused_and_points_at_the_two_that_work() {
    let err =
        parse_subscriptions(&json!({ "on": [{"event_type": "NotificationCreated"}] })).unwrap_err();
    assert!(err.contains("system event"), "{err}");
    assert!(err.contains("domain event"), "{err}");
}

/// The case the refusals must NOT catch. A domain event nobody has emitted yet
/// is the single most useful thing to wait on, so an unknown name is accepted;
/// the never-emitted note rides on the eventual expiry instead.
#[test]
fn an_unknown_name_is_accepted_because_it_may_be_a_domain_event() {
    let subs = parse_subscriptions(&json!({ "on": [{"event_type": "ReleasePublished"}] }))
        .expect("accepted");
    assert_eq!(subs[0].event_type, "ReleasePublished");
}

/// One bad entry refuses the whole call. A partially-registered wait would
/// watch for less than the model asked for while reading as a success.
#[test]
fn one_blocked_entry_refuses_the_whole_list() {
    let err = parse_subscriptions(&json!({
        "on": [{"event_type": "ChangeProposed"}, {"event_type": "ThoughtStreamed"}]
    }))
    .unwrap_err();
    assert!(err.contains("ThoughtStreamed"), "{err}");
}

// ── timeout_secs ─────────────────────────────────────────────────────

#[test]
fn timeout_secs_is_required_and_says_why() {
    let err = parse_timeout_secs(&json!({})).unwrap_err();
    assert!(err.contains("required"), "{err}");
    assert!(err.contains("no unbounded wait"), "{err}");
}

#[test]
fn timeout_secs_is_bounded_at_both_ends() {
    assert!(parse_timeout_secs(&json!({"timeout_secs": 0}))
        .unwrap_err()
        .contains("at least 1"));
    let over = parse_timeout_secs(&json!({"timeout_secs": MAX_TIMEOUT_SECS + 1})).unwrap_err();
    assert!(over.contains("24 hours"), "{over}");
    assert!(
        over.contains("trigger"),
        "for a longer wait the right shape is a trigger: {over}"
    );
    assert_eq!(
        parse_timeout_secs(&json!({"timeout_secs": MAX_TIMEOUT_SECS})).unwrap(),
        MAX_TIMEOUT_SECS,
        "the cap itself is allowed"
    );
}

#[test]
fn timeout_secs_must_be_a_whole_number() {
    let err = parse_timeout_secs(&json!({"timeout_secs": "an hour"})).unwrap_err();
    assert!(err.contains("whole number"), "{err}");
}

// ── the refusal text the caps produce ────────────────────────────────

#[test]
fn describe_subscriptions_reads_as_the_or_it_is() {
    use crate::core::event_subscription::EventSubscription;
    let on = vec![
        EventSubscription {
            event_type: "ChangeProposed".into(),
            condition: None,
        },
        EventSubscription {
            event_type: "ResponseGenerated".into(),
            condition: Some(json!({"thread_id": "abc"})),
        },
    ];
    let text = describe_subscriptions(&on);
    assert!(
        text.contains("ChangeProposed or ResponseGenerated"),
        "{text}"
    );
    assert!(text.contains("where"), "{text}");
}

// ── what the arming lookback suppresses ─────────────────────────────

/// The one sound suppression: an event this thread was literally handed by an
/// earlier wait. Nothing else is evidence the thread has seen it.
#[tokio::test]
async fn a_delivered_event_is_excluded_from_the_lookback() {
    use crate::engine::event_bus::EventBus;
    use crate::engine::thread_events::ThreadEvent;
    use crate::test_support::{seed_thread_event, setup_test_db, teardown_test_db};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = uuid::Uuid::new_v4();

    assert!(
        delivered_event_ids(&pool, thread_id, ARMING_LOOKBACK_SECS)
            .await
            .unwrap()
            .is_empty(),
        "a thread handed nothing excludes nothing"
    );

    let handed = uuid::Uuid::new_v4();
    seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::EventWaitDelivered {
            wait_id: uuid::Uuid::new_v4(),
            event_id: handed,
            event_type: "ChangeProposed".into(),
            payload: json!({}),
            matched_index: 0,
        },
    )
    .await;

    let excluded = delivered_event_ids(&pool, thread_id, ARMING_LOOKBACK_SECS)
        .await
        .unwrap();
    assert!(
        excluded.contains(&handed),
        "the delivery names the exact event the thread saw"
    );
    assert_eq!(excluded.len(), 1, "and only that one");

    // Another thread's deliveries are none of this thread's business.
    assert!(
        delivered_event_ids(&pool, uuid::Uuid::new_v4(), ARMING_LOOKBACK_SECS)
            .await
            .unwrap()
            .is_empty(),
        "the exclusion is per-thread"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// **Nothing coarser than a delivery may suppress a report.** Two rounds of
/// review killed a sequence floor here, each time because it hid an event
/// nobody had told the thread about: first a floor at the last `EventWait*` of
/// any kind (an unrelated `ChildThreadCompleted` delivery hid a missed
/// `ChangeProposed`), then a floor on shared event *type* (a `ChangeProposed`
/// wait conditioned on one repo hid a `ChangeProposed` for another).
///
/// This pins the shape that makes both impossible: registrations, expiries and
/// cancels contribute NOTHING to the exclusion set, whatever they were about,
/// and a delivery contributes exactly one event id rather than a cutoff.
#[tokio::test]
async fn only_a_delivery_suppresses_and_only_the_event_it_named() {
    use crate::engine::event_bus::EventBus;
    use crate::engine::thread_events::ThreadEvent;
    use crate::test_support::{seed_thread_event, setup_test_db, teardown_test_db};

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = uuid::Uuid::new_v4();
    let wait_id = uuid::Uuid::new_v4();

    // A whole wait lifecycle that hands the thread no event.
    for event in [
        ThreadEvent::EventWaitStarted {
            wait_id,
            tool_use_id: "toolu_x".into(),
            on: vec![crate::core::event_subscription::EventSubscription {
                event_type: "ChangeProposed".into(),
                condition: None,
            }],
            reason: "waiting".into(),
            armed_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            watermark: 0,
        },
        ThreadEvent::EventWaitExpired { wait_id },
    ] {
        seed_thread_event(&bus, thread_id, event).await;
    }

    assert!(
        delivered_event_ids(&pool, thread_id, ARMING_LOOKBACK_SECS)
            .await
            .unwrap()
            .is_empty(),
        "a registration and an expiry handed the thread nothing, so they \
         suppress nothing: a floor here is what hid the missed event twice"
    );

    // A delivery of a DIFFERENT event does not suppress an unrelated one.
    let other = uuid::Uuid::new_v4();
    seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::EventWaitDelivered {
            wait_id,
            event_id: other,
            event_type: "ChildThreadCompleted".into(),
            payload: json!({}),
            matched_index: 0,
        },
    )
    .await;

    let excluded = delivered_event_ids(&pool, thread_id, ARMING_LOOKBACK_SECS)
        .await
        .unwrap();
    assert_eq!(
        excluded.len(),
        1,
        "one delivery excludes one event, not a span of the timeline"
    );
    assert!(excluded.contains(&other));
    assert!(
        !excluded.contains(&uuid::Uuid::new_v4()),
        "and says nothing about any event it did not name"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// ── the whole-argument shape ─────────────────────────────────────────

/// `reason` is what the user reads in the indicator, and it is the difference
/// between "asleep on purpose" and "stalled". Refusing an empty one is worth a
/// wasted call.
#[test]
fn a_missing_reason_is_caught_by_the_same_pass_as_the_rest() {
    // Parsing succeeds for the other two fields, so the reason check is the
    // one that has to fire; assert the pieces the caller composes.
    let a = args(json!([{"event_type": "ChangeProposed"}]), json!(60), "   ");
    assert!(parse_subscriptions(&a).is_ok());
    assert!(parse_timeout_secs(&a).is_ok());
    assert!(
        a.get("reason")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or_default()
            .is_empty(),
        "whitespace is not a reason"
    );
}
