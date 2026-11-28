use super::super::wait_durations::{LocalStart, WaitSample};
use super::*;
use crate::core::event_subscription::EventSubscription;
use crate::core::prefs::PrefDefault;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::{PlannedWake, WakeMode, WakeSource};
use crate::engine::types::{ContextProducer, ContextPurpose};
use crate::llm::model_prices::{cache_shares_of, price_table};
use crate::test_support::{seed_thread_event, setup_test_db, start_cc_session, teardown_test_db};
use chrono::Duration;

fn now() -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000, 0).expect("valid")
}

const MINUTE: f64 = 60.0;

fn due() -> Candidate {
    Candidate {
        agent: Agent::ClaudeCode,
        claim: ThreadClaim::Free,
        awaiting_user: false,
        parked: None,
        keep_warm_wait_secs: Some(10.0 * MINUTE),
        keep_warm_kinds: kinds(&["BackgroundBashCompleted"]),
        children: ActiveChildren::None,
        wake: None,
        timeout_secs: None,
        secs_since_request: Some(240.0),
        secs_since_beat: None,
        cache_ttl: CacheTtl::Reported(CacheLifetime::FiveMinutes),
        cache: cache_shares_of("claude-opus-5-5"),
        cache_model: Some("claude-opus-5-5".to_string()),
        cache_effort: None,
        beat_model: Some("claude-opus-5-5".into()),
        beat_effort: None,
        turned_off_at: None,
        unattended_trigger: false,
        beat_missed: false,
        frozen_odds: None,
    }
}

fn due_on_the_hour() -> Candidate {
    Candidate {
        cache_ttl: CacheTtl::Reported(CacheLifetime::OneHour),
        secs_since_request: Some(3310.0),
        ..due()
    }
}

fn parked() -> Candidate {
    Candidate {
        claim: ThreadClaim::Busy,
        awaiting_user: true,
        parked: Some(Parked {
            card: ParkedCard::Question,
            secs: 240.0,
            pings_sent: 0,
            copy_ready: true,
            cap: ParkedCap::HalfReread,
        }),
        keep_warm_wait_secs: None,
        ..due()
    }
}

fn kinds(kinds: &[&str]) -> KeepWarmKinds {
    KeepWarmKinds::of(kinds.iter().copied()).expect("keep-warm kinds")
}

fn window_start(ttl: CacheLifetime) -> f64 {
    *ttl.beat_window(BeatStart::Cold).start()
}

fn skip(reason: SkipReason) -> Beat {
    Beat::Skip(reason)
}

fn decide(c: &Candidate) -> Beat {
    super::decide(c, GiveUpRule::PingPlan)
}

fn status(c: &Candidate, at: &Anchors, spent: Spent) -> CacheKeepAliveStatus {
    super::status(
        c,
        GiveUpRule::PingPlan,
        &WaitDurations::default(),
        at,
        spent,
    )
}

fn after_secs(at: DateTime<Utc>, secs: f64) -> DateTime<Utc> {
    at + Duration::milliseconds((secs * 1000.0).round() as i64)
}

fn expecting(c: Candidate, stated: f64, expected: f64, asked_again: ReEstimate) -> Candidate {
    let wait_began = secs_before_now(waiting_secs(&c).expect("a wait"));
    let stated_at = after_secs(wait_began, stated);
    let wake = WakeEstimate::new(
        stated_at,
        after_secs(stated_at, expected),
        &[],
        WakeSource::Agent,
        wait_began,
        asked_again,
    );
    Candidate {
        wake: Some(wake),
        ..c
    }
}

fn give_up_after_secs(c: &Candidate, rule: GiveUpRule) -> Option<f64> {
    super::plan(c, rule).map(|plan| plan.give_up_after_secs)
}

#[test]
fn a_due_thread_beats() {
    assert_eq!(decide(&due()), Beat::Send(BeatRoute::Resume));
}

#[test]
fn a_changed_model_or_effort_stops_every_route() {
    for base in [due(), parked(), lucidos(), lucidos_on_its_card()] {
        for (model, effort) in [
            ("claude-sonnet-5-5", None),
            ("claude-opus-5-5[1m]", None),
            ("claude-opus-5-5", Some("high")),
        ] {
            let candidate = Candidate {
                beat_model: Some(model.into()),
                beat_effort: effort.map(str::to_string),
                ..base.clone()
            };
            assert_eq!(decide(&candidate), skip(SkipReason::ModelChanged));
            let state = status(&candidate, &anchors_of(&candidate), Spent::default());
            assert_eq!(state, not_kept(CacheKeepAliveNotKeptReason::ModelChanged));
            assert_eq!(state.clone().recorded(), Some(state));
        }
    }
}

#[test]
fn an_unattended_trigger_gets_no_beat_on_any_route() {
    for (route, candidate) in [
        ("resume", due()),
        ("session copy", parked()),
        ("stable prefill", lucidos()),
        ("whole prefill", lucidos_on_its_card()),
    ] {
        assert!(
            matches!(decide(&candidate), Beat::Send(_)),
            "{route}: beats once attended"
        );
        let unattended = Candidate {
            unattended_trigger: true,
            ..candidate
        };
        assert_eq!(
            decide(&unattended),
            skip(SkipReason::UnattendedTrigger),
            "{route}"
        );
    }
}

#[test]
fn every_condition_can_stop_a_beat() {
    let cases: Vec<(&str, Candidate, Beat)> = vec![
        (
            "a beat missed the cache",
            Candidate {
                beat_missed: true,
                ..due()
            },
            skip(SkipReason::BeatMissed),
        ),
        (
            "the model changed",
            Candidate {
                beat_model: Some("claude-sonnet-5-5".into()),
                ..due()
            },
            skip(SkipReason::ModelChanged),
        ),
        (
            "a Codex thread",
            Candidate {
                agent: Agent::Codex,
                ..due()
            },
            skip(SkipReason::Codex),
        ),
        (
            "a trigger thread no person has written in",
            Candidate {
                unattended_trigger: true,
                ..due()
            },
            skip(SkipReason::UnattendedTrigger),
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

#[test]
fn the_budget_is_the_rewrite_cost_in_beats() {
    let secs = |model: &str, ttl: CacheLifetime| {
        let c = Candidate {
            cache_ttl: CacheTtl::Reported(ttl),
            cache: cache_shares_of(model),
            ..due()
        };
        give_up_after_secs(&c, GiveUpRule::PingPlan).expect("a budget")
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
            secs(model, CacheLifetime::FiveMinutes),
            secs(model, CacheLifetime::OneHour),
        );
        let want = (
            five * window_start(CacheLifetime::FiveMinutes),
            hour * window_start(CacheLifetime::OneHour),
        );
        assert!(
            (got.0 - want.0).abs() < 1e-6 && (got.1 - want.1).abs() < 1e-6,
            "{model}: {got:?}, want {want:?}"
        );
    }
}

#[test]
fn an_unknown_model_gets_the_shortest_budget() {
    let unknown = Candidate {
        cache: price_table().unpriced_cache_shares,
        ..due()
    };
    let budget = 12.5 * window_start(CacheLifetime::FiveMinutes);
    let waiting = |secs: f64| Candidate {
        keep_warm_wait_secs: Some(secs),
        ..unknown.clone()
    };
    assert!(decide(&waiting(budget - 0.1)) == Beat::Send(BeatRoute::Resume));
    assert_eq!(decide(&waiting(budget + 0.1)), skip(SkipReason::PastCutOff));
}

#[test]
fn the_cut_off_holds_at_the_budget() {
    let cases = [
        (
            "a 5-minute cache",
            due(),
            25.0 * window_start(CacheLifetime::FiveMinutes),
        ),
        (
            "a 1-hour cache",
            due_on_the_hour(),
            40.0 * window_start(CacheLifetime::OneHour),
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

#[test]
fn the_window_follows_the_reported_ttl() {
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
        cache_ttl: CacheTtl::Unknown,
        secs_since_request: Some(secs),
        ..due()
    };
    for secs in [229.9, 230.0, 260.0, 1560.0, 3300.0, 42_600.0] {
        assert_eq!(
            decide(&unknown(secs)),
            skip(SkipReason::UnknownTtl),
            "{secs}"
        );
    }

    let thirty = |secs: f64| Candidate {
        cache_ttl: CacheTtl::Reported(CacheLifetime::ThirtyMinutes),
        secs_since_request: Some(secs),
        ..due()
    };
    assert_eq!(decide(&thirty(240.0)), skip(SkipReason::OutsideWindow));
    assert!(decide(&thirty(1500.0)) == Beat::Send(BeatRoute::Resume));

    let requested = Candidate {
        cache_ttl: CacheTtl::Requested(CacheLifetime::FiveMinutes),
        ..due()
    };
    assert!(decide(&requested) == Beat::Send(BeatRoute::Resume));

    let beaten = |secs: f64| Candidate {
        secs_since_beat: Some(secs),
        ..due_on_the_hour()
    };
    assert_eq!(decide(&beaten(3299.0)), skip(SkipReason::BeatInFlight));
    assert!(decide(&beaten(3300.0)) == Beat::Send(BeatRoute::Resume));
}

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

#[test]
fn an_answer_restarts_the_beats_only_while_the_parents_cache_is_warm() {
    for ttl in [CacheLifetime::FiveMinutes, CacheLifetime::OneHour] {
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
                    cache_ttl: CacheTtl::Reported(ttl),
                    ..due()
                };
                decide(&parent) == Beat::Send(BeatRoute::Resume)
            });
            assert_eq!(beat, first_beat, "{label} on {ttl:?}");
        }
    }
}

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

