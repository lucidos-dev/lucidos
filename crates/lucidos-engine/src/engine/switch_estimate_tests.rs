use super::*;
use crate::engine::cache_ttl::CacheLifetime;
use crate::engine::event_bus::EventBus;
use crate::test_support::{seed_coding_thread, setup_test_db, teardown_test_db};

impl CacheRebuild {
    fn yes(self) -> bool {
        self == Self::Yes
    }
}

const FIVE_MINUTES: CacheTtl = CacheTtl::Reported(CacheLifetime::FiveMinutes);
const WARM: CacheRemaining = CacheRemaining::Warm(60);

fn price(model: &str) -> ModelPrice {
    price_of(model).expect("priced")
}

fn pick(model: Option<&str>, effort: Option<&str>) -> SwitchTarget {
    SwitchTarget {
        model: model.map(str::to_string),
        effort: effort.map(str::to_string),
    }
}

/// A main-session call `age_secs` ago, with a 5-minute cache write.
async fn call(pool: &sqlx::PgPool, thread_id: Uuid, model: &str, tokens: u64, age_secs: i64) {
    call_at(pool, thread_id, model, None, tokens, age_secs).await;
}

/// [`call`], at a reasoning effort.
async fn call_at(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    model: &str,
    effort: Option<&str>,
    tokens: u64,
    age_secs: i64,
) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id, created) \
         VALUES (gen_random_uuid(), 'ContextCaptured', $1, $2, 'thread', $2::text, \
                 now() - make_interval(secs => $3))",
    )
    .bind(serde_json::json!({
        "model": model,
        "reasoning_effort": effort,
        "usage": {
            "input_tokens": tokens,
            "cache_creation_tokens": 1000,
            "cache_creation_1h_tokens": 0,
        },
    }))
    .bind(thread_id)
    .bind(age_secs as f64)
    .execute(pool)
    .await
    .unwrap();
}

/// The cost is the context written at the new price, less the read it
/// replaces, from the price table.
#[test]
fn the_cost_follows_the_price_table() {
    let (opus, fable) = (price("claude-opus-5-5"), price("claude-fable-5-1"));
    let got = estimate(
        Some(200_000),
        WARM,
        true,
        FIVE_MINUTES,
        Some(&opus),
        Some(&fable),
    );
    let write = fable.input * fable.cache.write_5m;
    let read = opus.input * opus.cache.read;
    let want = 200_000.0 * (write - read) / 1e6;
    assert!(got.rebuilds_cache.yes());
    assert!((got.extra_cost_usd.unwrap() - want).abs() < 1e-9);
}

#[test]
fn the_one_hour_cache_writes_at_its_own_rate() {
    let opus = price("claude-opus-5-5");
    let cost = |ttl| {
        estimate(Some(100_000), WARM, true, ttl, Some(&opus), Some(&opus))
            .extra_cost_usd
            .unwrap()
    };
    assert!(cost(CacheTtl::Reported(CacheLifetime::OneHour)) > cost(FIVE_MINUTES));
}

/// A cold cache, an empty thread and a no-op pick rebuild nothing.
#[test]
fn only_a_real_change_on_a_warm_cache_rebuilds() {
    let opus = price("claude-opus-5-5");
    let run = |tokens, warm, changes| {
        estimate(
            tokens,
            warm,
            changes,
            FIVE_MINUTES,
            Some(&opus),
            Some(&opus),
        )
    };
    assert!(run(Some(50_000), WARM, true).rebuilds_cache.yes());
    for case in [
        run(Some(50_000), CacheRemaining::Cold, true),
        run(Some(50_000), WARM, false),
        run(Some(50_000), CacheRemaining::Unknown, false),
        run(None, WARM, true),
        run(Some(0), CacheRemaining::Unknown, true),
    ] {
        assert_eq!(case.rebuilds_cache, CacheRebuild::No);
        assert_eq!(case.extra_cost_usd, None);
    }
}

/// A change on a cache of unknown warmth may rewrite it, so the estimate
/// says so and prices the rewrite it may cost. An unsplit write bills at
/// the plain write rate.
#[test]
fn a_change_on_a_cache_of_unknown_warmth_may_rebuild() {
    let (opus, fable) = (price("claude-opus-5-5"), price("claude-fable-5-1"));
    let got = estimate(
        Some(200_000),
        CacheRemaining::Unknown,
        true,
        CacheTtl::Unknown,
        Some(&opus),
        Some(&fable),
    );
    assert_eq!(got.rebuilds_cache, CacheRebuild::Unknown);
    let want =
        200_000.0 * (fable.input * fable.cache.write_5m - opus.input * opus.cache.read) / 1e6;
    assert!((got.extra_cost_usd.unwrap() - want).abs() < 1e-9);
}

