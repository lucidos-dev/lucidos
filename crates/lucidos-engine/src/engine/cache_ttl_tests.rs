use super::*;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::ThreadEvent;
use crate::engine::types::{ApiUsage, ContextProducer, ContextPurpose};
use crate::test_support::{seed_thread_event, setup_test_db, start_cc_session, teardown_test_db};
use uuid::Uuid;

fn write(written: u32) -> ApiUsage {
    ApiUsage {
        input_tokens: written + 10,
        cache_creation_tokens: written,
        ..Default::default()
    }
}

/// Anthropic splits a write into its 1-hour part and the 5-minute rest. The
/// TTL is the shortest lifetime any written token got: that part expires first.
#[test]
fn an_anthropic_split_reports_the_shortest_lifetime_written() {
    let cases = [
        ("all to the 5-minute cache", 0, CacheLifetime::FiveMinutes),
        ("all to the 1-hour cache", 5_000, CacheLifetime::OneHour),
        ("some to each", 4_000, CacheLifetime::FiveMinutes),
    ];
    for (label, written_1h, expected) in cases {
        let usage = ApiUsage {
            cache_creation_1h_tokens: Some(written_1h),
            ..write(5_000)
        };
        assert_eq!(
            CacheTtl::of_write(&usage),
            CacheTtl::Reported(expected),
            "{label}"
        );
    }
}

/// OpenAI Responses splits a write into its 30-minute and 12-hour parts.
#[test]
fn an_openai_split_reports_the_shortest_lifetime_written() {
    let cases = [
        ("all 30 minutes", 5_000, 0, CacheLifetime::ThirtyMinutes),
        ("all 12 hours", 0, 5_000, CacheLifetime::TwelveHours),
        ("some to each", 1_000, 4_000, CacheLifetime::ThirtyMinutes),
    ];
    for (label, written_30m, written_12h, expected) in cases {
        let usage = ApiUsage {
            cache_creation_30m_tokens: Some(written_30m),
            cache_creation_12h_tokens: Some(written_12h),
            ..write(5_000)
        };
        assert_eq!(
            CacheTtl::of_write(&usage),
            CacheTtl::Reported(expected),
            "{label}"
        );
    }
}

/// OpenAI's parts must cover the write. A rest nobody named a lifetime for
/// could expire first, so the TTL is not known.
#[test]
fn an_openai_split_that_leaves_a_rest_is_unknown() {
    let usage = ApiUsage {
        cache_creation_30m_tokens: Some(3_000),
        cache_creation_12h_tokens: Some(0),
        ..write(5_000)
    };
    assert_eq!(CacheTtl::of_write(&usage), CacheTtl::Unknown);
}

/// Parts that add up to more than the write are no evidence.
#[test]
fn a_split_claiming_more_than_the_write_is_unknown() {
    let usage = ApiUsage {
        cache_creation_30m_tokens: Some(5_000),
        cache_creation_12h_tokens: Some(4_000),
        ..write(5_000)
    };
    assert_eq!(CacheTtl::of_write(&usage), CacheTtl::Unknown);
}

/// A requested TTL counts only where the response reports no split.
#[test]
fn a_requested_ttl_stands_in_only_for_a_missing_split() {
    let requested = ApiUsage {
        cache_creation_requested_ttl: Some(CacheLifetime::FiveMinutes),
        ..write(5_000)
    };
    assert_eq!(
        CacheTtl::of_write(&requested),
        CacheTtl::Requested(CacheLifetime::FiveMinutes)
    );
    let reported = ApiUsage {
        cache_creation_1h_tokens: Some(5_000),
        ..requested
    };
    assert_eq!(
        CacheTtl::of_write(&reported),
        CacheTtl::Reported(CacheLifetime::OneHour)
    );
}

/// No split and no honoured request is no evidence. A provider's documented
/// default never stands in.
#[test]
fn a_write_with_no_evidence_is_unknown() {
    assert_eq!(CacheTtl::of_write(&write(5_000)), CacheTtl::Unknown);
    let no_write = ApiUsage {
        cache_creation_1h_tokens: Some(0),
        ..write(0)
    };
    assert_eq!(CacheTtl::of_write(&no_write), CacheTtl::Unknown);
}