#[test]
fn a_parked_wait_gives_up_at_half_the_reread_cap() {
    let cases = [
        ("a 5-minute cache", CacheLifetime::FiveMinutes, 240.0, 12.5),
        ("a 1-hour cache", CacheLifetime::OneHour, 3310.0, 20.0),
    ];
    for (label, ttl, request, beats) in cases {
        let budget = beats * window_start(ttl);
        let waiting = |secs: f64| Candidate {
            cache_ttl: CacheTtl::Reported(ttl),
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

#[test]
fn a_parked_plan_names_half_the_reread_cap_and_never_an_estimate() {
    for c in [parked(), lucidos_on_its_card()] {
        let plan = plan(&c, GiveUpRule::PingPlan).expect("a plan");
        assert_eq!(plan.end, PlanEnd::HalfReread);
        assert!(!awaits_re_estimate(&c, plan));
        let CacheKeepAliveStatus::Warming { cap_source, .. } =
            status(&c, &anchors_of(&c), Spent::default())
        else {
            panic!("a parked card warms");
        };
        assert_eq!(cap_source, Some(CapSource::HalfReread));
    }
    let learning = expecting(parked_learning(2), 0.0, 60.0, ReEstimate::NotAsked);
    let plan = plan(&learning, GiveUpRule::PingPlan).expect("a plan");
    assert_eq!(plan.end, PlanEnd::Learned(learned_cap(2)));
    assert!(!awaits_re_estimate(&learning, plan), "a learned cap");
}

fn learned_cap(pings: u32) -> LearnedCap {
    LearnedCap {
        pings,
        samples: 40,
        hours: HourWindow { from: 10, to: 15 },
        utc_offset_mins: 0,
    }
}

#[test]
fn the_parked_cap_kinds_are_the_parked_sources() {
    let kind = |source: CapSource| serde_json::to_value(source).unwrap()["kind"].clone();
    let parked = [
        PlanEnd::HalfReread.cap_source(),
        PlanEnd::Learned(learned_cap(2)).cap_source(),
    ];
    for source in parked {
        assert!(ParkedCap::of_source(source).is_some());
    }
    assert_eq!(
        parked.map(kind),
        PARKED_CAP_KINDS.map(serde_json::Value::from)
    );
    let others = [
        CapSource::RereadCap,
        CapSource::Timeout,
        CapSource::Estimate {
            by: WakeSource::Agent,
        },
    ];
    for source in others {
        assert_eq!(ParkedCap::of_source(source), None);
    }
}

fn parked_learning(pings: u32) -> Candidate {
    Candidate {
        parked: parked().parked.map(|p| Parked {
            cap: ParkedCap::Learned(learned_cap(pings)),
            ..p
        }),
        ..parked()
    }
}

fn learning_at(pings: u32, sent: u32, secs: f64, since_request: f64) -> Candidate {
    let c = parked_learning(pings);
    Candidate {
        parked: c.parked.map(|p| Parked {
            secs,
            pings_sent: sent,
            ..p
        }),
        secs_since_request: Some(since_request),
        ..c
    }
}

#[test]
fn a_learned_plan_gives_up_a_window_after_its_last_ping_is_due() {
    let plan = plan(&learning_at(2, 1, 300.0, 50.0), GiveUpRule::PingPlan).expect("a plan");
    assert_eq!(plan.give_up_after_secs, 250.0 + 230.0 + 30.0);
    assert_eq!(
        plan.end.cap_source(),
        CapSource::Learned {
            pings: 2,
            samples: 40,
            from_hour: 10,
            to_hour: 15,
            utc_offset_mins: 0,
        }
    );
}

#[test]
fn a_learned_cap_of_c_sends_c_pings() {
    let every = window_start(CacheLifetime::FiveMinutes);
    let late = *CacheLifetime::FiveMinutes
        .beat_window(BeatStart::Cold)
        .end();
    let drifted = every + 26.0;
    let copy = Beat::Send(BeatRoute::SessionCopy);
    for c in [1, 2, 12] {
        for k in 1..=c {
            for since_request in [every, late] {
                let secs = f64::from(k - 1) * drifted + since_request;
                assert_eq!(
                    decide(&learning_at(c, k - 1, secs, since_request)),
                    copy,
                    "beat {k} of {c}, {since_request} s after the last"
                );
            }
        }
        let secs = f64::from(c) * drifted + every;
        assert_eq!(
            decide(&learning_at(c, c, secs, every)),
            skip(SkipReason::PastCutOff),
            "beat {} of {c}",
            c + 1
        );
    }
}

#[test]
fn a_learned_runs_give_up_time_is_the_time_decide_stops_at() {
    let at = |secs: f64| learning_at(3, 2, secs, secs - 500.0);
    let give_up_after = 500.0 + 230.0 + 30.0;
    let before = at(give_up_after - 1.0);
    assert_eq!(decide(&before), Beat::Send(BeatRoute::SessionCopy));
    let CacheKeepAliveStatus::Warming {
        give_up_at,
        budget_beats,
        cap_source,
        ..
    } = status(&before, &anchors_of(&before), pinged())
    else {
        panic!("a learned run before its give-up time is warming");
    };
    let turn_started = anchors_of(&before).turn_started_at.expect("a parked turn");
    assert_eq!(give_up_at, after_secs(turn_started, give_up_after));
    assert_eq!(budget_beats, 3, "the card shows the learned count");
    let source = Some(PlanEnd::Learned(learned_cap(3)).cap_source());
    assert_eq!(cap_source, source);

    let after = at(give_up_after + 1.0);
    assert_eq!(decide(&after), skip(SkipReason::PastCutOff));
    let CacheKeepAliveStatus::Stopped { cap_source, .. } =
        status(&after, &anchors_of(&after), pinged())
    else {
        panic!("a learned run past its give-up time stopped");
    };
    assert_eq!(cap_source, source);
}

#[test]
fn a_late_last_ping_never_reopens_a_stopped_run() {
    let in_flight = learning_at(3, 2, 500.0 + 270.0, 270.0);
    assert_eq!(decide(&in_flight), skip(SkipReason::PastCutOff));
    assert!(matches!(
        status(&in_flight, &anchors_of(&in_flight), pinged()),
        CacheKeepAliveStatus::Stopped { .. }
    ));
    let landed = learning_at(3, 3, 500.0 + 270.0, 25.0);
    assert_eq!(decide(&landed), skip(SkipReason::PastCutOff));
    assert!(matches!(
        status(&landed, &anchors_of(&landed), pinged()),
        CacheKeepAliveStatus::Stopped { .. }
    ));
}

#[test]
fn a_learned_cap_of_none_reads_past_give_up_time() {
    let c = learning_at(0, 0, 80.0, 80.0);
    assert_eq!(decide(&c), skip(SkipReason::PastCutOff));
    assert_eq!(
        status(&c, &anchors_of(&c), Spent::default()),
        CacheKeepAliveStatus::past_cap(
            CacheKeepAliveNotKeptReason::PastGiveUpTime,
            PlanEnd::Learned(learned_cap(0)).cap_source(),
        )
    );
}

#[test]
fn a_parked_waits_recorded_odds_stay_as_recorded() {
    let recorded = CapOdds {
        samples: 41,
        slots: vec![10, 20],
        after_cap: 11,
    };
    for (label, c) in [
        ("learned", learning_at(1, 0, 100.0, 100.0)),
        ("half", parked()),
    ] {
        let c = Candidate {
            frozen_odds: Some(recorded.clone()),
            ..c
        };
        let CacheKeepAliveStatus::Warming { odds, .. } = super::status(
            &c,
            GiveUpRule::PingPlan,
            &one_ping_history(),
            &anchors_of(&c),
            Spent::default(),
        ) else {
            panic!("a {label} run warms");
        };
        assert_eq!(odds, Some(recorded.clone()), "{label}");
    }
}

#[test]
fn a_learned_caps_odds_count_its_own_hours() {
    let evening = |secs: f64| WaitSample {
        secs,
        start: LocalStart {
            hour: 20,
            ..WaitSample::at_noon(secs).start
        },
    };
    let question = WaitType::Parked(ParkedCard::Question);
    let history = WaitDurations::from_samples(
        (0..30)
            .map(|_| WaitSample::at_noon(300.0))
            .chain((0..50).map(|_| evening(f64::INFINITY)))
            .map(|sample| (WaitAgent::ClaudeCode, question, sample)),
    );
    let c = learning_at(1, 0, 100.0, 100.0);
    let CacheKeepAliveStatus::Warming {
        odds: Some(odds), ..
    } = super::status(
        &c,
        GiveUpRule::PingPlan,
        &history,
        &anchors_of(&c),
        Spent::default(),
    )
    else {
        panic!("a learned run warms with odds");
    };
    assert_eq!(
        odds,
        CapOdds {
            samples: 30,
            slots: vec![0, 30],
            after_cap: 0,
        }
    );
}

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
        CacheKeepAliveMode::Off
    );
}

#[test]
fn the_catalog_default_is_off() {
    let PrefDefault::Value(value) = crate::core::prefs::CACHE_KEEP_ALIVE_MODE.spec.default else {
        panic!("the mode preference has a value default");
    };
    assert_eq!(CacheKeepAliveMode::parse(value), CacheKeepAliveMode::Off);
}

#[tokio::test]
async fn the_intermediate_merge_converts_retired_modes_to_on() {
    const MERGE: &str = include_str!(
        "../../../../lucidos-core/migrations/20261030213124_cache_keep_alive_mode_is_off_or_on.sql"
    );
    let (pool, db_name) = setup_test_db().await;
    let key = crate::core::prefs::CACHE_KEEP_ALIVE_MODE.key();
    let retired = ["break-even", "workspace-data"];
    let cases = retired
        .map(|mode| (mode, CacheKeepAliveMode::On))
        .into_iter()
        .chain([("off", CacheKeepAliveMode::Off)]);
    for (stored, read) in cases {
        sqlx::query("DELETE FROM preferences WHERE key = $1")
            .bind(key)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO preferences (key, value) VALUES ($1, $2)")
            .bind(key)
            .bind(stored)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::raw_sql(MERGE).execute(&pool).await.expect("re-runs");
        let value: String = sqlx::query_scalar("SELECT value FROM preferences WHERE key = $1")
            .bind(key)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(CacheKeepAliveMode::parse(&value), read, "{stored}");
        assert!(CACHE_KEEP_ALIVE_MODES.contains(&value.as_str()), "{stored}");
    }
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn the_catalog_lists_off_and_on() {
    assert_eq!(CACHE_KEEP_ALIVE_MODES, ["on", "off"]);
}

#[test]
fn each_mode_picks_its_rule() {
    let c = due();
    let decide_under = |mode| super::decide(&c, GiveUpRule::of(mode));
    assert_eq!(decide_under(CacheKeepAliveMode::On), decide(&c));
    assert_eq!(
        decide_under(CacheKeepAliveMode::Off),
        skip(SkipReason::Disabled)
    );
}

fn wait_on(kinds: &[&str], armed_minutes_ago: i64) -> LiveWait {
    LiveWait {
        background_timing: None,
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
    let bash_wait = Some(KeepWarmWait {
        armed_at: bash.armed_at,
        kinds: kinds(&["BackgroundBashCompleted"]),
        expires_at: bash.expires_at,
    });
    assert_eq!(newest_keep_warm_wait([&bash]), bash_wait);
    assert_eq!(newest_keep_warm_wait([&mixed, &other]), None);
    assert_eq!(
        newest_keep_warm_wait([&bash, &mixed]),
        bash_wait,
        "an unlisted kind disqualifies its own wait only"
    );
}

#[test]
fn the_newest_keep_warm_wait_names_the_kinds() {
    let lock = wait_on(&["E2ELockReleased"], 9);
    let agent = wait_on(&["ChangeApplied", "CodingAgentIdled"], 2);
    assert_eq!(
        newest_keep_warm_wait([&lock, &agent]).map(|w| w.kinds),
        Some(kinds(&["CodingAgentIdled", "ChangeApplied"]))
    );
    assert_eq!(KeepWarmKinds::of([]), None, "a wait on nothing");
}

#[test]
fn every_keep_warm_kind_is_one_a_wait_can_hold() {
    use crate::core::event_subscription::{validate_subscribable_event_type, SubscriptionSurface};
    for kind in KEEP_WARM_EVENT_TYPES {
        let verdict = validate_subscribable_event_type(kind, SubscriptionSurface::Wait);
        assert!(verdict.is_ok(), "{kind} is refused as a wait: {verdict:?}");
    }
}

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
        telemetry_owned_by_cli: false,
        session: None,
    }
}

fn turn_capture(duration_ms: u64) -> ThreadEvent {
    let usage = crate::engine::ApiUsage {
        input_tokens: 1,
        ..Default::default()
    };
    capture("claude-opus-5-5", duration_ms, Some(usage), None)
}

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

#[tokio::test]
async fn the_ttl_comes_from_the_latest_session_write() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/ttl", None).await;
    let ttl = || async { read_candidates(&pool, &[thread_id]).await.unwrap()[0].cache_ttl() };
    assert_eq!(ttl().await, CacheTtl::Unknown, "no capture yet");

    seed_thread_event(&bus, thread_id, capture_writing(40_000, 40_000, false)).await;
    assert_eq!(ttl().await, CacheTtl::Reported(CacheLifetime::OneHour));

    seed_thread_event(&bus, thread_id, capture_writing(0, 0, false)).await;
    assert_eq!(
        ttl().await,
        CacheTtl::Reported(CacheLifetime::OneHour),
        "a read-only request"
    );

    seed_thread_event(&bus, thread_id, capture_writing(300, 0, true)).await;
    assert_eq!(
        ttl().await,
        CacheTtl::Reported(CacheLifetime::OneHour),
        "a sub-agent's write"
    );

    seed_thread_event(&bus, thread_id, capture_writing(300, 0, false)).await;
    assert_eq!(ttl().await, CacheTtl::Reported(CacheLifetime::FiveMinutes));

    let openai = crate::engine::ApiUsage {
        input_tokens: 310,
        cache_creation_tokens: 300,
        cache_creation_30m_tokens: Some(300),
        cache_creation_12h_tokens: Some(0),
        ..Default::default()
    };
    seed_thread_event(
        &bus,
        thread_id,
        capture("gpt-6.1-sol", 1_000, Some(openai), None),
    )
    .await;
    assert_eq!(
        ttl().await,
        CacheTtl::Reported(CacheLifetime::ThirtyMinutes)
    );

    let no_split = crate::engine::ApiUsage {
        input_tokens: 310,
        cache_creation_tokens: 300,
        ..Default::default()
    };
    seed_thread_event(
        &bus,
        thread_id,
        capture("gpt-6.1-sol", 1_000, Some(no_split), None),
    )
    .await;
    assert_eq!(ttl().await, CacheTtl::Unknown, "a write with no split");

    sqlx::query("UPDATE thread_summaries SET coding_agent = $2 WHERE thread_id = $1")
        .bind(thread_id)
        .bind(CodingAgent::Codex.as_str())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        ttl().await,
        CacheTtl::Unknown,
        "a Codex thread skips the look-up"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

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

#[tokio::test]
async fn a_persisted_model_switch_cannot_beat_the_previous_models_cache() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/model-switch", None).await;
    seed_thread_event(
        &bus,
        thread_id,
        capture_writing_on("claude-sonnet-5-5", 100_000, 0, false),
    )
    .await;
    seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::CodingAgentSettingsChanged {
            model: Some("claude-opus-5-5".into()),
            reasoning_effort: Some("high".into()),
            coding_agent: CodingAgent::ClaudeCode,
            cc_session_id: None,
            claude_config_dir: None,
            claude_config_dir_explicit: None,
        },
    )
    .await;
    let row = candidate_row(&pool, thread_id).await.unwrap();
    let (model, effort): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT payload->>'model', payload->>'reasoning_effort' FROM events
         WHERE thread_id = $1 AND event_type = 'CodingAgentSettingsChanged'
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let candidate = Candidate {
        cache_model: row.last_write_model,
        cache_effort: row.last_write_effort,
        beat_model: model,
        beat_effort: effort,
        ..due()
    };
    assert_eq!(decide(&candidate), skip(SkipReason::ModelChanged));
    pool.close().await;
    teardown_test_db(&db_name).await;
}

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
        turn_end: None,
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
        "an answered card puts the child back to work, timed from its spawn"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_child_between_successor_sessions_still_works() {
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
    seed_thread_event(
        &bus,
        child,
        ThreadEvent::SessionHandedOff {
            plan_path: None,
            cause: None,
            starts: crate::engine::thread_events::SessionStart::Now,
            crossing: None,
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
    set_status(&pool, child, ThreadStatus::Idle).await;
    sqlx::query("UPDATE thread_summaries SET active_children_count = 0 WHERE thread_id = $1")
        .bind(parent)
        .execute(&pool)
        .await
        .unwrap();

    let row = read_candidates(&pool, &[])
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.thread_id == parent)
        .expect("a parent whose child is handing off stays a candidate");
    assert!(matches!(
        row.active_children(),
        ActiveChildren::Working { .. }
    ));
    assert!(
        row.children_wait_began_at.is_some(),
        "the wait keeps its start through the gap"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_real_turn_mid_wait_moves_no_wait_on_children() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (parent, child) = (Uuid::new_v4(), Uuid::new_v4());
    start_cc_session(&bus, parent, "claude-code/parent", None).await;
    start_cc_session(&bus, child, "claude-code/child", None).await;
    sqlx::query(
        "UPDATE thread_summaries SET parent_thread_id = $2, \
         created_at = now() - interval '31 minutes' WHERE thread_id = $1",
    )
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
    let spawned_turn_end = seed_thread_event(&bus, parent, real_idle()).await;
    age(&pool, spawned_turn_end, 30.0 * MINUTE).await;
    let parent_row = || async {
        read_candidates(&pool, &[])
            .await
            .unwrap()
            .into_iter()
            .find(|r| r.thread_id == parent)
            .expect("a parent with a working child is a candidate")
    };
    let wait_began = parent_row()
        .await
        .children_wait_began_at
        .expect("the wait began at the turn that spawned the child");

    let user_turn_end = seed_thread_event(&bus, parent, real_idle()).await;
    age(&pool, user_turn_end, 60.0).await;
    let after_the_user_turn = parent_row().await;
    assert_eq!(after_the_user_turn.children_wait_began_at, Some(wait_began));
    assert!(
        after_the_user_turn.idle_at > Some(wait_began),
        "the run moved"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

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

fn secs_before_now(secs: f64) -> DateTime<Utc> {
    now() - Duration::milliseconds((secs * 1000.0) as i64)
}

fn anchors_of(c: &Candidate) -> Anchors {
    let children_wait_began_at = match c.children {
        ActiveChildren::Working { wait_secs } => Some(secs_before_now(wait_secs)),
        _ => None,
    };
    Anchors {
        keep_warm_armed_at: c.keep_warm_wait_secs.map(secs_before_now),
        idle_at: children_wait_began_at,
        children_wait_began_at,
        request_started_at: c.secs_since_request.map(secs_before_now),
        turn_started_at: c.parked.map(|p| secs_before_now(p.secs)),
    }
}

#[derive(Debug, PartialEq)]
enum StatusKind {
    Warming,
    Stopped,
    TurnedOff,
    NotKept(CacheKeepAliveNotKeptReason),
}

fn kind_of(status: &CacheKeepAliveStatus) -> StatusKind {
    match status {
        CacheKeepAliveStatus::Warming { .. } => StatusKind::Warming,
        CacheKeepAliveStatus::Stopped { .. } => StatusKind::Stopped,
        CacheKeepAliveStatus::TurnedOff { .. } => StatusKind::TurnedOff,
        CacheKeepAliveStatus::NotKept { reason, .. } => StatusKind::NotKept(*reason),
        CacheKeepAliveStatus::Ended { .. } => panic!("no tick builds an ended status"),
    }
}

fn status_kind_for(rule: GiveUpRule, c: &Candidate, beat: &Beat) -> StatusKind {
    use CacheKeepAliveNotKeptReason as Reason;
    if matches!(rule, GiveUpRule::Off) {
        return StatusKind::NotKept(match c.agent {
            Agent::Codex => Reason::Codex,
            _ => Reason::Off,
        });
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
        Beat::Skip(SkipReason::PastCutOff) => StatusKind::NotKept(Reason::PastGiveUpTime),
        Beat::Skip(SkipReason::PlanRanOut) => StatusKind::Warming,
        Beat::Skip(SkipReason::WakePastCap) => StatusKind::NotKept(Reason::WakePastRereadCap),
        Beat::Skip(SkipReason::ChildrenAwaitingUser) => {
            StatusKind::NotKept(Reason::ChildrenAwaitingUser)
        }
        Beat::Skip(SkipReason::OnlyNoWakeChildren) => StatusKind::NotKept(Reason::NoWakeChildren),
        Beat::Skip(SkipReason::UnknownTtl) => StatusKind::NotKept(Reason::UnknownTtl),
        Beat::Skip(SkipReason::ModelChanged) => StatusKind::NotKept(Reason::ModelChanged),
        Beat::Skip(SkipReason::BeatMissed) => StatusKind::NotKept(Reason::BeatMissed),
        Beat::Skip(SkipReason::TurnedOff) => StatusKind::TurnedOff,
        Beat::Skip(SkipReason::UncacheableProvider) => {
            StatusKind::NotKept(Reason::UncacheableProvider)
        }
        Beat::Skip(SkipReason::ProviderPaused) => StatusKind::NotKept(Reason::ProviderPaused),
        Beat::Skip(SkipReason::PingsPaused) => StatusKind::NotKept(Reason::PingsPaused),
        Beat::Skip(SkipReason::CacheExpired) => StatusKind::NotKept(Reason::CacheExpired),
        Beat::Skip(SkipReason::Restarted | SkipReason::NoCopyPrompt) => {
            StatusKind::NotKept(Reason::Restarted)
        }
        Beat::Skip(SkipReason::Codex) => StatusKind::NotKept(Reason::Codex),
        Beat::Skip(SkipReason::UnattendedTrigger) => StatusKind::NotKept(Reason::UnattendedTrigger),
        Beat::Skip(SkipReason::Disabled) => StatusKind::NotKept(Reason::Off),
        Beat::Skip(SkipReason::AwaitingUser | SkipReason::NothingToWaitFor) => {
            StatusKind::NotKept(Reason::NothingPending)
        }
    }
}

fn one_candidate_per_verdict() -> Vec<Candidate> {
    let working = ActiveChildren::Working {
        wait_secs: 20.0 * MINUTE,
    };
    vec![
        due(),
        Candidate {
            beat_missed: true,
            ..due()
        },
        Candidate {
            beat_model: Some("claude-sonnet-5-5".into()),
            ..due()
        },
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
        Candidate {
            cache_ttl: CacheTtl::Unknown,
            ..due()
        },
        Candidate {
            cache_ttl: CacheTtl::Unknown,
            ..parked()
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
        remembering(Remembered::ProviderPaused),
        remembering(Remembered::Missed),
        remembering(Remembered::Expired),
        remembering(paused()),
        Candidate {
            agent: Agent::Lucidos(Remembered::Missed),
            ..lucidos_on_its_card()
        },
        Candidate {
            agent: Agent::Lucidos(paused()),
            ..lucidos_on_its_card()
        },
        remembering(Remembered::GaveUp),
        remembering(Remembered::Nothing),
        Candidate {
            unattended_trigger: true,
            ..due()
        },
        expecting(due(), 0.0, 5.0 * MINUTE, ReEstimate::NotAsked),
        expecting(due(), 0.0, 90.0 * MINUTE, ReEstimate::NotAsked),
    ]
}

#[test]
fn the_status_agrees_with_the_beat_on_every_verdict() {
    let mut reached = Vec::new();
    for rule in [GiveUpRule::PingPlan, GiveUpRule::Off] {
        for candidate in one_candidate_per_verdict() {
            let beat = super::decide(&candidate, rule);
            let status = super::status(
                &candidate,
                rule,
                &WaitDurations::default(),
                &anchors_of(&candidate),
                Spent::default(),
            );
            let want = status_kind_for(rule, &candidate, &beat);
            assert_eq!(kind_of(&status), want, "{rule:?}: {beat:?}");
            if !reached.contains(&beat) {
                reached.push(beat);
            }
        }
    }
    assert_eq!(reached.len(), 28, "every verdict is covered: {reached:?}");
}

#[test]
fn a_warming_status_dates_the_run_from_its_anchors() {
    let c = due();
    let spent = Spent {
        beats: 2,
        cost_usd: 0.05,
        context_tokens: 262_000,
        ..Spent::default()
    };
    let since = secs_before_now(10.0 * MINUTE);
    let budget = 25;
    assert_eq!(
        status(&c, &anchors_of(&c), spent),
        CacheKeepAliveStatus::Warming {
            since,
            beats: 2,
            budget_beats: budget,
            beat_every_secs: window_start(CacheLifetime::FiveMinutes) as u32,
            cost_usd: 0.05,
            context_tokens: 262_000,
            next_beat_at: Some(secs_before_now(
                240.0 - window_start(CacheLifetime::FiveMinutes)
            )),
            give_up_at: since
                + Duration::seconds(
                    i64::from(budget) * window_start(CacheLifetime::FiveMinutes) as i64
                ),
            ping_usd: None,
            reread_usd: None,
            odds: None,
            expected_wake: None,
            cap_source: Some(CapSource::RereadCap),
        }
    );
}

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
        ..Spent::default()
    };
    let give_up = give_up_after_secs(&c, GiveUpRule::PingPlan).expect("a budget");
    assert_eq!(
        status(&c, &anchors_of(&c), spent),
        CacheKeepAliveStatus::Stopped {
            at: secs_before_now(10.0 * 3600.0 - give_up),
            beats: 25,
            cost_usd: 0.6,
            context_tokens: 262_000,
            ping_usd: None,
            reread_usd: None,
            beat_every_secs: c.beat_every_secs().map(|secs| secs as u32),
            odds: None,
            cap_source: Some(CapSource::RereadCap),
        }
    );
}

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

fn since_and_give_up(status: &CacheKeepAliveStatus) -> (DateTime<Utc>, DateTime<Utc>) {
    let CacheKeepAliveStatus::Warming {
        since, give_up_at, ..
    } = *status
    else {
        panic!("a due thread is warming: {status:?}");
    };
    (since, give_up_at)
}

#[test]
fn a_real_idle_mid_wait_starts_a_run_but_keeps_the_give_up_time() {
    let on_children = Candidate {
        keep_warm_wait_secs: None,
        children: ActiveChildren::Working {
            wait_secs: 10.0 * MINUTE,
        },
        ..due()
    };
    for c in [due(), on_children] {
        let armed = anchors_of(&c);
        let (wait_since, give_up_at) = since_and_give_up(&status(&c, &armed, Spent::default()));
        assert_eq!(wait_since, secs_before_now(10.0 * MINUTE));

        let idle_mid_wait = Anchors {
            idle_at: Some(secs_before_now(2.0 * MINUTE)),
            ..armed
        };
        assert_eq!(
            since_and_give_up(&status(&c, &idle_mid_wait, Spent::default())),
            (secs_before_now(2.0 * MINUTE), give_up_at),
            "the user's turn names a new run, and moves no give-up time: {:?}",
            c.children
        );
        let budget = |anchors: &Anchors| match status(&c, anchors, Spent::default()) {
            CacheKeepAliveStatus::Warming { budget_beats, .. } => budget_beats,
            other => panic!("warming: {other:?}"),
        };
        assert!(
            budget(&idle_mid_wait) < budget(&armed),
            "the new run counts only the pings left to it"
        );

        let idle_before = Anchors {
            idle_at: Some(secs_before_now(30.0 * MINUTE)),
            ..armed
        };
        assert_eq!(
            since_and_give_up(&status(&c, &idle_before, Spent::default())),
            (wait_since, give_up_at)
        );
    }
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
        telemetry_owned_by_cli: false,
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
        GiveUpRule::PingPlan,
        &WaitDurations::default(),
        &anchors,
        None,
    )
    .await;
    let written = stored_status(&pool, thread_id).await;
    assert_eq!(
        kind_of(written.as_ref().expect("a status is written")),
        StatusKind::Warming
    );
    sync_status(
        &bus,
        thread_id,
        &candidate,
        GiveUpRule::PingPlan,
        &WaitDurations::default(),
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
        GiveUpRule::PingPlan,
        &WaitDurations::default(),
        &anchors_of(&busy),
        written,
    )
    .await;
    assert_eq!(
        status_events(&pool, thread_id).await,
        1,
        "a busy thread keeps its status"
    );

    close_stale_statuses(&bus, &[thread_id]).await;
    assert_eq!(
        status_events(&pool, thread_id).await,
        1,
        "a candidate keeps its status"
    );
    close_stale_statuses(&bus, &[]).await;
    assert!(
        matches!(
            stored_status(&pool, thread_id).await,
            Some(CacheKeepAliveStatus::Ended {
                beats: 0,
                saved_usd: None,
                ..
            })
        ),
        "a run that never pinged ends with no saving"
    );
    assert_eq!(status_events(&pool, thread_id).await, 2);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_answered_card_opens_no_run() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/answered", None).await;
    let candidate = parked();
    let anchors = anchors_of(&candidate);
    let durations = WaitDurations::default();
    let sync = || {
        sync_status(
            &bus,
            thread_id,
            &candidate,
            GiveUpRule::PingPlan,
            &durations,
            &anchors,
            None,
        )
    };

    set_status(&pool, thread_id, ThreadStatus::Running).await;
    sync().await;
    assert_eq!(
        status_events(&pool, thread_id).await,
        0,
        "the card was answered after the tick read it"
    );

    set_status(&pool, thread_id, ThreadStatus::WaitingForUserAnswer).await;
    sync().await;
    assert_eq!(
        kind_of(
            stored_status(&pool, thread_id)
                .await
                .as_ref()
                .expect("a parked card opens a run")
        ),
        StatusKind::Warming
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_run_that_pinged_ends_and_stays_ended() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/ends", None).await;
    set_status(&pool, thread_id, ThreadStatus::Idle).await;
    let c = due();
    let warming = |beats| {
        let spent = Spent {
            beats,
            cost_usd: 0.29,
            context_tokens: 118_000,
            ..Spent::default()
        };
        status(&c, &anchors_of(&c), spent)
    };
    let CacheKeepAliveStatus::Warming {
        since,
        beat_every_secs: interval,
        ref odds,
        ..
    } = warming(1)
    else {
        panic!("a due thread is warming");
    };

    emit_status(&bus, thread_id, Some(warming(23))).await;
    close_stale_statuses(&bus, &[]).await;
    let Some(CacheKeepAliveStatus::Ended {
        since: began,
        beats,
        cost_usd,
        saved_usd,
        beat_every_secs,
        odds: ended_odds,
        ..
    }) = stored_status(&pool, thread_id).await
    else {
        panic!("a run that pinged ends");
    };
    assert_eq!((began, beats, cost_usd, saved_usd), (since, 23, 0.29, None));
    assert_eq!(
        (beat_every_secs, &ended_odds),
        (Some(interval), odds),
        "the run closes on the math it showed"
    );
    let written = status_events(&pool, thread_id).await;
    close_stale_statuses(&bus, &[]).await;
    assert_eq!(
        status_events(&pool, thread_id).await,
        written,
        "an ended status is closed"
    );

    emit_status(&bus, thread_id, Some(warming(0))).await;
    set_status(&pool, thread_id, ThreadStatus::Running).await;
    close_stale_statuses(&bus, &[]).await;
    let Some(CacheKeepAliveStatus::Ended {
        beats,
        cost_usd,
        saved_usd,
        beat_every_secs,
        ..
    }) = stored_status(&pool, thread_id).await
    else {
        panic!("a run that never pinged ends too");
    };
    assert_eq!(
        (beats, cost_usd, saved_usd, beat_every_secs),
        (0, 0.29, None, Some(interval)),
        "it saved nothing and kept the math it showed, with no wake to wait for"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn place_pings_and_captures(pool: &sqlx::PgPool, thread_id: Uuid, ages_secs: &[f64]) {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM events WHERE thread_id = $1 \
            AND event_type IN ('ContextCaptured', 'PromptInjected') ORDER BY sequence",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(ids.len(), ages_secs.len(), "one age per event");
    sqlx::query(
        "UPDATE events e SET created = now() - make_interval(secs => a.secs) \
           FROM unnest($1::uuid[], $2::float8[]) AS a(id, secs) WHERE e.id = a.id",
    )
    .bind(ids)
    .bind(ages_secs)
    .execute(pool)
    .await
    .unwrap();
}

async fn run_end_of(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    began_secs: f64,
    ages_secs: &[f64],
) -> RunEnd {
    place_pings_and_captures(pool, thread_id, ages_secs).await;
    let since: DateTime<Utc> = sqlx::query_scalar("SELECT now() - make_interval(secs => $1)")
        .bind(began_secs)
        .fetch_one(pool)
        .await
        .unwrap();
    read_run_end(pool, thread_id, since, 118_000).await.unwrap()
}

#[tokio::test]
async fn the_saving_is_claimed_only_for_a_warm_wake() {
    let priced =
        reread_usd(118_000, "claude-opus-5-5", CacheLifetime::FiveMinutes).expect("priced");
    struct Case {
        label: &'static str,
        before_pings: [f64; 3],
        last_ping_age: f64,
        frames: &'static [(u32, f64)],
        input_tokens: u32,
        saved: Option<f64>,
    }
    let case = |label, last_ping_age, frames, saved| Case {
        label,
        before_pings: [1_200.0, 960.0, 959.0],
        last_ping_age,
        frames,
        input_tokens: 100_000,
        saved,
    };
    let cases = [
        case("a warm wake", 120.0, &[(100_000, 0.0)], Some(priced)),
        case("a system-prompt-only read", 120.0, &[(15_000, 0.0)], None),
        case("one token below half", 120.0, &[(49_999, 0.0)], None),
        case("exactly half", 120.0, &[(50_000, 0.0)], Some(priced)),
        case("a cold wake", 700.0, &[(100_000, 0.0)], None),
        case(
            "a wake that wrote the cache afresh",
            120.0,
            &[(0, 0.0)],
            None,
        ),
        case("a wait that stopped, with no request", 120.0, &[], None),
        case(
            "a cold wake of two frames, one earlier than the other",
            700.0,
            &[(100_000, 0.0), (100_000, 2.0)],
            None,
        ),
        case(
            "a warm wake of two frames",
            120.0,
            &[(100_000, 0.0), (100_000, 2.0)],
            Some(priced),
        ),
        Case {
            before_pings: [290.0, 240.0, 239.0],
            ..case(
                "a wake the last real request kept warm without the pings",
                5.0,
                &[(100_000, 0.0)],
                Some(0.0),
            )
        },
        Case {
            before_pings: [300.0, 240.0, 239.0],
            ..case(
                "a wake exactly one lifetime after the last real request",
                5.0,
                &[(100_000, 0.0)],
                Some(0.0),
            )
        },
        Case {
            before_pings: [301.0, 240.0, 239.0],
            ..case(
                "a wake just past the last real request's lifetime",
                5.0,
                &[(100_000, 0.0)],
                Some(priced),
            )
        },
    ];
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    for Case {
        label,
        before_pings,
        last_ping_age,
        frames,
        input_tokens,
        saved,
    } in cases
    {
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, "claude-code/saving", None).await;
        seed_thread_event(&bus, thread_id, capture_writing(40_000, 0, false)).await;
        seed_ping(&bus, thread_id, 100_000, 1).await;
        seed_ping(&bus, thread_id, 100_000, 1).await;
        let mut ages = before_pings.to_vec();
        ages.extend([last_ping_age + 1.0, last_ping_age]);
        for (read, age_secs) in frames {
            let mut capture = ping_capture(*read, "wake-call");
            if let ThreadEvent::ContextCaptured {
                usage: Some(usage), ..
            } = &mut capture
            {
                usage.input_tokens = input_tokens;
            }
            seed_thread_event(&bus, thread_id, capture).await;
            ages.push(*age_secs);
        }
        let end = run_end_of(&pool, thread_id, 1_100.0, &ages).await;
        assert_eq!(end.saved_usd, saved, "{label}");
        assert_eq!(
            end.at.is_some(),
            !frames.is_empty(),
            "{label}: when it woke"
        );
    }
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_run_ends_when_its_thread_moves_on_without_leaving_the_selection() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/moves-on", None).await;
    let c = due();
    let warming = |beats| {
        let spent = Spent {
            beats,
            cost_usd: 0.29,
            context_tokens: 118_000,
            ..Spent::default()
        };
        status(&c, &anchors_of(&c), spent)
    };
    let CacheKeepAliveStatus::Warming { since, .. } = warming(3) else {
        panic!("a due thread is warming");
    };
    emit_status(&bus, thread_id, Some(warming(3))).await;
    let stored = stored_status(&pool, thread_id).await;

    let idle = Candidate {
        keep_warm_wait_secs: None,
        ..due()
    };
    sync_status(
        &bus,
        thread_id,
        &idle,
        GiveUpRule::PingPlan,
        &WaitDurations::default(),
        &anchors_of(&idle),
        stored,
    )
    .await;
    let after = stored_status(&pool, thread_id).await;
    let Some(CacheKeepAliveStatus::Ended {
        since: began,
        beats,
        ..
    }) = after.clone()
    else {
        panic!("a run that pinged ends: {after:?}");
    };
    assert_eq!((began, beats), (since, 3));
    let written = status_events(&pool, thread_id).await;
    sync_status(
        &bus,
        thread_id,
        &idle,
        GiveUpRule::PingPlan,
        &WaitDurations::default(),
        &anchors_of(&idle),
        after,
    )
    .await;
    assert_eq!(
        status_events(&pool, thread_id).await,
        written,
        "an ended status stays"
    );

    let newer = Candidate {
        keep_warm_wait_secs: Some(60.0),
        ..due()
    };
    emit_status(&bus, thread_id, Some(warming(3))).await;
    let stored = stored_status(&pool, thread_id).await;
    sync_status(
        &bus,
        thread_id,
        &newer,
        GiveUpRule::PingPlan,
        &WaitDurations::default(),
        &anchors_of(&newer),
        stored,
    )
    .await;
    assert_eq!(
        kind_of_warming_or_ended(&stored_status(&pool, thread_id).await),
        "warming"
    );
    let history = status_history(&pool, thread_id).await;
    assert!(
        matches!(
            history[history.len() - 2],
            Some(CacheKeepAliveStatus::Ended { .. })
        ),
        "the old run ended before the new one: {history:?}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_run_that_pinged_ends_when_its_pings_stop_mid_wait() {
    let restarted = Candidate {
        agent: Agent::Lucidos(Remembered::Nothing),
        ..due()
    };
    let no_wake_children = Candidate {
        keep_warm_wait_secs: None,
        children: ActiveChildren::OnlyNoWake,
        ..due()
    };
    let cases = [
        (
            "the workspace mode turned off",
            due(),
            GiveUpRule::Off,
            None,
        ),
        (
            "a restart",
            restarted,
            GiveUpRule::PingPlan,
            Some(CacheKeepAliveNotKeptReason::Restarted),
        ),
        (
            "only no-wake children",
            no_wake_children,
            GiveUpRule::PingPlan,
            Some(CacheKeepAliveNotKeptReason::NoWakeChildren),
        ),
    ];
    for (label, candidate, rule, reason) in cases {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, "claude-code/pings-stop", None).await;
        let c = due();
        let spent = Spent {
            beats: 3,
            cost_usd: 0.29,
            context_tokens: 118_000,
            ..Spent::default()
        };
        emit_status(&bus, thread_id, Some(status(&c, &anchors_of(&c), spent))).await;
        let stored = stored_status(&pool, thread_id).await;

        let anchors = anchors_of(&candidate);
        sync_status(
            &bus,
            thread_id,
            &candidate,
            rule,
            &WaitDurations::default(),
            &anchors,
            stored,
        )
        .await;

        let history = status_history(&pool, thread_id).await;
        let after_warming = &history[1..];
        assert!(
            matches!(
                after_warming.first(),
                Some(Some(CacheKeepAliveStatus::Ended { beats: 3, .. }))
            ),
            "{label}: the run ends first: {history:?}"
        );
        let expected_last = reason.map(CacheKeepAliveStatus::not_kept);
        match expected_last {
            Some(last) => assert_eq!(
                after_warming,
                &[after_warming[0].clone(), Some(last)],
                "{label}: the reason follows"
            ),
            None => assert_eq!(after_warming.len(), 1, "{label}: {history:?}"),
        }
        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}

#[tokio::test]
async fn a_close_waits_for_the_wake_capture_while_the_thread_runs() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/wake", None).await;
    let since: DateTime<Utc> = sqlx::query_scalar("SELECT now() - interval '10 minutes'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let warming = CacheKeepAliveStatus::Warming {
        since,
        beats: 1,
        budget_beats: 12,
        beat_every_secs: 230,
        cost_usd: 0.05,
        context_tokens: 118_000,
        next_beat_at: None,
        give_up_at: since + Duration::minutes(40),
        ping_usd: None,
        reread_usd: None,
        odds: None,
        expected_wake: None,
        cap_source: Some(CapSource::HalfReread),
    };
    emit_status(&bus, thread_id, Some(warming.clone())).await;
    seed_ping(&bus, thread_id, 100_000, 1).await;
    set_status(&pool, thread_id, ThreadStatus::Running).await;

    close_stale_statuses(&bus, &[]).await;
    assert_eq!(
        stored_status(&pool, thread_id).await,
        Some(warming),
        "no wake capture yet, and the thread runs: the run stays open"
    );

    seed_thread_event(&bus, thread_id, ping_capture(100_000, "wake-call")).await;
    place_pings_and_captures(&pool, thread_id, &[120.0, 119.0, 0.0]).await;
    close_stale_statuses(&bus, &[]).await;
    let Some(CacheKeepAliveStatus::Ended { at, .. }) = stored_status(&pool, thread_id).await else {
        panic!("the run ends once the wake's capture landed");
    };
    let wake_started: DateTime<Utc> = sqlx::query_scalar(
        "SELECT max(created) - interval '1 second' FROM events \
          WHERE thread_id = $1 AND event_type = 'ContextCaptured'",
    )
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(at, wake_started, "the run ends when the wake began");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn beat_idle() -> ThreadEvent {
    let mut idle = real_idle();
    if let ThreadEvent::CodingAgentIdled { reason, .. } = &mut idle {
        *reason = Some(KEEP_ALIVE_IDLE_REASON.to_string());
    }
    idle
}

#[tokio::test]
async fn a_real_turn_mid_wait_ends_the_run_and_opens_the_next() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/mid-wait", None).await;
    let armed: DateTime<Utc> = sqlx::query_scalar("SELECT now() - interval '10 minutes'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let wait = Some(KeepWarmWait {
        armed_at: armed,
        kinds: kinds(&["BackgroundBashCompleted"]),
        expires_at: armed + Duration::hours(1),
    });
    let arming_idle = seed_thread_event(&bus, thread_id, real_idle()).await;
    age(&pool, arming_idle, 590.0).await;
    for idle_age in [478.0, 238.0] {
        seed_ping(&bus, thread_id, 100_000, 1).await;
        let idle = seed_thread_event(&bus, thread_id, beat_idle()).await;
        age(&pool, idle, idle_age).await;
    }
    place_pings_and_captures(&pool, thread_id, &[480.0, 479.0, 240.0, 239.0]).await;
    set_status(&pool, thread_id, ThreadStatus::Idle).await;
    let created = |id: Uuid| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, DateTime<Utc>>("SELECT created FROM events WHERE id = $1")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    let first_since = created(arming_idle).await;
    let tick = |stored: Option<CacheKeepAliveStatus>| {
        let (bus, pool) = (bus.clone(), pool.clone());
        async move {
            let row = candidate_row(&pool, thread_id).await.expect("a candidate");
            sync_status(
                &bus,
                thread_id,
                &due(),
                GiveUpRule::PingPlan,
                &WaitDurations::default(),
                &row.anchors(wait),
                stored,
            )
            .await;
            stored_status(&pool, thread_id).await
        }
    };

    let first = tick(None).await;
    assert!(
        matches!(
            first,
            Some(CacheKeepAliveStatus::Warming { since, beats: 2, .. }) if since == first_since
        ),
        "the run starts at the arming turn's idle, and the beats' idles move nothing: {first:?}"
    );

    seed_thread_event(&bus, thread_id, turn_capture(1_000)).await;
    place_pings_and_captures(&pool, thread_id, &[480.0, 479.0, 240.0, 239.0, 60.0]).await;
    let user_idle = seed_thread_event(&bus, thread_id, real_idle()).await;
    age(&pool, user_idle, 30.0).await;
    set_status(&pool, thread_id, ThreadStatus::Idle).await;
    let second_since = created(user_idle).await;

    let second = tick(first).await;
    assert!(
        matches!(
            second,
            Some(CacheKeepAliveStatus::Warming { since, beats: 0, .. }) if since == second_since
        ),
        "a new run, with no pings of its own: {second:?}"
    );
    let history = status_history(&pool, thread_id).await;
    assert!(
        matches!(
            history[history.len() - 2],
            Some(CacheKeepAliveStatus::Ended { since, beats: 2, .. }) if since == first_since
        ),
        "the old run ended before the new one warmed: {history:?}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

struct EndedByChild {
    pool: sqlx::PgPool,
    db_name: String,
    bus: EventBus,
    thread_id: Uuid,
    wait: Option<KeepWarmWait>,
    not_kept: Option<CacheKeepAliveStatus>,
}

impl EndedByChild {
    async fn new() -> Self {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, "claude-code/resumed-wait", None).await;
        let armed: DateTime<Utc> = sqlx::query_scalar("SELECT now() - interval '10 minutes'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let wait = Some(KeepWarmWait {
            armed_at: armed,
            kinds: kinds(&["BackgroundBashCompleted"]),
            expires_at: armed + Duration::hours(1),
        });
        let arming_idle = seed_thread_event(&bus, thread_id, real_idle()).await;
        age(&pool, arming_idle, 590.0).await;
        for idle_age in [478.0, 238.0] {
            seed_ping(&bus, thread_id, 100_000, 1).await;
            let idle = seed_thread_event(&bus, thread_id, beat_idle()).await;
            age(&pool, idle, idle_age).await;
        }
        place_pings_and_captures(&pool, thread_id, &[480.0, 479.0, 240.0, 239.0]).await;
        set_status(&pool, thread_id, ThreadStatus::Idle).await;
        let mut fixture = Self {
            pool,
            db_name,
            bus,
            thread_id,
            wait,
            not_kept: None,
        };
        let warming = fixture.tick(due(), None).await;
        assert!(
            matches!(
                warming,
                Some(CacheKeepAliveStatus::Warming { beats: 2, .. })
            ),
            "{warming:?}"
        );
        fixture.not_kept = fixture.tick(on_the_user(), warming).await;
        fixture
    }

    async fn tick(
        &self,
        candidate: Candidate,
        stored: Option<CacheKeepAliveStatus>,
    ) -> Option<CacheKeepAliveStatus> {
        let row = candidate_row(&self.pool, self.thread_id)
            .await
            .expect("a candidate");
        sync_status(
            &self.bus,
            self.thread_id,
            &candidate,
            GiveUpRule::PingPlan,
            &WaitDurations::default(),
            &row.anchors(self.wait),
            stored,
        )
        .await;
        stored_status(&self.pool, self.thread_id).await
    }

    async fn ended_runs(&self) -> Vec<u32> {
        status_history(&self.pool, self.thread_id)
            .await
            .into_iter()
            .filter_map(|s| match s {
                Some(CacheKeepAliveStatus::Ended { beats, .. }) => Some(beats),
                _ => None,
            })
            .collect()
    }

    async fn teardown(self) {
        self.pool.close().await;
        teardown_test_db(&self.db_name).await;
    }
}

fn on_the_user() -> Candidate {
    Candidate {
        keep_warm_wait_secs: None,
        children: ActiveChildren::AllAwaitingUser,
        ..due()
    }
}

#[tokio::test]
async fn a_resumed_wait_never_reopens_an_ended_run_as_stopped() {
    let fixture = EndedByChild::new().await;
    let resumed = Candidate {
        keep_warm_wait_secs: Some(10.0 * 60.0 * MINUTE),
        ..due()
    };
    fixture.tick(resumed, fixture.not_kept.clone()).await;

    let history = status_history(&fixture.pool, fixture.thread_id).await;
    let ended = history
        .iter()
        .position(|s| matches!(s, Some(CacheKeepAliveStatus::Ended { beats: 2, .. })))
        .unwrap_or_else(|| panic!("the run ended with its pings: {history:?}"));
    assert!(
        !history[ended..]
            .iter()
            .any(|s| matches!(s, Some(CacheKeepAliveStatus::Stopped { .. }))),
        "the ended run's pings never come back as a stopped run: {history:?}"
    );
    fixture.teardown().await;
}

#[tokio::test]
async fn a_resumed_run_ends_with_only_its_own_pings() {
    let fixture = EndedByChild::new().await;
    let resumed = fixture.tick(due(), fixture.not_kept.clone()).await;
    assert!(
        matches!(
            resumed,
            Some(CacheKeepAliveStatus::Warming { beats: 0, .. })
        ),
        "{resumed:?}"
    );
    seed_ping(&fixture.bus, fixture.thread_id, 100_000, 1).await;
    let warming = fixture.tick(due(), resumed).await;
    assert!(
        matches!(
            warming,
            Some(CacheKeepAliveStatus::Warming { beats: 1, .. })
        ),
        "{warming:?}"
    );
    fixture.tick(on_the_user(), warming).await;

    assert_eq!(fixture.ended_runs().await, vec![2, 1]);
    fixture.teardown().await;
}

fn kind_of_warming_or_ended(status: &Option<CacheKeepAliveStatus>) -> &'static str {
    match status {
        Some(CacheKeepAliveStatus::Warming { .. }) => "warming",
        Some(CacheKeepAliveStatus::Ended { .. }) => "ended",
        _ => "other",
    }
}

const ONE_GRAMMAR_MIGRATION: &str = include_str!(
    "../../../../lucidos-core/migrations/20261023203722_keep_alive_status_one_grammar.sql"
);

async fn insert_old_status(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    minutes_ago: i32,
    status: serde_json::Value,
) {
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2, 'CacheKeepAliveStatusChanged', $3, \
                 now() - make_interval(mins => $4), $5)",
    )
    .bind(Uuid::new_v4())
    .bind(thread_id.to_string())
    .bind(serde_json::json!({ "status": status }))
    .bind(minutes_ago)
    .bind(thread_id)
    .execute(pool)
    .await
    .unwrap();
}

