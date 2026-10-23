//! The keep-alive selection (I5), its keep-warm filter, and the schedule read
//! back from persisted events alone (I10).

use super::*;
use crate::core::event_subscription::EventSubscription;
use crate::core::prefs::PrefDefault;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::WakeMode;
use crate::engine::types::{ContextProducer, ContextPurpose};
use crate::llm::model_prices::{cache_shares_of, price_table};
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
        agent: Agent::ClaudeCode,
        claim: ThreadClaim::Free,
        awaiting_user: false,
        parked: None,
        keep_warm_wait_secs: Some(10.0 * MINUTE),
        keep_warm_kinds: kinds(&["BackgroundBashCompleted"]),
        children: ActiveChildren::None,
        secs_since_request: Some(240.0),
        secs_since_beat: None,
        cache_ttl: Some(CacheTtl::FiveMinutes),
        cache: cache_shares_of("claude-opus-5-5"),
        turned_off_at: None,
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

/// A session parked on a question card it asked about with the request 240 s
/// ago. Its session still holds the thread, and a copy would share its prefix.
fn parked() -> Candidate {
    Candidate {
        claim: ThreadClaim::Busy,
        awaiting_user: true,
        parked: Some(Parked {
            card: ParkedCard::Question,
            secs: 240.0,
            copy_ready: true,
        }),
        keep_warm_wait_secs: None,
        ..due()
    }
}

fn kinds(kinds: &[&str]) -> KeepWarmKinds {
    KeepWarmKinds::of(kinds.iter().copied()).expect("keep-warm kinds")
}

/// Where a TTL's beat window starts, in seconds.
fn window_start(ttl: CacheTtl) -> f64 {
    *ttl.beat_window(BeatStart::Cold).start()
}

fn skip(reason: SkipReason) -> Beat {
    Beat::Skip(reason)
}

/// The verdict under the `default` rule, which most cases here pin.
fn decide(c: &Candidate) -> Beat {
    super::decide(c, GiveUpRule::Default)
}

/// The status under the `default` rule.
fn status(c: &Candidate, at: &Anchors, spent: Spent) -> Option<CacheKeepAliveStatus> {
    super::status(c, GiveUpRule::Default, at, spent)
}

#[test]
fn a_due_thread_beats() {
    assert_eq!(decide(&due()), Beat::Send(BeatRoute::Resume));
}

#[test]
fn every_condition_can_stop_a_beat() {
    let cases: Vec<(&str, Candidate, Beat)> = vec![
        (
            "a Codex thread",
            Candidate {
                agent: Agent::Codex,
                ..due()
            },
            skip(SkipReason::Codex),
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
    assert!(decide(&at(230.0)) == Beat::Send(BeatRoute::Resume));
    assert!(decide(&at(260.0)) == Beat::Send(BeatRoute::Resume));
    assert_eq!(decide(&at(260.1)), skip(SkipReason::OutsideWindow));

    let beaten = |secs: f64| Candidate {
        secs_since_beat: Some(secs),
        ..due()
    };
    assert_eq!(decide(&beaten(229.0)), skip(SkipReason::BeatInFlight));
    assert!(decide(&beaten(230.0)) == Beat::Send(BeatRoute::Resume));
}

/// The budget in beats is the write rate over the read rate of the session's
/// model and TTL. The engine gives up that many beat windows after the wait
/// began.
#[test]
fn the_budget_is_the_rewrite_cost_in_beats() {
    let secs = |model: &str, ttl: CacheTtl| {
        let c = Candidate {
            cache_ttl: Some(ttl),
            cache: cache_shares_of(model),
            ..due()
        };
        give_up_after_secs(&c, GiveUpRule::Default).expect("a budget")
    };
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
        cache: price_table().unpriced_cache_shares,
        ..due()
    };
    let budget = 12.5 * window_start(CacheTtl::FiveMinutes);
    let waiting = |secs: f64| Candidate {
        keep_warm_wait_secs: Some(secs),
        ..unknown.clone()
    };
    assert!(decide(&waiting(budget - 0.1)) == Beat::Send(BeatRoute::Resume));
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
        assert!(
            decide(&waiting(budget - 0.1)) == Beat::Send(BeatRoute::Resume),
            "{label}"
        );
        assert!(
            decide(&waiting(budget)) == Beat::Send(BeatRoute::Resume),
            "{label}"
        );
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
    assert!(decide(&waiting("claude-fable-5-1")) == Beat::Send(BeatRoute::Resume));
    assert!(decide(&waiting("claude-opus-5-5")) == Beat::Send(BeatRoute::Resume));
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
    assert!(decide(&hour(3300.0)) == Beat::Send(BeatRoute::Resume));
    assert!(decide(&hour(3360.0)) == Beat::Send(BeatRoute::Resume));
    assert_eq!(decide(&hour(3360.1)), skip(SkipReason::OutsideWindow));

    let unknown = |secs: f64| Candidate {
        cache_ttl: None,
        secs_since_request: Some(secs),
        ..due()
    };
    assert_eq!(decide(&unknown(229.9)), skip(SkipReason::OutsideWindow));
    assert!(decide(&unknown(230.0)) == Beat::Send(BeatRoute::Resume));
    assert!(decide(&unknown(260.0)) == Beat::Send(BeatRoute::Resume));
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
    assert!(decide(&beaten(3300.0)) == Beat::Send(BeatRoute::Resume));
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
    assert!(decide(&parent) == Beat::Send(BeatRoute::Resume));

    let both = Candidate {
        keep_warm_wait_secs: Some(10.0 * MINUTE),
        children: ActiveChildren::Working {
            wait_secs: 300.0 * MINUTE,
        },
        ..due()
    };
    assert!(
        decide(&both) == Beat::Send(BeatRoute::Resume),
        "a fresh wait keeps a long-running parent warm"
    );

    let parked = Candidate {
        children: ActiveChildren::AllAwaitingUser,
        ..due()
    };
    assert!(
        decide(&parked) == Beat::Send(BeatRoute::Resume),
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
        let window = ttl.beat_window(BeatStart::Cold);
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
                decide(&parent) == Beat::Send(BeatRoute::Resume)
            });
            assert_eq!(beat, first_beat, "{label} on {ttl:?}");
        }
    }
}

// --- A session parked on a card (I1, I2) ---

fn parked_for(secs: f64) -> Candidate {
    Candidate {
        parked: parked().parked.map(|p| Parked { secs, ..p }),
        ..parked()
    }
}

#[test]
fn a_parked_session_beats_through_a_copy() {
    assert_eq!(decide(&parked()), Beat::Send(BeatRoute::SessionCopy));
    let permission = Candidate {
        parked: parked().parked.map(|p| Parked {
            card: ParkedCard::Permission,
            ..p
        }),
        ..parked()
    };
    assert_eq!(decide(&permission), Beat::Send(BeatRoute::SessionCopy));
}

/// Whatever its ages, a parked session is never resumed: a turn would
/// supersede its card (ADR 0082).
#[test]
fn a_parked_session_is_never_resumed() {
    for parked_secs in (0..=200).map(|m| f64::from(m) * MINUTE) {
        for request in (0..=40).map(|t| f64::from(t) * 10.0) {
            let c = Candidate {
                secs_since_request: Some(request),
                ..parked_for(parked_secs)
            };
            assert_ne!(decide(&c), Beat::Send(BeatRoute::Resume), "{c:?}");
        }
    }
}

#[test]
fn every_condition_can_stop_a_parked_beat() {
    let cases: Vec<(&str, Candidate, Beat)> = vec![
        (
            "a Codex thread",
            Candidate {
                agent: Agent::Codex,
                ..parked()
            },
            skip(SkipReason::Codex),
        ),
        (
            "a park that outlived its session",
            Candidate {
                claim: ThreadClaim::Free,
                ..parked()
            },
            skip(SkipReason::AwaitingUser),
        ),
        (
            "a session whose appended prompt the engine forgot",
            Candidate {
                parked: parked().parked.map(|p| Parked {
                    copy_ready: false,
                    ..p
                }),
                ..parked()
            },
            skip(SkipReason::NoCopyPrompt),
        ),
        (
            "a card that is not a Claude Code park",
            Candidate {
                parked: None,
                ..parked()
            },
            skip(SkipReason::Busy),
        ),
        (
            "a copy whose request is still on its way",
            Candidate {
                secs_since_beat: Some(30.0),
                ..parked()
            },
            skip(SkipReason::BeatInFlight),
        ),
        (
            "a refresh younger than the window",
            Candidate {
                secs_since_request: Some(100.0),
                ..parked()
            },
            skip(SkipReason::OutsideWindow),
        ),
    ];
    for (label, candidate, expected) in cases {
        assert_eq!(decide(&candidate), expected, "{label}");
    }
}

/// A parked wait gives up at half the break-even budget, timed from the
/// request that asked. Human answers thin out with time, so later beats mostly
/// pay for cards nobody answers.
#[test]
fn a_parked_wait_gives_up_at_half_the_break_even_budget() {
    let cases = [
        ("a 5-minute cache", CacheTtl::FiveMinutes, 240.0, 12.5),
        ("a 1-hour cache", CacheTtl::OneHour, 3310.0, 20.0),
    ];
    for (label, ttl, request, beats) in cases {
        let budget = beats * window_start(ttl);
        let waiting = |secs: f64| Candidate {
            cache_ttl: Some(ttl),
            secs_since_request: Some(request),
            ..parked_for(secs)
        };
        let copy = Beat::Send(BeatRoute::SessionCopy);
        assert_eq!(decide(&waiting(budget)), copy, "{label}");
        assert_eq!(
            decide(&waiting(budget + 0.1)),
            skip(SkipReason::PastCutOff),
            "{label}"
        );
    }
}

/// `off` beats nothing, on any wait.
#[test]
fn off_beats_nothing() {
    let working = Candidate {
        keep_warm_wait_secs: None,
        children: ActiveChildren::Working {
            wait_secs: 20.0 * MINUTE,
        },
        ..due()
    };
    for c in [due(), due_on_the_hour(), parked(), working] {
        assert_eq!(
            super::decide(&c, GiveUpRule::Off),
            skip(SkipReason::Disabled),
            "{c:?}"
        );
    }
}

