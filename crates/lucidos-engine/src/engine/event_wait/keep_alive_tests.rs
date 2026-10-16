//! The keep-alive selection (I5), its keep-warm filter, and the schedule read
//! back from persisted events alone (I10).

use super::*;
use crate::core::event_subscription::EventSubscription;
use crate::engine::event_bus::EventBus;
use crate::engine::types::{ContextProducer, ContextPurpose};
use crate::llm::model_prices::{cache_shares_of, STANDARD_CACHE};
use crate::test_support::{seed_thread_event, setup_test_db, start_cc_session, teardown_test_db};
use chrono::Duration;

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000, 0).expect("valid")
}

const MINUTE: f64 = 60.0;

/// A thread the selection beats: idle Claude Code on a 5-minute cache, a
/// keep-warm wait armed ten minutes ago, the last request 240 s ago, no beat.
fn due() -> Candidate {
    Candidate {
        coding_agent: CodingAgent::ClaudeCode,
        claim: ThreadClaim::Free,
        awaiting_user: false,
        keep_warm_wait_secs: Some(10.0 * MINUTE),
        children: ActiveChildren::None,
        secs_since_request: Some(240.0),
        secs_since_beat: None,
        cache_ttl: Some(CacheTtl::FiveMinutes),
        cache: cache_shares_of("claude-opus-5-5"),
    }
}

/// [`due`] on a 1-hour cache, its last request 3310 s ago.
fn due_on_the_hour() -> Candidate {
    Candidate {
        cache_ttl: Some(CacheTtl::OneHour),
        secs_since_request: Some(3310.0),
        ..due()
    }
}

/// Where a TTL's beat window starts, in seconds.
fn window_start(ttl: CacheTtl) -> f64 {
    *ttl.beat_window().start()
}

fn skip(reason: SkipReason) -> Beat {
    Beat::Skip(reason)
}

#[test]
fn a_due_thread_beats() {
    assert_eq!(decide(&due()), Beat::Send);
}

#[test]
fn every_condition_can_stop_a_beat() {
    let cases: Vec<(&str, Candidate, Beat)> = vec![
        (
            "a Codex thread",
            Candidate {
                coding_agent: CodingAgent::Codex,
                ..due()
            },
            skip(SkipReason::NotClaudeCode),
        ),
        (
            "a live, held or turning thread",
            Candidate {
                claim: ThreadClaim::Busy,
                ..due()
            },
            skip(SkipReason::Busy),
        ),
        (
            "a question or permission card",
            Candidate {
                awaiting_user: true,
                ..due()
            },
            skip(SkipReason::AwaitingUser),
        ),
        (
            "no keep-warm wait and no children",
            Candidate {
                keep_warm_wait_secs: None,
                ..due()
            },
            skip(SkipReason::NothingToWaitFor),
        ),
        (
            "children that all wait on the user",
            Candidate {
                keep_warm_wait_secs: None,
                children: ActiveChildren::AllAwaitingUser,
                ..due()
            },
            skip(SkipReason::ChildrenAwaitingUser),
        ),
        (
            "a session that never made a request",
            Candidate {
                secs_since_request: None,
                ..due()
            },
            skip(SkipReason::NoRequestYet),
        ),
        (
            "a beat whose request is still on its way",
            Candidate {
                secs_since_beat: Some(30.0),
                ..due()
            },
            skip(SkipReason::BeatInFlight),
        ),
    ];
    for (label, candidate, expected) in cases {
        assert_eq!(decide(&candidate), expected, "{label}");
    }
}

