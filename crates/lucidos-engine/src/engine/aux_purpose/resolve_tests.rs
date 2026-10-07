//! `AuxCall::resolve` against a router, end to end: what it picks, what the
//! router receives, and what the cost record names.

use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;

use super::*;
use crate::engine::event_bus::EventBus;
use crate::llm::model_registry::{load_from_db, ProviderKind};
use crate::llm::provider::{LlmResponse, Message, ModelSelection, TokenCallback, ToolDefinition};
use crate::test_support::{aux_captures, setup_test_db, teardown_test_db};

/// A router holding `configured`, recording the model and cap each request
/// reached it with.
struct StubRouter {
    configured: Vec<ProviderKind>,
    default_model: &'static str,
    seen: Mutex<Vec<(Option<String>, Option<Duration>)>>,
}

impl StubRouter {
    fn holding(configured: &[ProviderKind]) -> Arc<Self> {
        Self::defaulting_to(configured, "claude-opus-5-5")
    }

    /// A router whose own default, `LUCIDOS_MODEL` in production, is `model`.
    fn defaulting_to(configured: &[ProviderKind], model: &'static str) -> Arc<Self> {
        Arc::new(Self {
            configured: configured.to_vec(),
            default_model: model,
            seen: Mutex::default(),
        })
    }
}

#[async_trait]
impl LlmProvider for StubRouter {
    async fn chat(
        &self,
        _messages: Vec<Message>,
        _tools: Vec<ToolDefinition>,
        selection: ModelSelection<'_>,
        _system_prompt: Option<&str>,
        _on_token: Option<TokenCallback>,
        _call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        self.seen.lock().unwrap().push((
            selection.model.map(str::to_string),
            selection.attempt_timeout,
        ));
        Ok(LlmResponse {
            content: Some("A summary.".to_string()),
            input_tokens: Some(120),
            output_tokens: Some(8),
            ..LlmResponse::default()
        })
    }

    fn default_model(&self) -> &str {
        self.default_model
    }

    fn configured_providers(&self) -> Option<Vec<ProviderKind>> {
        Some(self.configured.clone())
    }
}