/// Sixty bash waits that each needed two beats: `workspace-data` caps that
/// type at two.
fn short_bash_waits() -> WaitDurations {
    let bash = WaitType::Events(kinds(&["BackgroundBashCompleted"]));
    let secs = 2.5 * window_start(CacheTtl::FiveMinutes);
    WaitDurations::from_samples((0..60).map(|_| (WaitAgent::ClaudeCode, bash, secs)))
}

/// `workspace-data` gives up at the cap this workspace's own waits price
/// cheapest. A type with too few waits keeps the default.
#[test]
fn workspace_data_gives_up_at_the_tuned_cap() {
    let beat_every = window_start(CacheTtl::FiveMinutes);
    let question = WaitType::Parked(ParkedCard::Question);
    let cards = WaitDurations::from_samples(
        (0..60).map(|_| (WaitAgent::ClaudeCode, question, 3.5 * beat_every)),
    );
    let rule = GiveUpRule::WorkspaceData(&cards);
    let copy = Beat::Send(BeatRoute::SessionCopy);
    assert_eq!(super::decide(&parked_for(3.0 * beat_every), rule), copy);
    assert_eq!(
        super::decide(&parked_for(3.0 * beat_every + 0.1), rule),
        skip(SkipReason::PastCutOff)
    );

    for minutes in 0..=200 {
        let c = Candidate {
            keep_warm_wait_secs: Some(f64::from(minutes) * MINUTE),
            ..due()
        };
        assert_eq!(super::decide(&c, rule), decide(&c), "{c:?}");
    }

    let bash = short_bash_waits();
    let tuned = GiveUpRule::WorkspaceData(&bash);
    let waiting = |secs: f64| Candidate {
        keep_warm_wait_secs: Some(secs),
        ..due()
    };
    assert_eq!(
        super::decide(&waiting(2.0 * beat_every), tuned),
        Beat::Send(BeatRoute::Resume)
    );
    assert_eq!(
        super::decide(&waiting(2.0 * beat_every + 0.1), tuned),
        skip(SkipReason::PastCutOff)
    );
}

/// The type is the one of the wait the clock measures: the card when parked,
/// else the newer of the event wait and the working children.
#[test]
fn the_wait_type_is_the_measured_waits() {
    let bash = WaitType::Events(kinds(&["BackgroundBashCompleted"]));
    let children = WaitType::Events(kinds(&["ChildThreadCompleted"]));
    let working = |wait_secs: f64| ActiveChildren::Working { wait_secs };
    let cases: Vec<(&str, Candidate, Option<WaitType>)> = vec![
        ("an event wait", due(), Some(bash)),
        (
            "a question card",
            parked(),
            Some(WaitType::Parked(ParkedCard::Question)),
        ),
        (
            "children alone",
            Candidate {
                keep_warm_wait_secs: None,
                children: working(MINUTE),
                ..due()
            },
            Some(children),
        ),
        (
            "an event wait newer than the children's",
            Candidate {
                children: working(20.0 * MINUTE),
                ..due()
            },
            Some(bash),
        ),
        (
            "children newer than the event wait",
            Candidate {
                children: working(MINUTE),
                ..due()
            },
            Some(children),
        ),
        (
            "nothing worth keeping warm",
            Candidate {
                keep_warm_wait_secs: None,
                ..due()
            },
            None,
        ),
    ];
    for (label, candidate, expected) in cases {
        assert_eq!(wait_type(&candidate), expected, "{label}");
    }
}

#[test]
fn the_mode_reads_back_its_catalog_values() {
    for value in CACHE_KEEP_ALIVE_MODES {
        assert_eq!(CacheKeepAliveMode::parse(value).as_str(), *value);
    }
    assert_eq!(
        CacheKeepAliveMode::parse("sometimes"),
        CacheKeepAliveMode::Default
    );
}

#[test]
fn the_catalog_default_is_the_default_mode() {
    let PrefDefault::Value(value) = crate::core::prefs::CACHE_KEEP_ALIVE_MODE.spec.default else {
        panic!("the mode preference has a value default");
    };
    assert_eq!(
        CacheKeepAliveMode::parse(value),
        CacheKeepAliveMode::Default
    );
}

#[test]
fn each_mode_picks_its_rule() {
    let bash = short_bash_waits();
    let c = Candidate {
        keep_warm_wait_secs: Some(2.0 * window_start(CacheTtl::FiveMinutes) + 0.1),
        ..due()
    };
    let decide_under = |mode| super::decide(&c, GiveUpRule::of(mode, &bash));
    assert_eq!(decide_under(CacheKeepAliveMode::Default), decide(&c));
    assert_eq!(
        decide_under(CacheKeepAliveMode::WorkspaceData),
        skip(SkipReason::PastCutOff)
    );
    assert_eq!(
        decide_under(CacheKeepAliveMode::Off),
        super::decide(&c, GiveUpRule::Off)
    );
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
    let bash_wait = Some((bash.armed_at, kinds(&["BackgroundBashCompleted"])));
    assert_eq!(newest_keep_warm_wait([&bash]), bash_wait);
    assert_eq!(newest_keep_warm_wait([&mixed, &other]), None);
    assert_eq!(
        newest_keep_warm_wait([&bash, &mixed]),
        bash_wait,
        "an unlisted kind disqualifies its own wait only"
    );
}

/// The newest keep-warm wait names the wait type, whatever the older ones
/// watch. Two waits on the same kinds share a type, in any order.
#[test]
fn the_newest_keep_warm_wait_names_the_kinds() {
    let lock = wait_on(&["E2ELockReleased"], 9);
    let agent = wait_on(&["ChangeApplied", "CodingAgentIdled"], 2);
    assert_eq!(
        newest_keep_warm_wait([&lock, &agent]).map(|(_, k)| k),
        Some(kinds(&["CodingAgentIdled", "ChangeApplied"]))
    );
    assert_eq!(KeepWarmKinds::of([]), None, "a wait on nothing");
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
        provider: None,
        stable_prefix_fingerprint: None,
        ttft_ms: None,
        request_id: None,
        session: None,
    }
}

