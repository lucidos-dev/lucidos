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
            ..response
        })
    }
}

/// Both rows keep the model Lucidos asked for and the one the reply named.
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

    let (_, capture) = turn_chat(
        &bus,
        thread,
        &provider,
        ask("hi"),
        vec![],
        ModelSelection::model("gemini-3.5-flash"),
        None,
        None,
    )
    .await
    .expect("the scripted model answers");
    capture
        .finish(EventMeta::NONE, |usage, served_model| {
            ThreadEvent::ContextCaptured {
                producer: ContextProducer::MainLlm,
                model: "gemini-3.5-flash".to_string(),
                context_window: 0,
                sections: Vec::new(),
                tools: Vec::new(),
                estimated_total_tokens: 0,
                usage,
                trimmed: false,
                trim_passes: Vec::new(),
                purpose: ContextPurpose::Turn,
                reconstructed: false,
                parent_tool_use_id: None,
                api_call_id: None,
                reasoning_effort: None,
                duration_ms: None,
                served_model,
            }
        })
        .await;
    let turn: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events WHERE thread_id = $1 \
           AND event_type = 'ContextCaptured' AND payload->>'producer' = 'main_llm'",
    )
    .bind(thread)
    .fetch_one(&pool)
    .await
    .expect("read the turn row");
    assert_eq!(turn["served_model"], "gemini-3.6-flash");
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
/// out reports a typed timeout and records nothing, since it reported nothing.
/// A call that answers in time keeps its answer and its row.
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
            usage: Some(crate::llm::usage_wire::ProviderUsage {
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
