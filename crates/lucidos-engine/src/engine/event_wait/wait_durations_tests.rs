//! The cap odds and the duration lists they count.

use super::*;
use crate::core::event_subscription::EventSubscription;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::{AnswerKind, ThreadEvent};
use crate::engine::types::{ContextProducer, ContextPurpose};
use crate::test_support::{seed_thread_event, setup_test_db, start_cc_session, teardown_test_db};
use uuid::Uuid;

const QUESTION: WaitType = WaitType::Parked(ParkedCard::Question);
const CC: WaitAgent = WaitAgent::ClaudeCode;

/// The odds count each wait in the slot of the beats it needed, and a wait
/// past the cap after it. Beats every 100 s, a cap of 2.
#[test]
fn the_odds_count_each_wait_in_its_slot() {
    let odds = CapOdds::of(&[10.0, 150.0, 250.0, 299.0, 300.0, 950.0], 100.0, 2);
    assert_eq!(
        odds,
        CapOdds {
            samples: 6,
            slots: vec![1, 1, 2],
            after_cap: 2,
        }
    );
}

/// One past wait gives odds. None gives none.
#[test]
fn a_single_wait_gives_odds() {
    let one = WaitDurations::from_samples([(CC, QUESTION, WaitSample::at_noon(150.0))]);
    assert_eq!(
        one.odds(CC, QUESTION, 100.0, 1, None)
            .map(|odds| odds.slots),
        Some(vec![0, 1])
    );
    assert_eq!(one.odds(WaitAgent::Lucidos, QUESTION, 100.0, 1, None), None);
}

/// The history takes the nearest-rank median and 80th percentile.
#[test]
fn the_history_gives_the_median_and_the_high_quantile() {
    let durations = WaitDurations::from_samples(
        [600.0, 60.0, 300.0, 1200.0, 120.0].map(|secs| (CC, QUESTION, WaitSample::at_noon(secs))),
    );
    let history = durations.history(CC, QUESTION).unwrap();
    assert_eq!(
        history,
        WaitHistory {
            samples: 5,
            median_secs: 300.0,
            high_secs: 600.0,
        }
    );
    assert_eq!(
        history.describe(),
        "Past waits like this here, last 30 days: 5. Half ended within about 5 min, \
         80% within about 10 min."
    );
    assert_eq!(durations.history(WaitAgent::Lucidos, QUESTION), None);
}

#[test]
fn a_rebuild_is_claimed_once_an_hour() {
    let cache = WaitDurationsCache::default();
    let start = Instant::now();
    assert!(cache.claim_refresh(start, Tz::UTC), "never built");
    assert!(!cache.claim_refresh(start + Duration::from_secs(60), Tz::UTC));
    assert!(cache.claim_refresh(start + REFRESH_EVERY, Tz::UTC));
}

/// A new `timezone` preference rebuilds the lists at once, so no wait reads
/// hours taken in the old zone for the rest of the hour.
#[test]
fn a_zone_change_claims_a_rebuild_at_once() {
    let cache = WaitDurationsCache::default();
    let start = Instant::now();
    assert!(cache.claim_refresh(start, Tz::UTC));
    let soon = start + Duration::from_secs(60);
    assert!(
        cache.claim_refresh(soon, Tz::Europe__Oslo),
        "the zone changed"
    );
    assert!(!cache.claim_refresh(soon, Tz::Europe__Oslo), "claimed once");
}

#[test]
fn an_unset_or_unknown_timezone_reads_utc() {
    assert_eq!(preference_zone(""), Tz::UTC);
    assert_eq!(preference_zone("Mars/Olympus"), Tz::UTC);
    assert_eq!(preference_zone("Asia/Tokyo"), Tz::Asia__Tokyo);
}

fn utc(rfc3339: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(rfc3339)
        .unwrap()
        .with_timezone(&chrono::Utc)
}

