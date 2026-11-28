mod browse_tests;
mod compaction_view_tests;
mod compactor_breaker_tests;
mod compactor_tests;
mod fold_tests;
mod log_tests;
mod shape_tests;
mod view_tests;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

use super::compactor::{CompactionModel, CompactorDeps};
use super::prompt::flatten;
use super::spend::StartEstimate;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
use crate::llm::provider::{LlmProvider, LlmResponse, Message};
use crate::llm::ServiceTier;

pub(super) fn event(value: serde_json::Value) -> ThreadEvent {
    serde_json::from_value(value).expect("fixture is a valid ThreadEvent")
}

pub(super) async fn seed(
    bus: &EventBus,
    thread_id: Uuid,
    channel: EventChannel,
    events: Vec<serde_json::Value>,
) -> Vec<Uuid> {
    let mut ids = Vec::new();
    for value in events {
        let emitted = bus
            .emit(BusEvent::Thread {
                thread_id,
                event: event(value),
                meta: EventMeta {
                    channel: Some(channel),
                    ..EventMeta::NONE
                },
            })
            .await
            .expect("seed emit")
            .expect("seed is persisted");
        ids.push(emitted.event_id);
    }
    ids
}

pub(super) async fn seed_artifact(bus: &EventBus, path: &str, commit: &str, writer: Option<Uuid>) {
    bus.emit(BusEvent::System(SystemEvent::ArtifactCreated {
        artifact_path: path.to_string(),
        commit: commit.to_string(),
        source: None,
        writer_thread_id: writer,
        writer: None,
    }))
    .await
    .expect("seed artifact");
}

pub(super) async fn choose_module(pool: &PgPool, bus: &EventBus, value: &str) {
    crate::core::PreferenceStore::set(
        pool,
        bus,
        crate::core::prefs::MEMORY_MODULE.key(),
        value,
        None,
    )
    .await
    .unwrap();
}

pub(super) async fn age_events(pool: &PgPool) {
    sqlx::query("UPDATE events SET created = created - interval '1 hour'")
        .execute(pool)
        .await
        .expect("age events");
}

pub(super) struct EchoProvider {
    pub(super) calls: AtomicUsize,
    pub(super) failing: Option<std::ops::Range<usize>>,
    pub(super) refuse_flex: bool,
    pub(super) selections: std::sync::Mutex<Vec<(Option<String>, Option<String>)>>,
    pub(super) tiers: std::sync::Mutex<Vec<Option<ServiceTier>>>,
    pub(super) cache_modes: std::sync::Mutex<Vec<crate::llm::CacheMode>>,
}

impl EchoProvider {
    pub(super) fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            failing: None,
            refuse_flex: false,
            selections: std::sync::Mutex::default(),
            tiers: std::sync::Mutex::default(),
            cache_modes: std::sync::Mutex::default(),
        }
    }

    pub(super) fn failing_calls(calls: std::ops::Range<usize>) -> Self {
        Self {
            failing: Some(calls),
            ..Self::new()
        }
    }
}

#[async_trait::async_trait]
impl LlmProvider for EchoProvider {
    async fn chat(
        &self,
        messages: Vec<Message>,
        _tools: Vec<crate::llm::provider::ToolDefinition>,
        selection: crate::llm::ModelSelection<'_>,
        _system_prompt: Option<&str>,
        _on_token: Option<crate::llm::provider::TokenCallback>,
        _call: crate::llm::metered::CallToken,
    ) -> Result<LlmResponse, Box<dyn std::error::Error + Send + Sync>> {
        self.selections.lock().unwrap().push((
            selection.model.map(str::to_string),
            selection.reasoning_effort.map(str::to_string),
        ));
        self.tiers.lock().unwrap().push(selection.service_tier);
        self.cache_modes.lock().unwrap().push(selection.cache_mode);
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.refuse_flex && selection.service_tier == Some(ServiceTier::Flex) {
            return Err("429 Resource Unavailable: no Flex capacity".into());
        }
        if self
            .failing
            .as_ref()
            .is_some_and(|calls| calls.contains(&n))
        {
            return Err("the model is down".into());
        }
        let request = messages[0].content.as_text();
        let step = request
            .rsplit_once("<input>\n")
            .and_then(|(_, rest)| rest.rsplit_once("\n</input>"))
            .map(|(input, _)| input.to_string())
            .unwrap_or(request.clone());
        if step.contains("POISON") {
            return Err("the model refused".into());
        }
        let line = if step.contains("OVERSHOOT") && messages.len() == 1 {
            "x".repeat(600)
        } else {
            format!(
                "sum[{}]",
                flatten(&step).chars().take(60).collect::<String>()
            )
        };
        let mut response = line_response(line);
        if let Some(usage) = response.usage.as_mut() {
            usage.service_tier = selection.service_tier;
        }
        Ok(response)
    }

