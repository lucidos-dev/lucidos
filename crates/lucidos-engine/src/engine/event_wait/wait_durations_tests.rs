//! The cheapest-cap search (I2, I4) and the duration lists it reads.

use super::*;
use crate::core::event_subscription::EventSubscription;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::{AnswerKind, ThreadEvent};
use crate::test_support::{seed_thread_event, setup_test_db, start_cc_session, teardown_test_db};
use uuid::Uuid;

const QUESTION: WaitType = WaitType::Parked(ParkedCard::Question);
const CC: WaitAgent = WaitAgent::ClaudeCode;

/// What a cap costs over `durations`, in beats, counted the slow way.
fn cost_of(cap: u32, durations: &[f64], beat_every: f64, rewrite: f64) -> f64 {
    durations
        .iter()
        .map(|d| {
            let needed = (d / beat_every).floor();
            if needed > f64::from(cap) {
                f64::from(cap) + rewrite
            } else {
                needed
            }
        })
        .sum()
}

/// Beats every 100 s, a rewrite costs 3. The waits need 1, 2, 2 and 9 beats.
/// Cap 0 costs 12, cap 1 costs 13, cap 2 costs 10 and cap 3 costs 11.
#[test]
fn the_cheapest_cap_wins() {
    assert_eq!(cheapest_cap(&[150.0, 250.0, 250.0, 950.0], 100.0, 3.0), 2);
}

/// The waits need 1 and 4 beats. Caps 1, 4, 5 and 6 all cost 5.
#[test]
fn a_tie_takes_the_shorter_cap() {
    assert_eq!(cheapest_cap(&[150.0, 450.0], 100.0, 3.0), 1);
}

/// Short waits all fit under a cap, so covering them beats rewriting them.
#[test]
fn short_waits_are_covered() {
    assert_eq!(cheapest_cap(&[99.0, 150.0, 199.0], 100.0, 3.0), 1);
    assert_eq!(cheapest_cap(&[10.0, 20.0], 100.0, 3.0), 0, "no beat needed");
}

/// A wait that never ends still yields a cap inside the bound (I2).
#[test]
fn waits_that_never_end_still_give_a_finite_cap() {
    let forever = vec![f64::INFINITY; 60];
    assert_eq!(cheapest_cap(&forever, 230.0, 24.6), 0);
    assert_eq!(cheapest_cap(&[], 230.0, 24.6), 0, "no waits at all");
}

/// Over a spread of waits, the search finds the cheapest cap in its range,
/// and that range stops at twice the break-even budget (I2, I4).
#[test]
fn the_search_matches_a_brute_force_count() {
    let mut seed: u64 = 7;
    let mut next = || {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        (seed >> 33) as f64 / f64::from(u32::MAX)
    };
    for rewrite in [2.5, 12.5, 24.6] {
        let durations: Vec<f64> = (0..400).map(|_| next().powi(3) * 20_000.0).collect();
        let cap = cheapest_cap(&durations, 230.0, rewrite);
        let longest = (2.0 * rewrite).floor() as u32;
        assert!(cap <= longest, "{cap} past {longest}");
        let best = cost_of(cap, &durations, 230.0, rewrite);
        for other in 0..=longest {
            assert!(
                best <= cost_of(other, &durations, 230.0, rewrite),
                "cap {cap} costs more than {other} at rewrite {rewrite}"
            );
        }
    }
}

/// Under 50 waits of a type, the type keeps the default rule.
#[test]
fn too_few_waits_fall_back_to_the_default() {
    let few = WaitDurations::from_samples((0..MIN_SAMPLES - 1).map(|_| (CC, QUESTION, 150.0)));
    assert_eq!(few.cap_beats(CC, QUESTION, 100.0, 3.0), None);
    let enough = WaitDurations::from_samples((0..MIN_SAMPLES).map(|_| (CC, QUESTION, 150.0)));
    assert_eq!(enough.cap_beats(CC, QUESTION, 100.0, 3.0), Some(1));
    let permission = WaitType::Parked(ParkedCard::Permission);
    assert_eq!(
        enough.cap_beats(CC, permission, 100.0, 3.0),
        None,
        "another type"
    );
    assert_eq!(
        enough.cap_beats(WaitAgent::Lucidos, QUESTION, 100.0, 3.0),
        None,
        "another agent"
    );
}

#[test]
fn a_rebuild_is_claimed_once_an_hour() {
    let cache = WaitDurationsCache::default();
    let start = Instant::now();
    assert!(cache.claim_refresh(start), "never built");
    assert!(!cache.claim_refresh(start + Duration::from_secs(60)));
    assert!(cache.claim_refresh(start + REFRESH_EVERY));
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
    }
}

fn answered(tool_use_id: &str) -> ThreadEvent {
    ThreadEvent::UserQuestionAnswered {
        tool_use_id: tool_use_id.to_string(),
        quiet: false,
        answer: AnswerKind::Selected {
            option_id: "a".to_string(),
        },
    }
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
        .map(|d| d.iter().map(|s| s.round() as i64).collect())
        .unwrap_or_default();
    secs.sort_unstable();
    secs
}

/// A card runs from its ask to the first answer with the same tool use id.
/// An unanswered card, an older one and a Codex thread's are left out.
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

    let durations = read_wait_durations(&pool).await.unwrap();
    assert_eq!(sorted(&durations, CC, QUESTION), vec![120]);
    assert_eq!(
        sorted(&durations, CC, WaitType::Parked(ParkedCard::Permission)),
        vec![10]
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

    let durations = read_wait_durations(&pool).await.unwrap();
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
/// Code's, so each agent tunes its own give-up point.
#[tokio::test]
async fn a_lucidos_agent_thread_keeps_its_own_durations() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = crate::test_support::seed_chat_thread(&bus, "hi").await;
    let ask = seed_thread_event(&bus, thread_id, asked("toolu_la")).await;
    let answer = seed_thread_event(&bus, thread_id, answered("toolu_la")).await;
    age(&pool, ask, 400).await;
    age(&pool, answer, 100).await;
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

    let durations = read_wait_durations(&pool).await.unwrap();
    let la = WaitAgent::Lucidos;
    assert_eq!(sorted(&durations, la, QUESTION), vec![300]);
    let children = WaitType::Events(KeepWarmKinds::of(["ChildThreadCompleted"]).unwrap());
    assert_eq!(sorted(&durations, la, children), vec![60]);
    assert!(sorted(&durations, CC, QUESTION).is_empty());

    pool.close().await;
    teardown_test_db(&db_name).await;
}