/// The window's edges and the cut-off's edge, each just inside and just out.
#[test]
fn the_window_and_the_cut_off_hold_at_their_boundaries() {
    let at = |secs: f64| Candidate {
        secs_since_request: Some(secs),
        ..due()
    };
    assert_eq!(decide(&at(229.9)), skip(SkipReason::OutsideWindow));
    assert!(decide(&at(230.0)) == Beat::Send);
    assert!(decide(&at(260.0)) == Beat::Send);
    assert_eq!(decide(&at(260.1)), skip(SkipReason::OutsideWindow));

    let beaten = |secs: f64| Candidate {
        secs_since_beat: Some(secs),
        ..due()
    };
    assert_eq!(decide(&beaten(229.0)), skip(SkipReason::BeatInFlight));
    assert!(decide(&beaten(230.0)) == Beat::Send);
}

/// The budget in beats is the write rate over the read rate of the session's
/// model and TTL. The engine gives up that many beat windows after the wait
/// began.
#[test]
fn the_budget_is_the_rewrite_cost_in_beats() {
    let secs = |model: &str, ttl: CacheTtl| ttl.give_up_after_secs(cache_shares_of(model));
    let cases = [
        ("claude-opus-5-5", 25.0, 40.0),
        ("claude-opus-5-5[1m]", 25.0, 40.0),
        ("claude-sonnet-5-5", 25.0, 40.0),
        ("claude-fable-5-1", 50.0, 80.0),
        ("claude-opus-5", 12.5, 20.0),
        ("claude-haiku-4-5", 12.5, 20.0),
        ("a-model-nobody-priced", 12.5, 20.0),
    ];
    for (model, five, hour) in cases {
        let got = (
            secs(model, CacheTtl::FiveMinutes),
            secs(model, CacheTtl::OneHour),
        );
        let want = (
            five * window_start(CacheTtl::FiveMinutes),
            hour * window_start(CacheTtl::OneHour),
        );
        assert!(
            (got.0 - want.0).abs() < 1e-6 && (got.1 - want.1).abs() < 1e-6,
            "{model}: {got:?}, want {want:?}"
        );
    }
}

/// An unknown model and an unknown TTL give the shortest budget there is.
#[test]
fn an_unknown_model_and_ttl_get_the_shortest_budget() {
    let unknown = Candidate {
        cache_ttl: None,
        cache: STANDARD_CACHE,
        ..due()
    };
    let budget = 12.5 * window_start(CacheTtl::FiveMinutes);
    let waiting = |secs: f64| Candidate {
        keep_warm_wait_secs: Some(secs),
        ..unknown.clone()
    };
    assert!(decide(&waiting(budget - 0.1)) == Beat::Send);
    assert_eq!(decide(&waiting(budget + 0.1)), skip(SkipReason::PastCutOff));
}

/// The cut-off's edge for each TTL, just inside and just out. Each budget is
/// beats times the window start.
#[test]
fn the_cut_off_holds_at_the_budget() {
    let cases = [
        (
            "a 5-minute cache",
            due(),
            25.0 * window_start(CacheTtl::FiveMinutes),
        ),
        (
            "a 1-hour cache",
            due_on_the_hour(),
            40.0 * window_start(CacheTtl::OneHour),
        ),
    ];
    for (label, base, budget) in cases {
        let waiting = |secs: f64| Candidate {
            keep_warm_wait_secs: Some(secs),
            ..base.clone()
        };
        assert!(decide(&waiting(budget - 0.1)) == Beat::Send, "{label}");
        assert!(decide(&waiting(budget)) == Beat::Send, "{label}");
        assert_eq!(
            decide(&waiting(budget + 0.1)),
            skip(SkipReason::PastCutOff),
            "{label}"
        );
    }
}

/// A session whose model pays a higher rewrite share beats for longer, and
/// the same session gives up sooner at standard prices.
#[test]
fn the_budget_follows_the_sessions_model() {
    let waiting = |model: &str| Candidate {
        keep_warm_wait_secs: Some(60.0 * MINUTE),
        cache: cache_shares_of(model),
        ..due()
    };
    assert!(decide(&waiting("claude-fable-5-1")) == Beat::Send);
    assert!(decide(&waiting("claude-opus-5-5")) == Beat::Send);
    assert_eq!(
        decide(&waiting("claude-opus-5")),
        skip(SkipReason::PastCutOff),
        "12.5 beats of 230 s end at 47.9 minutes"
    );
}

