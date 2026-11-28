use super::*;
use crate::core::event_subscription::EventSubscription;
use crate::engine::event_wait::LiveWaits;
use serde_json::json;

fn sub(event_type: &str, condition: Option<serde_json::Value>) -> EventSubscription {
    EventSubscription {
        event_type: event_type.to_string(),
        condition,
    }
}

fn wait_at(
    now: DateTime<Utc>,
    secs_ago: i64,
    expires_in_secs: i64,
    on: Vec<EventSubscription>,
) -> LiveWait {
    LiveWait {
        background_timing: None,
        wait_id: Uuid::nil(),
        thread_id: Uuid::nil(),
        tool_use_id: "toolu_x".into(),
        on,
        reason: "waiting for the release build to finish".into(),
        armed_at: now - Duration::seconds(secs_ago),
        expires_at: now + Duration::seconds(expires_in_secs),
        watermark: 0,
    }
}

#[test]
fn a_view_carries_what_the_agent_is_asked_about() {
    let now = Utc::now();
    let w = wait_at(
        now,
        90,
        3600,
        vec![sub(
            "ChangeProposed",
            Some(json!({"file_count": {"$gt": 0}})),
        )],
    );
    let view = EventWaitView::of(&w, now);

    assert_eq!(view.wait_id, w.wait_id);
    assert_eq!(view.reason, "waiting for the release build to finish");
    assert_eq!(
        view.subscription,
        "ChangeProposed where {\"file_count\":{\"$gt\":0}}"
    );
    assert_eq!(view.armed_ago, "1m");
    assert_eq!(view.expires_in, "1h 0m");
}

#[test]
fn a_span_reads_at_the_granularity_that_decides_something() {
    assert_eq!(humanize_span(Duration::seconds(0)), "0s");
    assert_eq!(humanize_span(Duration::seconds(18)), "18s");
    assert_eq!(humanize_span(Duration::seconds(59)), "59s");
    assert_eq!(humanize_span(Duration::seconds(60)), "1m");
    assert_eq!(humanize_span(Duration::minutes(59)), "59m");
    assert_eq!(humanize_span(Duration::minutes(125)), "2h 5m");
    assert_eq!(humanize_span(Duration::hours(27)), "1d 3h");
}

#[test]
fn an_overdue_deadline_never_reads_as_negative() {
    assert_eq!(humanize_span(Duration::seconds(-5)), "0s");
    let now = Utc::now();
    let w = wait_at(now, 600, -30, vec![sub("ChangeProposed", None)]);
    assert_eq!(EventWaitView::of(&w, now).expires_in, "0s");
}

#[test]
fn an_empty_list_says_nothing_will_reopen_the_thread() {
    let text = render_event_wait_list(&[]);
    assert!(text.contains("no live subscriptions"), "{text}");
    assert!(
        text.contains("Nothing will re-open it"),
        "the consequence, not just the count: {text}"
    );
    assert!(
        text.contains("no longer true"),
        "it must name the mistake it is there to prevent: {text}"
    );
}

#[test]
fn the_list_names_each_subscription_its_reason_and_both_ages() {
    let now = Utc::now();
    let views = vec![
        EventWaitView::of(
            &wait_at(now, 90, 3600, vec![sub("ChangeProposed", None)]),
            now,
        ),
        EventWaitView::of(
            &wait_at(now, 30, 120, vec![sub("ReleasePublished", None)]),
            now,
        ),
    ];
    let text = render_event_wait_list(&views);

    assert!(text.contains("2 live subscription(s)"), "{text}");
    assert!(text.contains("ChangeProposed"), "{text}");
    assert!(text.contains("ReleasePublished"), "{text}");
    assert!(
        text.contains("waiting for the release build to finish"),
        "{text}"
    );
    assert!(text.contains("armed 1m ago, times out in 1h 0m"), "{text}");
    assert!(text.contains("then is spent"), "{text}");
    assert!(text.contains("cancel_event_wait"), "{text}");
    assert!(text.contains("all=true"), "{text}");
}

