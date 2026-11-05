//! The model call service: every call records once, on a thread, and nothing
//! outside the service can make one.

use super::*;
use crate::llm::provider::MessageContent;
use crate::test_support::{aux_captures, setup_test_db, teardown_test_db, ScriptedProvider};

fn ask(text: &str) -> Vec<Message> {
    vec![Message {
        role: "user".to_string(),
        content: MessageContent::Text(text.to_string()),
    }]
}

async fn marked_home(pool: &sqlx::PgPool) -> Vec<Uuid> {
    sqlx::query_scalar("SELECT thread_id FROM thread_summaries WHERE is_home")
        .fetch_all(pool)
        .await
        .expect("read the home marker")
}

/// Each attempt is one row, naming the model, the tier and the time it took.
#[tokio::test]
async fn each_chat_attempt_records_one_row() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let capture = AuxCapture::new(&bus, thread, ContextPurpose::Title);
    let provider = ScriptedProvider::new("title-model", vec!["one", "two"]);

    for text in ["first", "second"] {
        let selection = ModelSelection::model("title-model").with_effort(Some("low"));
        capture
            .chat(
                &provider,
                ask(text),
                vec![],
                selection,
                Some("system"),
                None,
            )
            .await
            .expect("the scripted model answers");
    }

    let rows = aux_captures(&pool, thread, "title").await;
    assert_eq!(rows.len(), 2, "one row per attempt");
    for row in &rows {
        assert_eq!(row["model"], "title-model");
        assert_eq!(row["reasoning_effort"], "low");
        assert!(row["duration_ms"].is_u64(), "{row}");
        assert!(row["usage"]["input_tokens"].is_u64(), "{row}");
    }
    teardown_test_db(&db).await;
}

/// A call no thread caused still records, on the home thread. Made before
/// boot created Home, it creates Home itself, and that is the one Home.
#[tokio::test]
async fn a_call_with_no_thread_records_on_the_home_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let capture = AuxCapture::for_thread_or_home(&bus, None, ContextPurpose::Memory);

    capture
        .record_usage("gemini-3-flash-preview", 120, None)
        .await;
    capture
        .record_usage("gemini-3-flash-preview", 80, None)
        .await;

    let homes = marked_home(&pool).await;
    assert_eq!(homes.len(), 1, "both records share one home thread");
    assert_eq!(aux_captures(&pool, homes[0], "memory").await.len(), 2);
    assert_eq!(
        crate::engine::home_thread::home_thread_id(&pool)
            .await
            .unwrap(),
        Some(homes[0]),
    );
    teardown_test_db(&db).await;
}

/// An existing home thread is reused, and a call made for a thread records
/// there rather than on Home.
#[tokio::test]
async fn a_call_records_on_its_own_thread_and_reuses_home() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let home = crate::engine::home_thread::ensure_home_thread(&bus, &pool)
        .await
        .unwrap();
    let thread = Uuid::new_v4();

    AuxCapture::for_thread_or_home(&bus, None, ContextPurpose::Title)
        .record_usage("gpt-5.4", 10, None)
        .await;
    AuxCapture::new(&bus, thread, ContextPurpose::Title)
        .record_usage("gpt-5.4", 10, None)
        .await;

    assert_eq!(marked_home(&pool).await, vec![home]);
    assert_eq!(aux_captures(&pool, home, "title").await.len(), 1);
    assert_eq!(aux_captures(&pool, thread, "title").await.len(), 1);
    teardown_test_db(&db).await;
}

/// A script fire's proxied call records on Home, stamped with the fire's
/// trigger. The record's own task never sees the fire's ambient marker,
/// so an unstated capture stays unstamped.
#[tokio::test]
async fn a_capture_stated_as_a_trigger_fire_stamps_its_row() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());

    AuxCapture::for_thread_or_home(&bus, None, ContextPurpose::Proxy)
        .for_trigger_fire(Some("trg-hourly".into()))
        .record_usage("gpt-5.4", 10, None)
        .await;
    crate::scheduler::user_tasks::ACTIVE_TRIGGER_ID
        .scope("trg-ambient".into(), async {
            AuxCapture::for_thread_or_home(&bus, None, ContextPurpose::Proxy)
                .record_usage("gpt-5.4", 10, None)
                .await;
        })
        .await;

    let stamps: Vec<Option<String>> = sqlx::query_scalar(
        "SELECT emitting_trigger_id FROM events \
         WHERE event_type = 'ContextCaptured' ORDER BY sequence",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(stamps, vec![Some("trg-hourly".to_string()), None]);
    teardown_test_db(&db).await;
}