/// A row stored before the 30-minute, 12-hour and requested keys existed
/// still reads its TTL from the 1-hour split.
#[test]
fn a_stored_row_with_only_the_1h_split_still_reads_its_ttl() {
    let stored = serde_json::json!({
        "input_tokens": 5_010,
        "output_tokens": 5,
        "cache_read_tokens": 0,
        "cache_creation_tokens": 5_000,
        "cache_creation_1h_tokens": 5_000
    });
    let usage: ApiUsage = serde_json::from_value(stored).unwrap();
    assert_eq!(
        CacheTtl::of_write(&usage),
        CacheTtl::Reported(CacheLifetime::OneHour)
    );
}

/// A reader decodes only the write fields, so a stored block that lacks an
/// unrelated count still gives its TTL rather than failing the read.
#[test]
fn a_stored_block_missing_other_counts_still_reads_its_ttl() {
    let stored = serde_json::json!({
        "input_tokens": 1000,
        "cache_creation_tokens": 1000,
        "cache_creation_30m_tokens": 1000,
        "cache_creation_12h_tokens": 0
    });
    let split: WriteSplit = serde_json::from_value(stored).unwrap();
    assert_eq!(
        CacheTtl::of_write(split),
        CacheTtl::Reported(CacheLifetime::ThirtyMinutes)
    );
    let empty: WriteSplit = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(CacheTtl::of_write(empty), CacheTtl::Unknown);
}

/// The requested TTL travels under its wire name.
#[test]
fn a_lifetime_serializes_as_its_short_name() {
    for (lifetime, wire) in [
        (CacheLifetime::FiveMinutes, "5m"),
        (CacheLifetime::ThirtyMinutes, "30m"),
        (CacheLifetime::OneHour, "1h"),
        (CacheLifetime::TwelveHours, "12h"),
    ] {
        assert_eq!(serde_json::to_value(lifetime).unwrap(), wire);
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
    for lifetime in CacheLifetime::ALL {
        assert!(
            minutes(lifetime.blocking_cap_words()) < minutes(lifetime.lifetime_words()),
            "{lifetime:?}"
        );
        assert_eq!(
            f64::from(minutes(lifetime.lifetime_words()) * 60),
            lifetime.lifetime_secs(),
            "{lifetime:?}"
        );
    }
}

/// A turn capture from `producer` that wrote `written` tokens to the cache,
/// `written_1h` of them to the 1-hour cache, for the session or a sub-agent.
fn capture(
    producer: ContextProducer,
    written: u32,
    written_1h: Option<u32>,
    sub_agent: bool,
) -> ThreadEvent {
    ThreadEvent::ContextCaptured {
        producer,
        model: "claude-opus-5-5".to_string(),
        context_window: 0,
        sections: Vec::new(),
        tools: Vec::new(),
        estimated_total_tokens: 0,
        usage: Some(ApiUsage {
            output_tokens: 5,
            cache_creation_1h_tokens: written_1h,
            ..write(written)
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
/// repo's write leave it. A latest write with no split makes it Unknown.
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
    assert_eq!(ttl().await, CacheTtl::Unknown, "no capture yet");

    let session =
        |written, written_1h| capture(ContextProducer::ClaudeCode, written, written_1h, false);
    seed_thread_event(&bus, one, session(40_000, Some(40_000))).await;
    let one_hour = CacheTtl::Reported(CacheLifetime::OneHour);
    assert_eq!(ttl().await, one_hour);

    seed_thread_event(&bus, two, session(0, Some(0))).await;
    seed_thread_event(
        &bus,
        two,
        capture(ContextProducer::ClaudeCode, 300, Some(0), true),
    )
    .await;
    seed_thread_event(&bus, two, capture(ContextProducer::Codex, 300, None, false)).await;
    seed_thread_event(&bus, elsewhere, session(300, Some(0))).await;
    assert_eq!(
        ttl().await,
        one_hour,
        "a read-only request, a sub-agent's write, a Codex capture, another repo"
    );
    let five_minutes = CacheTtl::Reported(CacheLifetime::FiveMinutes);
    assert_eq!(repo_cache_ttl(&pool, Some(&repo_b)).await, five_minutes);

    seed_thread_event(&bus, two, session(300, Some(0))).await;
    assert_eq!(ttl().await, five_minutes);

    seed_thread_event(&bus, two, session(300, None)).await;
    assert_eq!(ttl().await, CacheTtl::Unknown, "a write with no split");

    pool.close().await;
    teardown_test_db(&db_name).await;
}