fn day(y: i32, m: u32, d: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).unwrap()
}

/// The local start follows the zone's offset on the wait's own date. Oslo
/// leaves summer time at 01:00 UTC on the last Sunday of October. So 02:00
/// local occurs twice, and 23:30 UTC then falls on the next local day.
#[test]
fn the_local_start_follows_the_october_switch() {
    let oslo = Tz::Europe__Oslo;
    let start = |at: &str| LocalStart::of(utc(at), oslo);
    assert_eq!(
        start("2026-10-24T23:30:00Z"),
        LocalStart {
            hour: 1,
            day: day(2026, 10, 25)
        },
        "summer time, UTC+2"
    );
    assert_eq!(start("2026-10-25T00:30:00Z").hour, 2, "the first 02:30");
    assert_eq!(start("2026-10-25T01:30:00Z").hour, 2, "the second 02:30");
    assert_eq!(
        start("2026-10-25T23:30:00Z"),
        LocalStart {
            hour: 0,
            day: day(2026, 10, 26)
        },
        "winter time, UTC+1"
    );
}

// --- The learned cap ---

const PERMISSION: WaitType = WaitType::Parked(ParkedCard::Permission);

/// Beats every 100 s, and a reread worth 10 pings.
const EVERY: f64 = 100.0;
const REREAD: f64 = 10.0;

/// `n` question cards of `secs` each, asked at `hour` and spread evenly over
/// `days` days.
fn cards(n: usize, secs: f64, days: u32, hour: u8) -> Vec<(WaitAgent, WaitType, WaitSample)> {
    (0..n)
        .map(|i| {
            let start = LocalStart {
                hour,
                day: day(2026, 10, 1 + i as u32 % days),
            };
            (CC, QUESTION, WaitSample { secs, start })
        })
        .collect()
}

fn noon() -> chrono::DateTime<chrono::Utc> {
    utc("2026-10-20T12:00:00Z")
}

/// The cap learned for a question card asked at noon UTC.
fn learned(durations: &WaitDurations) -> Option<LearnedCap> {
    durations.learned_cap(CC, ParkedCard::Question, noon(), EVERY, REREAD)
}

/// Below the floor the card keeps today's half cap (I3): no history, 29
/// cards, or 30 cards from only 3 days.
#[test]
fn no_cap_is_learned_below_the_floor() {
    assert_eq!(learned(&WaitDurations::default()), None, "no history");
    let few = WaitDurations::from_samples(cards(29, 150.0, 7, 12));
    assert_eq!(learned(&few), None, "29 cards");
    let crowded = WaitDurations::from_samples(cards(30, 150.0, 3, 12));
    assert_eq!(learned(&crowded), None, "3 days");
    let enough = WaitDurations::from_samples(cards(30, 150.0, 7, 12));
    assert_eq!(learned(&enough).map(|cap| cap.samples), Some(30));
}

/// The cap with the least cost wins. 20 cards need 1 beat and 10 need 50:
/// a cap of 1 costs 20 + 10 * 11 = 130 pings, a cap of 0 costs 300.
#[test]
fn the_cap_with_the_least_cost_wins() {
    let mut samples = cards(20, 150.0, 7, 12);
    samples.extend(cards(10, 5000.0, 7, 12));
    let cap = learned(&WaitDurations::from_samples(samples)).unwrap();
    assert_eq!(cap.pings, 1);

    let mut samples = cards(25, 650.0, 7, 12);
    samples.extend(cards(5, f64::INFINITY, 7, 12));
    let cap = learned(&WaitDurations::from_samples(samples)).unwrap();
    assert_eq!(
        cap.pings, 6,
        "25 cards needing 6 beats outweigh 5 abandoned"
    );
}

/// Abandoned cards cost every ping and the reread under any cap, so a history
/// of them learns a cap of none.
#[test]
fn abandoned_cards_learn_no_pings() {
    let abandoned = WaitDurations::from_samples(cards(30, f64::INFINITY, 7, 12));
    assert_eq!(learned(&abandoned).map(|cap| cap.pings), Some(0));
}