/// A turn row the loop never finished still records its usage.
#[tokio::test]
async fn an_unfinished_turn_capture_still_records() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let provider = ScriptedProvider::new("turn-model", vec!["hello"]);

    let (_, capture) = turn_chat(
        &bus,
        thread,
        EventMeta::NONE,
        1,
        &provider,
        ask("hi"),
        vec![],
        ModelSelection::model("turn-model"),
        None,
        None,
    )
    .await
    .expect("the scripted model answers");
    drop(capture);

    // A turn row writes no `purpose` key, so it is found by its producer.
    let mut rows: Vec<serde_json::Value> = Vec::new();
    for _ in 0..50 {
        rows = sqlx::query_scalar(
            "SELECT payload FROM events WHERE thread_id = $1 \
               AND event_type = 'ContextCaptured' AND payload->>'producer' = 'main_llm'",
        )
        .bind(thread)
        .fetch_all(&pool)
        .await
        .expect("read the turn rows");
        if !rows.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(rows.len(), 1, "the dropped capture recorded once");
    assert_eq!(rows[0]["model"], "turn-model");
    assert_eq!(rows[0]["producer"], "main_llm");
    assert!(rows[0]["usage"]["input_tokens"].is_u64());
    teardown_test_db(&db).await;
}

/// A provider whose reply names another model than the one asked for, the
/// way Google answers a retired Gemini id.
struct Rerouting(ScriptedProvider);

#[async_trait::async_trait]
impl LlmProvider for Rerouting {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        selection: ModelSelection<'_>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, BoxError> {
        let response = self
            .0
            .chat(messages, tools, selection, system_prompt, on_token, call)
            .await?;
        Ok(LlmResponse {
            served_model: Some("gemini-3.6-flash".to_string()),
            provider: Some(crate::llm::ProviderKind::ClaudeGateway),
            ..response
        })
    }
}

/// Both rows keep the model Lucidos asked for, the one the reply named, and
/// the backend the router sent it to.
#[tokio::test]
async fn a_capture_records_the_model_that_served_the_call() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let provider = Rerouting(ScriptedProvider::new("gemini-3.5-flash", vec!["a", "b"]));

    AuxCapture::new(&bus, thread, ContextPurpose::Title)
        .chat(
            &provider,
            ask("title"),
            vec![],
            ModelSelection::model("gemini-3.5-flash"),
            None,
            None,
        )
        .await
        .expect("the scripted model answers");
    let rows = aux_captures(&pool, thread, "title").await;
    assert_eq!(rows[0]["model"], "gemini-3.5-flash");
    assert_eq!(rows[0]["served_model"], "gemini-3.6-flash");
    assert_eq!(rows[0]["provider"], "claude-gateway");

    let (_, capture) = turn_chat(
        &bus,
        thread,
        EventMeta::NONE,
        1,
        &provider,
        ask("hi"),
        vec![],
        ModelSelection::model("gemini-3.5-flash"),
        None,
        None,
    )
    .await
    .expect("the scripted model answers");
    capture.finish(turn_row("gemini-3.5-flash")).await;
    let turn: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events WHERE thread_id = $1 \
           AND event_type = 'ContextCaptured' AND payload->>'producer' = 'main_llm'",
    )
    .bind(thread)
    .fetch_one(&pool)
    .await
    .expect("read the turn row");
    assert_eq!(turn["served_model"], "gemini-3.6-flash");
    assert_eq!(turn["provider"], "claude-gateway");
    teardown_test_db(&db).await;
}

/// The row the agentic loop builds from a turn's report for a call to
/// `model`, cut to the fields the call itself reported.
fn turn_row(model: &str) -> impl FnOnce(TurnReport) -> ThreadEvent + '_ {
    move |report| ThreadEvent::ContextCaptured {
        producer: ContextProducer::MainLlm,
        model: model.to_string(),
        context_window: 0,
        sections: Vec::new(),
        tools: Vec::new(),
        estimated_total_tokens: 0,
        usage: report.usage,
        trimmed: false,
        trim_passes: Vec::new(),
        purpose: ContextPurpose::Turn,
        reconstructed: false,
        parent_tool_use_id: None,
        api_call_id: None,
        reasoning_effort: report.detail.reasoning_effort,
        duration_ms: report.detail.duration_ms,
        served_model: report.detail.served_model,
        provider: report.detail.provider,
        stable_prefix_fingerprint: report.stable_prefix_fingerprint,
        ttft_ms: report.detail.ttft_ms,
        request_id: report.detail.request_id,
        session: None,
    }
}