fn old_warming(since: &str, beats: u32) -> serde_json::Value {
    serde_json::json!({
        "state": "warming", "since": since, "beats": beats, "budget_beats": 25,
        "beat_every_secs": 240, "cost_usd": 0.29, "context_tokens": 118_000,
        "give_up_at": "2026-10-23T18:00:00Z",
    })
}

async fn status_history(pool: &sqlx::PgPool, thread_id: Uuid) -> Vec<Option<CacheKeepAliveStatus>> {
    let rows: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'status' FROM events \
          WHERE thread_id = $1 AND event_type = 'CacheKeepAliveStatusChanged' \
          ORDER BY sequence",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    .unwrap();
    rows.into_iter()
        .map(|status| serde_json::from_value(status).expect("every row deserializes"))
        .collect()
}

#[tokio::test]
async fn the_migration_says_what_the_old_events_meant() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = |branch: &'static str| {
        let bus = &bus;
        async move {
            let thread_id = Uuid::new_v4();
            start_cc_session(bus, thread_id, branch, None).await;
            thread_id
        }
    };
    let began = "2026-10-23T14:02:00Z";
    let swapped = thread("claude-code/migrate-swapped").await;
    let turned_off = thread("claude-code/migrate-off").await;
    let never_pinged = thread("claude-code/migrate-idle").await;
    let stopped = thread("claude-code/migrate-stopped").await;
    let bare_clear = thread("claude-code/migrate-bare").await;
    let live = thread("claude-code/migrate-live").await;

    insert_old_status(&pool, swapped, 50, old_warming(began, 9)).await;
    insert_old_status(&pool, swapped, 40, old_warming("2026-10-23T14:30:00Z", 2)).await;
    insert_old_status(&pool, swapped, 30, serde_json::Value::Null).await;

    insert_old_status(&pool, turned_off, 50, old_warming(began, 9)).await;
    insert_old_status(
        &pool,
        turned_off,
        40,
        serde_json::json!({ "state": "not_kept", "reason": "turned_off" }),
    )
    .await;
    insert_old_status(&pool, turned_off, 30, serde_json::Value::Null).await;
    insert_old_status(
        &pool,
        turned_off,
        20,
        serde_json::json!({ "state": "not_kept", "reason": "turned_off" }),
    )
    .await;

    insert_old_status(&pool, never_pinged, 50, old_warming(began, 0)).await;
    insert_old_status(&pool, never_pinged, 40, serde_json::Value::Null).await;

    insert_old_status(
        &pool,
        stopped,
        50,
        serde_json::json!({
            "state": "stopped", "at": "2026-10-23T14:50:00Z", "beats": 31,
            "cost_usd": 0.41, "context_tokens": 118_000,
        }),
    )
    .await;
    insert_old_status(&pool, stopped, 40, serde_json::Value::Null).await;

    insert_old_status(&pool, bare_clear, 40, serde_json::Value::Null).await;
    insert_old_status(&pool, live, 40, old_warming(began, 4)).await;

    for _ in 0..2 {
        sqlx::raw_sql(ONE_GRAMMAR_MIGRATION)
            .execute(&pool)
            .await
            .unwrap();
    }

    let kinds = |history: Vec<Option<CacheKeepAliveStatus>>| -> Vec<&'static str> {
        history
            .iter()
            .map(|status| match status {
                None => "cleared",
                Some(CacheKeepAliveStatus::Warming { .. }) => "warming",
                Some(CacheKeepAliveStatus::Stopped { .. }) => "stopped",
                Some(CacheKeepAliveStatus::Ended { .. }) => "ended",
                Some(CacheKeepAliveStatus::TurnedOff { .. }) => "turned_off",
                Some(CacheKeepAliveStatus::NotKept { .. }) => "not_kept",
            })
            .collect()
    };
    let history = |id| status_history(&pool, id);

    let swapped_history = history(swapped).await;
    assert_eq!(
        kinds(swapped_history.clone()),
        ["warming", "warming", "ended"]
    );
    let Some(CacheKeepAliveStatus::Ended {
        since,
        beats,
        cost_usd,
        saved_usd,
        ..
    }) = swapped_history[2].clone()
    else {
        unreachable!("checked above")
    };
    assert_eq!(
        (since.to_rfc3339(), beats, cost_usd, saved_usd),
        ("2026-10-23T14:30:00+00:00".to_string(), 2, 0.29, None),
        "the run that closed, not the one before it"
    );

    let off_history = history(turned_off).await;
    assert_eq!(
        kinds(off_history.clone()),
        ["warming", "turned_off", "cleared", "turned_off"]
    );
    assert!(
        matches!(
            off_history[1],
            Some(CacheKeepAliveStatus::TurnedOff {
                beats: 9,
                context_tokens: 118_000,
                ..
            })
        ),
        "the interrupted run's pings and context: {:?}",
        off_history[1]
    );
    assert!(
        matches!(
            off_history[3],
            Some(CacheKeepAliveStatus::TurnedOff { beats: 0, .. })
        ),
        "no run before it: {:?}",
        off_history[3]
    );

    assert_eq!(kinds(history(never_pinged).await), ["warming", "cleared"]);
    assert_eq!(kinds(history(stopped).await), ["stopped", "cleared"]);
    assert_eq!(kinds(history(bare_clear).await), ["cleared"]);
    assert_eq!(kinds(history(live).await), ["warming"]);

    assert_eq!(
        stored_status(&pool, swapped).await,
        history(swapped).await[2],
        "the projection follows the newest event"
    );
    assert_eq!(stored_status(&pool, never_pinged).await, None);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