/// A tie goes to the smaller cap. Half the cards need 5 beats and half never
/// end: caps of 0 and 5 both cost 300 pings.
#[test]
fn a_tie_goes_to_the_smaller_cap() {
    let mut samples = cards(15, 550.0, 7, 12);
    samples.extend(cards(15, f64::INFINITY, 7, 12));
    let durations = WaitDurations::from_samples(samples);
    assert_eq!(learned(&durations).map(|cap| cap.pings), Some(0));
}

/// No learned cap passes one reread cap (I2), even where every card needs
/// far more beats than that.
#[test]
fn no_learned_cap_passes_the_reread_cap() {
    for (secs, reread) in [(100.0 * EVERY, 25.0), (2600.0, 25.9), (2400.0, 25.9)] {
        let durations = WaitDurations::from_samples(cards(40, secs, 7, 12));
        let cap = durations
            .learned_cap(CC, ParkedCard::Question, noon(), EVERY, reread)
            .unwrap();
        assert!(
            f64::from(cap.pings) <= reread,
            "{secs} s at a reread of {reread}: {}",
            cap.pings
        );
    }
}

/// Only the cards asked within two hours of the ask's hour count, and the
/// window wraps at midnight.
#[test]
fn the_window_spans_two_hours_either_side() {
    let window = HourWindow::around(23);
    assert_eq!(window, HourWindow { from: 21, to: 2 });
    for hour in [21, 22, 23, 0, 1] {
        assert!(window.contains(hour), "{hour}");
    }
    for hour in [20, 2, 12] {
        assert!(!window.contains(hour), "{hour}");
    }

    let mut samples = cards(30, 150.0, 7, 14);
    samples.extend(cards(30, f64::INFINITY, 7, 15));
    let cap = learned(&WaitDurations::from_samples(samples)).unwrap();
    assert_eq!(
        (cap.samples, cap.pings, cap.hours),
        (30, 1, HourWindow { from: 10, to: 15 }),
        "the 15:00 cards fall outside 10:00 to 15:00"
    );
}

/// The ask's hour is taken in the zone the history was dated in, so both
/// agree (I7). Noon UTC is 13:00 in Oslo in winter.
#[test]
fn the_ask_hour_is_taken_in_the_history_zone() {
    let oslo = WaitDurations {
        zone: Tz::Europe__Oslo,
        ..WaitDurations::from_samples(cards(30, 150.0, 7, 15))
    };
    let cap = oslo
        .learned_cap(
            CC,
            ParkedCard::Question,
            utc("2026-11-20T12:00:00Z"),
            EVERY,
            REREAD,
        )
        .unwrap();
    assert_eq!(cap.hours, HourWindow { from: 11, to: 16 });
}

/// Each card type and agent learns from its own history alone (I4): question
/// cards that would set a cap leave a permission card below the floor.
#[test]
fn a_permission_card_never_learns_from_question_cards() {
    let durations = WaitDurations::from_samples(cards(60, 150.0, 7, 12));
    assert!(learned(&durations).is_some());
    assert_eq!(
        durations.learned_cap(CC, ParkedCard::Permission, noon(), EVERY, REREAD),
        None
    );
    assert_eq!(
        durations.learned_cap(
            WaitAgent::Lucidos,
            ParkedCard::Question,
            noon(),
            EVERY,
            REREAD
        ),
        None
    );
    assert_eq!(durations.odds(CC, PERMISSION, EVERY, 1, None), None);
}