/// A provider that reports a cache write split by lifetime and the
/// fingerprint of the prefix it sent, the way the Anthropic wire does.
struct CacheReporting(ScriptedProvider);

#[async_trait::async_trait]
impl LlmProvider for CacheReporting {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        selection: ModelSelection<'_>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, BoxError> {
        let response = self
            .0
            .chat(messages, tools, selection, system_prompt, on_token, call)
            .await?;
        Ok(LlmResponse {
            usage: Some(crate::engine::ApiUsage {
                input_tokens: 50_000,
                cache_creation_tokens: 40_000,
                cache_creation_1h_tokens: Some(0),
                ..response.usage.unwrap_or_default()
            }),
            stable_prefix_fingerprint: Some("0123456789abcdef".to_string()),
            ..response
        })
    }
}

/// A turn row carries four things its call reported: the time it took, its
/// effort, its cache write's lifetime split and its prefix fingerprint. The
/// time places the request start, which a cache-gap measurement needs.
#[tokio::test]
async fn a_turn_row_carries_what_its_call_reported() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let provider = CacheReporting(ScriptedProvider::new("claude-opus-5-5", vec!["a"]));

    let (_, capture) = turn_chat(
        &bus,
        thread,
        EventMeta::NONE,
        1,
        &provider,
        ask("hi"),
        vec![],
        ModelSelection::model("claude-opus-5-5").with_effort(Some("high")),
        None,
        None,
    )
    .await
    .expect("the scripted model answers");
    capture.finish(turn_row("claude-opus-5-5")).await;
    let turn: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events WHERE thread_id = $1 AND event_type = 'ContextCaptured'",
    )
    .bind(thread)
    .fetch_one(&pool)
    .await
    .expect("read the turn row");
    assert_eq!(turn["producer"], "main_llm");
    assert!(turn["duration_ms"].is_u64(), "the call was timed: {turn}");
    assert_eq!(turn["reasoning_effort"], "high");
    assert_eq!(turn["usage"]["cache_creation_1h_tokens"], 0);
    assert_eq!(turn["stable_prefix_fingerprint"], "0123456789abcdef");
    teardown_test_db(&db).await;
}

/// A later round sends the same stable prefix as round 1, so only round 1
/// keeps the fingerprint.
#[tokio::test]
async fn only_round_one_keeps_the_stable_prefix_fingerprint() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let provider = CacheReporting(ScriptedProvider::new("claude-opus-5-5", vec!["a", "b"]));

    for round in [1, 2] {
        let (_, capture) = turn_chat(
            &bus,
            Uuid::new_v4(),
            EventMeta::NONE,
            round,
            &provider,
            ask("hi"),
            vec![],
            ModelSelection::model("claude-opus-5-5"),
            None,
            None,
        )
        .await
        .expect("the scripted model answers");
        let mut fingerprint = None;
        capture
            .finish(|report| {
                fingerprint = report.stable_prefix_fingerprint.clone();
                turn_row("claude-opus-5-5")(report)
            })
            .await;
        assert_eq!(
            fingerprint.is_some(),
            round == 1,
            "round {round}: {fingerprint:?}"
        );
    }
    teardown_test_db(&db).await;
}

/// A turn row nobody finished still records the call's time from `Drop`.
#[tokio::test]
async fn an_unfinished_turn_row_still_records_the_call_time() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let provider = CacheReporting(ScriptedProvider::new("claude-opus-5-5", vec!["a"]));

    let (_, capture) = turn_chat(
        &bus,
        thread,
        EventMeta::NONE,
        1,
        &provider,
        ask("hi"),
        vec![],
        ModelSelection::model("claude-opus-5-5"),
        None,
        None,
    )
    .await
    .expect("the scripted model answers");
    drop(capture);
    let mut turn = None;
    for _ in 0..50 {
        turn = sqlx::query_scalar::<_, serde_json::Value>(
            "SELECT payload FROM events WHERE thread_id = $1 AND event_type = 'ContextCaptured'",
        )
        .bind(thread)
        .fetch_optional(&pool)
        .await
        .expect("read the turn row");
        if turn.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let turn = turn.expect("Drop recorded the row");
    assert!(turn["duration_ms"].is_u64(), "the call was timed: {turn}");
    assert!(
        turn.get("stable_prefix_fingerprint").is_none(),
        "Drop cannot tell round 1, so it records no fingerprint"
    );
    teardown_test_db(&db).await;
}