/// An Anthropic-only install: no Vertex, no extractor. The summary still runs,
/// on Sonnet 5.5, with the purpose's attempt cap, and its cost names Sonnet.
#[tokio::test]
async fn an_install_without_vertex_runs_and_records_the_model_it_ran() {
    let (pool, db_name) = setup_test_db().await;
    let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
    let router = StubRouter::holding(&[ProviderKind::Anthropic]);
    let purpose = ContextPurpose::ConversationSummary;

    let call = AuxCall::resolve(&pool, &registry, router.clone(), purpose).await;
    assert_eq!(call.model(), "claude-sonnet-5-5");
    assert_eq!(call.selection().source, ModelSource::Default);
    assert!(call.selection().reachable);

    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = uuid::Uuid::new_v4();
    let capture = crate::engine::AuxCapture::new(&bus, thread_id, purpose);
    crate::memory::summarize_conversation(
        call.provider().as_ref(),
        "turns",
        call.reasoning(),
        &capture,
    )
    .await
    .expect("the stub answers");

    assert_eq!(
        router.seen.lock().unwrap().as_slice(),
        [(
            Some("claude-sonnet-5-5".to_string()),
            Some(budget_for(purpose).attempt_timeout)
        )]
    );
    let captures = aux_captures(&pool, thread_id, "conversation_summary").await;
    assert_eq!(captures.len(), 1);
    assert_eq!(captures[0]["model"], "claude-sonnet-5-5");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The selection reads the router it is handed, so a credential added at
/// runtime moves the very next call.
#[tokio::test]
async fn a_swapped_router_moves_the_next_call() {
    let (pool, db_name) = setup_test_db().await;
    let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
    let purpose = ContextPurpose::Title;

    let before = AuxCall::resolve(
        &pool,
        &registry,
        StubRouter::holding(&[ProviderKind::OpenAi]),
        purpose,
    )
    .await;
    let after = AuxCall::resolve(
        &pool,
        &registry,
        StubRouter::holding(&[ProviderKind::OpenAi, ProviderKind::Vertex]),
        purpose,
    )
    .await;
    assert_eq!(before.model(), "gpt-5.4-mini");
    assert_eq!(after.model(), "gemini-3-flash-preview");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A mock engine runs these calls on its chat model, the mock, so an e2e run
/// spends nothing and the record names the mock.
#[tokio::test]
async fn a_mock_engine_runs_on_its_mock() {
    let (pool, db_name) = setup_test_db().await;
    let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
    let mock: Arc<dyn LlmProvider> = Arc::new(crate::llm::mock::MockProvider::new(
        crate::llm::MOCK_MODEL.to_string(),
    ));
    for purpose in [
        ContextPurpose::Title,
        ContextPurpose::Memory,
        ContextPurpose::CommandJudge,
        ContextPurpose::SummaryCompaction,
        ContextPurpose::ImageDescribe,
    ] {
        let call = AuxCall::resolve(&pool, &registry, mock.clone(), purpose).await;
        assert_eq!(call.model(), crate::llm::MOCK_MODEL, "{purpose:?}");
        assert_eq!(call.selection().source, ModelSource::ChatModel);
        assert_eq!(call.provider().default_model(), crate::llm::MOCK_MODEL);
        assert_eq!(call.selection().refusal(), None, "{purpose:?}");
    }
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// With nothing recommended reachable and no `chat_model` stored, the call
/// runs on the model turns run on: the router's own default. The catalog's
/// chat default names a model this install cannot reach.
#[tokio::test]
async fn the_last_resort_is_the_model_turns_run_on() {
    let (pool, db_name) = setup_test_db().await;
    let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
    let router = StubRouter::defaulting_to(&[ProviderKind::XAi], "grok-4.6");
    let call = AuxCall::resolve(&pool, &registry, router, ContextPurpose::Title).await;
    assert_eq!(call.model(), "grok-4.6");
    assert_eq!(call.selection().source, ModelSource::ChatModel);
    assert!(call.selection().reachable);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// With no recommended model that reads images reachable, image description
/// falls back to the chat model only to refuse it. Titles still run on it.
#[tokio::test]
async fn image_description_refuses_a_text_only_chat_model() {
    let (pool, db_name) = setup_test_db().await;
    let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
    let router = StubRouter::defaulting_to(&[ProviderKind::XAi], "grok-4.6");
    let call = AuxCall::resolve(
        &pool,
        &registry,
        router.clone(),
        ContextPurpose::ImageDescribe,
    )
    .await;
    assert_eq!(call.model(), "grok-4.6");
    assert_eq!(call.selection().source, ModelSource::ChatModel);
    assert!(!call.selection().vision);
    let refusal = call.selection().refusal().expect("refused");
    assert!(refusal.contains("'grok-4.6' cannot read them"), "{refusal}");

    let title = AuxCall::resolve(&pool, &registry, router, ContextPurpose::Title).await;
    assert_eq!(title.selection().refusal(), None);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A chat model that reads images is a fallback image description runs on.
#[tokio::test]
async fn image_description_runs_on_a_chat_model_that_reads_images() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let looker = crate::core::models::ModelFields {
        label: "Looker".to_string(),
        routes: vec![crate::core::models::Route::bare("local")],
        preferred_provider: None,
        vision: true,
        sort_order: 1000,
    };
    crate::core::ModelStore::create(&pool, &bus, "local-looker", &looker, None)
        .await
        .unwrap();
    let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
    let router = StubRouter::defaulting_to(&[ProviderKind::Local], "local-looker");
    let call = AuxCall::resolve(&pool, &registry, router, ContextPurpose::ImageDescribe).await;
    assert_eq!(call.model(), "local-looker");
    assert_eq!(call.selection().source, ModelSource::ChatModel);
    assert_eq!(call.selection().refusal(), None);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A stored pick that cannot read images is refused by name, as the user's
/// own choice.
#[tokio::test]
async fn a_stored_text_only_pick_is_refused_by_name() {
    let (pool, db_name) = setup_test_db().await;
    let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
    crate::core::PreferenceStore::set_row_for_test(
        &pool,
        prefs::MODEL_IMAGE_DESCRIPTION.key(),
        "z-ai/glm-5.2",
    )
    .await
    .unwrap();
    let router = StubRouter::holding(&[ProviderKind::OpenRouter]);
    let call = AuxCall::resolve(&pool, &registry, router, ContextPurpose::ImageDescribe).await;
    assert_eq!(call.model(), "z-ai/glm-5.2");
    assert_eq!(call.selection().source, ModelSource::Preference);
    assert!(call.selection().reachable);
    let refusal = call.selection().refusal().expect("refused");
    assert!(refusal.contains("chosen model 'z-ai/glm-5.2'"), "{refusal}");
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Settings once stored `default` for "the extractor's own model". That
/// reads as unset now, never as a model named `default`.
#[tokio::test]
async fn a_stored_default_reads_as_unset() {
    let (pool, db_name) = setup_test_db().await;
    let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
    crate::core::PreferenceStore::set_row_for_test(&pool, prefs::MODEL_TITLE.key(), "default")
        .await
        .unwrap();
    let router = StubRouter::holding(&[ProviderKind::Anthropic]);
    let call = AuxCall::resolve(&pool, &registry, router, ContextPurpose::Title).await;
    assert_eq!(call.model(), "claude-haiku-4-5");
    assert_eq!(call.selection().source, ModelSource::Default);
    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A stored `model_memory` and `reasoning_memory` bind extraction alone. The
/// other memory tasks resolve their own defaults, model and effort.
#[tokio::test]
async fn a_stored_memory_pair_binds_extraction_alone() {
    let (pool, db_name) = setup_test_db().await;
    let registry: ModelRegistry = Arc::new(RwLock::new(load_from_db(&pool).await));
    for (key, value) in [
        (prefs::MODEL_MEMORY.key(), "gemini-3.5-flash"),
        (prefs::REASONING_MEMORY.key(), "high"),
    ] {
        crate::core::PreferenceStore::set_row_for_test(&pool, key, value)
            .await
            .unwrap();
    }
    let router = StubRouter::holding(&[ProviderKind::Vertex]);
    let resolve = |purpose| {
        let router: Arc<dyn LlmProvider> = router.clone();
        let (pool, registry) = (&pool, &registry);
        async move { resolve_selection(pool, registry, router.as_ref(), purpose).await }
    };

    let extraction = resolve(ContextPurpose::Memory).await;
    assert_eq!(extraction.model, "gemini-3.5-flash");
    assert_eq!(extraction.source, ModelSource::Preference);
    assert_eq!(extraction.reasoning.as_deref(), Some("high"));

    for purpose in [
        ContextPurpose::QueryClassification,
        ContextPurpose::ConversationSummary,
        ContextPurpose::MemoryFind,
    ] {
        let selection = resolve(purpose).await;
        let pair = model_source(purpose).prefs().expect("prefs");
        assert_eq!(
            selection.model,
            recommended(purpose, &registry)
                .into_iter()
                .find(|m| is_reachable(m, &registry, |k| k == ProviderKind::Vertex))
                .expect("Vertex serves a recommended model"),
            "{purpose:?}"
        );
        assert_eq!(selection.source, ModelSource::Default, "{purpose:?}");
        assert_eq!(
            selection.reasoning.as_deref(),
            pair.reasoning.map(|r| r.default_text()),
            "{purpose:?}"
        );
    }
    pool.close().await;
    teardown_test_db(&db_name).await;
}