/// Odds over an hour window count only the cards asked inside it (I8).
#[test]
fn the_odds_count_the_window_that_set_the_cap() {
    let mut samples = cards(30, 150.0, 7, 12);
    samples.extend(cards(50, f64::INFINITY, 7, 20));
    let durations = WaitDurations::from_samples(samples);
    let hours = HourWindow::around(12);
    let window = durations.odds(CC, QUESTION, EVERY, 1, Some(hours)).unwrap();
    assert_eq!((window.samples, window.after_cap), (30, 0));
    let whole = durations.odds(CC, QUESTION, EVERY, 1, None).unwrap();
    assert_eq!((whole.samples, whole.after_cap), (80, 50));
}

// --- The duration lists, read back from events ---

async fn age(pool: &sqlx::PgPool, event_id: Uuid, secs: i64) {
    sqlx::query("UPDATE events SET created = now() - make_interval(secs => $2) WHERE id = $1")
        .bind(event_id)
        .bind(secs as f64)
        .execute(pool)
        .await
        .unwrap();
}

fn asked(tool_use_id: &str) -> ThreadEvent {
    ThreadEvent::UserQuestionAsked {
        tool_use_id: tool_use_id.to_string(),
        cc_session_id: "cc-session".to_string(),
        question: "Which one?".to_string(),
        options: Vec::new(),
        worktree_path: None,
        multi_select: false,
        owner_approval: None,
        unattested_act: None,
        permission_request: None,
    }
}

fn answered(tool_use_id: &str) -> ThreadEvent {
    ThreadEvent::UserQuestionAnswered {
        compose: None,
        tool_use_id: tool_use_id.to_string(),
        quiet: false,
        answer: AnswerKind::Selected {
            option_id: "a".to_string(),
        },
    }
}

/// A request of `purpose` on the thread's main session, starting when it
/// lands.
fn request(purpose: ContextPurpose) -> ThreadEvent {
    ThreadEvent::ContextCaptured {
        producer: ContextProducer::ClaudeCode,
        model: "claude-opus-5-5".to_string(),
        context_window: 0,
        sections: Vec::new(),
        tools: Vec::new(),
        estimated_total_tokens: 0,
        usage: Some(crate::engine::ApiUsage {
            input_tokens: 1_000,
            ..Default::default()
        }),
        trimmed: false,
        trim_passes: Vec::new(),
        purpose,
        reconstructed: false,
        parent_tool_use_id: None,
        api_call_id: None,
        reasoning_effort: None,
        duration_ms: Some(0),
        served_model: None,
        provider: None,
        stable_prefix_fingerprint: None,
        ttft_ms: None,
        request_id: None,
        session: None,
    }
}

/// Seed the real request that reads the cache after a card closed, aged
/// `secs` like the event before it.
async fn read_after(bus: &EventBus, pool: &sqlx::PgPool, thread_id: Uuid, secs: i64) {
    let read = seed_thread_event(bus, thread_id, request(ContextPurpose::Turn)).await;
    age(pool, read, secs).await;
}

fn wait_started(wait_id: Uuid, kinds: &[&str]) -> ThreadEvent {
    ThreadEvent::EventWaitStarted {
        wait_id,
        tool_use_id: "toolu_wait".to_string(),
        on: kinds
            .iter()
            .map(|kind| EventSubscription {
                event_type: kind.to_string(),
                condition: None,
            })
            .collect(),
        reason: "Waiting".to_string(),
        armed_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        watermark: 0,
    }
}

fn sorted(durations: &WaitDurations, agent: WaitAgent, wait_type: WaitType) -> Vec<i64> {
    let mut secs: Vec<i64> = durations
        .by_type
        .get(&(agent, wait_type))
        .map(|d| d.iter().map(|s| s.secs.round() as i64).collect())
        .unwrap_or_default();
    secs.sort_unstable();
    secs
}