// --- the lock -------------------------------------------------------------

/// Every engine source a check judges, minus the provider layer and the
/// crate's test doubles.
fn engine_sources() -> Vec<(String, String)> {
    crate::test_support::source_scan::production_sources()
        .into_iter()
        .filter(|(path, _)| !path.starts_with("llm/") && path != "test_support.rs")
        .collect()
}

/// **Each billable provider method takes a `CallToken`.** That is what makes
/// a model call outside `llm::metered` fail to compile. A new billable
/// trait, or a method that drops the token, fails here first.
#[test]
fn every_billable_provider_method_takes_a_call_token() {
    let traits = [
        (
            include_str!("../../llm/provider.rs"),
            "pub trait LlmProvider",
            "async fn chat(",
        ),
        (
            include_str!("../../llm/judgment/mod.rs"),
            "pub trait JudgmentProvider",
            "async fn ask(",
        ),
        (
            include_str!("../../llm/image.rs"),
            "pub trait ImageProvider",
            "async fn generate(",
        ),
        (
            include_str!("../../llm/web_search/mod.rs"),
            "pub trait WebSearchProvider",
            "async fn search(",
        ),
    ];
    for (source, name, method) in traits {
        let body = source.split(name).nth(1).expect("the trait is defined");
        let signature = body
            .split(method)
            .nth(1)
            .and_then(|rest| rest.split(';').next())
            .unwrap_or_else(|| panic!("{name} has {method}"));
        assert!(
            signature.contains("CallToken"),
            "{name}'s {method}..) takes no CallToken, so it can be called \
             without the model call service recording it"
        );
    }
}

/// The eval constructor is the one token outside this module, and only
/// `lucidos-eval` may use it.
#[test]
fn no_engine_source_mints_the_eval_token() {
    let minting: Vec<String> = engine_sources()
        .into_iter()
        .filter(|(_, body)| body.contains("for_eval_harness"))
        .map(|(path, _)| path)
        .collect();
    assert!(
        minting.is_empty(),
        "engine sources mint the eval token, so their calls record nothing: {minting:?}"
    );
}

/// What keeps embedding outside the service honest.
///
/// Embedding costs nothing because it runs in-process on fastembed. A remote
/// embedder would be billable spend with no purpose and no row, so it fails
/// here first and the decision gets made deliberately.
#[test]
fn the_only_embedders_run_in_process() {
    let found: Vec<String> = engine_sources()
        .into_iter()
        .filter(|(_, body)| body.contains("impl EmbeddingProvider for"))
        .map(|(path, _)| path)
        .collect();
    assert_eq!(
        found,
        vec!["memory/embedder_slot.rs", "memory/fastembed.rs"],
        "an embedding provider was added or moved. One calling a remote \
         endpoint spends real tokens, so it needs a ContextPurpose and a \
         CallToken on its method"
    );
}

// --- deadlines and the other call kinds -------------------------------------

/// A provider that answers after `delay`.
struct Slow {
    delay: std::time::Duration,
    inner: ScriptedProvider,
}

#[async_trait::async_trait]
impl LlmProvider for Slow {
    async fn chat(
        &self,
        messages: Vec<Message>,
        tools: Vec<ToolDefinition>,
        selection: ModelSelection<'_>,
        system_prompt: Option<&str>,
        on_token: Option<TokenCallback>,
        call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, BoxError> {
        tokio::time::sleep(self.delay).await;
        self.inner
            .chat(messages, tools, selection, system_prompt, on_token, call)
            .await
    }
}

/// The deadline bounds the provider call and nothing else. A call that runs
/// out reports a typed timeout and records no capture, since it reported
/// nothing. A call that answers in time keeps its answer and its row.
#[tokio::test]
async fn a_deadline_bounds_the_call_never_its_record() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let capture = AuxCapture::new(&bus, thread, ContextPurpose::Title);
    let slow = Slow {
        delay: std::time::Duration::from_millis(300),
        inner: ScriptedProvider::new("title-model", vec!["late", "on time"]),
    };
    let soon = tokio::time::Instant::now() + std::time::Duration::from_millis(20);

    let late = capture
        .until(soon)
        .chat(
            &slow,
            ask("hi"),
            vec![],
            ModelSelection::default(),
            None,
            None,
        )
        .await
        .expect_err("the call outran its deadline");
    assert!(late.is::<tokio::time::error::Elapsed>(), "{late}");
    assert!(aux_captures(&pool, thread, "title").await.is_empty());