    fn default_model(&self) -> &str {
        "echo-model"
    }
}

pub(super) fn line_response(line: String) -> LlmResponse {
    LlmResponse {
        content: Some(line),
        tool_calls: vec![],
        stop_reason: Some("end_turn".to_string()),
        usage: Some(crate::engine::types::ApiUsage {
            input_tokens: 100,
            output_tokens: 10,
            ..Default::default()
        }),
        stable_prefix_fingerprint: None,
        served_model: None,
        provider: None,
        thinking_chars: None,
        thinking_blocks: None,
        unknown_sse_dropped: 0,
        model_only_text: None,
        progress_notes: Vec::new(),
        prefill: None,
    }
}

pub(super) const COMPACTOR_MODEL: &str = "compactor-model";
pub(super) const COMPACTOR_EFFORT: &str = "low";
pub(super) const TEST_LANE: &str = "test/compactor-model";

pub(super) const NO_ESTIMATE: &str = "this test sets no estimate";

pub(super) struct TestDeps {
    pub(super) provider: Option<Arc<EchoProvider>>,
    pub(super) artifacts: Vec<(String, String)>,
    pub(super) estimate: Option<StartEstimate>,
}

#[async_trait::async_trait]
impl CompactorDeps for TestDeps {
    async fn notify_model_down(&self, _model: &str) {}

    async fn model(&self) -> Result<CompactionModel, String> {
        let provider = self.provider.clone().ok_or("no background model")?;
        Ok(CompactionModel {
            provider,
            model: COMPACTOR_MODEL.to_string(),
            effort: Some(COMPACTOR_EFFORT.to_string()),
            deadline: Duration::from_secs(5),
            lane: TEST_LANE.to_string(),
            flex: false,
        })
    }

    fn read_artifact(&self, path: &str, _commit: &str) -> Option<String> {
        self.artifacts
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, c)| c.clone())
    }

    async fn backfill_estimate(&self) -> Result<StartEstimate, String> {
        self.estimate.clone().ok_or_else(|| NO_ESTIMATE.to_string())
    }
}

pub(super) fn deps(provider: EchoProvider) -> Arc<TestDeps> {
    deps_with(Arc::new(provider))
}

pub(super) fn deps_with(provider: Arc<EchoProvider>) -> Arc<TestDeps> {
    Arc::new(TestDeps {
        provider: Some(provider),
        artifacts: vec![(
            "notes/plan.md".to_string(),
            format!("# Plan\n{}", "A line of the plan. ".repeat(40)),
        )],
        estimate: None,
    })
}

#[test]
fn the_ruler_is_exactly_the_node_size() {
    let ruler = super::prompt::ruler();
    assert_eq!(ruler.len(), super::NODE_BYTES);
    assert!(ruler.chars().all(|c| c == '-'));
    assert!(super::prompt::compress_step("m").contains(&format!("{ruler}\n<input>\nm\n</input>")));
    assert!(super::prompt::merge_step("a", "b").contains(&ruler));
}

#[test]
fn the_workspace_leaf_index_lists_every_leaf_event_type() {
    let migration = include_str!(
        "../../../../../lucidos-core/migrations/20261004132639_create_summary_trees.sql"
    );
    let index = migration
        .split("idx_events_summary_tree_workspace_leaves")
        .nth(1)
        .expect("the migration creates the index");
    for name in super::workspace_log::WORKSPACE_LEAF_EVENT_TYPES {
        assert!(index.contains(&format!("'{name}'")), "{name} is missing");
    }
    assert_eq!(
        index.matches('\'').count(),
        super::workspace_log::WORKSPACE_LEAF_EVENT_TYPES.len() * 2
    );
}

#[test]
fn every_turn_end_also_ends_a_workspace_turn() {
    for name in super::log::TURN_END_EVENT_TYPES {
        assert!(super::workspace_log::WORKSPACE_LEAF_EVENT_TYPES.contains(name));
    }
}