/// A card runs from its ask to the real request after its answer, matched by
/// tool use id. An unanswered card, an older one and a Codex thread's are
/// left out.
#[tokio::test]
async fn a_card_runs_from_ask_to_answer() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/cards", None).await;

    let ask = seed_thread_event(&bus, thread_id, asked("toolu_a")).await;
    let other = seed_thread_event(&bus, thread_id, asked("toolu_open")).await;
    let answer = seed_thread_event(&bus, thread_id, answered("toolu_a")).await;
    age(&pool, ask, 500).await;
    age(&pool, other, 450).await;
    age(&pool, answer, 380).await;
    read_after(&bus, &pool, thread_id, 380).await;

    let old_ask = seed_thread_event(&bus, thread_id, asked("toolu_old")).await;
    let old_answer = seed_thread_event(&bus, thread_id, answered("toolu_old")).await;
    age(&pool, old_ask, 31 * 86_400).await;
    age(&pool, old_answer, 31 * 86_400 - 60).await;

    let permission = seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::CodingAgentPermissionRequest {
            request_id: "perm-1".to_string(),
            tool_use_id: "toolu_p".to_string(),
            tool_name: "Bash".to_string(),
            input: serde_json::json!({}),
            summary: "Run a command".to_string(),
            grant_offer: None,
        },
    )
    .await;
    let resolved = seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::CodingAgentPermissionResolved {
            request_id: "perm-1".to_string(),
            allowed: true,
            reason: None,
            persist_scope: None,
        },
    )
    .await;
    age(&pool, permission, 300).await;
    age(&pool, resolved, 290).await;
    read_after(&bus, &pool, thread_id, 290).await;

    let codex = Uuid::new_v4();
    start_cc_session(&bus, codex, "codex/cards", None).await;
    sqlx::query("UPDATE thread_summaries SET coding_agent = $2 WHERE thread_id = $1")
        .bind(codex)
        .bind(CodingAgent::Codex.as_str())
        .execute(&pool)
        .await
        .unwrap();
    seed_thread_event(&bus, codex, asked("toolu_codex")).await;
    seed_thread_event(&bus, codex, answered("toolu_codex")).await;

    let durations = read_wait_durations(&pool, Tz::UTC).await.unwrap();
    assert_eq!(sorted(&durations, CC, QUESTION), vec![120]);
    assert_eq!(
        sorted(&durations, CC, WaitType::Parked(ParkedCard::Permission)),
        vec![10]
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

async fn stamp(pool: &sqlx::PgPool, event_id: Uuid, at: chrono::DateTime<chrono::Utc>) {
    sqlx::query("UPDATE events SET created = $2 WHERE id = $1")
        .bind(event_id)
        .bind(at)
        .execute(pool)
        .await
        .unwrap();
}

/// A wait keeps the hour and day of its ask, never of its answer. The clock
/// is the zone the lists were read in.
#[tokio::test]
async fn a_card_keeps_the_local_hour_of_its_ask() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/hours", None).await;

    let ask = seed_thread_event(&bus, thread_id, asked("toolu_late")).await;
    let answer = seed_thread_event(&bus, thread_id, answered("toolu_late")).await;
    let asked_at = (chrono::Utc::now() - chrono::Duration::days(2))
        .date_naive()
        .and_hms_opt(14, 50, 0)
        .unwrap()
        .and_utc();
    stamp(&pool, ask, asked_at).await;
    stamp(&pool, answer, asked_at + chrono::Duration::minutes(20)).await;
    let read = seed_thread_event(&bus, thread_id, request(ContextPurpose::Turn)).await;
    stamp(&pool, read, asked_at + chrono::Duration::minutes(20)).await;

    let tokyo = read_wait_durations(&pool, Tz::Asia__Tokyo).await.unwrap();
    assert_eq!(
        tokyo.by_type[&(CC, QUESTION)],
        vec![WaitSample {
            secs: 1200.0,
            start: LocalStart {
                hour: 23,
                day: asked_at.date_naive(),
            },
        }],
        "14:50 UTC is 23:50 in Tokyo, UTC+9 all year"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

fn canceled(tool_use_id: &str) -> ThreadEvent {
    ThreadEvent::UserQuestionAnswered {
        compose: None,
        tool_use_id: tool_use_id.to_string(),
        quiet: false,
        answer: AnswerKind::Canceled,
    }
}

/// A card's wait ends at the first real request after it closes, since that
/// request reads the cache. A dismissal followed by a message ends there. With
/// no real request the card is undecided for a day, then abandoned. A copy
/// beat is no real request, and a permission card follows the same rule. An
/// allowed one ends at the allow, however long its tool then runs.
#[tokio::test]
async fn a_cards_wait_ends_at_the_next_real_request() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let day = 86_400;

    let dismissed = Uuid::new_v4();
    start_cc_session(&bus, dismissed, "claude-code/dismissed", None).await;
    let ask = seed_thread_event(&bus, dismissed, asked("toolu_dismissed")).await;
    let cancel = seed_thread_event(&bus, dismissed, canceled("toolu_dismissed")).await;
    age(&pool, ask, 1_000).await;
    age(&pool, cancel, 990).await;
    read_after(&bus, &pool, dismissed, 700).await;

    let gone = Uuid::new_v4();
    start_cc_session(&bus, gone, "claude-code/gone", None).await;
    let ask = seed_thread_event(&bus, gone, asked("toolu_gone")).await;
    let cancel = seed_thread_event(&bus, gone, canceled("toolu_gone")).await;
    let beat = seed_thread_event(&bus, gone, request(ContextPurpose::CacheKeepAlive)).await;
    age(&pool, ask, 2 * day).await;
    age(&pool, cancel, 2 * day - 10).await;
    age(&pool, beat, 2 * day - 100).await;

    let undecided = Uuid::new_v4();
    start_cc_session(&bus, undecided, "claude-code/undecided", None).await;
    let ask = seed_thread_event(&bus, undecided, asked("toolu_recent")).await;
    let cancel = seed_thread_event(&bus, undecided, canceled("toolu_recent")).await;
    age(&pool, ask, 600).await;
    age(&pool, cancel, 590).await;

    let permission = seed_thread_event(
        &bus,
        gone,
        ThreadEvent::CodingAgentPermissionRequest {
            request_id: "perm-ended".to_string(),
            tool_use_id: "toolu_p".to_string(),
            tool_name: "Bash".to_string(),
            input: serde_json::json!({}),
            summary: "Run a command".to_string(),
            grant_offer: None,
        },
    )
    .await;
    let ended = seed_thread_event(
        &bus,
        gone,
        ThreadEvent::CodingAgentPermissionResolved {
            request_id: "perm-ended".to_string(),
            allowed: false,
            reason: Some(crate::engine::cc_permission::SESSION_ENDED_REASON.to_string()),
            persist_scope: None,
        },
    )
    .await;
    age(&pool, permission, 2 * day - 200).await;
    age(&pool, ended, 2 * day - 260).await;

    let allowed = Uuid::new_v4();
    start_cc_session(&bus, allowed, "claude-code/allowed", None).await;
    let asked_to_run = seed_thread_event(
        &bus,
        allowed,
        ThreadEvent::CodingAgentPermissionRequest {
            request_id: "perm-allowed".to_string(),
            tool_use_id: "toolu_run".to_string(),
            tool_name: "Bash".to_string(),
            input: serde_json::json!({}),
            summary: "Run the tests".to_string(),
            grant_offer: None,
        },
    )
    .await;
    let allow = seed_thread_event(
        &bus,
        allowed,
        ThreadEvent::CodingAgentPermissionResolved {
            request_id: "perm-allowed".to_string(),
            allowed: true,
            reason: None,
            persist_scope: None,
        },
    )
    .await;
    age(&pool, asked_to_run, 2_000).await;
    age(&pool, allow, 1_940).await;
    read_after(&bus, &pool, allowed, 1_340).await;

    let durations = read_wait_durations(&pool, Tz::UTC).await.unwrap();
    assert_eq!(
        sorted(&durations, CC, QUESTION),
        vec![300, i64::MAX],
        "the dismissal ends at its message; the card with no request is abandoned"
    );
    let permission = WaitType::Parked(ParkedCard::Permission);
    assert_eq!(
        sorted(&durations, CC, permission),
        vec![60, i64::MAX],
        "the allowed card ends at the allow, the ended one is abandoned"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An event wait runs from its start to its delivery, expiry or cancel, and
/// takes the type of the kinds it watched. A wait on any other kind is
/// dropped.
#[tokio::test]
async fn an_event_wait_runs_from_start_to_resolution() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/waits", None).await;

    let (bash, lock, other) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let started = seed_thread_event(
        &bus,
        thread_id,
        wait_started(bash, &["BackgroundBashCompleted"]),
    )
    .await;
    let delivered = seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::EventWaitDelivered {
            wait_id: bash,
            event_id: Uuid::new_v4(),
            event_type: "BackgroundBashCompleted".to_string(),
            payload: serde_json::json!({}),
            matched_index: 0,
        },
    )
    .await;
    age(&pool, started, 900).await;
    age(&pool, delivered, 600).await;

    let started = seed_thread_event(
        &bus,
        thread_id,
        wait_started(lock, &["E2ELockReleased", "ChangeApplied"]),
    )
    .await;
    let expired = seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::EventWaitExpired { wait_id: lock },
    )
    .await;
    age(&pool, started, 500).await;
    age(&pool, expired, 455).await;

    let started = seed_thread_event(&bus, thread_id, wait_started(other, &["MailReceived"])).await;
    let expired = seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::EventWaitExpired { wait_id: other },
    )
    .await;
    age(&pool, started, 400).await;
    age(&pool, expired, 100).await;

    let durations = read_wait_durations(&pool, Tz::UTC).await.unwrap();
    let kinds = |k: &[&str]| WaitType::Events(KeepWarmKinds::of(k.iter().copied()).unwrap());
    assert_eq!(
        sorted(&durations, CC, kinds(&["BackgroundBashCompleted"])),
        vec![300]
    );
    assert_eq!(
        sorted(&durations, CC, kinds(&["ChangeApplied", "E2ELockReleased"])),
        vec![45],
        "the order of the kinds does not change the type"
    );
    assert_eq!(durations.by_type.len(), 2, "the other kind is dropped");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A Lucidos Agent thread's card and event wait count apart from Claude
