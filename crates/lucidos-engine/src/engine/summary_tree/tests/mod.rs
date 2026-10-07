//! Summary tree tests. The pure halves (the log projection, the tree
//! arithmetic, the prompt) run without a database. The compactor's invariants
//! (I1, I3, I8, I9) run against a real Postgres on a scripted model.

mod browse_tests;
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
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
use crate::llm::provider::{LlmProvider, LlmResponse, Message};

/// A thread event from its wire JSON, so a fixture names only the fields it
/// cares about.
pub(super) fn event(value: serde_json::Value) -> ThreadEvent {
    serde_json::from_value(value).expect("fixture is a valid ThreadEvent")
}

/// Emit `events` on `thread_id` under `channel`, returning each row's id.
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

/// An artifact write, by `writer` when a thread's turn made it.
pub(super) async fn seed_artifact(bus: &EventBus, path: &str, commit: &str, writer: Option<Uuid>) {
    bus.emit(BusEvent::System(SystemEvent::ArtifactCreated {
        artifact_path: path.to_string(),
        commit: commit.to_string(),
        source: None,
        writer_thread_id: writer,
    }))
    .await
    .expect("seed artifact");
}

/// Push every event past the compactor's horizon.
pub(super) async fn age_events(pool: &PgPool) {
    sqlx::query("UPDATE events SET created = created - interval '1 hour'")
        .execute(pool)
        .await
        .expect("age events");
}

/// A deterministic model. It answers with a short line built from the step it
/// was asked, so two builds of one log write the same text.
///
/// A step containing `POISON` always fails. One containing `OVERSHOOT` gets a
/// line over the limit first, to drive the size loop. After `fail_after`
/// calls, every call fails, which is how a test stops the compactor mid-build.
pub(super) struct EchoProvider {
    pub(super) calls: AtomicUsize,
    pub(super) fail_after: Option<usize>,
    /// The model and effort each call named.
    pub(super) selections: std::sync::Mutex<Vec<(Option<String>, Option<String>)>>,
}

impl EchoProvider {
    pub(super) fn new() -> Self {
        Self {
            calls: AtomicUsize::new(0),
            fail_after: None,
            selections: std::sync::Mutex::default(),
        }
    }

    pub(super) fn failing_after(calls: usize) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            fail_after: Some(calls),
            selections: std::sync::Mutex::default(),
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
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_after.is_some_and(|limit| n >= limit) {
            return Err("the model is down".into());
        }
        let request = messages[0].content.as_text();
        let step = request
            .split_once("bytes:\n")
            .and_then(|(_, rest)| rest.rsplit_once("bytes:\n"))
            .map(|(_, step)| step.to_string())
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
        Ok(line_response(line))
    }

    fn default_model(&self) -> &str {
        "echo-model"
    }
}

/// A scripted model's answer: one line, with token counts to record.
pub(super) fn line_response(line: String) -> LlmResponse {
    LlmResponse {
        content: Some(line),
        tool_calls: vec![],
        stop_reason: Some("end_turn".to_string()),
        output_tokens: Some(10),
        input_tokens: Some(100),
        cache_creation_tokens: None,
        cache_read_tokens: None,
        thinking_chars: None,
        thinking_blocks: None,
        unknown_sse_dropped: 0,
        model_only_text: None,
        content_is_progress_notes: false,
    }
}

/// The model and effort [`TestDeps`] select. Distinct from
/// [`EchoProvider`]'s own default, so a test can tell which one ran.
pub(super) const COMPACTOR_MODEL: &str = "compactor-model";
pub(super) const COMPACTOR_EFFORT: &str = "low";
/// The lane [`TestDeps`] calls through.
pub(super) const TEST_LANE: &str = "test/compactor-model";

/// Engine stand-ins: a model, and artifacts held in a map.
pub(super) struct TestDeps {
    pub(super) provider: Option<Arc<EchoProvider>>,
    pub(super) artifacts: Vec<(String, String)>,
}

#[async_trait::async_trait]
impl CompactorDeps for TestDeps {
    async fn model(&self) -> Result<CompactionModel, String> {
        let provider = self.provider.clone().ok_or("no background model")?;
        Ok(CompactionModel {
            provider,
            model: COMPACTOR_MODEL.to_string(),
            effort: Some(COMPACTOR_EFFORT.to_string()),
            deadline: Duration::from_secs(5),
            lane: TEST_LANE.to_string(),
        })
    }

    fn read_artifact(&self, path: &str, _commit: &str) -> Option<String> {
        self.artifacts
            .iter()
            .find(|(p, _)| p == path)
            .map(|(_, c)| c.clone())
    }
}

pub(super) fn deps(provider: EchoProvider) -> Arc<TestDeps> {
    deps_with(Arc::new(provider))
}

/// [`deps`], keeping a handle on the model so a test can count its calls.
pub(super) fn deps_with(provider: Arc<EchoProvider>) -> Arc<TestDeps> {
    Arc::new(TestDeps {
        provider: Some(provider),
        artifacts: vec![(
            "notes/plan.md".to_string(),
            format!("# Plan\n{}", "A line of the plan. ".repeat(40)),
        )],
    })
}

#[test]
fn the_scale_line_is_exactly_the_node_size() {
    assert_eq!(super::prompt::SCALE.len(), super::NODE_BYTES);
}

/// The partial index only serves a query whose literal list matches it.
#[test]
fn the_workspace_leaf_index_lists_every_leaf_event_type() {
    let migration = include_str!("../../../../migrations/20261004132639_create_summary_trees.sql");
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

/// A turn end is a workspace leaf type, so the partial index serves the
/// workspace log's turn query.
#[test]
fn every_turn_end_also_ends_a_workspace_turn() {
    for name in super::log::TURN_END_EVENT_TYPES {
        assert!(super::workspace_log::WORKSPACE_LEAF_EVENT_TYPES.contains(name));
    }
}