/// A 1-hour cache beats once, just under the hour, and never in the 5-minute
/// window. An unknown TTL beats as 5 minutes, the side that keeps the cache.
#[test]
fn the_window_follows_the_observed_ttl() {
    let hour = |secs: f64| Candidate {
        secs_since_request: Some(secs),
        ..due_on_the_hour()
    };
    assert_eq!(decide(&hour(210.0)), skip(SkipReason::OutsideWindow));
    assert_eq!(decide(&hour(260.0)), skip(SkipReason::OutsideWindow));
    assert_eq!(decide(&hour(3299.9)), skip(SkipReason::OutsideWindow));
    assert!(decide(&hour(3300.0)) == Beat::Send);
    assert!(decide(&hour(3360.0)) == Beat::Send);
    assert_eq!(decide(&hour(3360.1)), skip(SkipReason::OutsideWindow));

    let unknown = |secs: f64| Candidate {
        cache_ttl: None,
        secs_since_request: Some(secs),
        ..due()
    };
    assert_eq!(decide(&unknown(229.9)), skip(SkipReason::OutsideWindow));
    assert!(decide(&unknown(230.0)) == Beat::Send);
    assert!(decide(&unknown(260.0)) == Beat::Send);
    assert_eq!(decide(&unknown(260.1)), skip(SkipReason::OutsideWindow));
    assert_eq!(decide(&unknown(3300.0)), skip(SkipReason::OutsideWindow));
    let unknown_beaten = Candidate {
        cache_ttl: None,
        secs_since_beat: Some(229.0),
        ..due()
    };
    assert_eq!(decide(&unknown_beaten), skip(SkipReason::BeatInFlight));

    let beaten = |secs: f64| Candidate {
        secs_since_beat: Some(secs),
        ..due_on_the_hour()
    };
    assert_eq!(decide(&beaten(3299.0)), skip(SkipReason::BeatInFlight));
    assert!(decide(&beaten(3300.0)) == Beat::Send);
}

/// The TTL a cache write shows: the shortest lifetime any of its tokens got.
#[test]
fn the_observed_ttl_is_the_shortest_lifetime_written() {
    let cases: [(&str, i64, Option<i64>, Option<CacheTtl>); 5] = [
        ("no write", 0, Some(0), None),
        ("a write with no split", 5_000, None, None),
        (
            "all to the 5-minute cache",
            5_000,
            Some(0),
            Some(CacheTtl::FiveMinutes),
        ),
        (
            "all to the 1-hour cache",
            5_000,
            Some(5_000),
            Some(CacheTtl::OneHour),
        ),
        (
            "some to each",
            5_000,
            Some(4_000),
            Some(CacheTtl::FiveMinutes),
        ),
    ];
    for (label, written, written_1h, expected) in cases {
        assert_eq!(CacheTtl::observed(written, written_1h), expected, "{label}");
    }
}

/// Working children are the second trigger, and the later start wins.
#[test]
fn children_keep_a_parent_warm_and_the_later_start_wins() {
    let parent = Candidate {
        keep_warm_wait_secs: None,
        children: ActiveChildren::Working {
            wait_secs: 20.0 * MINUTE,
        },
        ..due()
    };
    assert!(decide(&parent) == Beat::Send);

    let both = Candidate {
        keep_warm_wait_secs: Some(10.0 * MINUTE),
        children: ActiveChildren::Working {
            wait_secs: 300.0 * MINUTE,
        },
        ..due()
    };
    assert!(
        decide(&both) == Beat::Send,
        "a fresh wait keeps a long-running parent warm"
    );

    let parked = Candidate {
        children: ActiveChildren::AllAwaitingUser,
        ..due()
    };
    assert!(
        decide(&parked) == Beat::Send,
        "a parked child does not cancel the parent's own keep-warm wait"
    );
}