/// Code's, so each agent reads its own odds.
#[tokio::test]
async fn a_lucidos_agent_thread_keeps_its_own_durations() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = crate::test_support::seed_chat_thread(&bus, "hi").await;
    let ask = seed_thread_event(&bus, thread_id, asked("toolu_la")).await;
    let answer = seed_thread_event(&bus, thread_id, answered("toolu_la")).await;
    age(&pool, ask, 400).await;
    age(&pool, answer, 100).await;
    read_after(&bus, &pool, thread_id, 100).await;
    let wait = Uuid::new_v4();
    let started = seed_thread_event(
        &bus,
        thread_id,
        wait_started(wait, &["ChildThreadCompleted"]),
    )
    .await;
    let expired = seed_thread_event(
        &bus,
        thread_id,
        ThreadEvent::EventWaitExpired { wait_id: wait },
    )
    .await;
    age(&pool, started, 90).await;
    age(&pool, expired, 30).await;

    let durations = read_wait_durations(&pool, Tz::UTC).await.unwrap();
    let la = WaitAgent::Lucidos;
    assert_eq!(sorted(&durations, la, QUESTION), vec![300]);
    let children = WaitType::Events(KeepWarmKinds::of(["ChildThreadCompleted"]).unwrap());
    assert_eq!(sorted(&durations, la, children), vec![60]);
    assert!(sorted(&durations, CC, QUESTION).is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}
