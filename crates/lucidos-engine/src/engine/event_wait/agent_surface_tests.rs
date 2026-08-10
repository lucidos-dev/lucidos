//! Unit tests for the agent's own subscription surface.
//!
//! The pure seams are here: what a subscription looks like to the agent, how
//! the list reads, which `wait_id` / `on` / `all` combinations are legal, and
//! which subscriptions an `on` stop addresses. The
//! engine-level halves (thread scoping and the `AgentStandDown` cause) need a
//! real engine and a real bus, so they live in the e2e-api suite
//! (`crates/lucidos-e2e/tests/api_support/event_wait_test.rs`), which drives
//! the same two routes the CLI calls.

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

/// A live subscription positioned relative to an explicit `now`, so the age
/// assertions below are exact rather than a race with the clock: building the
/// wait and rendering it a microsecond later would floor a 3600s deadline to
/// 59m.
fn wait_at(
    now: DateTime<Utc>,
    secs_ago: i64,
    expires_in_secs: i64,
    on: Vec<EventSubscription>,
) -> LiveWait {
    LiveWait {
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

// ── the view ────────────────────────────────────────────────────────

/// Every field the agent was asked for and could not answer: which one, what
/// it watches, why, and both ages.
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
    // The condition is not hidden: a subscription that watches a slice of an
    // event type behaves differently from one that watches all of it, and the
    // agent has to be able to tell them apart when deciding to re-subscribe.
    assert_eq!(
        view.subscription,
        "ChangeProposed where {\"file_count\":{\"$gt\":0}}"
    );
    assert_eq!(view.armed_ago, "1m");
    assert_eq!(view.expires_in, "1h 0m");
}

/// The two ages are spelled out beside the timestamps because a model is worst
/// at exactly the arithmetic that decides the answer.
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

/// An overdue deadline reads as `0s`, not as a minus sign the model has to
/// interpret. It is a real state: the sweep resolves an expiry up to ten
/// seconds late, so a subscription can genuinely be past its deadline and still
/// live.
#[test]
fn an_overdue_deadline_never_reads_as_negative() {
    assert_eq!(humanize_span(Duration::seconds(-5)), "0s");
    let now = Utc::now();
    let w = wait_at(now, 600, -30, vec![sub("ChangeProposed", None)]);
    assert_eq!(EventWaitView::of(&w, now).expires_in, "0s");
}

// ── the list text ───────────────────────────────────────────────────

/// The empty answer is the one the whole tool exists for. A thread that
/// believes it is watching and is not is the reported bug, so "none" has to say
/// what that means rather than just being empty.
#[test]
fn an_empty_list_says_nothing_will_wake_the_thread() {
    let text = render_event_wait_list(&[]);
    assert!(text.contains("no live subscriptions"), "{text}");
    assert!(
        text.contains("Nothing will wake it"),
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
    // "Each wakes it once, then is spent" is the fact a re-arm decision turns
    // on, and the reason a live list is not the same as a standing watch.
    assert!(text.contains("then is spent"), "{text}");
    // The read hands off to the cancel, including how to stop all of them.
    assert!(text.contains("cancel_event_wait"), "{text}");
    assert!(text.contains("all=true"), "{text}");
}

// ── the cancel arguments ────────────────────────────────────────────

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

/// No silent default is right for a destructive verb: `all` would stop four
/// subscriptions when the agent meant one, and a no-op would report success for
/// nothing.
#[test]
fn a_cancel_with_no_target_is_refused_rather_than_defaulted() {
    let err = resolve_cancel_target(None, None, false).unwrap_err();
    assert!(err.starts_with("Error:"), "{err}");
    assert!(err.contains("wait_id"), "{err}");
    assert!(err.contains("on"), "{err}");
    assert!(err.contains("all"), "{err}");
    // Actionable: it says where to get the id.
    assert!(err.contains("list_event_waits"), "{err}");
}

/// Every pair, and the triple. They address different sets, so more than one is
/// a caller that has not decided which set it means.
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

/// A whitespace-only `on` is the caller's typo, not an event type nothing is
/// watching. Reading it as present would refuse with "nothing is watching for
/// ` `", which reads as a fact about the thread rather than about the call.
#[test]
fn a_blank_on_is_absent_rather_than_an_event_type() {
    let err = resolve_cancel_target(None, Some("   "), false).unwrap_err();
    assert!(err.contains("list_event_waits"), "{err}");
    // And a padded one still names the event type it meant.
    assert_eq!(
        resolve_cancel_target(None, Some("  E2ELockReleased "), false).unwrap(),
        CancelTarget::On("E2ELockReleased".into())
    );
}

// ── which subscriptions an `on` stop addresses ──────────────────────

/// The name only, ignoring any `condition`. "I no longer need to be told about
/// X" means every watch that could fire on an X, whatever slice of it each one
/// asked for: a thread holding a filtered and an unfiltered watch on the same
/// event would otherwise be left with the filtered one still armed.
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

    // A wait watching SEVERAL event types answers yes to each, so a stand-down
    // by one of its names ends the whole thing. Intended, and the sharpest
    // edge on this verb: a wait is one rendezvous with several triggers, spent
    // by the first match, not several watches sharing a row, so there is no
    // `ReleasePublished` leg left to be woken by once the other is gone. The
    // alternative is replacing it with a subscription the caller never armed,
    // and nothing in this family mutates a wait (ADR 0059).
    assert!(among_several.watches("E2ELockReleased"));
    assert!(among_several.watches("ReleasePublished"));
    // What makes that honest rather than silent: the report names every type it
    // ended, so the caller reads that the other leg went with it.
    assert_eq!(
        describe_subscriptions(&among_several.on),
        "ReleasePublished or E2ELockReleased"
    );
    // Exact, not a prefix or a case-fold: event types are exact names
    // everywhere else, and a loose match here would end a watch nobody named.
    assert!(!unfiltered.watches("E2ELock"));
    assert!(!unfiltered.watches("e2elockreleased"));
    assert!(!unfiltered.watches("E2ELockAcquired"));
}

/// The success sentence says what is now true AND what it deliberately left
/// alone, because an agent that reads only the first clause tells the user it
/// stood everything down.
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

// ── thread scoping ──────────────────────────────────────────────────

/// The scoping the whole surface rests on: neither verb takes a thread id, and
/// the cancel re-checks membership under the cache's own lock, so a `wait_id`
/// lifted from another thread resolves nothing.
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

/// The list is scoped the same way, and newest first so a thread that
/// re-subscribed reads its current watch at the top.
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

// ── stopping several of them ────────────────────────────────────────

fn outcome_text(o: CancelEventWaitOutcome) -> (bool, String) {
    match o {
        CancelEventWaitOutcome::Stopped(t) => (true, t),
        CancelEventWaitOutcome::Refused(t) => (false, t),
    }
}

/// The `all` scope's own settled sentence, as `cancel_all_for_agent` passes it.
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

/// The success case is the only thing the two scopes say differently, so an
/// `on` stop reports what it stopped and then its own closing sentence, never
/// `all`'s claim that the thread is now watching nothing.
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

/// **A partial stop is a failure.** An `all` stop is one emit per subscription,
/// so one can fail while the rest land, and a failed one is re-armed and will
/// still wake the thread. Reporting "nothing is subscribed any more" there is
/// the exact lie this whole surface exists to stop the agent telling.
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
    // Names the survivor, so the agent can tell the user which watch is still
    // running rather than which ones it managed to stop.
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

/// Plural agreement, because the sentence is read by a model that will repeat
/// it to a person.
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