/// A child parked on the user stops its parent's beats. Its answer restarts
/// them only while the parent's last request is young enough to reach the
/// window. Otherwise the cache has expired and the final wake pays the one
/// re-write. Each case is when the answer arrives, in seconds after the
/// parent's last request started, and the tick that beats first.
#[test]
fn an_answer_restarts_the_beats_only_while_the_parents_cache_is_warm() {
    for ttl in [CacheTtl::FiveMinutes, CacheTtl::OneHour] {
        let window = ttl.beat_window();
        let (from, until) = (*window.start(), *window.end());
        let cases: [(&str, f64, Option<f64>); 5] = [
            ("answered at once", 30.0, Some(from)),
            ("answered inside the window", from + 30.0, Some(from + 30.0)),
            ("answered on the window's last tick", until, Some(until)),
            ("answered just after the window", until + 10.0, None),
            ("answered two hours later", 120.0 * MINUTE, None),
        ];
        for (label, answered_at, first_beat) in cases {
            let beat = (0..=400).map(|tick| f64::from(tick) * 10.0).find(|&t| {
                let children = if t < answered_at {
                    ActiveChildren::AllAwaitingUser
                } else {
                    ActiveChildren::Working {
                        wait_secs: 20.0 * MINUTE + t,
                    }
                };
                let parent = Candidate {
                    keep_warm_wait_secs: None,
                    children,
                    secs_since_request: Some(t),
                    cache_ttl: Some(ttl),
                    ..due()
                };
                decide(&parent) == Beat::Send
            });
            assert_eq!(beat, first_beat, "{label} on {ttl:?}");
        }
    }
}

fn wait_on(kinds: &[&str], armed_minutes_ago: i64) -> LiveWait {
    LiveWait {
        wait_id: Uuid::new_v4(),
        thread_id: Uuid::new_v4(),
        tool_use_id: "toolu_test".into(),
        on: kinds
            .iter()
            .map(|k| EventSubscription {
                event_type: (*k).to_string(),
                condition: None,
            })
            .collect(),
        reason: "the suite".into(),
        armed_at: now() - Duration::minutes(armed_minutes_ago),
        expires_at: now() + Duration::hours(1),
        watermark: 0,
    }
}

#[test]
fn only_a_wait_on_keep_warm_kinds_alone_counts() {
    let bash = wait_on(&["BackgroundBashCompleted"], 5);
    let mixed = wait_on(&["BackgroundBashCompleted", "E2ETestsPassed"], 1);
    let other = wait_on(&["E2ETestsPassed"], 1);
    assert_eq!(keep_warm_armed_at([&bash]), Some(bash.armed_at));
    assert_eq!(keep_warm_armed_at([&mixed, &other]), None);
    assert_eq!(
        keep_warm_armed_at([&bash, &mixed]),
        Some(bash.armed_at),
        "an unlisted kind disqualifies its own wait only"
    );
}

/// Each kind must be one a wait can hold. The two lock releases are domain
/// events the e2e and suite scripts emit, which the validator accepts as names
/// it does not own.
#[test]
fn every_keep_warm_kind_is_one_a_wait_can_hold() {
    use crate::core::event_subscription::{validate_subscribable_event_type, SubscriptionSurface};
    for kind in KEEP_WARM_EVENT_TYPES {
        let verdict = validate_subscribable_event_type(kind, SubscriptionSurface::Wait);
        assert!(verdict.is_ok(), "{kind} is refused as a wait: {verdict:?}");
    }
}

/// SQL reads a keep-alive anchor by this constant, so it must be the tag.
#[test]
fn the_reason_kind_constant_is_the_serde_tag() {
    let tagged = serde_json::to_value(EngineReason::CacheKeepAlive).expect("serializes");
    assert_eq!(
        tagged["kind"],
        crate::engine::thread_events::CACHE_KEEP_ALIVE_REASON_KIND
    );
}