const TURNED_OFF_NEVER_ENDS_MIGRATION: &str =
    include_str!("../../../../lucidos-core/migrations/20261024061334_keep_alive_status_turned_off_never_ends.sql");

#[tokio::test]
async fn a_turned_off_run_never_ends_after_both_migrations() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let off = Uuid::new_v4();
    let restarted = Uuid::new_v4();
    start_cc_session(&bus, off, "claude-code/migrate-off-later", None).await;
    start_cc_session(&bus, restarted, "claude-code/migrate-restarted", None).await;
    let began = "2026-10-23T14:02:00Z";
    let not_kept = |reason: &str| serde_json::json!({ "state": "not_kept", "reason": reason });

    insert_old_status(&pool, off, 50, old_warming(began, 9)).await;
    insert_old_status(&pool, off, 40, not_kept("turned_off")).await;
    insert_old_status(&pool, off, 35, not_kept("children_awaiting_user")).await;
    insert_old_status(&pool, off, 30, serde_json::Value::Null).await;

    insert_old_status(&pool, restarted, 50, old_warming(began, 9)).await;
    insert_old_status(&pool, restarted, 40, not_kept("restarted")).await;
    insert_old_status(&pool, restarted, 30, serde_json::Value::Null).await;

    sqlx::raw_sql(ONE_GRAMMAR_MIGRATION)
        .execute(&pool)
        .await
        .unwrap();
    for _ in 0..2 {
        sqlx::raw_sql(TURNED_OFF_NEVER_ENDS_MIGRATION)
            .execute(&pool)
            .await
            .unwrap();
    }

    let off_history = status_history(&pool, off).await;
    assert!(
        matches!(off_history[1], Some(CacheKeepAliveStatus::TurnedOff { .. })),
        "{off_history:?}"
    );
    assert_eq!(off_history[3], None, "the clear stays a clear");
    assert_eq!(stored_status(&pool, off).await, None);
    assert_eq!(
        kind_of_warming_or_ended(&status_history(&pool, restarted).await[2]),
        "ended",
        "a restarted run still ends"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn age(pool: &sqlx::PgPool, event_id: Uuid, secs: f64) {
    sqlx::query("UPDATE events SET created = now() - make_interval(secs => $2) WHERE id = $1")
        .bind(event_id)
        .bind(secs)
        .execute(pool)
        .await
        .unwrap();
}

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
        permission_request: None,
    }
}