    let later = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    capture
        .until(later)
        .chat(
            &slow,
            ask("hi"),
            vec![],
            ModelSelection::default(),
            None,
            None,
        )
        .await
        .expect("the call answered in time");
    assert_eq!(aux_captures(&pool, thread, "title").await.len(), 1);
    teardown_test_db(&db).await;
}

/// Every `ModelCallFailed` row on a thread, oldest first.
async fn failure_rows(pool: &sqlx::PgPool, thread: Uuid) -> Vec<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT payload FROM events \
         WHERE thread_id = $1 AND event_type = 'ModelCallFailed' ORDER BY sequence ASC",
    )
    .bind(thread)
    .fetch_all(pool)
    .await
    .expect("read the failure rows")
}

async fn capture_count(pool: &sqlx::PgPool, thread: Uuid) -> i64 {
    sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE thread_id = $1 AND event_type = 'ContextCaptured'",
    )
    .bind(thread)
    .fetch_one(pool)
    .await
    .expect("count the captures")
}

/// A failed call leaves a `ModelCallFailed`, never a capture a rollup could
/// bill. The row holds facts only: the provider's message, which can echo the
/// request, appears nowhere in it, while the caller still gets the error.
#[tokio::test]
async fn a_failed_call_records_facts_and_never_a_capture() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let capture = AuxCapture::new(&bus, thread, ContextPurpose::Title);
    let provider = crate::test_support::JudgmentChatStub::failing("body echoed the SECRET prompt");

    let error = capture
        .chat(
            &provider,
            ask("hi"),
            vec![],
            ModelSelection::model("title-model").with_effort(Some("low")),
            None,
            None,
        )
        .await
        .expect_err("the provider failed");
    assert!(error.to_string().contains("SECRET"), "the caller keeps it");

    let rows = failure_rows(&pool, thread).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    let row = &rows[0];
    assert_eq!(row["model"], "title-model");
    assert_eq!(row["purpose"], "title");
    assert_eq!(row["reasoning_effort"], "low");
    assert_eq!(row["failure"], "other", "no send, no status, no deadline");
    assert_eq!(row["attempts"], 0);
    assert!(row["duration_ms"].is_u64(), "{row}");
    assert!(!row.to_string().contains("SECRET"), "{row}");
    assert_eq!(
        capture_count(&pool, thread).await,
        0,
        "a failure bills nothing"
    );
    teardown_test_db(&db).await;
}

/// A call that runs past its deadline fails as a timeout, which no send
/// attempt sees.
#[tokio::test]
async fn a_call_past_its_deadline_fails_as_a_timeout() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let slow = Slow {
        delay: std::time::Duration::from_millis(300),
        inner: ScriptedProvider::new("title-model", vec!["late"]),
    };
    AuxCapture::new(&bus, thread, ContextPurpose::Title)
        .until(tokio::time::Instant::now() + std::time::Duration::from_millis(20))
        .chat(
            &slow,
            ask("hi"),
            vec![],
            ModelSelection::default(),
            None,
            None,
        )
        .await
        .expect_err("the call outran its deadline");

    let rows = failure_rows(&pool, thread).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["failure"], "timeout");
    teardown_test_db(&db).await;
}

/// A turn's failure row keeps the turn's `request_event_id`, so the exporter
/// can name the prompt it belongs to.
#[tokio::test]
async fn a_failed_turn_call_keeps_the_turns_request() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let origin = Uuid::new_v4();
    let meta = EventMeta {
        request_event_id: Some(origin),
        ..EventMeta::NONE
    };
    let provider = crate::test_support::JudgmentChatStub::failing("down");

    turn_chat(
        &bus,
        thread,
        meta,
        1,
        &provider,
        ask("hi"),
        vec![],
        ModelSelection::model("turn-model"),
        None,
        None,
    )
    .await
    .err()
    .expect("the provider failed");

    let rows = failure_rows(&pool, thread).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0]["request_event_id"], origin.to_string());
    assert!(rows[0].get("purpose").is_none(), "a turn writes no purpose");
    assert_eq!(capture_count(&pool, thread).await, 0);
    teardown_test_db(&db).await;
}