fn capture(
    model: &str,
    duration_ms: u64,
    usage: Option<crate::engine::ApiUsage>,
    parent_tool_use_id: Option<&str>,
) -> ThreadEvent {
    ThreadEvent::ContextCaptured {
        producer: ContextProducer::ClaudeCode,
        model: model.to_string(),
        context_window: 0,
        sections: Vec::new(),
        tools: Vec::new(),
        estimated_total_tokens: 0,
        usage,
        trimmed: false,
        trim_passes: Vec::new(),
        purpose: ContextPurpose::Turn,
        reconstructed: false,
        parent_tool_use_id: parent_tool_use_id.map(String::from),
        api_call_id: None,
        reasoning_effort: None,
        duration_ms: Some(duration_ms),
        served_model: None,
        stable_prefix_fingerprint: None,
    }
}

fn turn_capture(duration_ms: u64) -> ThreadEvent {
    capture("claude-opus-5-5", duration_ms, None, None)
}

/// A turn capture that wrote `written` tokens to the cache, `written_1h` of
/// them to the 1-hour cache, for the session or for a sub-agent.
fn capture_writing(written: u32, written_1h: u32, sub_agent: bool) -> ThreadEvent {
    capture_writing_on("claude-opus-5-5", written, written_1h, sub_agent)
}

fn capture_writing_on(model: &str, written: u32, written_1h: u32, sub_agent: bool) -> ThreadEvent {
    let usage = crate::engine::ApiUsage {
        input_tokens: written + 10,
        output_tokens: 5,
        cache_creation_tokens: written,
        cache_creation_1h_tokens: Some(written_1h),
        ..Default::default()
    };
    capture(
        model,
        1_000,
        Some(usage),
        sub_agent.then_some("toolu_agent"),
    )
}

/// The TTL is the latest session write's, read back from the events alone. A
/// read-only request and a sub-agent's write leave it as it was.
#[tokio::test]
async fn the_ttl_comes_from_the_latest_session_write() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/ttl", None).await;
    let ttl = || async { read_candidates(&pool, &[thread_id]).await.unwrap()[0].cache_ttl() };
    assert_eq!(ttl().await, None, "no capture yet");

    seed_thread_event(&bus, thread_id, capture_writing(40_000, 40_000, false)).await;
    assert_eq!(ttl().await, Some(CacheTtl::OneHour));

    seed_thread_event(&bus, thread_id, capture_writing(0, 0, false)).await;
    assert_eq!(ttl().await, Some(CacheTtl::OneHour), "a read-only request");

    seed_thread_event(&bus, thread_id, capture_writing(300, 0, true)).await;
    assert_eq!(ttl().await, Some(CacheTtl::OneHour), "a sub-agent's write");

    seed_thread_event(&bus, thread_id, capture_writing(300, 0, false)).await;
    assert_eq!(ttl().await, Some(CacheTtl::FiveMinutes));

    sqlx::query("UPDATE thread_summaries SET coding_agent = $2 WHERE thread_id = $1")
        .bind(thread_id)
        .bind(CodingAgent::Codex.as_str())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(ttl().await, None, "a Codex thread skips the look-up");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The model is the one that wrote the decisive cache entry: the latest