#[test]
fn a_cancel_names_one_subscription_an_event_type_or_all_of_them() {
    let id = Uuid::new_v4();
    assert_eq!(
        resolve_cancel_target(Some(id), None, false).unwrap(),
        CancelTarget::One(id)
    );
    assert_eq!(
        resolve_cancel_target(None, Some("E2ELockReleased"), false).unwrap(),
        CancelTarget::On("E2ELockReleased".into())
    );
    assert_eq!(
        resolve_cancel_target(None, None, true).unwrap(),
        CancelTarget::All
    );
}

#[test]
fn a_cancel_with_no_target_is_refused_rather_than_defaulted() {
    let err = resolve_cancel_target(None, None, false).unwrap_err();
    assert!(err.starts_with("Error:"), "{err}");
    assert!(err.contains("wait_id"), "{err}");
    assert!(err.contains("on"), "{err}");
    assert!(err.contains("all"), "{err}");
    assert!(err.contains("list_event_waits"), "{err}");
}

#[test]
fn a_cancel_naming_several_targets_is_refused_as_ambiguous() {
    let id = Uuid::new_v4();
    for (wait_id, on, all) in [
        (Some(id), None, true),
        (Some(id), Some("E2ELockReleased"), false),
        (None, Some("E2ELockReleased"), true),
        (Some(id), Some("E2ELockReleased"), true),
    ] {
        let err = resolve_cancel_target(wait_id, on, all).unwrap_err();
        assert!(err.contains("exactly one"), "{err}");
    }
}

#[test]
fn a_blank_on_is_absent_rather_than_an_event_type() {
    let err = resolve_cancel_target(None, Some("   "), false).unwrap_err();
    assert!(err.contains("list_event_waits"), "{err}");
    assert_eq!(
        resolve_cancel_target(None, Some("  E2ELockReleased "), false).unwrap(),
        CancelTarget::On("E2ELockReleased".into())
    );
}

#[test]
fn an_on_stop_addresses_every_watch_for_that_event_whatever_it_filters() {
    let now = Utc::now();
    let unfiltered = wait_at(now, 10, 600, vec![sub("E2ELockReleased", None)]);
    let filtered = wait_at(
        now,
        10,
        600,
        vec![sub("E2ELockReleased", Some(json!({"outcome": "released"})))],
    );
    let among_several = wait_at(
        now,
        10,
        600,
        vec![sub("ReleasePublished", None), sub("E2ELockReleased", None)],
    );
    let unrelated = wait_at(now, 10, 600, vec![sub("ChangeProposed", None)]);

    assert!(unfiltered.watches("E2ELockReleased"));
    assert!(filtered.watches("E2ELockReleased"));
    assert!(!unrelated.watches("E2ELockReleased"));

    assert!(among_several.watches("E2ELockReleased"));
    assert!(among_several.watches("ReleasePublished"));
    assert_eq!(
        describe_subscriptions(&among_several.on),
        "ReleasePublished or E2ELockReleased"
    );
    assert!(!unfiltered.watches("E2ELock"));
    assert!(!unfiltered.watches("e2elockreleased"));
    assert!(!unfiltered.watches("E2ELockAcquired"));
}

#[test]
fn an_on_stop_counts_the_watches_it_left_standing() {
    let none_left = on_stop_settled("E2ELockReleased", 0);
    assert!(none_left.contains("Nothing on this thread watches E2ELockReleased"));
    assert!(
        !none_left.contains("other subscription"),
        "a thread with nothing else live says so by their absence: {none_left}"
    );
    assert!(on_stop_settled("E2ELockReleased", 1)
        .contains("1 other subscription(s) on this thread is still live"));
    assert!(on_stop_settled("E2ELockReleased", 3)
        .contains("3 other subscription(s) on this thread are still live"));
}

#[tokio::test]
async fn a_wait_id_from_another_thread_cannot_be_taken() {
    let waits = LiveWaits::new();
    let mine = Uuid::new_v4();
    let theirs = Uuid::new_v4();
    let mut theirs_wait = wait_at(Utc::now(), 10, 600, vec![sub("ChangeProposed", None)]);
    theirs_wait.wait_id = Uuid::new_v4();
    theirs_wait.thread_id = theirs;
    waits.insert(theirs_wait.clone()).await;

    assert!(
        waits
            .take_on_thread(mine, theirs_wait.wait_id)
            .await
            .is_none(),
        "another thread's subscription must not be reachable by id"
    );
    assert_eq!(
        waits.for_thread(theirs).await.len(),
        1,
        "and it is still live afterwards"
    );
    assert!(waits
        .take_on_thread(theirs, theirs_wait.wait_id)
        .await
        .is_some());
}