/// A main-session turn capture that reports an input size, as every real one does.
fn turn_capture(duration_ms: u64) -> ThreadEvent {
    let usage = crate::engine::ApiUsage {
        input_tokens: 1,
        ..Default::default()
    };
    capture("claude-opus-5-5", duration_ms, Some(usage), None)
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
    assert_eq!(
        model().await,
        (None, price_table().unpriced_cache_shares),
        "no capture yet"
    );

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

/// A `no-wake` child's clean finish never wakes its parent, so it keeps
/// nobody warm.
#[tokio::test]
async fn a_no_wake_child_keeps_nobody_warm() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = (Uuid::new_v4(), Uuid::new_v4());
    start_cc_session(&bus, parent, "claude-code/parent", None).await;
    seed_thread_event(&bus, parent, real_idle()).await;
    start_cc_session(&bus, child, "claude-code/child", None).await;
    sqlx::query(
        "UPDATE thread_summaries SET parent_thread_id = $2, wake_mode = $3 WHERE thread_id = $1",
    )
    .bind(child)
    .bind(parent)
    .bind(WakeMode::NoWake.as_str())
    .execute(&pool)
    .await
    .unwrap();
    set_status(&pool, child, ThreadStatus::Running).await;
    sqlx::query("UPDATE thread_summaries SET active_children_count = 1 WHERE thread_id = $1")
        .bind(parent)
        .execute(&pool)
        .await
        .unwrap();

    let children_of_parent = |wake_mode: WakeMode| {
        let pool = pool.clone();
        async move {
            sqlx::query("UPDATE thread_summaries SET wake_mode = $2 WHERE thread_id = $1")
                .bind(child)
                .bind(wake_mode.as_str())
                .execute(&pool)
                .await
                .unwrap();
            read_candidates(&pool, &[])
                .await
                .unwrap()
                .into_iter()
                .find(|r| r.thread_id == parent)
                .map(|r| r.active_children())
        }
    };
    assert_eq!(
        children_of_parent(WakeMode::NoWake).await,
        Some(ActiveChildren::OnlyNoWake),
        "the parent waits, and nothing would read its warm cache"
    );
    for waking in [WakeMode::Wake, WakeMode::WakeWhenAllDone] {
        assert!(
            matches!(
                children_of_parent(waking).await,
                Some(ActiveChildren::Working { .. })
            ),
            "a {} child keeps its parent warm",
            waking.as_str()
        );
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// --- The cache keep-alive status (I3, I4, I6) ---

fn secs_before_now(secs: f64) -> DateTime<Utc> {
    now() - Duration::milliseconds((secs * 1000.0) as i64)
}

/// The anchors a candidate's ages were measured from, at [`now`].
fn anchors_of(c: &Candidate) -> Anchors {
    let idle_secs = match c.children {
        ActiveChildren::Working { wait_secs } => Some(wait_secs),
        _ => None,
    };
    Anchors {
        keep_warm_armed_at: c.keep_warm_wait_secs.map(secs_before_now),
        idle_at: idle_secs.map(secs_before_now),
        request_started_at: c.secs_since_request.map(secs_before_now),
        turn_started_at: c.parked.map(|p| secs_before_now(p.secs)),
    }
}

/// What a status says, without its numbers.
#[derive(Debug, PartialEq)]
enum StatusKind {
    Absent,
    Warming,
    Stopped,
    NotKept(CacheKeepAliveNotKeptReason),
}

fn kind_of(status: &Option<CacheKeepAliveStatus>) -> StatusKind {
    match status {
        None => StatusKind::Absent,
        Some(CacheKeepAliveStatus::Warming { .. }) => StatusKind::Warming,
        Some(CacheKeepAliveStatus::Stopped { .. }) => StatusKind::Stopped,
        Some(CacheKeepAliveStatus::NotKept { reason }) => StatusKind::NotKept(*reason),
    }
}

/// The status each verdict must come with. A skip that only waits for the
/// next beat keeps the thread warming. `off` and a parked thread show none.
/// A claim hides the rest of the verdict, so a busy thread takes the status of
/// its free twin.
fn status_kind_for(rule: GiveUpRule, c: &Candidate, beat: &Beat) -> StatusKind {
    if matches!(rule, GiveUpRule::Off) || c.awaiting_user {
        return StatusKind::Absent;
    }
    match beat {
        Beat::Skip(SkipReason::Busy) => {
            let free = Candidate {
                claim: ThreadClaim::Free,
                ..c.clone()
            };
            status_kind_for(rule, &free, &super::decide(&free, rule))
        }
        Beat::Send(_)
        | Beat::Skip(
            SkipReason::NoRequestYet | SkipReason::OutsideWindow | SkipReason::BeatInFlight,
        ) => StatusKind::Warming,
        Beat::Skip(SkipReason::PastCutOff) => StatusKind::Stopped,
        Beat::Skip(SkipReason::ChildrenAwaitingUser) => {
            StatusKind::NotKept(CacheKeepAliveNotKeptReason::ChildrenAwaitingUser)
        }
        Beat::Skip(SkipReason::OnlyNoWakeChildren) => {
            StatusKind::NotKept(CacheKeepAliveNotKeptReason::NoWakeChildren)
        }
        Beat::Skip(SkipReason::TurnedOff) => {
            StatusKind::NotKept(CacheKeepAliveNotKeptReason::TurnedOff)
        }
        Beat::Skip(SkipReason::UncacheableProvider) => {
            StatusKind::NotKept(CacheKeepAliveNotKeptReason::UncacheableProvider)
        }
        Beat::Skip(SkipReason::Restarted) => {
            StatusKind::NotKept(CacheKeepAliveNotKeptReason::Restarted)
        }
        Beat::Skip(
            SkipReason::Codex
            | SkipReason::Disabled
            | SkipReason::AwaitingUser
            | SkipReason::NoCopyPrompt
            | SkipReason::NothingToWaitFor,
        ) => StatusKind::Absent,
    }
}

/// One candidate per verdict `decide` can reach.
fn one_candidate_per_verdict() -> Vec<Candidate> {
    let working = ActiveChildren::Working {
        wait_secs: 20.0 * MINUTE,
    };
    vec![
        due(),
        Candidate {
            agent: Agent::Codex,
            ..due()
        },
        Candidate {
            claim: ThreadClaim::Busy,
            ..due()
        },
        Candidate {
            awaiting_user: true,
            ..due()
        },
        Candidate {
            keep_warm_wait_secs: None,
            ..due()
        },
        Candidate {
            keep_warm_wait_secs: None,
            children: ActiveChildren::AllAwaitingUser,
            ..due()
        },
        Candidate {
            keep_warm_wait_secs: None,
            children: ActiveChildren::OnlyNoWake,
            ..due()
        },
        Candidate {
            keep_warm_wait_secs: None,
            children: working,
            ..due()
        },
        Candidate {
            keep_warm_wait_secs: Some(10.0 * 3600.0),
            ..due()
        },
        Candidate {
            secs_since_request: None,
            ..due()
        },
        Candidate {
            secs_since_request: Some(100.0),
            ..due()
        },
        Candidate {
            secs_since_beat: Some(30.0),
            ..due()
        },
        parked(),
        Candidate {
            parked: parked().parked.map(|p| Parked {
                copy_ready: false,
                ..p
            }),
            ..parked()
        },
        Candidate {
            parked: parked().parked.map(|p| Parked {
                secs: 10.0 * 3600.0,
                ..p
            }),
            ..parked()
        },
        Candidate {
            secs_since_request: Some(100.0),
            ..parked()
        },
        turned_off(due()),
        turned_off(parked()),
        lucidos(),
        lucidos_on_its_card(),
        remembering(Remembered::Uncacheable),
        remembering(Remembered::Nothing),
    ]
}

/// The status and the beat read the same inputs, so they agree on every
/// verdict: the panel never says "kept warm" once the engine stopped (I3).
#[test]
fn the_status_agrees_with_the_beat_on_every_verdict() {
    let mut reached = Vec::new();
    let tuned = short_bash_waits();
    for rule in [
        GiveUpRule::Default,
        GiveUpRule::WorkspaceData(&tuned),
        GiveUpRule::Off,
    ] {
        for candidate in one_candidate_per_verdict() {
            let beat = super::decide(&candidate, rule);
            let status = super::status(&candidate, rule, &anchors_of(&candidate), Spent::default());
            let want = status_kind_for(rule, &candidate, &beat);
            assert_eq!(kind_of(&status), want, "{rule:?}: {beat:?}");
            if !reached.contains(&beat) {
                reached.push(beat);
            }
        }
    }
    assert_eq!(reached.len(), 19, "every verdict is covered: {reached:?}");
}

/// A warming status dates everything from the database's own stamps: the wait
/// start, the stop at the break-even budget, and the next beat's window.
#[test]
fn a_warming_status_dates_the_run_from_its_anchors() {
    let c = due();
    let spent = Spent {
        beats: 2,
        cost_usd: 0.05,
        context_tokens: 262_000,
    };
    let since = secs_before_now(10.0 * MINUTE);
    let budget = 25;
    assert_eq!(
        status(&c, &anchors_of(&c), spent),
        Some(CacheKeepAliveStatus::Warming {
            since,
            beats: 2,
            budget_beats: budget,
            beat_every_secs: window_start(CacheTtl::FiveMinutes) as u32,
            cost_usd: 0.05,
            context_tokens: 262_000,
            next_beat_at: Some(secs_before_now(240.0 - window_start(CacheTtl::FiveMinutes))),
            give_up_at: since
                + Duration::seconds(i64::from(budget) * window_start(CacheTtl::FiveMinutes) as i64),
        })
    );
}

/// Past the budget the status says when the engine stopped, with what it spent.
#[test]
fn a_stopped_status_says_when_the_budget_ran_out() {
    let c = Candidate {
        keep_warm_wait_secs: Some(10.0 * 3600.0),
        ..due()
    };
    let spent = Spent {
        beats: 25,
        cost_usd: 0.6,
        context_tokens: 262_000,
    };
    let give_up = give_up_after_secs(&c, GiveUpRule::Default).expect("a budget");
    assert_eq!(
        status(&c, &anchors_of(&c), spent),
        Some(CacheKeepAliveStatus::Stopped {
            at: secs_before_now(10.0 * 3600.0 - give_up),
            beats: 25,
            cost_usd: 0.6,
            context_tokens: 262_000,
        })
    );
}

/// The ages drift between ticks, the anchors do not, so a second tick builds
/// the same status and the sync writes nothing new (I4).
#[test]
fn the_status_does_not_move_with_the_clock() {
    let c = due();
    let later = Candidate {
        keep_warm_wait_secs: c.keep_warm_wait_secs.map(|s| s + 10.0),
        secs_since_request: c.secs_since_request.map(|s| s + 10.0),
        ..c.clone()
    };
    assert_eq!(
        status(&c, &anchors_of(&c), Spent::default()),
        status(&later, &anchors_of(&c), Spent::default())
    );
}

fn keep_alive_anchor() -> ThreadEvent {
    ThreadEvent::PromptInjected {
        text: "keep-alive".to_string(),
        mode: ActorMode::Engine,
        origin: Some(MessageOrigin::engine(EngineReason::CacheKeepAlive)),
        injected_message_id: None,
        delivered_event_id: None,
    }
}

/// A ping's call: Opus 5.5 rereading `read` cached tokens.
fn ping_capture(read: u32, api_call_id: &str) -> ThreadEvent {
    let ThreadEvent::ContextCaptured {
        producer,
        model,
        context_window,
        sections,
        tools,
        estimated_total_tokens,
        trimmed,
        trim_passes,
        purpose,
        reconstructed,
        parent_tool_use_id,
        reasoning_effort,
        duration_ms,
        served_model,
        provider,
        stable_prefix_fingerprint,
        ..
    } = turn_capture(1_000)
    else {
        unreachable!("turn_capture builds a ContextCaptured")
    };
    ThreadEvent::ContextCaptured {
        producer,
        model,
        context_window,
        sections,
        tools,
        estimated_total_tokens,
        usage: Some(crate::engine::ApiUsage {
            input_tokens: read,
            output_tokens: 1,
            cache_read_tokens: read,
            ..Default::default()
        }),
        trimmed,
        trim_passes,
        purpose,
        reconstructed,
        parent_tool_use_id,
        api_call_id: Some(api_call_id.to_string()),
        reasoning_effort,
        duration_ms,
        served_model,
        provider,
        stable_prefix_fingerprint,
        ttft_ms: None,
        request_id: None,
        session: None,
    }
}

async fn seed_ping(bus: &EventBus, thread_id: Uuid, read: u32, frames: usize) -> Uuid {
    let anchor = seed_thread_event(bus, thread_id, keep_alive_anchor()).await;
    let call = Uuid::new_v4().to_string();
    for _ in 0..frames {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ping_capture(read, &call),
            meta: EventMeta {
                request_event_id: Some(anchor),
                ..EventMeta::NONE
            },
        })
        .await
        .expect("capture persists");
    }
    anchor
}