/// session write, whatever model wrote the earlier ones. A read-only request
/// and a sub-agent's write leave it as it was.
#[tokio::test]
async fn the_model_comes_from_the_latest_session_write() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/model", None).await;
    let model = || async {
        let row = read_candidates(&pool, &[thread_id])
            .await
            .unwrap()
            .remove(0);
        (row.last_write_model.clone(), row.cache())
    };
    assert_eq!(model().await, (None, STANDARD_CACHE), "no capture yet");

    seed_thread_event(
        &bus,
        thread_id,
        capture_writing_on("claude-opus-5", 300, 0, false),
    )
    .await;
    assert_eq!(model().await.1, cache_shares_of("claude-opus-5"));

    seed_thread_event(
        &bus,
        thread_id,
        capture_writing_on("claude-sonnet-5-5[1m]", 300, 0, false),
    )
    .await;
    assert_eq!(
        model().await,
        (
            Some("claude-sonnet-5-5[1m]".to_string()),
            cache_shares_of("claude-sonnet-5-5")
        )
    );

    seed_thread_event(
        &bus,
        thread_id,
        capture_writing_on("claude-fable-5-1", 300, 0, true),
    )
    .await;
    seed_thread_event(
        &bus,
        thread_id,
        capture_writing_on("claude-fable-5-1", 0, 0, false),
    )
    .await;
    assert_eq!(
        model().await.0.as_deref(),
        Some("claude-sonnet-5-5[1m]"),
        "a sub-agent's write and a read-only request"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// After a restart the in-memory beat map is empty, so the schedule must come
/// back from the events alone: the request's start is the capture's `created`
/// minus its duration, and the real idle is the latest `CodingAgentIdled`.
#[tokio::test]
async fn the_schedule_comes_back_from_persisted_events() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/schedule", None).await;
    let capture = seed_thread_event(&bus, thread_id, turn_capture(10_000)).await;
    sqlx::query("UPDATE events SET created = now() - interval '200 seconds' WHERE id = $1")
        .bind(capture)
        .execute(&pool)
        .await
        .unwrap();

    let rows = read_candidates(&pool, &[thread_id]).await.unwrap();
    assert_eq!(rows.len(), 1);
    let secs = rows[0].secs_since_request.expect("a request ran");
    assert!(
        (209.0..=215.0).contains(&secs),
        "200 s since it was written plus its 10 s duration, got {secs}"
    );
    assert_eq!(rows[0].coding_agent.as_deref(), Some("claude-code"));
    assert!(!rows[0].awaiting_user);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn real_idle() -> ThreadEvent {
    ThreadEvent::CodingAgentIdled {
        has_changes: false,
        is_external_repo: false,
        requires_restart: false,
        cc_session_id: None,
        coding_agent: CodingAgent::ClaudeCode,
        reason: None,
        worktree_path: None,
        worktree_head_sha: None,
        bg_bash_pending: false,
    }
}

async fn set_status(pool: &sqlx::PgPool, thread_id: Uuid, status: ThreadStatus) {
    sqlx::query("UPDATE thread_summaries SET status = $2 WHERE thread_id = $1")
        .bind(thread_id)
        .bind(status.as_str())
        .execute(pool)
        .await
        .unwrap();
}

/// A parent's children read back as parked while every active one waits on
/// the user, and as working the moment one of them runs again.
#[tokio::test]
async fn a_child_counts_as_working_only_while_it_runs() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = Uuid::new_v4();
    let (asking, approving) = (Uuid::new_v4(), Uuid::new_v4());
    start_cc_session(&bus, parent, "claude-code/parent", None).await;
    seed_thread_event(&bus, parent, real_idle()).await;
    for child in [asking, approving] {
        start_cc_session(&bus, child, "claude-code/child", None).await;
        sqlx::query("UPDATE thread_summaries SET parent_thread_id = $2 WHERE thread_id = $1")
            .bind(child)
            .bind(parent)
            .execute(&pool)
            .await
            .unwrap();
        set_status(&pool, child, ThreadStatus::WaitingForUserAnswer).await;
    }
    sqlx::query("UPDATE thread_summaries SET active_children_count = 2 WHERE thread_id = $1")
        .bind(parent)
        .execute(&pool)
        .await
        .unwrap();

    let children_of_parent = || async {
        read_candidates(&pool, &[])
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.thread_id == parent)
            .expect("a parent with active children is a candidate")
            .active_children()
    };
    assert_eq!(children_of_parent().await, ActiveChildren::AllAwaitingUser);

    set_status(&pool, approving, ThreadStatus::Running).await;
    assert!(
        matches!(children_of_parent().await, ActiveChildren::Working { wait_secs } if wait_secs < 60.0),
        "an answered card puts the child back to work, timed from the parent's real idle"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