fn question_answered() -> ThreadEvent {
    ThreadEvent::UserQuestionAnswered {
        compose: None,
        tool_use_id: "toolu_ask".to_string(),
        answer: crate::engine::thread_events::AnswerKind::Selected {
            option_id: "opt-0".to_string(),
        },
        quiet: false,
    }
}

fn permission_requested() -> ThreadEvent {
    ThreadEvent::CodingAgentPermissionRequest {
        request_id: "perm-1".to_string(),
        tool_use_id: "toolu_p".to_string(),
        tool_name: "Bash".to_string(),
        input: serde_json::json!({}),
        summary: "Run a command".to_string(),
        grant_offer: None,
    }
}

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
    let line_secs = || async {
        use crate::engine::switch_estimate::{cache_remaining, CacheRemaining};
        match cache_remaining(&pool, thread_id).await {
            Ok(CacheRemaining::Warm(secs)) => Ok(Some(secs)),
            Ok(_) => Ok(None),
            Err(e) => Err(e),
        }
    };

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

    seed_thread_event(&bus, thread_id, question_answered()).await;
    seed_thread_event(&bus, thread_id, permission_requested()).await;
    set_status(&pool, thread_id, ThreadStatus::WaitingForUserAnswer).await;
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

#[tokio::test]
async fn a_held_parked_session_is_not_a_candidate() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/paused", None).await;
    seed_thread_event(&bus, thread_id, turn_capture(1_000)).await;
    seed_thread_event(&bus, thread_id, question_asked()).await;
    set_status(&pool, thread_id, ThreadStatus::WaitingForUserAnswer).await;
    let is_candidate = || async {
        read_candidates(&pool, &[])
            .await
            .unwrap()
            .iter()
            .any(|r| r.thread_id == thread_id)
    };
    assert!(is_candidate().await, "parked and not on hold");

    crate::test_support::seed_hold(&bus, thread_id).await;
    assert!(!is_candidate().await, "a thread on hold is never kept warm");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_parked_session_writes_its_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/parked-status", None).await;
    set_status(&pool, thread_id, ThreadStatus::WaitingForUserAnswer).await;
    let parked = parked();
    sync_status(
        &bus,
        thread_id,
        &parked,
        GiveUpRule::PingPlan,
        &WaitDurations::default(),
        &anchors_of(&parked),
        None,
    )
    .await;
    let written = stored_status(&pool, thread_id).await;
    let Some(CacheKeepAliveStatus::Warming { since, .. }) = written else {
        panic!("a parked session is warming: {written:?}");
    };
    assert_eq!(
        Some(since),
        anchors_of(&parked).turn_started_at,
        "the run began with the turn request"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_row_counts_the_pings_since_the_parking() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/pings-sent", None).await;
    let beat = |call: &str| {
        let mut event = copy_capture(1_000);
        if let ThreadEvent::ContextCaptured { api_call_id, .. } = &mut event {
            *api_call_id = Some(call.to_string());
        }
        event
    };
    seed_thread_event(&bus, thread_id, beat("earlier-wait")).await;
    seed_thread_event(&bus, thread_id, question_asked()).await;
    set_status(&pool, thread_id, ThreadStatus::WaitingForUserAnswer).await;
    let pings = || async {
        candidate_row(&pool, thread_id)
            .await
            .expect("a parked session")
            .pings_sent
    };
    assert_eq!(pings().await, 0);

    seed_thread_event(&bus, thread_id, beat("first")).await;
    seed_thread_event(&bus, thread_id, beat("first")).await;
    seed_thread_event(&bus, thread_id, beat("second")).await;
    assert_eq!(pings().await, 2);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn one_ping_history() -> WaitDurations {
    let question = WaitType::Parked(ParkedCard::Question);
    WaitDurations::from_samples((0..24u8).flat_map(|hour| {
        (0..10u32).map(move |i| {
            let start = LocalStart {
                hour,
                day: chrono::NaiveDate::from_ymd_opt(2026, 10, 1 + i % 7).expect("a date"),
            };
            (
                WaitAgent::ClaudeCode,
                question,
                WaitSample { secs: 300.0, start },
            )
        })
    }))
}

fn stopped_at(pings: u32) -> CacheKeepAliveStatus {
    CacheKeepAliveStatus::Stopped {
        at: now(),
        beats: pings,
        cost_usd: 0.1,
        context_tokens: 100_000,
        ping_usd: None,
        reread_usd: None,
        beat_every_secs: Some(230),
        odds: None,
        cap_source: Some(PlanEnd::Learned(learned_cap(pings)).cap_source()),
    }
}

async fn question_cap(pool: &sqlx::PgPool, thread_id: Uuid, history: &WaitDurations) -> ParkedCap {
    candidate_row(pool, thread_id)
        .await
        .expect("a parked session")
        .parked_cap(Agent::ClaudeCode, ParkedCard::Question, history)
}

#[tokio::test]
async fn a_parked_waits_cap_is_read_back_from_its_own_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/frozen-cap", None).await;
    seed_thread_event(&bus, thread_id, capture_writing(1_000, 0, false)).await;
    emit_status(&bus, thread_id, Some(stopped_at(5))).await;
    seed_thread_event(&bus, thread_id, question_asked()).await;
    set_status(&pool, thread_id, ThreadStatus::WaitingForUserAnswer).await;
    let learned = one_ping_history();
    let empty = WaitDurations::default();
    let cap = |history| question_cap(&pool, thread_id, history);
    let fresh =
        |cap: ParkedCap| matches!(cap, ParkedCap::Learned(c) if c.pings == 1 && c.samples == 50);

    assert!(
        fresh(cap(&learned).await),
        "an earlier wait's status never counts"
    );
    assert_eq!(cap(&empty).await, ParkedCap::HalfReread, "below the floor");

    let ended = CacheKeepAliveStatus::Ended {
        since: now(),
        at: now(),
        beats: 5,
        cost_usd: 0.1,
        saved_usd: None,
        context_tokens: None,
        ping_usd: None,
        reread_usd: None,
        beat_every_secs: Some(230),
        odds: None,
        cap_source: Some(PlanEnd::Learned(learned_cap(5)).cap_source()),
    };
    emit_status(&bus, thread_id, Some(ended)).await;
    assert!(fresh(cap(&learned).await), "an ended run never counts");
    emit_status(
        &bus,
        thread_id,
        Some(CacheKeepAliveStatus::past_cap(
            CacheKeepAliveNotKeptReason::PastGiveUpTime,
            CapSource::RereadCap,
        )),
    )
    .await;
    assert!(
        fresh(cap(&learned).await),
        "a source no parked plan has never counts"
    );

    emit_status(&bus, thread_id, Some(stopped_at(2))).await;
    emit_status(
        &bus,
        thread_id,
        Some(CacheKeepAliveStatus::past_cap(
            CacheKeepAliveNotKeptReason::PastGiveUpTime,
            CapSource::HalfReread,
        )),
    )
    .await;
    for (label, history) in [("a rebuild", &learned), ("a restart", &empty)] {
        assert_eq!(
            cap(history).await,
            ParkedCap::Learned(learned_cap(2)),
            "{label} keeps the first status's cap"
        );
    }

    seed_thread_event(&bus, thread_id, question_asked()).await;
    assert!(fresh(cap(&learned).await), "a new card starts a new wait");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn not_kept(reason: CacheKeepAliveNotKeptReason) -> CacheKeepAliveStatus {
    CacheKeepAliveStatus::not_kept(reason)
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

#[test]
fn the_status_names_why_nothing_keeps_a_thread_warm() {
    use CacheKeepAliveNotKeptReason as Reason;
    let cases: Vec<(&str, Candidate, GiveUpRule, Reason, Option<SkipReason>)> = vec![
        (
            "a Codex thread",
            Candidate {
                agent: Agent::Codex,
                ..parked()
            },
            GiveUpRule::PingPlan,
            Reason::Codex,
            Some(SkipReason::Codex),
        ),
        (
            "the mode off",
            parked(),
            GiveUpRule::Off,
            Reason::Off,
            Some(SkipReason::Disabled),
        ),
        (
            "a session that outlived a restart",
            restarted(),
            GiveUpRule::PingPlan,
            Reason::Restarted,
            Some(SkipReason::NoCopyPrompt),
        ),
        (
            "a parked thread no session holds",
            Candidate {
                claim: ThreadClaim::Free,
                ..parked()
            },
            GiveUpRule::PingPlan,
            Reason::Restarted,
            Some(SkipReason::AwaitingUser),
        ),
        (
            "a thread waiting on a card no session is parked beside",
            Candidate {
                awaiting_user: true,
                ..due()
            },
            GiveUpRule::PingPlan,
            Reason::NothingPending,
            Some(SkipReason::AwaitingUser),
        ),
        (
            "a thread that waits for nothing",
            Candidate {
                keep_warm_wait_secs: None,
                ..due()
            },
            GiveUpRule::PingPlan,
            Reason::NothingPending,
            Some(SkipReason::NothingToWaitFor),
        ),
    ];
    for (name, candidate, rule, reason, skipped) in cases {
        assert_eq!(
            super::status(
                &candidate,
                rule,
                &WaitDurations::default(),
                &anchors_of(&candidate),
                Spent::default()
            ),
            not_kept(reason),
            "{name}: the status"
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

#[test]
fn a_reason_that_names_no_wait_records_nothing() {
    use CacheKeepAliveNotKeptReason as Reason;
    for reason in [
        Reason::Codex,
        Reason::Off,
        Reason::NothingPending,
        Reason::UnattendedTrigger,
    ] {
        assert_eq!(not_kept(reason).recorded(), None, "{reason:?}");
    }
    for reason in [
        Reason::NoWakeChildren,
        Reason::ChildrenAwaitingUser,
        Reason::UncacheableProvider,
        Reason::ProviderPaused,
        Reason::PrefillPaused,
        Reason::PingsPaused,
        Reason::CacheExpired,
        Reason::Restarted,
    ] {
        assert!(not_kept(reason).recorded().is_some(), "{reason:?}");
    }
}

#[test]
fn a_parked_runs_give_up_time_is_the_time_decide_stops_at() {
    let give_up_after = give_up_after_secs(&parked(), GiveUpRule::PingPlan).expect("a budget");
    let at = |secs: f64| Candidate {
        parked: parked().parked.map(|p| Parked { secs, ..p }),
        secs_since_request: Some(window_start(CacheLifetime::FiveMinutes) + 1.0),
        ..parked()
    };
    let give_up_at = |c: &Candidate| {
        anchors_of(c).turn_started_at.expect("a parked turn")
            + Duration::milliseconds((give_up_after * 1000.0) as i64)
    };

    let before = at(give_up_after - 1.0);
    assert_eq!(
        super::decide(&before, GiveUpRule::PingPlan),
        Beat::Send(BeatRoute::SessionCopy)
    );
    let spent = Spent {
        beats: 3,
        cost_usd: 0.41,
        context_tokens: 182_000,
        ..Spent::default()
    };
    let CacheKeepAliveStatus::Warming {
        beats,
        cost_usd,
        context_tokens,
        give_up_at: until,
        since,
        ..
    } = super::status(
        &before,
        GiveUpRule::PingPlan,
        &WaitDurations::default(),
        &anchors_of(&before),
        spent,
    )
    else {
        panic!("a parked run before its give-up time is warming");
    };
    assert_eq!(
        (beats, cost_usd, context_tokens, until, Some(since)),
        (
            3,
            0.41,
            182_000,
            give_up_at(&before),
            anchors_of(&before).turn_started_at
        )
    );

    let after = at(give_up_after + 1.0);
    assert_eq!(
        super::decide(&after, GiveUpRule::PingPlan),
        Beat::Skip(SkipReason::PastCutOff)
    );
    assert_eq!(
        super::status(
            &after,
            GiveUpRule::PingPlan,
            &WaitDurations::default(),
            &anchors_of(&after),
            spent
        ),
        CacheKeepAliveStatus::Stopped {
            at: give_up_at(&after),
            beats: 3,
            cost_usd: 0.41,
            context_tokens: 182_000,
            ping_usd: None,
            reread_usd: None,
            beat_every_secs: after.beat_every_secs().map(|secs| secs as u32),
            odds: None,
            cap_source: Some(CapSource::HalfReread),
        }
    );
}

fn copy_ping(read: u32) -> ThreadEvent {
    copy_frame(read, &Uuid::new_v4().to_string())
}

fn copy_frame(read: u32, api_call_id: &str) -> ThreadEvent {
    let mut event = ping_capture(read, api_call_id);
    if let ThreadEvent::ContextCaptured { purpose, .. } = &mut event {
        *purpose = ContextPurpose::CacheKeepAlive;
    }
    event
}

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

#[tokio::test]
async fn a_thread_that_does_not_exist_is_not_a_candidate() {
    let (pool, db_name) = setup_test_db().await;
    let rows = read_candidates(&pool, &[Uuid::new_v4()]).await.unwrap();
    assert!(rows.is_empty());
    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn turned_off(c: Candidate) -> Candidate {
    Candidate {
        turned_off_at: Some(secs_before_now(MINUTE)),
        ..c
    }
}

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

#[test]
fn the_switch_shows_on_the_status() {
    let spent = Spent {
        beats: 3,
        cost_usd: 0.07,
        context_tokens: 118_000,
        ..Spent::default()
    };
    let off = |c: &Candidate| {
        super::status(
            c,
            GiveUpRule::PingPlan,
            &WaitDurations::default(),
            &anchors_of(c),
            spent,
        )
    };
    let expected = |c: &Candidate| CacheKeepAliveStatus::TurnedOff {
        at: secs_before_now(MINUTE),
        beats: 3,
        cost_usd: 0.07,
        context_tokens: 118_000,
        ping_usd: None,
        reread_usd: None,
        beat_every_secs: c.beat_every_secs().map(|secs| secs as u32),
        odds: None,
        cap_source: plan(c, GiveUpRule::PingPlan).map(|plan| plan.end.cap_source()),
    };
    for (label, c) in [
        ("an event wait", turned_off(due())),
        ("a parked card", turned_off(parked())),
        (
            "an idle thread",
            turned_off(Candidate {
                keep_warm_wait_secs: None,
                ..due()
            }),
        ),
    ] {
        assert_eq!(off(&c), expected(&c), "{label}");
    }
    let parked_off = turned_off(parked());
    assert_eq!(
        super::status(
            &parked_off,
            GiveUpRule::Off,
            &WaitDurations::default(),
            &anchors_of(&parked_off),
            spent
        ),
        not_kept(CacheKeepAliveNotKeptReason::Off),
        "the workspace setting says more"
    );
}

#[test]
fn a_warming_status_carries_the_reread_price() {
    let spent = Spent {
        context_tokens: 118_000,
        ..pinged()
    };
    let prices = |model: &str| match super::status(
        &due(),
        GiveUpRule::PingPlan,
        &WaitDurations::default(),
        &anchors_of(&due()),
        spent.priced(Some(model), CacheTtl::Reported(CacheLifetime::FiveMinutes)),
    ) {
        CacheKeepAliveStatus::Warming {
            ping_usd,
            reread_usd,
            ..
        } => (ping_usd, reread_usd),
        other => panic!("not warming: {other:?}"),
    };
    assert_eq!(
        prices("claude-opus-5-5"),
        (
            ping_usd(118_000, "claude-opus-5-5"),
            reread_usd(118_000, "claude-opus-5-5", CacheLifetime::FiveMinutes)
        )
    );
    assert_eq!(prices("no-such-model"), (None, None));
}

#[test]
fn a_warming_status_carries_the_odds_of_its_cap() {
    let c = due();
    let history = WaitDurations::from_samples([30.0, 400.0, 50_000.0].map(|secs| {
        (
            WaitAgent::ClaudeCode,
            wait_type(&c).expect("a wait"),
            WaitSample::at_noon(secs),
        )
    }));
    let CacheKeepAliveStatus::Warming {
        budget_beats, odds, ..
    } = super::status(
        &c,
        GiveUpRule::PingPlan,
        &history,
        &anchors_of(&c),
        pinged(),
    )
    else {
        panic!("not warming");
    };
    let odds = odds.expect("odds");
    assert_eq!(odds.samples, 3);
    assert_eq!(odds.slots.len(), budget_beats as usize + 1);
    assert_eq!(odds.slots[0] + odds.slots[1], 2, "30 s and 400 s stay warm");
    assert_eq!(odds.after_cap, 1, "50000 s outlasts the cap");
}

#[test]
fn a_closed_status_carries_the_odds_and_interval_of_its_cap() {
    let c = due();
    let history = WaitDurations::from_samples([30.0, 400.0, 50_000.0].map(|secs| {
        (
            WaitAgent::ClaudeCode,
            wait_type(&c).expect("a wait"),
            WaitSample::at_noon(secs),
        )
    }));
    let CacheKeepAliveStatus::Warming {
        beat_every_secs,
        odds,
        ..
    } = super::status(
        &c,
        GiveUpRule::PingPlan,
        &history,
        &anchors_of(&c),
        pinged(),
    )
    else {
        panic!("not warming");
    };
    assert!(odds.is_some());
    let long = Candidate {
        keep_warm_wait_secs: Some(10.0 * 3600.0),
        ..due()
    };
    let off = turned_off(due());
    for (label, c) in [("stopped", &long), ("turned off", &off)] {
        let closed = super::status(c, GiveUpRule::PingPlan, &history, &anchors_of(c), pinged());
        let (CacheKeepAliveStatus::Stopped {
            beat_every_secs: got,
            odds: got_odds,
            ..
        }
        | CacheKeepAliveStatus::TurnedOff {
            beat_every_secs: got,
            odds: got_odds,
            ..
        }) = closed
        else {
            panic!("{label}: not closed: {closed:?}");
        };
        assert_eq!(got, Some(beat_every_secs), "{label}");
        assert_eq!(got_odds, odds, "{label}");
    }
}

#[test]
fn a_status_stored_before_prices_still_deserializes() {
    for stored in [
        r#"{"state":"stopped","at":"2026-10-19T09:00:00Z","beats":3,"cost_usd":0.1,"context_tokens":1000}"#,
        r#"{"state":"ended","since":"2026-10-19T09:00:00Z","at":"2026-10-19T09:30:00Z","beats":3,"cost_usd":0.1}"#,
        r#"{"state":"warming","since":"2026-10-19T09:00:00Z","beats":1,"budget_beats":12,"beat_every_secs":270,"cost_usd":0.06,"context_tokens":1000,"give_up_at":"2026-10-19T09:54:00Z"}"#,
    ] {
        let status: CacheKeepAliveStatus = serde_json::from_str(stored).expect(stored);
        assert_eq!(
            serde_json::to_string(&status).unwrap(),
            stored,
            "round trip"
        );
    }
}

#[test]
fn the_reread_is_priced_as_a_cache_write() {
    let five = reread_usd(118_000, "claude-opus-5-5", CacheLifetime::FiveMinutes).expect("priced");
    let hour = reread_usd(118_000, "claude-opus-5-5", CacheLifetime::OneHour).expect("priced");
    assert!(five > 0.0 && hour > five, "5 min {five}, 1 h {hour}");
    assert_eq!(
        reread_usd(118_000, "no-such-model", CacheLifetime::FiveMinutes),
        None
    );
}

#[test]
fn a_ping_is_priced_as_a_cache_read() {
    let ping = ping_usd(118_000, "claude-opus-5-5").expect("priced");
    let reread =
        reread_usd(118_000, "claude-opus-5-5", CacheLifetime::FiveMinutes).expect("priced");
    assert!(
        ping > 0.0 && ping * 5.0 < reread,
        "ping {ping}, reread {reread}"
    );
    assert_eq!(ping_usd(118_000, "no-such-model"), None);
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

#[tokio::test]
async fn a_beat_is_never_a_real_request() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/real-request", None).await;
    let real = || async {
        let sql = format!(
            "SELECT {} AS started",
            crate::engine::session_capture_sql::last_real_request_started_sql("$1")
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
        crate::engine::session_capture_sql::last_turn_started_sql("$1")
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

#[tokio::test]
async fn a_trigger_thread_is_unattended_until_a_human_writes() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let fired = Uuid::new_v4();
    let agent_written = Uuid::new_v4();
    let human_written = Uuid::new_v4();
    let not_a_trigger = Uuid::new_v4();
    for thread_id in [fired, agent_written, human_written, not_a_trigger] {
        start_cc_session(&bus, thread_id, &format!("claude-code/{thread_id}"), None).await;
        if thread_id != not_a_trigger {
            sqlx::query(
                "UPDATE thread_summaries SET trigger_id = 'daily-digest' WHERE thread_id = $1",
            )
            .bind(thread_id)
            .execute(&pool)
            .await
            .unwrap();
        }
    }
    seed_thread_event(&bus, fired, keep_alive_anchor()).await;
    let mut agent_message = crate::test_support::message_received("check the feed");
    if let ThreadEvent::MessageReceived { mode, .. } = &mut agent_message {
        *mode = ActorMode::Agent;
    }
    seed_thread_event(&bus, agent_written, agent_message).await;
    seed_thread_event(
        &bus,
        human_written,
        crate::test_support::message_received("what did you find?"),
    )
    .await;

    for (thread_id, unattended, label) in [
        (fired, true, "only an injected prompt"),
        (agent_written, true, "only an agent-mode message"),
        (human_written, false, "a human message"),
        (not_a_trigger, false, "no trigger"),
    ] {
        let row = candidate_row(&pool, thread_id).await.expect(label);
        assert_eq!(row.unattended_trigger, unattended, "{label}");
    }

    pool.close().await;
    teardown_test_db(&db_name).await;
}

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
            sandbox: None,
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
            plan_path: None,
            cause: None,
            starts: crate::engine::thread_events::SessionStart::Now,
            crossing: None,
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

fn lucidos() -> Candidate {
    Candidate {
        agent: Agent::Lucidos(Remembered::Pair),
        secs_since_request: Some(265.0),
        ..due()
    }
}

fn remembering(remembered: Remembered) -> Candidate {
    Candidate {
        agent: Agent::Lucidos(remembered),
        ..lucidos()
    }
}

fn paused() -> Remembered {
    Remembered::Paused {
        until: chrono::DateTime::from_timestamp(1_900_000_000, 0).unwrap(),
    }
}

#[test]
fn a_remembered_pair_decides_a_stable_prefill_beat() {
    let candidate = remembering(Remembered::Pair);
    assert_eq!(decide(&candidate), prefill(PrefillCut::Stable));
}

#[test]
fn a_provider_pause_has_its_own_durable_reason() {
    let candidate = remembering(Remembered::ProviderPaused);
    assert_eq!(decide(&candidate), skip(SkipReason::ProviderPaused));
    let state = status_at(&candidate);
    assert_eq!(state, not_kept(CacheKeepAliveNotKeptReason::ProviderPaused));
    assert_eq!(state.clone().recorded(), Some(state));
}

fn lucidos_on_its_card() -> Candidate {
    Candidate {
        claim: ThreadClaim::Busy,
        awaiting_user: true,
        parked: Some(Parked {
            card: ParkedCard::Question,
            secs: 265.0,
            pings_sent: 0,
            copy_ready: false,
            cap: ParkedCap::HalfReread,
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

#[test]
fn a_prefill_beats_in_the_direct_window() {
    for (ttl, start, end) in [
        (CacheLifetime::FiveMinutes, 260.0, 280.0),
        (CacheLifetime::ThirtyMinutes, 1560.0, 1620.0),
    ] {
        let at = |secs: f64| Candidate {
            cache_ttl: CacheTtl::Reported(ttl),
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

#[test]
fn a_lucidos_agent_pair_with_an_unknown_ttl_never_beats() {
    let unknown = |secs: f64| Candidate {
        cache_ttl: CacheTtl::Unknown,
        secs_since_request: Some(secs),
        ..lucidos()
    };
    for secs in [265.0, 1560.0, 3500.0] {
        assert_eq!(
            decide(&unknown(secs)),
            skip(SkipReason::UnknownTtl),
            "{secs}"
        );
    }
    assert_eq!(
        super::status(
            &unknown(265.0),
            GiveUpRule::PingPlan,
            &WaitDurations::default(),
            &anchors_of(&unknown(265.0)),
            Spent::default()
        ),
        not_kept(CacheKeepAliveNotKeptReason::UnknownTtl)
    );
    let on_its_card = Candidate {
        cache_ttl: CacheTtl::Unknown,
        ..lucidos_on_its_card()
    };
    assert_eq!(decide(&on_its_card), skip(SkipReason::UnknownTtl));
}

#[test]
fn a_lucidos_agent_card_gives_up_at_half_the_budget() {
    let event = give_up_after_secs(&lucidos(), GiveUpRule::PingPlan).expect("a budget");
    assert!((event - 25.0 * 260.0).abs() < 1e-6, "{event}");
    let card = give_up_after_secs(&lucidos_on_its_card(), GiveUpRule::PingPlan).expect("a budget");
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

#[test]
fn an_openai_pair_spends_its_budget_at_the_thirty_minute_pace() {
    let gpt = |base: Candidate| Candidate {
        agent: Agent::Lucidos(Remembered::Pair),
        cache_ttl: CacheTtl::Reported(CacheLifetime::ThirtyMinutes),
        cache: cache_shares_of("gpt-6.1-sol"),
        ..base
    };
    let beats = CacheLifetime::ThirtyMinutes.rewrite_in_beats(cache_shares_of("gpt-6.1-sol"));
    let unpriced =
        CacheLifetime::ThirtyMinutes.rewrite_in_beats(price_table().unpriced_cache_shares);
    assert!(beats > unpriced, "GPT-6.1 is priced: {beats} beats");
    let event = give_up_after_secs(&gpt(lucidos()), GiveUpRule::PingPlan).expect("a budget");
    assert!((event - beats * 1560.0).abs() < 1e-6, "{event}");
    let card =
        give_up_after_secs(&gpt(lucidos_on_its_card()), GiveUpRule::PingPlan).expect("a budget");
    assert!((card - event / 2.0).abs() < 1e-6, "{card}");
    let waiting = |secs: f64| Candidate {
        keep_warm_wait_secs: Some(secs),
        secs_since_request: Some(1560.0),
        ..gpt(lucidos())
    };
    assert_eq!(decide(&waiting(event - 0.1)), prefill(PrefillCut::Stable));
    assert_eq!(decide(&waiting(event + 0.1)), skip(SkipReason::PastCutOff));
}

#[test]
fn off_beats_no_lucidos_agent_thread() {
    assert_eq!(
        super::decide(&lucidos(), GiveUpRule::Off),
        skip(SkipReason::Disabled)
    );
}

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
            "a beat that missed",
            remembering(Remembered::Missed),
            SkipReason::BeatMissed,
        ),
        (
            "a paused provider",
            remembering(paused()),
            SkipReason::PingsPaused,
        ),
        (
            "a request that outlived its cache",
            remembering(Remembered::Expired),
            SkipReason::CacheExpired,
        ),
        (
            "a wait that gave up",
            remembering(Remembered::GaveUp),
            SkipReason::PastCutOff,
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

#[test]
fn a_restart_causes_no_beat_burst() {
    let store = super::super::RememberedRequests::default();
    let thread = Uuid::new_v4();
    let booted = remembering(store.state(thread, chrono::Utc::now()));
    assert_eq!(decide(&booted), skip(SkipReason::Restarted));
    assert_eq!(
        kind_of(&status(&booted, &anchors_of(&booted), Spent::default())),
        StatusKind::NotKept(CacheKeepAliveNotKeptReason::Restarted)
    );
}

#[test]
fn a_stopped_run_stays_stopped_after_its_request_is_forgotten() {
    let past = Candidate {
        keep_warm_wait_secs: Some(10.0 * 3600.0),
        ..remembering(Remembered::Nothing)
    };
    assert_eq!(decide(&past), skip(SkipReason::PastCutOff));
    assert_eq!(
        kind_of(&status(&past, &anchors_of(&past), pinged())),
        StatusKind::Stopped
    );
}

#[test]
fn a_run_that_never_pinged_never_reads_stopped() {
    use CacheKeepAliveNotKeptReason as Reason;
    let past = |c: Candidate| Candidate {
        keep_warm_wait_secs: Some(10.0 * 3600.0),
        ..c
    };
    let kind = |c: &Candidate| kind_of(&status(c, &anchors_of(c), Spent::default()));
    assert_eq!(
        kind(&past(remembering(Remembered::Uncacheable))),
        StatusKind::NotKept(Reason::UncacheableProvider)
    );
    assert_eq!(
        kind(&past(remembering(Remembered::Nothing))),
        StatusKind::NotKept(Reason::Restarted)
    );
    assert_eq!(
        kind(&past(due())),
        StatusKind::NotKept(Reason::PastGiveUpTime)
    );
    assert_eq!(
        kind(&past(lucidos())),
        StatusKind::NotKept(Reason::PastGiveUpTime)
    );
}

#[test]
fn a_budget_under_one_beat_reads_past_give_up_time() {
    let c = Candidate {
        keep_warm_wait_secs: Some(10.0 * 3600.0),
        cache: CacheShares {
            read: 1.0,
            write_5m: 0.5,
            write_1h: 0.5,
            write_12h: 0.5,
        },
        ..remembering(Remembered::Nothing)
    };
    let budget = give_up_after_secs(&c, GiveUpRule::PingPlan).expect("a budget")
        / c.beat_every_secs().expect("a known TTL");
    assert!(budget < 1.0, "the budget is {budget} beats");
    assert_eq!(
        kind_of(&status(&c, &anchors_of(&c), Spent::default())),
        StatusKind::NotKept(CacheKeepAliveNotKeptReason::PastGiveUpTime)
    );
}

#[test]
fn a_lucidos_agent_status_beats_every_direct_window() {
    let c = lucidos();
    let at = anchors_of(&c);
    let CacheKeepAliveStatus::Warming {
        beat_every_secs,
        next_beat_at,
        ..
    } = status(&c, &at, Spent::default())
    else {
        panic!("a waiting Lucidos Agent thread is warming");
    };
    assert_eq!(beat_every_secs, 260);
    assert_eq!(
        next_beat_at,
        at.request_started_at.map(|t| t + Duration::seconds(260))
    );
}

#[test]
fn the_status_covers_a_lucidos_agent_card() {
    use CacheKeepAliveNotKeptReason as Reason;
    assert_eq!(
        kind_of(&status_at(&lucidos_on_its_card())),
        StatusKind::Warming
    );
    let on_card = |remembered| Candidate {
        agent: Agent::Lucidos(remembered),
        ..lucidos_on_its_card()
    };
    assert_eq!(
        status_at(&on_card(Remembered::Uncacheable)),
        not_kept(Reason::UncacheableProvider)
    );
    assert_eq!(
        status_at(&on_card(Remembered::Nothing)),
        not_kept(Reason::Restarted)
    );
    assert_eq!(
        status_at(&on_card(Remembered::Missed)),
        not_kept(Reason::BeatMissed)
    );
    assert_eq!(
        status_at(&on_card(Remembered::Expired)),
        not_kept(Reason::CacheExpired)
    );
    let Remembered::Paused { until } = paused() else {
        unreachable!()
    };
    assert_eq!(
        status_at(&on_card(paused())),
        CacheKeepAliveStatus::pings_paused(until)
    );
    assert_eq!(
        status_at(&Candidate {
            keep_warm_wait_secs: None,
            ..lucidos()
        }),
        not_kept(Reason::NothingPending)
    );
}

#[test]
fn a_lucidos_agent_card_past_its_budget_reads_stopped() {
    let card = give_up_after_secs(&lucidos_on_its_card(), GiveUpRule::PingPlan).expect("a budget");
    let past = Candidate {
        agent: Agent::Lucidos(Remembered::Nothing),
        parked: lucidos_on_its_card().parked.map(|p| Parked {
            secs: card + 0.1,
            ..p
        }),
        ..lucidos_on_its_card()
    };
    assert_eq!(
        kind_of(&status(&past, &anchors_of(&past), pinged())),
        StatusKind::Stopped
    );
}

fn pinged() -> Spent {
    Spent {
        beats: 1,
        ..Spent::default()
    }
}

fn status_at(c: &Candidate) -> CacheKeepAliveStatus {
    status(c, &anchors_of(c), Spent::default())
}

#[test]
fn a_beat_is_judged_by_its_usage_block() {
    let usage = |input: u32, wrote: u32, output: u32| crate::engine::ApiUsage {
        input_tokens: input,
        cache_creation_tokens: wrote,
        cache_read_tokens: input - wrote,
        output_tokens: output,
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
            "only the system prompt held",
            crate::engine::ApiUsage {
                cache_read_tokens: 15_000,
                ..usage(100_000, 0, 0)
            },
            Some(0),
            BeatOutcome::Missed,
        ),
        (
            "a cold write keeps the prefill probe guard",
            usage(100_000, 85_000, 0),
            Some(0),
            BeatOutcome::Wrote,
        ),
        (
            "no input reported",
            usage(0, 0, 0),
            Some(0),
            BeatOutcome::Missed,
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
    let idle = (Utc::now() - waiting.idle_at.expect("its turn ended")).num_seconds();
    assert!((idle - 120).abs() < 5, "{idle}");

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
        usage: Some(crate::engine::ApiUsage {
            input_tokens: input,
            cache_read_tokens: input - wrote,
            cache_creation_tokens: wrote,
            output_tokens: output,
            ..Default::default()
        }),
        ..Default::default()
    }))
}

fn beat_body() -> PrefillBody {
    let mut body =
        PrefillBody::for_test(Some(crate::llm::ProviderKind::Anthropic), "claude-opus-5-5");
    body.reasoning_effort = Some("high".into());
    body.stable_prefix_fingerprint = Some("0123456789abcdef".into());
    body
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

    let mut provider = beat_usage(90_000, 0, 0);
    provider.0.as_mut().unwrap().provider = Some(crate::llm::ProviderKind::Vertex);
    beat_once(
        &bus,
        &store,
        &provider,
        thread_id,
        &store.pair(thread_id, Utc::now()).unwrap(),
        PrefillCut::Stable,
    )
    .await;

    assert_eq!(event_count(&pool, thread_id).await, events + 1, "one event");
    let captures =
        crate::test_support::aux_captures(&pool, thread_id, &ContextPurpose::CacheKeepAlive.wire())
            .await;
    let [capture] = captures.as_slice() else {
        panic!("expected one beat capture, saw {}", captures.len());
    };
    assert_eq!(capture["provider"], "vertex");
    assert!(capture["duration_ms"].is_u64(), "{capture}");
    assert_eq!(capture["usage"]["cache_read_tokens"], 90_000, "{capture}");
    assert_eq!(capture["model"], "claude-opus-5-5", "{capture}");
    assert_eq!(
        capture["stable_prefix_fingerprint"], "0123456789abcdef",
        "{capture}"
    );
    assert_eq!(
        summary_row(&pool, thread_id).await,
        summary,
        "the summary stands"
    );

    let refresh: f64 = sqlx::query_scalar(&format!(
        "SELECT EXTRACT(EPOCH FROM now() - {})::float8",
        crate::engine::session_capture_sql::last_cache_refresh_started_sql("$1")
    ))
    .bind(thread_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(refresh < 5.0, "the refresh is the beat's start: {refresh}");
    assert_eq!(
        store.state(thread_id, chrono::Utc::now()),
        Remembered::Pair,
        "a read keeps the request"
    );
    assert!(!store.unproven(crate::llm::ProviderKind::Anthropic));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_writing_beat_ends_only_its_own_run() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = crate::test_support::seed_chat_thread(&bus, "hello").await;
    let store = remembered_store(thread_id);
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
    assert!(
        store.unproven(crate::llm::ProviderKind::Anthropic),
        "the first beat"
    );

    beat_once(
        &bus,
        &store,
        &beat_usage(100_000, 5_000, 0),
        thread_id,
        &store.pair(thread_id, Utc::now()).unwrap(),
        PrefillCut::Stable,
    )
    .await;

    let now = chrono::Utc::now();
    assert_eq!(store.state(thread_id, now), Remembered::Missed);
    assert!(store.pair(thread_id, now).is_none(), "the run ended");
    assert_eq!(store.state(other, now), Remembered::Pair);
    assert!(store.pair(other, now).is_some(), "the other thread beats");
    let missed = remembering(store.state(thread_id, now));
    assert_eq!(decide(&missed), skip(SkipReason::BeatMissed));
    assert_eq!(
        status_at(&missed),
        not_kept(CacheKeepAliveNotKeptReason::BeatMissed)
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_run_a_beat_ended_counts_that_beat() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = crate::test_support::seed_chat_thread(&bus, "hello").await;
    set_status(&pool, thread_id, ThreadStatus::Idle).await;
    let since: DateTime<Utc> = sqlx::query_scalar("SELECT now() - interval '5 minutes'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let warming = CacheKeepAliveStatus::Warming {
        since,
        beats: 0,
        budget_beats: 12,
        beat_every_secs: 260,
        cost_usd: 0.0,
        context_tokens: 79_000,
        next_beat_at: None,
        give_up_at: since + Duration::minutes(40),
        ping_usd: None,
        reread_usd: None,
        odds: None,
        expected_wake: None,
        cap_source: Some(CapSource::HalfReread),
    };
    emit_status(&bus, thread_id, Some(warming)).await;

    let store = remembered_store(thread_id);
    beat_once(
        &bus,
        &store,
        &beat_usage(100_000, 79_000, 0),
        thread_id,
        &store.pair(thread_id, Utc::now()).unwrap(),
        PrefillCut::Stable,
    )
    .await;
    close_stale_statuses(&bus, &[]).await;

    let Some(CacheKeepAliveStatus::Ended {
        beats, cost_usd, ..
    }) = stored_status(&pool, thread_id).await
    else {
        panic!("the run ends");
    };
    assert_eq!(beats, 1, "the missed beat counts");
    assert!(cost_usd > 0.0, "and so does its write: {cost_usd}");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_refused_first_beat_marks_its_provider() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = crate::test_support::seed_chat_thread(&bus, "hello").await;
    let refused = PrefillStub(Err("max_tokens: must be at least 1"));

    let first = remembered_store(thread_id);
    beat_once(
        &bus,
        &first,
        &refused,
        thread_id,
        &first.pair(thread_id, Utc::now()).unwrap(),
        PrefillCut::Stable,
    )
    .await;
    assert_eq!(
        first.state(thread_id, chrono::Utc::now()),
        Remembered::ProviderPaused
    );

    let proven = remembered_store(thread_id);
    proven.mark_proven(crate::llm::ProviderKind::Anthropic);
    beat_once(
        &bus,
        &proven,
        &refused,
        thread_id,
        &proven.pair(thread_id, Utc::now()).unwrap(),
        PrefillCut::Stable,
    )
    .await;
    assert_eq!(
        proven.state(thread_id, chrono::Utc::now()),
        Remembered::Pair
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

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
        &store.pair(thread_id, Utc::now()).unwrap(),
        PrefillCut::Stable,
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
    assert_eq!(
        store.state(other, chrono::Utc::now()),
        Remembered::ProviderPaused
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn a_missed_beat_stops_every_route_even_after_forgetting_the_pair() {
    for base in [
        due(),
        parked(),
        lucidos(),
        lucidos_on_its_card(),
        remembering(Remembered::Nothing),
        remembering(Remembered::Uncacheable),
    ] {
        let candidate = Candidate {
            beat_missed: true,
            ..base
        };
        assert_eq!(decide(&candidate), skip(SkipReason::BeatMissed));
        let state = status_at(&candidate);
        assert_eq!(state, not_kept(CacheKeepAliveNotKeptReason::BeatMissed));
        assert_eq!(state.clone().recorded(), Some(state));
    }
}

#[tokio::test]
async fn the_held_predicate_agrees_with_sql_at_every_boundary() {
    let (pool, db_name) = setup_test_db().await;
    for (read, input, want) in [
        (0, 0, false),
        (0, 1, false),
        (1, 1, true),
        (49_999, 100_000, false),
        (50_000, 100_000, true),
        (50_000, 100_001, false),
        (50_001, 100_001, true),
        (98_000, 100_000, true),
    ] {
        let sql: bool = sqlx::query_scalar(&format!(
            "SELECT {}",
            held_context_sql("$1::bigint", "$2::bigint")
        ))
        .bind(read as i64)
        .bind(input as i64)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(held_context(read, input), want, "{read}/{input}");
        assert_eq!(sql, want, "SQL: {read}/{input}");
    }
    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn policy_capture(route: BeatRoute, read: u32, input: u32, call: &str) -> ThreadEvent {
    let mut event = ping_capture(read, call);
    if let ThreadEvent::ContextCaptured {
        usage: Some(usage),
        purpose,
        ..
    } = &mut event
    {
        usage.input_tokens = input;
        usage.cache_creation_tokens = input.saturating_sub(read);
        if route != BeatRoute::Resume {
            *purpose = ContextPurpose::CacheKeepAlive;
        }
    }
    event
}

async fn policy_beat(
    bus: &EventBus,
    thread_id: Uuid,
    route: BeatRoute,
    read: u32,
    input: u32,
    call: &str,
) {
    let anchor = match route {
        BeatRoute::Resume => Some(seed_thread_event(bus, thread_id, keep_alive_anchor()).await),
        _ => None,
    };
    bus.emit(BusEvent::Thread {
        thread_id,
        event: policy_capture(route, read, input, call),
        meta: EventMeta {
            request_event_id: anchor,
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_missed_beat_latches_from_events_until_a_real_request() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let restarted = sqlx::PgPool::connect_with(pool.connect_options().as_ref().clone())
        .await
        .unwrap();
    for route in [
        BeatRoute::Resume,
        BeatRoute::SessionCopy,
        BeatRoute::Prefill(PrefillCut::Stable),
    ] {
        let thread_id = if matches!(route, BeatRoute::Prefill(_)) {
            crate::test_support::seed_chat_thread(&bus, "hello").await
        } else {
            let thread_id = Uuid::new_v4();
            start_cc_session(&bus, thread_id, "claude-code/beat-policy", None).await;
            thread_id
        };
        let real = seed_thread_event(&bus, thread_id, turn_capture(1_000)).await;
        age(&pool, real, 120.0).await;
        if route == BeatRoute::SessionCopy {
            let mut capture = policy_capture(route, 98_000, 100_000, "held-copy");
            if let ThreadEvent::ContextCaptured {
                usage: Some(usage), ..
            } = &mut capture
            {
                usage.cache_creation_tokens = 1_800;
            }
            seed_thread_event(&bus, thread_id, capture).await;
        } else {
            policy_beat(&bus, thread_id, route, 98_000, 100_000, "held-copy").await;
        }
        assert!(candidate_row(&pool, thread_id)
            .await
            .unwrap()
            .missed_beat
            .is_none());
        policy_beat(&bus, thread_id, route, 15_000, 100_000, "missed-beat").await;
        for connection in [&pool, &restarted, &pool] {
            let row = candidate_row(connection, thread_id).await.unwrap();
            let miss = row
                .missed_beat
                .expect("the miss survives reopening the connection");
            assert_eq!((miss.read, miss.input), (15_000, 100_000));
            let candidate = Candidate {
                beat_missed: true,
                ..due()
            };
            assert_eq!(decide(&candidate), skip(SkipReason::BeatMissed));
        }
        policy_beat(&bus, thread_id, route, 98_000, 100_000, "late-held-beat").await;
        assert!(
            candidate_row(&pool, thread_id)
                .await
                .unwrap()
                .missed_beat
                .is_some(),
            "a later beat cannot re-arm the run"
        );
        seed_thread_event(&bus, thread_id, ping_capture(100_000, "real-wake")).await;
        assert!(
            candidate_row(&restarted, thread_id)
                .await
                .unwrap()
                .missed_beat
                .is_none(),
            "the next real request re-arms it"
        );
    }
    restarted.close().await;
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn only_the_final_frame_judges_a_beat() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/beat-frames", None).await;
    let real = seed_thread_event(&bus, thread_id, turn_capture(1_000)).await;
    age(&pool, real, 120.0).await;
    let anchor = seed_thread_event(&bus, thread_id, keep_alive_anchor()).await;
    for read in [0, 98_000] {
        bus.emit(BusEvent::Thread {
            thread_id,
            event: policy_capture(BeatRoute::Resume, read, 100_000, "one-call"),
            meta: EventMeta {
                request_event_id: Some(anchor),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();
    }
    assert!(candidate_row(&pool, thread_id)
        .await
        .unwrap()
        .missed_beat
        .is_none());
    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn a_released_request_reports_give_up_instead_of_restart() {
    assert_eq!(
        status_at(&remembering(Remembered::GaveUp)),
        CacheKeepAliveStatus::past_cap(
            CacheKeepAliveNotKeptReason::PastGiveUpTime,
            CapSource::RereadCap
        )
    );
}

#[tokio::test]
async fn an_old_prefill_reply_leaves_the_new_request_eligible() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = crate::test_support::seed_chat_thread(&bus, "hello").await;
    for wrote in [0, 79_000] {
        let store = remembered_store(thread_id);
        let sent = store.pair(thread_id, Utc::now()).unwrap();
        store.remember(thread_id, Some((*sent).clone()), std::time::Instant::now());
        beat_once(
            &bus,
            &store,
            &beat_usage(100_000, wrote, 0),
            thread_id,
            &sent,
            PrefillCut::Stable,
        )
        .await;
        assert_eq!(store.state(thread_id, Utc::now()), Remembered::Pair);
    }
    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn warming_times(status: CacheKeepAliveStatus) -> (DateTime<Utc>, DateTime<Utc>) {
    let CacheKeepAliveStatus::Warming {
        since, give_up_at, ..
    } = status
    else {
        panic!("a warming run, got {status:?}");
    };
    (since, give_up_at)
}

#[test]
fn a_wake_past_the_cap_sends_no_pings_and_says_why() {
    for base in [due(), lucidos()] {
        let c = expecting(base, 0.0, 90.0 * MINUTE, ReEstimate::NotAsked);
        assert_eq!(decide(&c), skip(SkipReason::WakePastCap));
        let state = status(&c, &anchors_of(&c), Spent::default());
        assert_eq!(
            state,
            CacheKeepAliveStatus::past_cap(
                CacheKeepAliveNotKeptReason::WakePastRereadCap,
                CapSource::Estimate {
                    by: WakeSource::Agent
                }
            ),
            "the status names who expects the wake"
        );
        assert_eq!(state.clone().recorded(), Some(state), "the card says why");
    }
    let c = expecting(due(), 3.0 * MINUTE, 90.0 * MINUTE, ReEstimate::NotAsked);
    let spent = Spent {
        beats: 1,
        cost_usd: 0.02,
        ..Spent::default()
    };
    let CacheKeepAliveStatus::Stopped { at, .. } = status(&c, &anchors_of(&c), spent) else {
        panic!("a run that pinged stops");
    };
    assert_eq!(at, c.wake.expect("an estimate").stated_at);
}

#[test]
fn a_plan_that_runs_out_asks_again_once_then_stops() {
    let ran_out = |asked| expecting(due(), 0.0, 5.0 * MINUTE, asked);
    for asked in [ReEstimate::NotAsked, ReEstimate::Asking] {
        let c = ran_out(asked);
        assert_eq!(decide(&c), skip(SkipReason::PlanRanOut), "{asked:?}");
        let state = status(&c, &anchors_of(&c), Spent::default());
        assert_eq!(kind_of(&state), StatusKind::Warming, "{asked:?}");
    }
    let c = ran_out(ReEstimate::Asked);
    assert_eq!(decide(&c), skip(SkipReason::PastCutOff));
    let spent = Spent {
        beats: 2,
        ..Spent::default()
    };
    assert_eq!(
        kind_of(&status(&c, &anchors_of(&c), spent)),
        StatusKind::Stopped
    );
    let restated = expecting(due(), 10.0 * MINUTE, 5.0 * MINUTE, ReEstimate::NotAsked);
    assert_eq!(decide(&restated), Beat::Send(BeatRoute::Resume));
}

#[test]
fn no_estimate_moves_a_run_past_the_safety_cap() {
    let safety = 25.0 * window_start(CacheLifetime::FiveMinutes);
    let late = Candidate {
        keep_warm_wait_secs: Some(safety - 60.0),
        ..due()
    };
    for minutes in [1.0, 5.0, 20.0] {
        let c = expecting(
            late.clone(),
            safety - 60.0,
            minutes * MINUTE,
            ReEstimate::NotAsked,
        );
        let give_up = give_up_after_secs(&c, GiveUpRule::PingPlan).expect("a plan");
        assert!(give_up <= safety + 1e-6, "{minutes} min: {give_up}");
    }
}

#[test]
fn no_ping_lands_past_the_wait_timeout() {
    let expired = Candidate {
        timeout_secs: Some(8.0 * MINUTE),
        ..due()
    };
    assert_eq!(decide(&expired), skip(SkipReason::PastCutOff));
    let open = Candidate {
        timeout_secs: Some(12.0 * MINUTE),
        ..due()
    };
    assert_eq!(decide(&open), Beat::Send(BeatRoute::Resume));
    let (since, give_up_at) = warming_times(status(&open, &anchors_of(&open), Spent::default()));
    assert_eq!(give_up_at, after_secs(since, 12.0 * MINUTE));
}

#[test]
fn a_user_message_moves_the_give_up_only_through_a_new_estimate() {
    let c = expecting(due(), 0.0, 20.0 * MINUTE, ReEstimate::NotAsked);
    let at = anchors_of(&c);
    let later_idle = Anchors {
        idle_at: Some(secs_before_now(2.0 * MINUTE)),
        ..at
    };
    let (first_run, before) = warming_times(status(&c, &at, Spent::default()));
    let (second_run, after) = warming_times(status(&c, &later_idle, Spent::default()));
    assert_eq!(before, after);
    assert_ne!(first_run, second_run, "the idle opens a new run");
    let restated = expecting(due(), 8.0 * MINUTE, 20.0 * MINUTE, ReEstimate::NotAsked);
    let (_, moved) = warming_times(status(&restated, &later_idle, Spent::default()));
    assert!(
        moved > before,
        "the new estimate moves it: {moved} after {before}"
    );
}

#[test]
fn a_warming_status_shows_the_expected_wake() {
    let c = expecting(due(), 0.0, 20.0 * MINUTE, ReEstimate::NotAsked);
    let CacheKeepAliveStatus::Warming {
        since,
        give_up_at,
        budget_beats,
        expected_wake,
        ..
    } = status(&c, &anchors_of(&c), Spent::default())
    else {
        panic!("a warming run");
    };
    let planned = 25.0 * MINUTE;
    assert_eq!(give_up_at, after_secs(since, planned));
    assert_eq!(
        budget_beats,
        (planned / window_start(CacheLifetime::FiveMinutes)).floor() as u32
    );
    let at = c.wake.as_ref().expect("an estimate").shown.at;
    assert_eq!(
        expected_wake,
        Some(PlannedWake {
            at,
            source: WakeSource::Agent,
        })
    );
    let parked_with_a_wake = expecting(parked(), 0.0, 1.0, ReEstimate::NotAsked);
    assert_eq!(
        give_up_after_secs(&parked_with_a_wake, GiveUpRule::PingPlan),
        give_up_after_secs(&parked(), GiveUpRule::PingPlan),
    );
}

#[tokio::test]
async fn an_expected_wake_counts_for_the_wait_its_turn_covered() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/expected-wake", None).await;
    let stated: DateTime<Utc> = sqlx::query_scalar("SELECT now()")
        .fetch_one(&pool)
        .await
        .unwrap();
    let wake = ThreadEvent::WakeExpected {
        at: stated + Duration::minutes(20),
        distribution: vec![],
        source: WakeSource::Agent,
        reason: "the release build".into(),
    };
    seed_thread_event(&bus, thread_id, wake).await;
    seed_thread_event(&bus, thread_id, beat_idle()).await;
    let row = candidate_row(&pool, thread_id).await.expect("a candidate");
    assert!(row.wake_stated_at.is_some());
    assert_eq!(row.wake_turn_ended_at, None, "a beat's idle ends no turn");

    seed_thread_event(&bus, thread_id, real_idle()).await;
    let row = candidate_row(&pool, thread_id).await.expect("a candidate");
    let ended = row.wake_turn_ended_at.expect("the turn ended");
    let estimate = row
        .wake_estimate(ended - Duration::minutes(1), ReEstimate::NotAsked)
        .expect("it covers a wait armed in its turn");
    assert_eq!(estimate.shown.source, WakeSource::Agent);
    assert!((estimate.expected_secs - 20.0 * MINUTE).abs() < 1.0);
    assert_eq!(
        row.wake_estimate(ended + Duration::seconds(1), ReEstimate::NotAsked),
        None,
        "a wait armed after its turn ended is another wait"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn picked(pool: &sqlx::PgPool, thread: Uuid) -> bool {
    read_candidates(pool, &[thread])
        .await
        .unwrap()
        .iter()
        .any(|r| r.thread_id == thread)
}

#[tokio::test]
async fn no_beat_past_the_ceiling() {
    use crate::engine::context_ceiling::{CeilingSource, ContextCeiling};
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    start_cc_session(&bus, thread, "claude-code/past-the-ceiling", None).await;
    seed_thread_event(&bus, thread, real_idle()).await;
    assert!(picked(&pool, thread).await);

    seed_thread_event(
        &bus,
        thread,
        ThreadEvent::ContextCeilingReached {
            context_tokens: 400_000,
            ceiling: 300_000,
            window: None,
        },
    )
    .await;
    assert!(!picked(&pool, thread).await);

    seed_thread_event(
        &bus,
        thread,
        ThreadEvent::ContextCeilingSet {
            ceiling: ContextCeiling::Tokens(500_000),
            source: CeilingSource::Thread,
        },
    )
    .await;
    assert!(picked(&pool, thread).await, "the raise ends the hold");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn copy_beats_record_billing_even_without_an_answer() {
    use crate::runtime::billing_provider::BillingAttribution;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    for billing in [
        BillingAttribution::claude_initialize(&serde_json::json!({
            "account":{"apiProvider":"gateway"}
        })),
        BillingAttribution::default(),
    ] {
        let reply = crate::runtime::claude_code::SessionCopyReply {
            answer: Err("turn cap".into()),
            usage: Some(crate::engine::ApiUsage {
                input_tokens: 90_000,
                cache_read_tokens: 90_000,
                output_tokens: 2,
                ..Default::default()
            }),
            model: Some("test-model".into()),
            billing,
        };
        record_copy_usage(
            &bus,
            thread,
            "test-model",
            Some("high"),
            &reply,
            std::time::Duration::from_millis(15),
        )
        .await;
    }
    let captures = crate::test_support::aux_captures(&pool, thread, "cache_keep_alive").await;
    assert_eq!(captures.len(), 2);
    let mut providers: Vec<_> = captures
        .iter()
        .map(|c| c["provider"].as_str().unwrap())
        .collect();
    providers.sort();
    assert_eq!(providers, ["claude-gateway", "unknown"]);
    for capture in captures {
        assert_eq!(capture["model"], "test-model");
        assert_eq!(capture["usage"]["cache_read_tokens"], 90_000);
        assert_eq!(capture["duration_ms"], 15);
        assert_eq!(capture["reasoning_effort"], "high");
        assert_eq!(capture["producer"], "auxiliary");
        assert_eq!(capture["telemetry_owned_by_cli"], true);
    }
    pool.close().await;
    teardown_test_db(&db).await;
}