#[tokio::test]
async fn the_list_is_per_thread_and_newest_first() {
    let waits = LiveWaits::new();
    let mine = Uuid::new_v4();
    for (secs_ago, event_type) in [(300, "Older"), (10, "Newer")] {
        let mut w = wait_at(Utc::now(), secs_ago, 600, vec![sub(event_type, None)]);
        w.wait_id = Uuid::new_v4();
        w.thread_id = mine;
        waits.insert(w).await;
    }
    let mut other = wait_at(Utc::now(), 5, 600, vec![sub("SomeoneElse", None)]);
    other.wait_id = Uuid::new_v4();
    other.thread_id = Uuid::new_v4();
    waits.insert(other).await;

    let mut mine_waits = waits.for_thread(mine).await;
    mine_waits.sort_by(|a, b| b.armed_at.cmp(&a.armed_at));
    let names: Vec<&str> = mine_waits
        .iter()
        .map(|w| w.on[0].event_type.as_str())
        .collect();
    assert_eq!(names, vec!["Newer", "Older"]);
}

fn outcome_text(o: CancelEventWaitOutcome) -> (bool, String) {
    match o {
        CancelEventWaitOutcome::Stopped(t) => (true, t),
        CancelEventWaitOutcome::Refused(t) => (false, t),
    }
}

const ALL_SETTLED: &str = "Nothing is subscribed on this thread any more.";

#[test]
fn stopping_all_of_them_reports_success_only_when_nothing_is_left() {
    let (ok, text) = outcome_text(stop_outcome(
        &["ChangeProposed".into(), "ReleasePublished".into()],
        &[],
        ALL_SETTLED,
    ));
    assert!(ok);
    assert!(text.contains("ChangeProposed"), "{text}");
    assert!(text.contains("ReleasePublished"), "{text}");
    assert!(text.contains("Nothing is subscribed"), "{text}");
}

#[test]
fn stopping_by_event_type_never_claims_the_thread_is_watching_nothing() {
    let (ok, text) = outcome_text(stop_outcome(
        &["E2ELockReleased".into()],
        &[],
        &on_stop_settled("E2ELockReleased", 2),
    ));
    assert!(ok);
    assert!(
        text.contains("Stopped watching for E2ELockReleased"),
        "{text}"
    );
    assert!(
        !text.contains("Nothing is subscribed"),
        "two other watches are still live: {text}"
    );
    assert!(text.contains("2 other subscription(s)"), "{text}");
}

#[test]
fn a_partial_stop_is_refused_and_names_what_is_still_running() {
    let (ok, text) = outcome_text(stop_outcome(
        &["A".into(), "B".into(), "C".into()],
        &["B".into()],
        ALL_SETTLED,
    ));
    assert!(!ok, "a subscription that is still live is not a success");
    assert!(text.starts_with("Error:"), "{text}");
    assert!(text.contains("stopped 2 of 3"), "{text}");
    assert!(text.contains("is still live"), "{text}");
    assert!(text.contains("B"), "{text}");
}

#[test]
fn a_total_failure_says_every_subscription_is_still_running() {
    let (ok, text) = outcome_text(stop_outcome(
        &["A".into(), "B".into()],
        &["A".into(), "B".into()],
        ALL_SETTLED,
    ));
    assert!(!ok);
    assert!(text.contains("still live"), "{text}");
    assert!(
        !text.contains("stopped 0 of"),
        "nothing landed, so say so plainly: {text}"
    );
}

#[test]
fn a_partial_stop_agrees_with_how_many_survived() {
    let (_, one) = outcome_text(stop_outcome(
        &["A".into(), "B".into()],
        &["B".into()],
        ALL_SETTLED,
    ));
    assert!(
        one.contains("1 could not be recorded and is still live"),
        "{one}"
    );
    let (_, many) = outcome_text(stop_outcome(
        &["A".into(), "B".into(), "C".into()],
        &["B".into(), "C".into()],
        ALL_SETTLED,
    ));
    assert!(
        many.contains("2 could not be recorded and are still live"),
        "{many}"
    );
}
