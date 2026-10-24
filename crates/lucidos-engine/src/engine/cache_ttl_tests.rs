use super::*;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::ThreadEvent;
use crate::engine::types::{ContextProducer, ContextPurpose};
use crate::test_support::{seed_thread_event, setup_test_db, start_cc_session, teardown_test_db};
use uuid::Uuid;

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

/// A cap shorter than its lifetime, so the next request lands inside it.
#[test]
fn each_cap_sits_inside_its_lifetime() {
    let minutes = |words: &str| -> u32 {
        let (n, unit) = words.split_once(' ').expect("a number and a unit");
        let n: u32 = n.parse().expect("a number");
        if unit.starts_with("hour") {
            n * 60
        } else {
            n
        }
    };
    for ttl in [CacheTtl::FiveMinutes, CacheTtl::OneHour] {
        assert!(
            minutes(ttl.blocking_cap_words()) < minutes(ttl.lifetime_words()),
            "{ttl:?}"
        );
    }
}

/// A turn capture from `producer` that wrote `written` tokens to the cache,
/// `written_1h` of them to the 1-hour cache, for the session or a sub-agent.
fn capture(
    producer: ContextProducer,
    written: u32,
    written_1h: u32,
    sub_agent: bool,
) -> ThreadEvent {
    ThreadEvent::ContextCaptured {
        producer,
        model: "claude-opus-5-5".to_string(),
        context_window: 0,
        sections: Vec::new(),
        tools: Vec::new(),
        estimated_total_tokens: 0,
        usage: Some(crate::engine::ApiUsage {
            input_tokens: written + 10,
            output_tokens: 5,
            cache_creation_tokens: written,
            cache_creation_1h_tokens: Some(written_1h),
            ..Default::default()
        }),
        trimmed: false,
        trim_passes: Vec::new(),
        purpose: ContextPurpose::Turn,
        reconstructed: false,
        parent_tool_use_id: sub_agent.then(|| "toolu_agent".to_string()),
        api_call_id: None,
        reasoning_effort: None,
        duration_ms: Some(1_000),
        served_model: None,
        provider: None,
        stable_prefix_fingerprint: None,
        ttft_ms: None,
        request_id: None,
        session: None,
    }
}

async fn bind_repo(pool: &sqlx::PgPool, thread_id: Uuid, repo_id: &str) {
    sqlx::query("UPDATE thread_summaries SET cc_repo_id = $2 WHERE thread_id = $1")
        .bind(thread_id)
        .bind(repo_id)
        .execute(pool)
        .await
        .unwrap();
}

/// A repo's TTL is its latest Claude Code session write, across its threads.
/// A read-only request, a sub-agent's write, a Codex capture and another
/// repo's write leave it.
#[tokio::test]
async fn the_repo_ttl_comes_from_its_latest_session_write() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let [one, two, elsewhere] = [Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
    // A repo id is a registered repo's uuid; the projection reads it as one.
    let (repo_a, repo_b) = (Uuid::new_v4().to_string(), Uuid::new_v4().to_string());
    for (thread_id, branch, repo) in [
        (one, "claude-code/one", &repo_a),
        (two, "claude-code/two", &repo_a),
        (elsewhere, "claude-code/elsewhere", &repo_b),
    ] {
        start_cc_session(&bus, thread_id, branch, None).await;
        bind_repo(&pool, thread_id, repo).await;
    }
    let ttl = || repo_cache_ttl(&pool, Some(&repo_a));
    assert_eq!(ttl().await, None, "no capture yet");

    seed_thread_event(
        &bus,
        one,
        capture(ContextProducer::ClaudeCode, 40_000, 40_000, false),
    )
    .await;
    assert_eq!(ttl().await, Some(CacheTtl::OneHour));

    seed_thread_event(&bus, two, capture(ContextProducer::ClaudeCode, 0, 0, false)).await;
    seed_thread_event(
        &bus,
        two,
        capture(ContextProducer::ClaudeCode, 300, 0, true),
    )
    .await;
    seed_thread_event(&bus, two, capture(ContextProducer::Codex, 300, 0, false)).await;
    seed_thread_event(
        &bus,
        elsewhere,
        capture(ContextProducer::ClaudeCode, 300, 0, false),
    )
    .await;
    assert_eq!(
        ttl().await,
        Some(CacheTtl::OneHour),
        "a read-only request, a sub-agent's write, a Codex capture, another repo"
    );
    assert_eq!(
        repo_cache_ttl(&pool, Some(&repo_b)).await,
        Some(CacheTtl::FiveMinutes)
    );

    seed_thread_event(
        &bus,
        two,
        capture(ContextProducer::ClaudeCode, 300, 0, false),
    )
    .await;
    assert_eq!(ttl().await, Some(CacheTtl::FiveMinutes));

    pool.close().await;
    teardown_test_db(&db_name).await;
}