/// The spend counts the pings since the wait began and prices each call once,
/// however many frames it recorded.
#[tokio::test]
async fn the_spend_counts_the_pings_since_the_wait_began() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/spend", None).await;
    let before = seed_ping(&bus, thread_id, 100_000, 1).await;
    sqlx::query("UPDATE events SET created = now() - interval '1 hour' WHERE id = $1")
        .bind(before)
        .execute(&pool)
        .await
        .unwrap();
    let since: DateTime<Utc> = sqlx::query_scalar("SELECT now() - interval '10 minutes'")
        .fetch_one(&pool)
        .await
        .unwrap();
    seed_ping(&bus, thread_id, 200_000, 2).await;
    seed_ping(&bus, thread_id, 200_000, 1).await;

    let spent = read_spent(&pool, thread_id, since).await.unwrap();
    assert_eq!(
        spent.beats, 2,
        "the ping before the wait began is not counted"
    );
    let one_ping = crate::engine::ApiUsage {
        input_tokens: 200_000,
        output_tokens: 1,
        cache_read_tokens: 200_000,
        ..Default::default()
    }
    .usd_on("claude-opus-5-5", crate::llm::model_prices::today())
    .unwrap();
    assert!(
        (spent.cost_usd - 2.0 * one_ping).abs() < 1e-6,
        "each call priced once: {}",
        spent.cost_usd
    );
    assert_eq!(spent.context_tokens, 200_000);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn stored_status(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<CacheKeepAliveStatus> {
    let stored: Option<sqlx::types::Json<CacheKeepAliveStatus>> = sqlx::query_scalar(
        "SELECT cache_keep_alive_status FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap();
    stored.map(|s| s.0)
}

async fn status_events(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'CacheKeepAliveStatusChanged'",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// The sync writes a status once, writes nothing while it holds, and clears it
/// when the thread stops being a candidate. None of it moves the thread's
/// recency (I2, I4, I6).
#[tokio::test]
async fn the_status_is_written_on_change_and_cleared_when_the_wait_ends() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/status", None).await;
    let activity = || async {
        sqlx::query_as::<_, (DateTime<Utc>, String)>(
            "SELECT last_activity, status FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let before = activity().await;
    let candidate = due();
    let anchors = anchors_of(&candidate);

    sync_status(
        &bus,
        thread_id,
        &candidate,
        GiveUpRule::Default,
        &anchors,
        None,
    )
    .await;
    let written = stored_status(&pool, thread_id).await;
    assert_eq!(kind_of(&written), StatusKind::Warming);
    sync_status(
        &bus,
        thread_id,
        &candidate,
        GiveUpRule::Default,
        &anchors,
        written.clone(),
    )
    .await;
    assert_eq!(
        status_events(&pool, thread_id).await,
        1,
        "an unchanged status writes nothing"
    );
    assert_eq!(
        activity().await,
        before,
        "a status moves no recency and no status"
    );

    let busy = Candidate {
        claim: ThreadClaim::Busy,
        keep_warm_wait_secs: Some(1.0),
        ..candidate.clone()
    };
    sync_status(
        &bus,
        thread_id,
        &busy,
        GiveUpRule::Default,
        &anchors_of(&busy),
        written,
    )
    .await;
    assert_eq!(
        status_events(&pool, thread_id).await,
        1,
        "a busy thread keeps its status"
    );

    clear_stale_statuses(&bus, &[thread_id]).await;
    assert_eq!(
        status_events(&pool, thread_id).await,
        1,
        "a candidate keeps its status"
    );
    clear_stale_statuses(&bus, &[]).await;
    assert_eq!(stored_status(&pool, thread_id).await, None);
    assert_eq!(status_events(&pool, thread_id).await, 2);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// --- A parked session, read back from events ---

async fn age(pool: &sqlx::PgPool, event_id: Uuid, secs: f64) {
    sqlx::query("UPDATE events SET created = now() - make_interval(secs => $2) WHERE id = $1")
        .bind(event_id)
        .bind(secs)
        .execute(pool)
        .await
        .unwrap();
}

/// What a copy beat records: a capture under its own purpose.
fn copy_capture(duration_ms: u64) -> ThreadEvent {
    let mut event = turn_capture(duration_ms);
    if let ThreadEvent::ContextCaptured { purpose, .. } = &mut event {
        *purpose = ContextPurpose::CacheKeepAlive;
    }
    event
}

fn question_asked() -> ThreadEvent {
    ThreadEvent::UserQuestionAsked {
        tool_use_id: "toolu_ask".to_string(),
        cc_session_id: "cc-session".to_string(),
        question: "Which one?".to_string(),
        options: Vec::new(),
        worktree_path: None,
        multi_select: false,
        owner_approval: None,
    }
}

fn permission_requested() -> ThreadEvent {
    ThreadEvent::CodingAgentPermissionRequest {
        request_id: "perm-1".to_string(),
        tool_use_id: "toolu_p".to_string(),
        tool_name: "Bash".to_string(),
        input: serde_json::json!({}),
        summary: "Run a command".to_string(),
    }
}

/// A copy beat refreshes the cache, so the next beat times from it. The
/// parked wait still counts from the turn request that asked (I5).
#[tokio::test]
async fn a_copy_beat_moves_the_request_anchor_but_not_the_parked_clock() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/copy-anchor", None).await;
    let turn = seed_thread_event(&bus, thread_id, turn_capture(10_000)).await;
    age(&pool, turn, 600.0).await;
    let row = || async {
        read_candidates(&pool, &[thread_id])
            .await
            .unwrap()
            .remove(0)
    };

    let before = row().await;
    let secs = before.secs_since_request.expect("a request ran");
    assert!(
        (609.0..=615.0).contains(&secs),
        "the turn's start, got {secs}"
    );
    assert_eq!(before.secs_since_turn_request, before.secs_since_request);

    let copy = seed_thread_event(&bus, thread_id, copy_capture(20_000)).await;
    age(&pool, copy, 100.0).await;
    let after = row().await;
    let secs = after.secs_since_request.expect("a request ran");
    assert!(
        (119.0..=125.0).contains(&secs),
        "the copy's launch, got {secs}"
    );
    assert!(after.request_started_at > before.request_started_at);
    let parked_secs = after.secs_since_turn_request.expect("a turn ran");
    assert!(
        (609.0..=615.0).contains(&parked_secs),
        "the parked clock stays on the turn, got {parked_secs}"
    );

    let newer_turn = seed_thread_event(&bus, thread_id, turn_capture(1_000)).await;
    age(&pool, newer_turn, 10.0).await;
    let secs = row().await.secs_since_request.expect("a request ran");
    assert!(
        (10.0..=15.0).contains(&secs),
        "a newer turn wins, got {secs}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The candidate read and the cache-warm line share one definition of the last
/// refresh. A fresh copy beat over an old turn is warm to both. A sub-agent's
/// call over an old turn is cold to both.
#[tokio::test]
async fn the_candidate_and_the_cache_warm_line_agree_on_the_last_refresh() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/one-refresh", None).await;
    let turn = seed_thread_event(&bus, thread_id, capture_writing(1_000, 0, false)).await;
    age(&pool, turn, 900.0).await;
    let sub_agent = seed_thread_event(&bus, thread_id, capture_writing(1_000, 0, true)).await;
    age(&pool, sub_agent, 10.0).await;
    let candidate_secs = || async {
        read_candidates(&pool, &[thread_id])
            .await
            .unwrap()
            .remove(0)
            .secs_since_request
            .expect("a request ran")
    };
    let line_secs = || crate::engine::switch_estimate::cache_remaining_secs(&pool, thread_id);

    assert!(
        candidate_secs().await > 890.0,
        "a sub-agent refreshes nothing"
    );
    assert_eq!(line_secs().await.unwrap(), None);

    let beat = seed_thread_event(&bus, thread_id, copy_capture(2_000)).await;
    age(&pool, beat, 60.0).await;
    let secs = candidate_secs().await;
    assert!(
        (60.0..=66.0).contains(&secs),
        "the beat's start, got {secs}"
    );
    let left = line_secs().await.unwrap().expect("a beaten cache is warm");
    assert!(
        (232..=240).contains(&left),
        "the same start, less the TTL, got {left}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A Claude Code thread parked on a card is a candidate with no wait and no
/// children, and its card is its latest parking. A Codex one is not.
#[tokio::test]
async fn a_parked_session_is_a_candidate_with_its_card() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/parked", None).await;
    seed_thread_event(&bus, thread_id, turn_capture(1_000)).await;
    let candidate = || async {
        read_candidates(&pool, &[])
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.thread_id == thread_id)
    };
    assert!(
        candidate().await.is_none(),
        "a running thread waits for nothing"
    );

    seed_thread_event(&bus, thread_id, question_asked()).await;
    set_status(&pool, thread_id, ThreadStatus::WaitingForUserAnswer).await;
    let row = candidate().await.expect("a parked session is a candidate");
    assert!(row.awaiting_user);
    assert_eq!(row.parked_card(), Some(ParkedCard::Question));
    assert!(row.secs_since_turn_request.is_some());

    seed_thread_event(&bus, thread_id, permission_requested()).await;
    let row = candidate().await.expect("a parked session is a candidate");
    assert_eq!(row.parked_card(), Some(ParkedCard::Permission));

    set_status(&pool, thread_id, ThreadStatus::Running).await;
    let row = candidate().await;
    assert!(row.is_none(), "an answered card ends the candidacy");

    set_status(&pool, thread_id, ThreadStatus::WaitingForUserAnswer).await;
    sqlx::query("UPDATE thread_summaries SET coding_agent = $2 WHERE thread_id = $1")
        .bind(thread_id)
        .bind(CodingAgent::Codex.as_str())
        .execute(&pool)
        .await
        .unwrap();
    assert!(candidate().await.is_none(), "a parked Codex thread");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A parked thread shows no status, so a busy one drops the status it held
/// before it asked (ADR 0540).
#[tokio::test]
async fn a_parked_session_drops_its_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/parked-status", None).await;
    let waiting = due();
    sync_status(
        &bus,
        thread_id,
        &waiting,
        GiveUpRule::Default,
        &anchors_of(&waiting),
        None,
    )
    .await;
    let written = stored_status(&pool, thread_id).await;
    assert_eq!(kind_of(&written), StatusKind::Warming);

    let parked = parked();
    sync_status(
        &bus,
        thread_id,
        &parked,
        GiveUpRule::Default,
        &anchors_of(&parked),
        written,
    )
    .await;
    assert_eq!(stored_status(&pool, thread_id).await, None);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// --- The keep-alive on the cache-warm line (I3, I4) ---

fn warm_line(c: &Candidate, rule: GiveUpRule) -> WarmLineKeepAlive {
    cache_warm_line(c, rule, &anchors_of(c), Spent::default())
}

fn not_kept(reason: WarmLineNotKept) -> WarmLineKeepAlive {
    WarmLineKeepAlive::NotKept { reason }
}

fn restarted() -> Candidate {
    Candidate {
        parked: parked().parked.map(|p| Parked {
            copy_ready: false,
            ..p
        }),
        ..parked()
    }
}

/// One row per reason nothing keeps a thread warm, each agreeing with the
/// tick: a candidate the line calls `NotKept` is one `decide` never beats by
/// copy.
#[test]
fn the_warm_line_names_why_nothing_keeps_a_thread_warm() {
    let cases: Vec<(
        &str,
        Candidate,
        GiveUpRule,
        WarmLineNotKept,
        Option<SkipReason>,
    )> = vec![
        (
            "a Codex thread",
            Candidate {
                agent: Agent::Codex,
                ..parked()
            },
            GiveUpRule::Default,
            WarmLineNotKept::Codex,
            Some(SkipReason::Codex),
        ),
        (
            "the mode off",
            parked(),
            GiveUpRule::Off,
            WarmLineNotKept::Off,
            Some(SkipReason::Disabled),
        ),
        (
            "a session that outlived a restart",
            restarted(),
            GiveUpRule::Default,
            WarmLineNotKept::Restarted,
            Some(SkipReason::NoCopyPrompt),
        ),
        (
            "a parked thread no session holds",
            Candidate {
                claim: ThreadClaim::Free,
                ..parked()
            },
            GiveUpRule::Default,
            WarmLineNotKept::Restarted,
            Some(SkipReason::AwaitingUser),
        ),
        (
            "a thread parked on nothing",
            due(),
            GiveUpRule::Default,
            WarmLineNotKept::NothingPending,
            None,
        ),
    ];
    for (name, candidate, rule, reason, skipped) in cases {
        assert_eq!(
            warm_line(&candidate, rule),
            not_kept(reason),
            "{name}: the line"
        );
        if let Some(skipped) = skipped {
            assert_eq!(
                super::decide(&candidate, rule),
                Beat::Skip(skipped),
                "{name}: the tick"
            );
        }
    }
}

/// `until` is the give-up time `decide` stops at: a second before it the tick
/// still beats by copy and the line says `Kept`. A second after it the tick
/// is past its cut-off and the line says `Stopped`, at the same time.
#[test]
fn the_kept_until_time_is_the_give_up_time_decide_stops_at() {
    let give_up_after = give_up_after_secs(&parked(), GiveUpRule::Default).expect("a budget");
    let at = |secs: f64| Candidate {
        parked: parked().parked.map(|p| Parked { secs, ..p }),
        secs_since_request: Some(window_start(CacheTtl::FiveMinutes) + 1.0),
        ..parked()
    };
    let give_up_at = |c: &Candidate| {
        anchors_of(c).turn_started_at.expect("a parked turn")
            + Duration::milliseconds((give_up_after * 1000.0) as i64)
    };

    let before = at(give_up_after - 1.0);
    assert_eq!(
        super::decide(&before, GiveUpRule::Default),
        Beat::Send(BeatRoute::SessionCopy)
    );
    let spent = Spent {
        beats: 3,
        cost_usd: 0.41,
        context_tokens: 182_000,
    };
    assert_eq!(
        cache_warm_line(&before, GiveUpRule::Default, &anchors_of(&before), spent),
        WarmLineKeepAlive::Kept {
            route: WarmLineRoute::Copy,
            until: give_up_at(&before),
            beats: 3,
            cost_usd: 0.41,
            context_tokens: 182_000,
        }
    );

    let after = at(give_up_after + 1.0);
    assert_eq!(
        super::decide(&after, GiveUpRule::Default),
        Beat::Skip(SkipReason::PastCutOff)
    );
    assert_eq!(
        cache_warm_line(&after, GiveUpRule::Default, &anchors_of(&after), spent),
        WarmLineKeepAlive::Stopped {
            at: give_up_at(&after),
            beats: 3,
            cost_usd: 0.41,
            context_tokens: 182_000,
        }
    );
}

/// A copy beat's capture under its own purpose, rereading `read` cached tokens.
fn copy_ping(read: u32) -> ThreadEvent {
    copy_frame(read, &Uuid::new_v4().to_string())
}

/// One frame of the copy beat's API call `api_call_id`.
fn copy_frame(read: u32, api_call_id: &str) -> ThreadEvent {
    let mut event = ping_capture(read, api_call_id);
    if let ThreadEvent::ContextCaptured { purpose, .. } = &mut event {
        *purpose = ContextPurpose::CacheKeepAlive;
    }
    event
}

/// The line's spend counts the copy beats since the parked turn began, each
/// priced once, and ignores the ones before and the turn's own calls.
#[tokio::test]
async fn the_copy_spend_counts_the_beats_since_the_turn_began() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/copy-spend", None).await;
    let before = seed_thread_event(&bus, thread_id, copy_ping(100_000)).await;
    age(&pool, before, 3_600.0).await;
    let since: DateTime<Utc> = sqlx::query_scalar("SELECT now() - interval '10 minutes'")
        .fetch_one(&pool)
        .await
        .unwrap();
    seed_thread_event(&bus, thread_id, copy_ping(200_000)).await;
    seed_thread_event(&bus, thread_id, copy_ping(200_000)).await;
    seed_thread_event(&bus, thread_id, turn_capture(1_000)).await;

    let spent = read_capture_spent(&pool, thread_id, since).await.unwrap();
    assert_eq!(spent.beats, 2, "the beat before the turn is not counted");
    let one_beat = crate::engine::ApiUsage {
        input_tokens: 200_000,
        output_tokens: 1,
        cache_read_tokens: 200_000,
        ..Default::default()
    }
    .usd_on("claude-opus-5-5", crate::llm::model_prices::today())
    .unwrap();
    assert!(
        (spent.cost_usd - 2.0 * one_beat).abs() < 1e-6,
        "each beat priced once: {}",
        spent.cost_usd
    );
    assert_eq!(spent.context_tokens, 1, "the turn's own context");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A copy beat's call may record several frames of growing usage. The line
/// counts the call once, at its last frame.
#[tokio::test]
async fn the_copy_spend_counts_a_multi_frame_beat_once() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/copy-frames", None).await;
    let since: DateTime<Utc> = sqlx::query_scalar("SELECT now() - interval '10 minutes'")
        .fetch_one(&pool)
        .await
        .unwrap();
    seed_thread_event(&bus, thread_id, copy_frame(100_000, "call-a")).await;
    seed_thread_event(&bus, thread_id, copy_frame(200_000, "call-a")).await;

    let spent = read_capture_spent(&pool, thread_id, since).await.unwrap();
    assert_eq!(spent.beats, 1, "two frames of one call are one beat");
    let last_frame = crate::engine::ApiUsage {
        input_tokens: 200_000,
        output_tokens: 1,
        cache_read_tokens: 200_000,
        ..Default::default()
    }
    .usd_on("claude-opus-5-5", crate::llm::model_prices::today())
    .unwrap();
    assert!(
        (spent.cost_usd - last_frame).abs() < 1e-6,
        "{}",
        spent.cost_usd
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Only a thread that does not exist has no candidate row.
#[tokio::test]
async fn a_thread_that_does_not_exist_is_not_a_candidate() {
    let (pool, db_name) = setup_test_db().await;
    let rows = read_candidates(&pool, &[Uuid::new_v4()]).await.unwrap();
    assert!(rows.is_empty());
    pool.close().await;
    teardown_test_db(&db_name).await;
}

// --- The per-thread switch (ADR 0554) ---

/// `c` with the switch in force: turned off a minute ago, and no real request
/// started since.
fn turned_off(c: Candidate) -> Candidate {
    Candidate {
        turned_off_at: Some(secs_before_now(MINUTE)),
        ..c
    }
}

/// The switch stops the resume and the copy route alike. Without it, the same
/// threads beat: the SQL drops it once a newer real request started.
#[test]
fn the_switch_stops_every_route() {
    for (label, c, route) in [
        ("an event wait", due(), BeatRoute::Resume),
        ("a parked card", parked(), BeatRoute::SessionCopy),
    ] {
        assert_eq!(
            decide(&c),
            Beat::Send(route),
            "{label}, switch not in force"
        );
        assert_eq!(
            decide(&turned_off(c)),
            skip(SkipReason::TurnedOff),
            "{label}"
        );
    }
}

/// The switch changes no other verdict: a thread that would not beat anyway
/// keeps its own reason.
#[test]
fn the_switch_leaves_the_other_skips_alone() {
    let cases = [
        (
            Candidate {
                claim: ThreadClaim::Busy,
                ..due()
            },
            SkipReason::Busy,
        ),
        (
            Candidate {
                keep_warm_wait_secs: None,
                ..due()
            },
            SkipReason::NothingToWaitFor,
        ),
        (
            Candidate {
                keep_warm_wait_secs: None,
                children: ActiveChildren::AllAwaitingUser,
                ..due()
            },
            SkipReason::ChildrenAwaitingUser,
        ),
        (
            Candidate {
                agent: Agent::Codex,
                ..due()
            },
            SkipReason::Codex,
        ),
    ];
    for (c, reason) in cases {
        assert_eq!(decide(&turned_off(c)), skip(reason));
    }
}

/// Once the user turned it off, the status says why nothing keeps a thread
/// warm. The line says when, with the context a cold reply rereads.
#[test]
fn the_switch_shows_on_the_status_and_the_warm_line() {
    let off = turned_off(due());
    assert_eq!(
        status(&off, &anchors_of(&off), Spent::default()),
        Some(CacheKeepAliveStatus::NotKept {
            reason: CacheKeepAliveNotKeptReason::TurnedOff
        })
    );
    let parked_off = turned_off(parked());
    let spent = Spent {
        beats: 3,
        cost_usd: 0.07,
        context_tokens: 118_000,
    };
    assert_eq!(
        cache_warm_line(
            &parked_off,
            GiveUpRule::Default,
            &anchors_of(&parked_off),
            spent
        ),
        WarmLineKeepAlive::TurnedOff {
            at: secs_before_now(MINUTE),
            context_tokens: 118_000,
        }
    );
    assert_eq!(
        warm_line(&parked_off, GiveUpRule::Off),
        not_kept(WarmLineNotKept::Off),
        "the workspace setting says more"
    );
}

/// A reread is a cache write of the whole context, dearer on a 1-hour cache,
/// and unknown for an unpriced model.
#[test]
fn the_reread_is_priced_as_a_cache_write() {
    let five = reread_usd(118_000, "claude-opus-5-5", CacheTtl::FiveMinutes).expect("priced");
    let hour = reread_usd(118_000, "claude-opus-5-5", CacheTtl::OneHour).expect("priced");
    assert!(five > 0.0 && hour > five, "5 min {five}, 1 h {hour}");
    assert_eq!(
        reread_usd(118_000, "no-such-model", CacheTtl::FiveMinutes),
        None
    );
}

async fn emit_switch(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) -> Uuid {
    seed_thread_event(bus, thread_id, event).await
}

async fn off_at(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<DateTime<Utc>> {
    sqlx::query_scalar("SELECT cache_keep_alive_off_at FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The column is a projection of the two events alone: off stamps the event's
/// own time, on clears it, and the events read back the same answer.
#[tokio::test]
async fn the_switch_projects_from_its_events() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/switch", None).await;
    assert_eq!(off_at(&pool, thread_id).await, None);

    let off = emit_switch(&bus, thread_id, ThreadEvent::CacheKeepAliveTurnedOff).await;
    let created: DateTime<Utc> = sqlx::query_scalar("SELECT created FROM events WHERE id = $1")
        .bind(off)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(off_at(&pool, thread_id).await, Some(created));

    let replayed: Option<DateTime<Utc>> = sqlx::query_scalar(
        "SELECT CASE WHEN event_type = 'CacheKeepAliveTurnedOff' THEN created END \
           FROM events WHERE thread_id = $1 \
            AND event_type IN ('CacheKeepAliveTurnedOff', 'CacheKeepAliveTurnedOn') \
          ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(off_at(&pool, thread_id).await, replayed, "a replay agrees");

    emit_switch(&bus, thread_id, ThreadEvent::CacheKeepAliveTurnedOn).await;
    assert_eq!(off_at(&pool, thread_id).await, None);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The real request leaves out a resume beat's hidden turn and a copy beat,
/// and counts a real turn.
#[tokio::test]
async fn a_beat_is_never_a_real_request() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/real-request", None).await;
    let real = || async {
        let sql = format!(
            "SELECT {} AS started",
            crate::engine::successor::last_real_request_started_sql("$1")
        );
        sqlx::query_scalar::<_, Option<DateTime<Utc>>>(&sql)
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap()
    };
    assert_eq!(real().await, None, "no request yet");

    let turn = seed_thread_event(&bus, thread_id, turn_capture(1_000)).await;
    age(&pool, turn, 900.0).await;
    let first = real().await.expect("a real turn");

    seed_ping(&bus, thread_id, 1_000, 1).await;
    let copy = seed_thread_event(&bus, thread_id, copy_capture(1_000)).await;
    age(&pool, copy, 10.0).await;
    assert_eq!(real().await, Some(first), "beats are not real requests");
    let turn_started: Option<DateTime<Utc>> = sqlx::query_scalar(&format!(
        "SELECT {} AS started",
        crate::engine::successor::last_turn_started_sql("$1")
    ))
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        turn_started > Some(first),
        "the beat timing still counts the resume beat's turn"
    );

    seed_thread_event(&bus, thread_id, turn_capture(1_000)).await;
    assert!(real().await > Some(first), "a newer real turn counts");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn candidate_row(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<CandidateRow> {
    read_candidates(pool, &[thread_id])
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.thread_id == thread_id)
}

/// A parked card, switched off, reads back off on the next tick. The answer's
/// next request re-arms it with no event.
#[tokio::test]
async fn the_switch_holds_until_the_next_real_request() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/switch-parked", None).await;
    let turn = seed_thread_event(&bus, thread_id, turn_capture(1_000)).await;
    age(&pool, turn, 200.0).await;
    seed_thread_event(&bus, thread_id, question_asked()).await;
    set_status(&pool, thread_id, ThreadStatus::WaitingForUserAnswer).await;
    let row = candidate_row(&pool, thread_id).await.expect("parked");
    assert_eq!(row.turned_off_at, None);

    emit_switch(&bus, thread_id, ThreadEvent::CacheKeepAliveTurnedOff).await;
    let row = candidate_row(&pool, thread_id).await.expect("parked");
    assert_eq!(row.turned_off_at, off_at(&pool, thread_id).await);
    assert!(row.turned_off_at.is_some());

    let beat = seed_thread_event(&bus, thread_id, copy_capture(1_000)).await;
    age(&pool, beat, 0.0).await;
    let row = candidate_row(&pool, thread_id).await.expect("parked");
    assert!(
        row.turned_off_at.is_some(),
        "a beat in flight re-arms nothing"
    );

    seed_thread_event(&bus, thread_id, turn_capture(1)).await;
    let row = candidate_row(&pool, thread_id).await.expect("parked");
    assert_eq!(row.turned_off_at, None, "the answer's request re-arms it");
    assert!(
        off_at(&pool, thread_id).await.is_some(),
        "re-arming writes no event"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A switch a real request re-armed stays re-armed when a new session opens,
/// before that session's first request.
#[tokio::test]
async fn a_new_session_never_turns_a_re_armed_switch_off_again() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/switch-session", None).await;
    let turn = seed_thread_event(&bus, thread_id, turn_capture(1_000)).await;
    age(&pool, turn, 200.0).await;
    emit_switch(&bus, thread_id, ThreadEvent::CacheKeepAliveTurnedOff).await;
    seed_thread_event(&bus, thread_id, turn_capture(1)).await;

    seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::SessionStarted {
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            session_id: "fresh-session".into(),
            branch: "claude-code/switch-session".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: Some(crate::engine::thread_events::SessionContinuity::Fresh {
                reason: crate::engine::thread_events::FreshSessionReason::ChangeApplied,
            }),
        },
    )
    .await;
    seed_thread_event(&bus, thread_id, question_asked()).await;
    set_status(&pool, thread_id, ThreadStatus::WaitingForUserAnswer).await;
    let row = candidate_row(&pool, thread_id).await.expect("parked");
    assert_eq!(row.turned_off_at, None);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A thread with a pending handoff is no candidate: its session is ending.
#[tokio::test]
async fn a_pending_handoff_leaves_the_selection() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/handoff-pending", None).await;
    let turn = seed_thread_event(&bus, thread_id, turn_capture(1_000)).await;
    age(&pool, turn, 200.0).await;
    seed_thread_event(&bus, thread_id, question_asked()).await;
    set_status(&pool, thread_id, ThreadStatus::WaitingForUserAnswer).await;
    assert!(candidate_row(&pool, thread_id).await.is_some(), "parked");

    seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::SessionHandedOff {
            opening: crate::engine::thread_events::HandoffOpening::Narrative,
            message: "A fresh session starts here.".into(),
            checkpoint: None,
            progress: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            coding_agent_model: None,
            reasoning_effort: None,
            request: None,
        },
    )
    .await;
    set_status(&pool, thread_id, ThreadStatus::WaitingForUserAnswer).await;
    assert!(candidate_row(&pool, thread_id).await.is_none());

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The switch acts on its own thread only: off on a parent leaves its working
/// child's role alone, and off on the child leaves the parent's beats alone.
#[tokio::test]
async fn the_switch_never_reaches_another_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = (Uuid::new_v4(), Uuid::new_v4());
    start_cc_session(&bus, parent, "claude-code/parent", None).await;
    seed_thread_event(&bus, parent, real_idle()).await;
    start_cc_session(&bus, child, "claude-code/child", None).await;
    sqlx::query("UPDATE thread_summaries SET parent_thread_id = $2 WHERE thread_id = $1")
        .bind(child)
        .bind(parent)
        .execute(&pool)
        .await
        .unwrap();
    set_status(&pool, child, ThreadStatus::Running).await;
    sqlx::query("UPDATE thread_summaries SET active_children_count = 1 WHERE thread_id = $1")
        .bind(parent)
        .execute(&pool)
        .await
        .unwrap();

    emit_switch(&bus, child, ThreadEvent::CacheKeepAliveTurnedOff).await;
    let row = candidate_row(&pool, parent).await.expect("a parent");
    assert_eq!(row.turned_off_at, None, "the child's switch");
    assert!(matches!(
        row.active_children(),
        ActiveChildren::Working { .. }
    ));

    emit_switch(&bus, parent, ThreadEvent::CacheKeepAliveTurnedOff).await;
    let row = candidate_row(&pool, parent).await.expect("a parent");
    assert!(row.turned_off_at.is_some());
    assert!(
        matches!(row.active_children(), ActiveChildren::Working { .. }),
        "the child still keeps it waiting"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// --- The Lucidos Agent (ADR 0556) ---

/// A Lucidos Agent thread the selection beats: a remembered request on a
/// 5-minute cache, a keep-warm wait armed ten minutes ago, the last request
/// 265 s ago, no beat.
fn lucidos() -> Candidate {
    Candidate {
        agent: Agent::Lucidos(Remembered::Pair(CacheTtl::FiveMinutes)),
        secs_since_request: Some(265.0),
        cache_ttl: None,
        ..due()
    }
}

/// [`lucidos`] remembering `remembered` instead.
fn remembering(remembered: Remembered) -> Candidate {
    Candidate {
        agent: Agent::Lucidos(remembered),
        ..lucidos()
    }
}

/// A Lucidos Agent turn parked on its own question card. The turn holds the
/// thread while it waits for the answer.
fn lucidos_on_its_card() -> Candidate {
    Candidate {
        claim: ThreadClaim::Busy,
        awaiting_user: true,
        parked: Some(Parked {
            card: ParkedCard::Question,
            secs: 265.0,
            copy_ready: false,
        }),
        keep_warm_wait_secs: None,
        ..lucidos()
    }
}

fn prefill(cut: PrefillCut) -> Beat {
    Beat::Send(BeatRoute::Prefill(cut))
}

#[test]
fn a_waiting_lucidos_agent_thread_beats_its_stable_prefix() {
    assert_eq!(decide(&lucidos()), prefill(PrefillCut::Stable));
    let children = Candidate {
        keep_warm_wait_secs: None,
        children: ActiveChildren::Working {
            wait_secs: 10.0 * MINUTE,
        },
        ..lucidos()
    };
    assert_eq!(decide(&children), prefill(PrefillCut::Stable));
}

/// The thread's own card beats the whole request, beside the turn that waits.
/// Any other parking has no bound, so it never beats.
#[test]
fn only_its_own_card_beats_the_whole_request() {
    assert_eq!(decide(&lucidos_on_its_card()), prefill(PrefillCut::Whole));
    let permission = Candidate {
        parked: None,
        ..lucidos_on_its_card()
    };
    assert_eq!(decide(&permission), skip(SkipReason::Busy));
    let no_turn = Candidate {
        claim: ThreadClaim::Free,
        ..lucidos_on_its_card()
    };
    assert_eq!(decide(&no_turn), skip(SkipReason::AwaitingUser));
    let turning = Candidate {
        claim: ThreadClaim::Busy,
        ..lucidos()
    };
    assert_eq!(decide(&turning), skip(SkipReason::Busy));
}

/// A prefill has no cold start, so its window sits later than a Claude Code
/// beat's: 260 to 280 s on a 5-minute cache, 1560 to 1620 s on 30 minutes.
#[test]
fn a_prefill_beats_in_the_direct_window() {
    for (ttl, start, end) in [
        (CacheTtl::FiveMinutes, 260.0, 280.0),
        (CacheTtl::ThirtyMinutes, 1560.0, 1620.0),
    ] {
        let at = |secs: f64| Candidate {
            agent: Agent::Lucidos(Remembered::Pair(ttl)),
            secs_since_request: Some(secs),
            keep_warm_wait_secs: Some(secs),
            cache: cache_shares_of("claude-fable-5-1"),
            ..lucidos()
        };
        let stable = prefill(PrefillCut::Stable);
        assert_eq!(
            decide(&at(start - 0.1)),
            skip(SkipReason::OutsideWindow),
            "{ttl:?}"
        );
        assert_eq!(decide(&at(start)), stable, "{ttl:?}");
        assert_eq!(decide(&at(end)), stable, "{ttl:?}");
        assert_eq!(
            decide(&at(end + 0.1)),
            skip(SkipReason::OutsideWindow),
            "{ttl:?}"
        );
        let beaten = Candidate {
            secs_since_beat: Some(start - 1.0),
            ..at(start)
        };
        assert_eq!(decide(&beaten), skip(SkipReason::BeatInFlight), "{ttl:?}");
    }
}

/// The budget counts beats at the direct window, and a card gets half.
#[test]
fn a_lucidos_agent_card_gives_up_at_half_the_budget() {
    let event = give_up_after_secs(&lucidos(), GiveUpRule::Default).expect("a budget");
    assert!((event - 25.0 * 260.0).abs() < 1e-6, "{event}");
    let card = give_up_after_secs(&lucidos_on_its_card(), GiveUpRule::Default).expect("a budget");
    assert!((card - event / 2.0).abs() < 1e-6, "{card}");
    let past = Candidate {
        parked: lucidos_on_its_card().parked.map(|p| Parked {
            secs: card + 0.1,
            ..p
        }),
        ..lucidos_on_its_card()
    };
    assert_eq!(decide(&past), skip(SkipReason::PastCutOff));
}

/// An OpenAI pair beats once per 30-minute window, so its budget counts beats
/// at the 1560 s pace, and a card still gets half.
#[test]
fn an_openai_pair_spends_its_budget_at_the_thirty_minute_pace() {
    let gpt = |base: Candidate| Candidate {
        agent: Agent::Lucidos(Remembered::Pair(CacheTtl::ThirtyMinutes)),
        cache: cache_shares_of("gpt-6.1-sol"),
        ..base
    };
    let beats = CacheTtl::ThirtyMinutes.rewrite_in_beats(cache_shares_of("gpt-6.1-sol"));
    let unpriced = CacheTtl::ThirtyMinutes.rewrite_in_beats(price_table().unpriced_cache_shares);
    assert!(beats > unpriced, "GPT-6.1 is priced: {beats} beats");
    let event = give_up_after_secs(&gpt(lucidos()), GiveUpRule::Default).expect("a budget");
    assert!((event - beats * 1560.0).abs() < 1e-6, "{event}");
    let card =
        give_up_after_secs(&gpt(lucidos_on_its_card()), GiveUpRule::Default).expect("a budget");
    assert!((card - event / 2.0).abs() < 1e-6, "{card}");
    let waiting = |secs: f64| Candidate {
        keep_warm_wait_secs: Some(secs),
        secs_since_request: Some(1560.0),
        ..gpt(lucidos())
    };
    assert_eq!(decide(&waiting(event - 0.1)), prefill(PrefillCut::Stable));
    assert_eq!(decide(&waiting(event + 0.1)), skip(SkipReason::PastCutOff));
}

/// `off` beats nothing, and `workspace-data` tunes each agent from its own
/// waits: Claude Code's never cap a Lucidos Agent thread.
#[test]
fn every_mode_rules_a_lucidos_agent_thread() {
    assert_eq!(
        super::decide(&lucidos(), GiveUpRule::Off),
        skip(SkipReason::Disabled)
    );
    let waiting = |secs: f64| Candidate {
        keep_warm_wait_secs: Some(secs),
        ..lucidos()
    };
    let claude_code = short_bash_waits();
    let theirs = GiveUpRule::WorkspaceData(&claude_code);
    assert_eq!(
        super::decide(&waiting(3.0 * 260.0), theirs),
        prefill(PrefillCut::Stable)
    );
    let bash = WaitType::Events(kinds(&["BackgroundBashCompleted"]));
    let own = WaitDurations::from_samples((0..60).map(|_| (WaitAgent::Lucidos, bash, 2.5 * 260.0)));
    let tuned = GiveUpRule::WorkspaceData(&own);
    assert_eq!(
        super::decide(&waiting(2.0 * 260.0), tuned),
        prefill(PrefillCut::Stable)
    );
    assert_eq!(
        super::decide(&waiting(2.0 * 260.0 + 0.1), tuned),
        skip(SkipReason::PastCutOff)
    );
}

/// One row per wait that never beats, and per request the engine cannot send.
#[test]
fn every_exception_keeps_a_lucidos_agent_thread_cold() {
    let cases: Vec<(&str, Candidate, SkipReason)> = vec![
        (
            "a next-message wait, or a finished trigger run",
            Candidate {
                keep_warm_wait_secs: None,
                ..lucidos()
            },
            SkipReason::NothingToWaitFor,
        ),
        (
            "sub-threads parked on the user",
            Candidate {
                keep_warm_wait_secs: None,
                children: ActiveChildren::AllAwaitingUser,
                ..lucidos()
            },
            SkipReason::ChildrenAwaitingUser,
        ),
        (
            "no-wake sub-threads",
            Candidate {
                keep_warm_wait_secs: None,
                children: ActiveChildren::OnlyNoWake,
                ..lucidos()
            },
            SkipReason::OnlyNoWakeChildren,
        ),
        (
            "a provider that cannot prefill",
            remembering(Remembered::Uncacheable),
            SkipReason::UncacheableProvider,
        ),
        (
            "a restart since the last request",
            remembering(Remembered::Nothing),
            SkipReason::Restarted,
        ),
        (
            "the per-thread switch",
            turned_off(lucidos()),
            SkipReason::TurnedOff,
        ),
        (
            "the switch on its card",
            turned_off(lucidos_on_its_card()),
            SkipReason::TurnedOff,
        ),
    ];
    for (label, candidate, reason) in cases {
        assert_eq!(decide(&candidate), skip(reason), "{label}");
    }
}

/// A restart leaves no request in memory, so no thread beats in the first
/// tick after boot, and each shows why.
#[test]
fn a_restart_causes_no_beat_burst() {
    let store = super::super::RememberedRequests::default();
    let thread = Uuid::new_v4();
    let booted = remembering(store.state(thread));
    assert_eq!(decide(&booted), skip(SkipReason::Restarted));
    assert_eq!(
        kind_of(&status(&booted, &anchors_of(&booted), Spent::default())),
        StatusKind::NotKept(CacheKeepAliveNotKeptReason::Restarted)
    );
}

/// A run that ran out of budget shows it stopped, even once give-up has let
/// its request go.
#[test]
fn a_stopped_run_stays_stopped_after_its_request_is_forgotten() {
    let past = Candidate {
        keep_warm_wait_secs: Some(10.0 * 3600.0),
        ..remembering(Remembered::Nothing)
    };
    assert_eq!(decide(&past), skip(SkipReason::PastCutOff));
    assert_eq!(
        kind_of(&status(&past, &anchors_of(&past), Spent::default())),
        StatusKind::Stopped
    );
}

#[test]
fn a_lucidos_agent_status_beats_every_direct_window() {
    let c = lucidos();
    let at = anchors_of(&c);
    let Some(CacheKeepAliveStatus::Warming {
        beat_every_secs,
        next_beat_at,
        ..
    }) = status(&c, &at, Spent::default())
    else {
        panic!("a waiting Lucidos Agent thread is warming");
    };
    assert_eq!(beat_every_secs, 260);
    assert_eq!(
        next_beat_at,
        at.request_started_at.map(|t| t + Duration::seconds(260))
    );
}

/// The cache-warm line on a Lucidos Agent card: kept while its request is
/// remembered, and why not otherwise.
#[test]
fn the_warm_line_covers_a_lucidos_agent_card() {
    let Some(_) = parked_wait(&lucidos_on_its_card()) else {
        panic!("its own card parks it");
    };
    assert!(matches!(
        cache_warm_line_at(&lucidos_on_its_card()),
        WarmLineKeepAlive::Kept {
            route: WarmLineRoute::Prefill,
            ..
        }
    ));
    let on_card = |remembered| Candidate {
        agent: Agent::Lucidos(remembered),
        ..lucidos_on_its_card()
    };
    assert_eq!(
        cache_warm_line_at(&on_card(Remembered::Uncacheable)),
        not_kept(WarmLineNotKept::UncacheableProvider)
    );
    assert_eq!(
        cache_warm_line_at(&on_card(Remembered::Nothing)),
        not_kept(WarmLineNotKept::Restarted)
    );
    assert_eq!(
        cache_warm_line_at(&lucidos()),
        not_kept(WarmLineNotKept::NothingPending)
    );
}

/// The tick forgets the pair at give-up. The line then still says the run
/// stopped, never that Lucidos restarted.
#[test]
fn a_lucidos_agent_card_past_its_budget_reads_stopped() {
    let card = give_up_after_secs(&lucidos_on_its_card(), GiveUpRule::Default).expect("a budget");
    let past = Candidate {
        agent: Agent::Lucidos(Remembered::Nothing),
        parked: lucidos_on_its_card().parked.map(|p| Parked {
            secs: card + 0.1,
            ..p
        }),
        ..lucidos_on_its_card()
    };
    assert!(matches!(
        cache_warm_line_at(&past),
        WarmLineKeepAlive::Stopped { .. }
    ));
}

fn cache_warm_line_at(c: &Candidate) -> WarmLineKeepAlive {
    super::cache_warm_line(c, GiveUpRule::Default, &anchors_of(c), Spent::default())
}

#[test]
fn a_beat_is_judged_by_its_usage_block() {
    let usage = |input: u32, wrote: u32, output: u32| crate::llm::provider::LlmResponse {
        input_tokens: Some(input),
        cache_creation_tokens: Some(wrote),
        cache_read_tokens: Some(input - wrote),
        output_tokens: Some(output),
        stop_reason: Some("refusal".to_string()),
        ..Default::default()
    };
    let cases = [
        (
            "a full read",
            usage(100_000, 0, 0),
            Some(0),
            BeatOutcome::Read,
        ),
        (
            "a small write",
            usage(100_000, 2_000, 0),
            Some(0),
            BeatOutcome::Read,
        ),
        (
            "a write over 2%",
            usage(100_000, 2_001, 0),
            Some(0),
            BeatOutcome::Wrote,
        ),
        (
            "output past a 0 cap",
            usage(100_000, 0, 1),
            Some(0),
            BeatOutcome::Talkative,
        ),
        (
            "output within a cap of 16",
            usage(100_000, 0, 5),
            Some(16),
            BeatOutcome::Read,
        ),
        (
            "output with no cap, as after a hang-up at message_start",
            usage(100_000, 0, 4),
            None,
            BeatOutcome::Read,
        ),
    ];
    for (label, usage, cap, outcome) in cases {
        assert_eq!(judge_beat(&usage, cap), outcome, "{label}");
    }
}

/// A Lucidos Agent thread with a keep-warm wait is a candidate. Its idle
/// anchor is its latest `ResponseGenerated`, and its own card parks it.
#[tokio::test]
async fn a_lucidos_agent_thread_is_a_candidate() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = crate::test_support::seed_chat_thread(&bus, "hello").await;
    seed_thread_event(&bus, thread_id, turn_capture(1_000)).await;
    let done = seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::ResponseGenerated {
            text: "spawned a sub-thread".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
    )
    .await;
    age(&pool, done, 120.0).await;
    let row = |rows: Vec<CandidateRow>| rows.into_iter().find(|r| r.thread_id == thread_id);

    let waiting = row(read_candidates(&pool, &[thread_id]).await.unwrap())
        .expect("a waiting Lucidos Agent thread is a candidate");
    assert!(!waiting.is_coding_agent);
    assert!(waiting.secs_since_request.is_some());
    let idle = waiting.secs_since_idle.expect("its turn ended");
    assert!((idle - 120.0).abs() < 5.0, "{idle}");

    seed_thread_event(&bus, thread_id, question_asked()).await;
    set_status(&pool, thread_id, ThreadStatus::WaitingForUserAnswer).await;
    let parked = row(read_candidates(&pool, &[]).await.unwrap())
        .expect("a Lucidos Agent thread on its card is a candidate");
    assert!(parked.awaiting_user);
    assert_eq!(parked.parked_card(), Some(ParkedCard::Question));

    set_status(&pool, thread_id, ThreadStatus::Idle).await;
    assert!(
        row(read_candidates(&pool, &[]).await.unwrap()).is_none(),
        "an idle thread with nothing pending waits for nothing"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

// --- A prefill beat, sent (ADR 0556) ---

/// A provider whose every prefill answers with one reply: a usage block, or
/// a refusal.
struct PrefillStub(Result<crate::llm::provider::LlmResponse, &'static str>);

#[async_trait::async_trait]
impl crate::llm::LlmProvider for PrefillStub {
    async fn chat(
        &self,
        _messages: Vec<crate::llm::Message>,
        _tools: Vec<crate::llm::provider::ToolDefinition>,
        _selection: crate::llm::ModelSelection<'_>,
        _system_prompt: Option<&str>,
        _on_token: Option<crate::llm::TokenCallback>,
        _call: crate::llm::metered::CallToken,
    ) -> Result<crate::llm::provider::LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        Err("a beat never chats".into())
    }

    async fn prefill(
        &self,
        _body: &PrefillBody,
        _call: crate::llm::metered::CallToken,
    ) -> Result<crate::llm::provider::LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        match &self.0 {
            Ok(usage) => Ok(usage.clone()),
            Err(message) => Err(Box::new(crate::llm::prefill::PrefillRefused::new(
                message.to_string(),
            ))),
        }
    }
}

fn beat_usage(input: u32, wrote: u32, output: u32) -> PrefillStub {
    PrefillStub(Ok(crate::llm::provider::LlmResponse {
        input_tokens: Some(input),
        cache_read_tokens: Some(input - wrote),
        cache_creation_tokens: Some(wrote),
        output_tokens: Some(output),
        ..Default::default()
    }))
}

fn beat_body() -> PrefillBody {
    PrefillBody {
        provider: Some(crate::llm::ProviderKind::Anthropic),
        model: "claude-opus-5-5".into(),
        reasoning_effort: Some("high".into()),
        output_cap: Some(0),
        reply: crate::llm::prefill::ReplyShape::Json,
        wire_model: "claude-opus-5-5".into(),
        betas: Vec::new(),
        body: bytes::Bytes::from_static(b"{}"),
    }
}

fn remembered_store(thread_id: Uuid) -> super::super::RememberedRequests {
    let store = super::super::RememberedRequests::default();
    let body = beat_body();
    store.remember(
        thread_id,
        Some(crate::llm::prefill::PrefillPair {
            stable: body.clone(),
            whole: body,
        }),
        std::time::Instant::now(),
    );
    store
}

async fn event_count(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM events WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn summary_row(pool: &sqlx::PgPool, thread_id: Uuid) -> serde_json::Value {
    sqlx::query_scalar("SELECT to_jsonb(ts) FROM thread_summaries ts WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// A beat writes one event, its auxiliary capture, timed and priced. The
/// thread's summary does not move, and the next beat times from its start.
#[tokio::test]
async fn a_beat_leaves_one_capture_and_moves_the_refresh() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = crate::test_support::seed_chat_thread(&bus, "hello").await;
    let turn = seed_thread_event(&bus, thread_id, turn_capture(1_000)).await;
    age(&pool, turn, 265.0).await;
    let store = remembered_store(thread_id);
    let (events, summary) = (
        event_count(&pool, thread_id).await,
        summary_row(&pool, thread_id).await,
    );

    beat_once(
        &bus,
        &store,
        &beat_usage(90_000, 0, 0),
        thread_id,
        &beat_body(),
    )
    .await;

    assert_eq!(event_count(&pool, thread_id).await, events + 1, "one event");
    let captures =
        crate::test_support::aux_captures(&pool, thread_id, &ContextPurpose::CacheKeepAlive.wire())
            .await;
    let [capture] = captures.as_slice() else {
        panic!("expected one beat capture, saw {}", captures.len());
    };
    assert!(capture["duration_ms"].is_u64(), "{capture}");
    assert_eq!(capture["usage"]["cache_read_tokens"], 90_000, "{capture}");
    assert_eq!(capture["model"], "claude-opus-5-5", "{capture}");
    assert_eq!(
        summary_row(&pool, thread_id).await,
        summary,
        "the summary stands"
    );

    let refresh: f64 = sqlx::query_scalar(&format!(
        "SELECT EXTRACT(EPOCH FROM now() - {})::float8",
        crate::engine::successor::last_cache_refresh_started_sql("$1")
    ))
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(refresh < 5.0, "the refresh is the beat's start: {refresh}");
    assert_eq!(
        store.state(thread_id),
        Remembered::Pair(CacheTtl::FiveMinutes),
        "a read keeps the request"
    );
    assert!(!store.unproven(crate::llm::ProviderKind::Anthropic));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A beat that writes over 2% ends the run. On an unproven provider it is
/// the first beat, so the provider beats no more.
#[tokio::test]
async fn a_writing_beat_ends_the_run() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = crate::test_support::seed_chat_thread(&bus, "hello").await;
    let store = remembered_store(thread_id);
    let anthropic = crate::llm::ProviderKind::Anthropic;
    store.mark_proven(anthropic);
    beat_once(
        &bus,
        &store,
        &beat_usage(100_000, 5_000, 0),
        thread_id,
        &beat_body(),
    )
    .await;
    assert_eq!(store.state(thread_id), Remembered::Nothing, "the run ended");

    let fresh = remembered_store(thread_id);
    beat_once(
        &bus,
        &fresh,
        &beat_usage(100_000, 5_000, 0),
        thread_id,
        &beat_body(),
    )
    .await;
    assert!(
        !fresh.unproven(anthropic) && fresh.pair(thread_id).is_none(),
        "a first beat that writes marks its provider"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A refused first beat marks the provider. A refusal after a good beat is
/// not retried, and marks nothing.
#[tokio::test]
async fn a_refused_first_beat_marks_its_provider() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = crate::test_support::seed_chat_thread(&bus, "hello").await;
    let refused = PrefillStub(Err("max_tokens: must be at least 1"));

    let first = remembered_store(thread_id);
    beat_once(&bus, &first, &refused, thread_id, &beat_body()).await;
    assert_eq!(first.state(thread_id), Remembered::Uncacheable);

    let proven = remembered_store(thread_id);
    proven.mark_proven(crate::llm::ProviderKind::Anthropic);
    beat_once(&bus, &proven, &refused, thread_id, &beat_body()).await;
    assert_eq!(
        proven.state(thread_id),
        Remembered::Pair(CacheTtl::FiveMinutes)
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A beat that outputs past its cap was no prefill: its provider beats no
/// more, whichever beat it was.
#[tokio::test]
async fn a_talkative_beat_marks_its_provider() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = crate::test_support::seed_chat_thread(&bus, "hello").await;
    let store = remembered_store(thread_id);
    store.mark_proven(crate::llm::ProviderKind::Anthropic);
    beat_once(
        &bus,
        &store,
        &beat_usage(90_000, 0, 12),
        thread_id,
        &beat_body(),
    )
    .await;
    let other = Uuid::new_v4();
    let body = beat_body();
    store.remember(
        other,
        Some(crate::llm::prefill::PrefillPair {
            stable: body.clone(),
            whole: body,
        }),
        std::time::Instant::now(),
    );
    assert_eq!(store.state(other), Remembered::Uncacheable);

    pool.close().await;
    teardown_test_db(&db_name).await;
}