#[test]
fn an_unpriced_model_still_warns_without_a_figure() {
    let got = estimate(
        Some(50_000),
        WARM,
        true,
        FIVE_MINUTES,
        Some(&price("claude-opus-5-5")),
        None,
    );
    assert!(got.rebuilds_cache.yes());
    assert_eq!(got.extra_cost_usd, None);
}

/// A pick that is cheaper to write than the read it replaces never reports
/// a negative figure.
#[test]
fn the_figure_is_never_negative() {
    let got = estimate(
        Some(100_000),
        WARM,
        true,
        FIVE_MINUTES,
        Some(&price("claude-fable-5-1")),
        Some(&price("claude-haiku-5-5")),
    );
    assert_eq!(got.extra_cost_usd, Some(0.0));
}

/// The warm path against the events table: a recent call, a model change.
#[tokio::test]
async fn a_model_change_on_a_warm_thread_rebuilds_and_a_cold_one_does_not() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let agent = SwitchSubject::CodingAgent(CodingAgent::ClaudeCode);
    let current = || {
        (
            Some("claude-opus-5-5".to_string()),
            Some("medium".to_string()),
        )
    };

    let warm = seed_coding_thread(&bus, None).await;
    call(&pool, warm, "claude-opus-5-5", 200_000, 30).await;
    let got = switch_estimate(
        &pool,
        warm,
        agent,
        current(),
        &pick(Some("claude-fable-5-1"), Some("medium")),
    )
    .await
    .unwrap();
    assert_eq!(got.context_tokens, Some(200_000));
    assert!(got.rebuilds_cache.yes());
    assert!(got.extra_cost_usd.unwrap() > 0.0);

    let cold = seed_coding_thread(&bus, None).await;
    call(&pool, cold, "claude-opus-5-5", 200_000, 3_000).await;
    let got = switch_estimate(
        &pool,
        cold,
        agent,
        current(),
        &pick(Some("claude-fable-5-1"), None),
    )
    .await
    .unwrap();
    assert!(
        !got.rebuilds_cache.yes(),
        "a 5-minute cache is gone after 50"
    );

    teardown_test_db(&db).await;
}

/// An effort change rewrites the messages cache as a model change does, and
/// the same pair rewrites nothing.
#[tokio::test]
async fn an_effort_only_change_rebuilds_and_the_same_pair_does_not() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    call(&pool, thread, "claude-opus-5-5", 80_000, 10).await;
    let current = || {
        (
            Some("claude-opus-5-5".to_string()),
            Some("medium".to_string()),
        )
    };
    let at = |model, effort| {
        let (pool, thread) = (pool.clone(), thread);
        async move {
            switch_estimate(
                &pool,
                thread,
                SwitchSubject::CodingAgent(CodingAgent::ClaudeCode),
                current(),
                &pick(model, effort),
            )
            .await
            .unwrap()
        }
    };

    let effort = at(Some("claude-opus-5-5"), Some("high")).await;
    assert!(effort.rebuilds_cache.yes());
    assert!(effort.extra_cost_usd.unwrap() > 0.0);
    assert!(!at(Some("claude-opus-5-5"), Some("medium"))
        .await
        .rebuilds_cache
        .yes());
    assert!(!at(None, None).await.rebuilds_cache.yes());
    assert!(
        at(Some("claude-opus-5-5[1m]"), None)
            .await
            .rebuilds_cache
            .yes(),
        "the context variant is part of the cache key"
    );

    teardown_test_db(&db).await;
}

/// A settings event naming `effort`, as a user's pick writes one.
async fn settings_at(pool: &sqlx::PgPool, thread_id: Uuid, effort: &str) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
         VALUES (gen_random_uuid(), 'CodingAgentSettingsChanged', $1, $2, 'thread', $2::text)",
    )
    .bind(serde_json::json!({ "reasoning_effort": effort }))
    .bind(thread_id)
    .execute(pool)
    .await
    .unwrap();
}