/// The `Drop` fallback records the turn's row under the turn's meta, not the
/// empty one it wrote before.
#[tokio::test]
async fn an_unfinished_turn_row_keeps_the_turns_request() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let origin = Uuid::new_v4();
    let meta = EventMeta {
        request_event_id: Some(origin),
        ..EventMeta::NONE
    };
    let provider = ScriptedProvider::new("turn-model", vec!["hello"]);

    let (_, capture) = turn_chat(
        &bus,
        thread,
        meta,
        1,
        &provider,
        ask("hi"),
        vec![],
        ModelSelection::model("turn-model"),
        None,
        None,
    )
    .await
    .expect("the scripted model answers");
    drop(capture);

    let mut row = None;
    for _ in 0..50 {
        row = sqlx::query_scalar::<_, serde_json::Value>(
            "SELECT payload FROM events WHERE thread_id = $1 AND event_type = 'ContextCaptured'",
        )
        .bind(thread)
        .fetch_optional(&pool)
        .await
        .expect("read the turn row");
        if row.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let row = row.expect("Drop recorded the row");
    assert_eq!(row["request_event_id"], origin.to_string());
    teardown_test_db(&db).await;
}

/// A search backend that answers with fixed usage.
struct Answering;

#[async_trait::async_trait]
impl crate::llm::web_search::WebSearchProvider for Answering {
    async fn search(
        &self,
        _query: &str,
        _max_results: usize,
        _call: crate::llm::metered::CallToken,
    ) -> Result<crate::llm::web_search::WebSearchResult, BoxError> {
        Ok(crate::llm::web_search::WebSearchResult {
            text: "found it".to_string(),
            usage: Some(crate::engine::ApiUsage {
                input_tokens: 70,
                output_tokens: 9,
                ..Default::default()
            }),
            model: "search-model".to_string(),
        })
    }

    fn id(&self) -> &'static str {
        "answering"
    }
}

/// Each web search records one row, naming the backend's model and what it
/// reported.
#[tokio::test]
async fn a_web_search_records_the_backend_that_answered() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let chain = WebSearchChain::new(vec![std::sync::Arc::new(Answering)]);

    let text = AuxCapture::new(&bus, thread, ContextPurpose::WebSearch)
        .search(&chain, "weather in Oslo", 5)
        .await
        .expect("the backend answers");

    assert_eq!(text, "found it");
    let rows = aux_captures(&pool, thread, "web_search").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["model"], "search-model");
    assert_eq!(rows[0]["usage"]["input_tokens"], 70);
    teardown_test_db(&db).await;
}

/// A judgment provider the router sent to a backend, whose reply named the
/// model that served it.
struct Routed;

#[async_trait::async_trait]
impl crate::llm::judgment::JudgmentProvider for Routed {
    async fn ask(
        &self,
        _state: serde_json::Value,
        _questions: Vec<(String, crate::llm::judgment::Question)>,
        _call: crate::llm::metered::CallToken,
    ) -> Result<crate::llm::judgment::Judgment, BoxError> {
        crate::llm::call_trace::routed_to(crate::llm::ProviderKind::ClaudeGateway);
        Ok(crate::llm::judgment::Judgment {
            model: Some("gemini-3.5-flash".to_string()),
            served_model: Some("gemini-3.6-flash".to_string()),
            ..Default::default()
        })
    }
}

/// A judgment row keeps the call's clock, the backend the router chose, and
/// the model the reply named.
#[tokio::test]
async fn a_judgment_records_its_time_backend_and_served_model() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();

    AuxCapture::new(&bus, thread, ContextPurpose::CommandJudge)
        .judge(&Routed, serde_json::json!({}), vec![])
        .await
        .expect("the judge answers");

    let rows = aux_captures(&pool, thread, "command_judge").await;
    assert_eq!(rows.len(), 1);
    assert!(rows[0]["duration_ms"].is_u64(), "row: {}", rows[0]);
    assert_eq!(rows[0]["provider"], "claude-gateway");
    assert_eq!(rows[0]["served_model"], "gemini-3.6-flash");
    teardown_test_db(&db).await;
}

/// A web search row keeps the call's clock too.
#[tokio::test]
async fn a_web_search_records_its_time() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    let chain = WebSearchChain::new(vec![std::sync::Arc::new(Answering)]);

    AuxCapture::new(&bus, thread, ContextPurpose::WebSearch)
        .search(&chain, "weather in Oslo", 5)
        .await
        .expect("the backend answers");

    let rows = aux_captures(&pool, thread, "web_search").await;
    assert!(rows[0]["duration_ms"].is_u64(), "row: {}", rows[0]);
    teardown_test_db(&db).await;
}