/// The effort in force is what the latest main-session call ran at. Before
/// any call it is the settings' effort, and a later pick does not move it.
#[tokio::test]
async fn the_effort_in_force_is_the_latest_calls() {
    use crate::engine::handoff_estimate::thread_effort_in_force;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    assert_eq!(thread_effort_in_force(&pool, thread).await.unwrap(), None);

    settings_at(&pool, thread, "medium").await;
    assert_eq!(
        thread_effort_in_force(&pool, thread)
            .await
            .unwrap()
            .as_deref(),
        Some("medium"),
        "before its first call a thread runs at its settings"
    );

    call_at(&pool, thread, "claude-opus-5-5", Some("medium"), 80_000, 10).await;
    settings_at(&pool, thread, "xhigh").await;
    assert_eq!(
        thread_effort_in_force(&pool, thread)
            .await
            .unwrap()
            .as_deref(),
        Some("medium"),
        "a pick no call ran at is not in force"
    );

    teardown_test_db(&db).await;
}

/// The user picked xhigh, and the calls still run at medium. Going back to
/// medium rewrites nothing, and xhigh still prices the rewrite.
#[tokio::test]
async fn a_switch_compares_against_the_effort_in_force() {
    use crate::engine::handoff_estimate::thread_effort_in_force;
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    call_at(&pool, thread, "claude-opus-5-5", Some("medium"), 80_000, 10).await;
    settings_at(&pool, thread, "xhigh").await;
    let current = (
        Some("claude-opus-5-5".to_string()),
        thread_effort_in_force(&pool, thread).await.unwrap(),
    );
    let at = |effort| {
        let (pool, current) = (pool.clone(), current.clone());
        async move {
            switch_estimate(
                &pool,
                thread,
                SwitchSubject::CodingAgent(CodingAgent::ClaudeCode),
                current,
                &pick(None, Some(effort)),
            )
            .await
            .unwrap()
        }
    };

    assert!(!at("medium").await.rebuilds_cache.yes());
    assert!(at("xhigh").await.rebuilds_cache.yes());

    teardown_test_db(&db).await;
}

/// With no settings event, the thread's latest call names its model.
#[tokio::test]
async fn an_unset_model_reads_the_latest_call() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    call(&pool, thread, "claude-opus-5-5", 80_000, 10).await;
    let got = switch_estimate(
        &pool,
        thread,
        SwitchSubject::CodingAgent(CodingAgent::ClaudeCode),
        (None, None),
        &pick(Some("claude-sonnet-5-5"), None),
    )
    .await
    .unwrap();
    assert!(got.rebuilds_cache.yes());
    teardown_test_db(&db).await;
}

/// A chat thread has no session, so its current pair is what its latest turn
/// ran: a different model on a warm cache rebuilds, the same pair does not.
#[tokio::test]
async fn a_chat_thread_reads_its_pair_from_its_latest_turn() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    call_at(
        &pool,
        thread,
        "claude-opus-5-5",
        Some("medium"),
        120_000,
        20,
    )
    .await;
    let current = last_turn_settings(&pool, thread).await.unwrap();
    assert_eq!(
        current,
        (
            Some("claude-opus-5-5".to_string()),
            Some("medium".to_string())
        )
    );
    let at = |model, effort| {
        let (pool, current) = (pool.clone(), current.clone());
        async move {
            switch_estimate(
                &pool,
                thread,
                SwitchSubject::LucidosAgent,
                current,
                &pick(model, effort),
            )
            .await
            .unwrap()
        }
    };

    let other = at(Some("claude-fable-5-1"), Some("medium")).await;
    assert!(other.rebuilds_cache.yes());
    assert!(other.extra_cost_usd.unwrap() > 0.0);
    assert!(at(Some("claude-opus-5-5"), Some("high"))
        .await
        .rebuilds_cache
        .yes());
    assert!(!at(Some("claude-opus-5-5"), Some("medium"))
        .await
        .rebuilds_cache
        .yes());
    teardown_test_db(&db).await;
}

/// A chat thread that has not run a turn has no cache to rebuild.
#[tokio::test]
async fn a_chat_thread_with_no_turn_rebuilds_nothing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    let current = last_turn_settings(&pool, thread).await.unwrap();
    assert_eq!(current, (None, None));
    let got = switch_estimate(
        &pool,
        thread,
        SwitchSubject::LucidosAgent,
        current,
        &pick(Some("claude-fable-5-1"), None),
    )
    .await
    .unwrap();
    assert!(!got.rebuilds_cache.yes());
    teardown_test_db(&db).await;
}

/// A main-session call `age_secs` ago, whose 1,000-token cache write
/// reported `split`.
async fn call_with_split(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    age_secs: i64,
    split: serde_json::Value,
) {
    let mut usage = serde_json::json!({
        "input_tokens": 1000,
        "cache_creation_tokens": 1000,
    });
    usage
        .as_object_mut()
        .unwrap()
        .extend(split.as_object().unwrap().clone());
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id, created) \
         VALUES (gen_random_uuid(), 'ContextCaptured', $1, $2, 'thread', $2::text, \
                 now() - make_interval(secs => $3))",
    )
    .bind(serde_json::json!({ "model": "claude-opus-5-5", "usage": usage }))
    .bind(thread_id)
    .bind(age_secs as f64)
    .execute(pool)
    .await
    .unwrap();
}

/// What is left of the lifetime, cold once it is spent or unaged, and unknown
/// with no lifetime to spend.
#[test]
fn the_warm_remainder_is_the_lifetime_less_the_age() {
    let one_hour = CacheTtl::Reported(CacheLifetime::OneHour);
    let thirty = CacheTtl::Reported(CacheLifetime::ThirtyMinutes);
    assert_eq!(
        remaining_of(Some(60.0), FIVE_MINUTES),
        CacheRemaining::Warm(240)
    );
    assert_eq!(
        remaining_of(Some(60.0), one_hour),
        CacheRemaining::Warm(3540)
    );
    assert_eq!(remaining_of(Some(60.0), thirty), CacheRemaining::Warm(1740));
    assert_eq!(
        remaining_of(Some(300.0), FIVE_MINUTES),
        CacheRemaining::Cold
    );
    assert_eq!(
        remaining_of(Some(900.0), FIVE_MINUTES),
        CacheRemaining::Cold
    );
    assert_eq!(remaining_of(None, one_hour), CacheRemaining::Cold);
    assert_eq!(
        remaining_of(Some(60.0), CacheTtl::Unknown),
        CacheRemaining::Unknown
    );
    assert_eq!(remaining_of(None, CacheTtl::Unknown), CacheRemaining::Cold);
}

/// Whole seconds a warm reading has left, or `None` when it is not warm.
async fn warm_secs(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<i64> {
    match cache_remaining(pool, thread_id).await.unwrap() {
        CacheRemaining::Warm(secs) => Some(secs),
        _ => None,
    }
}

/// The countdown the browser draws follows the TTL the write reported: ~300 s
/// for a fresh 5-minute write, ~1800 s for OpenAI's 30-minute one, ~3600 s for
/// a 1-hour one. It is cold once each lapsed or nothing was written.
#[tokio::test]
async fn the_remaining_warmth_follows_the_reported_ttl() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let anthropic = |one_hour: u32| serde_json::json!({ "cache_creation_1h_tokens": one_hour });
    let openai_30m = serde_json::json!({
        "cache_creation_30m_tokens": 1000,
        "cache_creation_12h_tokens": 0,
    });

    let five = seed_coding_thread(&bus, None).await;
    call_with_split(&pool, five, 30, anthropic(0)).await;
    let left = warm_secs(&pool, five).await.unwrap();
    assert!(
        (265..=270).contains(&left),
        "a 5-minute write at 30s: {left}"
    );

    let thirty = seed_coding_thread(&bus, None).await;
    call_with_split(&pool, thirty, 30, openai_30m.clone()).await;
    let left = warm_secs(&pool, thirty).await.unwrap();
    assert!(
        (1765..=1770).contains(&left),
        "a 30-minute write at 30s: {left}"
    );

    let hour = seed_coding_thread(&bus, None).await;
    call_with_split(&pool, hour, 30, anthropic(1000)).await;
    let left = warm_secs(&pool, hour).await.unwrap();
    assert!(
        (3565..=3570).contains(&left),
        "a 1-hour write at 30s: {left}"
    );

    let lapsed = seed_coding_thread(&bus, None).await;
    call_with_split(&pool, lapsed, 400, anthropic(0)).await;
    assert_eq!(
        cache_remaining(&pool, lapsed).await.unwrap(),
        CacheRemaining::Cold
    );

    let outlived = seed_coding_thread(&bus, None).await;
    call_with_split(&pool, outlived, 4000, anthropic(1000)).await;
    assert_eq!(
        cache_remaining(&pool, outlived).await.unwrap(),
        CacheRemaining::Cold
    );

    let silent = seed_coding_thread(&bus, None).await;
    assert_eq!(
        cache_remaining(&pool, silent).await.unwrap(),
        CacheRemaining::Cold
    );

    teardown_test_db(&db).await;
}

/// A write that reports no lifetime, as Codex's and OpenRouter's non-Claude
/// models do, is never called warm or cold, however long ago it was. A
/// switch there may rewrite it.
#[tokio::test]
async fn a_write_with_no_reported_ttl_has_unknown_warmth() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    call_with_split(&pool, thread, 900, serde_json::json!({})).await;
    assert_eq!(
        cache_remaining(&pool, thread).await.unwrap(),
        CacheRemaining::Unknown
    );
    let got = switch_estimate(
        &pool,
        thread,
        SwitchSubject::LucidosAgent,
        (Some("claude-opus-5-5".to_string()), None),
        &pick(Some("claude-fable-5-1"), None),
    )
    .await
    .unwrap();
    assert_eq!(got.rebuilds_cache, CacheRebuild::Unknown);
    assert!(got.extra_cost_usd.unwrap() > 0.0);
    teardown_test_db(&db).await;
}

/// A thread whose calls never wrote to a prompt cache has nothing warm and
/// nothing a switch would rebuild.
#[tokio::test]
async fn a_thread_that_never_wrote_a_cache_is_never_warm() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
         VALUES (gen_random_uuid(), 'ContextCaptured', $1, $2, 'thread', $2::text)",
    )
    .bind(serde_json::json!({
        "model": "gpt-5",
        "usage": { "input_tokens": 1000 },
    }))
    .bind(thread)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        cache_remaining(&pool, thread).await.unwrap(),
        CacheRemaining::Cold
    );
    let got = switch_estimate(
        &pool,
        thread,
        SwitchSubject::LucidosAgent,
        (Some("gpt-5".to_string()), None),
        &pick(Some("claude-opus-5-5"), None),
    )
    .await
    .unwrap();
    assert_eq!(got.rebuilds_cache, CacheRebuild::No);
    teardown_test_db(&db).await;
}

/// A `ContextCaptured` `age_secs` ago, as the main session's turn, a keep-alive
/// copy beat or a sub-agent's call. Every one wrote to a 5-minute cache.
async fn capture_at(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    purpose: Option<&str>,
    parent_tool_use_id: Option<&str>,
    age_secs: i64,
) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id, created) \
         VALUES (gen_random_uuid(), 'ContextCaptured', $1, $2, 'thread', $2::text, \
                 now() - make_interval(secs => $3))",
    )
    .bind(serde_json::json!({
        "model": "claude-opus-5-5",
        "purpose": purpose,
        "parent_tool_use_id": parent_tool_use_id,
        "usage": {
            "input_tokens": 1000,
            "cache_creation_tokens": 1000,
            "cache_creation_1h_tokens": 0,
        },
    }))
    .bind(thread_id)
    .bind(age_secs as f64)
    .execute(pool)
    .await
    .unwrap();
}

/// A copy beat refreshes the cache. The line and the estimate see a thread
/// beaten under one TTL ago as warm, whatever its last turn's age.
#[tokio::test]
async fn a_keep_alive_copy_beat_keeps_the_cache_warm() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    capture_at(&pool, thread, None, None, 900).await;
    assert_eq!(
        cache_remaining(&pool, thread).await.unwrap(),
        CacheRemaining::Cold
    );

    capture_at(&pool, thread, Some("cache_keep_alive"), None, 60).await;
    let left = warm_secs(&pool, thread).await.unwrap();
    assert!((235..=240).contains(&left), "the beat's age rules: {left}");
    let got = switch_estimate(
        &pool,
        thread,
        SwitchSubject::CodingAgent(CodingAgent::ClaudeCode),
        (Some("claude-opus-5-5".to_string()), None),
        &pick(Some("claude-fable-5-1"), None),
    )
    .await
    .unwrap();
    assert!(
        got.rebuilds_cache.yes(),
        "a switch rewrites the beaten cache"
    );
    teardown_test_db(&db).await;
}

/// A sub-agent's call, and an auxiliary call that is no copy beat, never
/// refresh the main session's cache.
#[tokio::test]
async fn only_the_main_session_and_its_copy_beats_refresh_the_cache() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = seed_coding_thread(&bus, None).await;
    capture_at(&pool, thread, None, None, 900).await;
    capture_at(&pool, thread, None, Some("toolu_agent"), 10).await;
    capture_at(&pool, thread, Some("title"), None, 10).await;
    assert_eq!(
        cache_remaining(&pool, thread).await.unwrap(),
        CacheRemaining::Cold
    );
    teardown_test_db(&db).await;
}
