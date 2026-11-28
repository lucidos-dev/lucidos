use crate::core::store::with_event_address;
use crate::engine::chat::process::ChatProcessExt;
use crate::engine::tools::todo::TodoExt;
use crate::engine::tools::ToolDispatchExt;
use crate::engine::{InjectedPrompt, InjectedPromptKind};
use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use crate::llm::{ContentBlock, Message, MessageContent};
use std::collections::HashSet;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::super::chat::process::working_understanding as wu;
use super::super::LucidosEngine;

pub(crate) fn meta_with_cancel_actor(
    engine: &LucidosEngine,
    thread_id: Uuid,
    base: &crate::engine::thread_events::EventMeta,
) -> crate::engine::thread_events::EventMeta {
    let mut out = base.clone();
    if out.actor.is_none() {
        if let Some(actor) = engine.take_cancel_actor(thread_id) {
            out.actor = Some(actor);
        }
    }
    out
}

pub(crate) fn cancel_cause_for_turn(
    engine: &LucidosEngine,
    thread_id: Uuid,
) -> crate::engine::thread_events::CancelCause {
    engine
        .take_cancel_cause(thread_id)
        .unwrap_or(crate::engine::thread_events::CancelCause::UserStop)
}

/// INVARIANT: Race a tool execution future against the per-thread cancel token. On
/// cancel, returns `Err("Error: canceled by user")` so the agent loop's
/// `tool_use → tool_result` pairing invariant survives — every emitted
/// `ToolCalled` gets a matching `ToolResult` (with `success: false`)
/// even when the work is aborted mid-await. The outer loop's pre-iter
/// `is_cancelled()` then emits `ResponseCanceled` on the next iteration.
///
/// `biased` poll order means cancel ALWAYS wins when the token is already
/// cancelled — without it, an instantly-ready tool result could race with
/// a pending cancel and leak one extra iteration past the user's Stop.
///
/// For subprocess-spawning tools (`run_python`, `run_bash`, MCP), this is
/// only half the fix: the inner future being dropped here must also tear
/// down the OS child, which requires `kill_on_drop(true)` on the
/// `tokio::process::Command`. Without that, dropping the future leaks the
/// child process even though the agent loop unblocks.
pub(crate) async fn run_tool_with_cancel<F>(
    fut: F,
    cancel_token: &CancellationToken,
) -> super::super::tools::ToolOutcome
where
    F: std::future::Future<Output = super::super::tools::ToolOutcome>,
{
    tokio::select! {
        biased;
        _ = cancel_token.cancelled() => Err("Error: canceled by user".to_string()),
        r = fut => r,
    }
}

pub(crate) const PARALLEL_SAFE_TOOLS: &[&str] = &[
    tn::READ_FILE,
    tn::LIST_FILES,
    tn::GLOB_FILES,
    tn::GREP_FILES,
    tn::WEB_SEARCH,
    tn::FETCH_NEWS,
    tn::QUERY_EVENTS,
    tn::COUNT_EVENTS,
    tn::LIST_EVENT_TYPES,
];

pub(crate) const MAX_PARALLEL_TOOL_CALLS: usize = 4;

pub(crate) fn is_parallel_safe(name: &str, args: &serde_json::Value) -> bool {
    crate::engine::tool_arguments::dispatch_name(name, args)
        .is_ok_and(|dispatched| PARALLEL_SAFE_TOOLS.contains(&dispatched))
}

pub(crate) fn parallel_run_end(calls: &[crate::llm::provider::ToolCall], start: usize) -> usize {
    start
        + calls[start..]
            .iter()
            .take_while(|call| is_parallel_safe(&call.name, &call.arguments))
            .count()
}

pub(crate) async fn run_concurrently<F: std::future::Future>(calls: Vec<F>) -> Vec<F::Output> {
    use futures::StreamExt;
    let mut finished: Vec<(usize, F::Output)> = futures::stream::iter(
        calls
            .into_iter()
            .enumerate()
            .map(|(index, call)| async move { (index, call.await) }),
    )
    .buffer_unordered(MAX_PARALLEL_TOOL_CALLS)
    .collect()
    .await;
    finished.sort_by_key(|(index, _)| *index);
    finished.into_iter().map(|(_, output)| output).collect()
}

pub(super) struct ParallelRead {
    pub(super) tool_called_event_id: Option<Uuid>,
    pub(super) result: String,
    pub(super) llm_text: String,
    pub(super) is_error: bool,
}

#[easy_ext::ext(HelpersExt)]
pub(super) impl LucidosEngine {
    async fn emit_tool_called(
        &self,
        thread_id: Uuid,
        meta: &crate::engine::thread_events::EventMeta,
        tool_call: &crate::llm::provider::ToolCall,
        step: usize,
        max: usize,
    ) -> Option<Uuid> {
        let mut redacted_args = tool_call.arguments.clone();
        crate::core::redact_postgres_secrets_in_json(&mut redacted_args);
        let description = self.describe_tool(&tool_call.name, &redacted_args);
        log_legacy!("[AgentLoop] Step {}/{}: {}", step, max, description);
        self.event_bus
            .emit_for_id(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::ToolCalled {
                    name: tool_call.name.clone(),
                    description,
                    args: redacted_args,
                },
                meta: meta.clone(),
            })
            .await
    }

    async fn emit_tool_result(
        &self,
        thread_id: Uuid,
        meta: &crate::engine::thread_events::EventMeta,
        name: &str,
        split: &mut ToolResultSplit,
        success: bool,
        tool_called_event_id: Option<Uuid>,
    ) {
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::ToolResult {
                        name: name.to_string(),
                        result: crate::core::sanitize_for_jsonb(split.event_text()),
                        images: std::mem::take(&mut split.images),
                        success,
                        tool_called_event_id,
                    },
                    meta: meta.clone(),
                },
                "[AgenticLoop] ToolResult",
            )
            .await;
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_parallel_reads(
        &self,
        calls: &[crate::llm::provider::ToolCall],
        first_step: usize,
        max_steps: usize,
        thread_id: Uuid,
        meta: &crate::engine::thread_events::EventMeta,
        extraction_ctx: &str,
        request_id: Uuid,
        device_id: Option<&str>,
        cancel_token: &CancellationToken,
    ) -> Vec<ParallelRead> {
        let reads = calls.iter().enumerate().map(|(offset, call)| async move {
            let tool_called_event_id = self
                .emit_tool_called(thread_id, meta, call, first_step + offset, max_steps)
                .await;
            let work = self.execute_tool(
                &call.name,
                &call.arguments,
                extraction_ctx,
                request_id,
                device_id,
                thread_id,
            );
            let (result, is_error) = match run_tool_with_cancel(work, cancel_token).await {
                Ok(text) => (text, false),
                Err(text) => (text, true),
            };
            let mut split = split_tool_result(&result);
            self.emit_tool_result(
                thread_id,
                meta,
                &call.name,
                &mut split,
                !is_error,
                tool_called_event_id,
            )
            .await;
            ParallelRead {
                tool_called_event_id,
                result,
                llm_text: split.llm_text,
                is_error,
            }
        });
        run_concurrently(reads.collect()).await
    }
}

/// INVARIANT: Race a **read-only** future against the per-thread cancel token, yielding
/// `None` when the token wins. The turn's setup phase (history load, query
/// classification, memory retrieval, system-prompt and context assembly) uses
/// this so a user Stop ends the turn from wherever it is, instead of only at
/// the agentic loop's pre-iteration check. On a large thread that setup is tens
/// of seconds, and every one of them was spent with the UI stuck on
/// "Canceling…" (see
/// `docs/plans/2026-08-04-chat-stop-honored-during-turn-setup.md`).
///
/// **Only wrap a pure read.** Losing the race DROPS the inner future, so a call
/// that emits an event, writes the DB, or touches the filesystem must never go
/// through here: it would be torn down half-done with no record. That is the
/// difference from `run_tool_with_cancel` above, which deliberately returns an
/// `Err` outcome rather than nothing precisely so its caller can still emit the
/// paired `ToolResult`.
///
/// `biased` for the same reason `run_tool_with_cancel` needs it: an
/// instantly-ready future must not win against an already-cancelled token, or a
/// Stop leaks one more phase of work.
pub(crate) async fn until_canceled<F>(cancel_token: &CancellationToken, fut: F) -> Option<F::Output>
where
    F: std::future::Future,
{
    tokio::select! {
        biased;
        _ = cancel_token.cancelled() => None,
        v = fut => Some(v),
    }
}

#[derive(Debug)]
pub(crate) enum InjectedPromptGroup {
    UserText(Vec<InjectedPrompt>),
    Standalone(Box<InjectedPrompt>),
}

pub(crate) fn group_injected_prompts(prompts: Vec<InjectedPrompt>) -> Vec<InjectedPromptGroup> {
    let mut groups = Vec::new();
    let mut user_batch = Vec::new();

    for prompt in prompts {
        if prompt.kind.is_engine_reentry() {
            if !user_batch.is_empty() {
                groups.push(InjectedPromptGroup::UserText(std::mem::take(
                    &mut user_batch,
                )));
            }
            groups.push(InjectedPromptGroup::Standalone(Box::new(prompt)));
        } else {
            user_batch.push(prompt);
        }
    }

    if !user_batch.is_empty() {
        groups.push(InjectedPromptGroup::UserText(user_batch));
    }

    groups
}

pub(crate) fn injections_reopen_a_finished_answer(prompts: &[InjectedPrompt]) -> bool {
    prompts.iter().any(|p| p.kind.reopens_a_finished_answer())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InjectionDelivery {
    MidTurn,
    NewTurn,
}

pub(crate) fn framed_injected_prompt(
    prompt: &InjectedPrompt,
    delivery: InjectionDelivery,
) -> String {
    use super::super::thread_events::ActorMode;
    match (prompt.mode, delivery) {
        (ActorMode::Human, InjectionDelivery::MidTurn) => format!(
            "[USER INTERJECTION — the user sent this while you were working. \
             Answer it, then carry on with the work you had in progress, in this \
             same turn; answering is not a reason to end your turn. If it \
             redirects you, drop the old plan and follow the new direction \
             instead.]\n\n{}",
            prompt.text
        ),
        (ActorMode::Human, InjectionDelivery::NewTurn) => format!(
            "[USER MESSAGE — sent while the previous turn was still finishing, \
             so it is being handled now as its own turn.]\n\n{}",
            prompt.text
        ),
        (ActorMode::Agent | ActorMode::Engine, InjectionDelivery::MidTurn) => format!(
            "[SYSTEM UPDATE — new information arrived while you were working. \
             Incorporate this into your current response.]\n\n{}",
            prompt.text
        ),
        (ActorMode::Agent | ActorMode::Engine, InjectionDelivery::NewTurn) => format!(
            "[SYSTEM UPDATE — this arrived while the previous turn was still \
             finishing, so it is being handled now as its own turn.]\n\n{}",
            prompt.text
        ),
    }
}

pub(crate) fn prompts_have_images(prompts: &[InjectedPrompt]) -> bool {
    prompts
        .iter()
        .any(|prompt| prompt.images.as_ref().is_some_and(|imgs| !imgs.is_empty()))
}

pub(crate) fn coalesced_user_text_message(
    workspace: &std::path::Path,
    prompts: &[InjectedPrompt],
) -> Message {
    let has_images = prompts_have_images(prompts);

    let content = if prompts.len() == 1 && !has_images {
        MessageContent::Text(framed_injected_prompt(
            &prompts[0],
            InjectionDelivery::MidTurn,
        ))
    } else {
        let mut blocks = Vec::new();
        for prompt in prompts {
            let mut text = framed_injected_prompt(prompt, InjectionDelivery::MidTurn);
            if let Some(imgs) = prompt.images.as_deref().filter(|imgs| !imgs.is_empty()) {
                let noun = if imgs.len() == 1 { "image" } else { "images" };
                let handles: Vec<String> =
                    crate::engine::chat::current_image_handles(workspace, imgs)
                        .into_iter()
                        .flatten()
                        .collect();
                let note = crate::engine::chat::handles_note(&handles);
                text.push_str(&format!("\n\n[{noun} attached to this message{note}]"));
            }
            blocks.push(ContentBlock::Text { text });
            if let Some(imgs) = &prompt.images {
                for img in imgs {
                    let fitted = img.clone().fit_for_llm();
                    blocks.push(ContentBlock::Image {
                        source_type: "base64".to_string(),
                        media_type: fitted.mime_type,
                        data: fitted.base64,
                    });
                }
            }
        }
        MessageContent::Blocks(blocks)
    };

    Message {
        role: "user".to_string(),
        content,
    }
}

pub(crate) fn coalesced_user_text_for_reprocess(prompts: &[InjectedPrompt]) -> String {
    prompts
        .iter()
        .map(|prompt| framed_injected_prompt(prompt, InjectionDelivery::NewTurn))
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}

pub(crate) fn coalesced_images_for_reprocess(
    prompts: &[InjectedPrompt],
) -> Option<Vec<crate::engine::types::ChatImage>> {
    let images: Vec<_> = prompts
        .iter()
        .filter_map(|prompt| prompt.images.as_ref())
        .flat_map(|imgs| imgs.iter().cloned())
        .collect();
    (!images.is_empty()).then_some(images)
}

pub(crate) async fn filter_removed_queued_prompts(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    prompts: Vec<InjectedPrompt>,
) -> Vec<InjectedPrompt> {
    let ids: Vec<String> = prompts
        .iter()
        .filter(|prompt| matches!(prompt.kind, InjectedPromptKind::UserText))
        .filter_map(|prompt| prompt.event_id.map(|id| id.to_string()))
        .collect();
    if ids.is_empty() {
        return prompts;
    }

    let removed = match sqlx::query_scalar::<_, String>(
        "SELECT payload->>'removed_message_id'
           FROM events
          WHERE aggregate = 'thread'
            AND aggregate_id = $1
            AND event_type = 'QueuedMessageRemoved'
            AND payload->>'removed_message_id' = ANY($2::text[])",
    )
    .bind(thread_id.to_string())
    .bind(&ids)
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows.into_iter().collect::<HashSet<_>>(),
        Err(e) => {
            crate::log_legacy!(
                "[Inject] queued-message removal lookup failed for thread {}: {}",
                thread_id,
                e
            );
            HashSet::new()
        }
    };
    if removed.is_empty() {
        return prompts;
    }

    let before = prompts.len();
    let filtered: Vec<_> = prompts
        .into_iter()
        .filter(|prompt| {
            if !matches!(prompt.kind, InjectedPromptKind::UserText) {
                return true;
            }
            prompt
                .event_id
                .map(|id| !removed.contains(&id.to_string()))
                .unwrap_or(true)
        })
        .collect();
    let skipped = before.saturating_sub(filtered.len());
    if skipped > 0 {
        crate::log_legacy!(
            "[Inject] Skipped {} removed queued prompt(s) for thread {}",
            skipped,
            thread_id
        );
    }
    filtered
}

#[derive(Debug, Default)]
pub(crate) struct AppendedInjections {
    pub appended: bool,
    pub image_message_idxs: Vec<usize>,
}

pub(crate) async fn append_injected_prompts_to_messages(
    workspace: &std::path::Path,
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    meta: &crate::engine::thread_events::EventMeta,
    messages: &mut Vec<Message>,
    prompts: Vec<InjectedPrompt>,
) -> AppendedInjections {
    let mut result = AppendedInjections::default();
    for group in group_injected_prompts(prompts) {
        match group {
            InjectedPromptGroup::Standalone(prompt) => {
                if prompt.text.trim().is_empty() {
                    crate::log_legacy!(
                        "[Inject] Dropped an empty engine re-entry {:?} on thread {}",
                        prompt.kind,
                        thread_id
                    );
                    continue;
                }
                crate::log_legacy!(
                    "[Inject] Engine re-entry {:?} (spawning_event {:?}) into active thread {}",
                    prompt.kind,
                    prompt.spawning_event_id,
                    thread_id
                );
                messages.push(Message {
                    role: "user".to_string(),
                    content: MessageContent::Text(prompt.text),
                });
                result.appended = true;
            }
            InjectedPromptGroup::UserText(queued) => {
                let mut batch = Vec::with_capacity(queued.len());
                for prompt in queued {
                    crate::log_legacy!(
                        "[Inject] Mid-flight {:?} prompt injected into thread {}: {}",
                        prompt.mode,
                        thread_id,
                        &prompt.text[..prompt.text.floor_char_boundary(80)]
                    );
                    if emit_prompt_injected_event(bus, thread_id, meta, &prompt).await {
                        batch.push(prompt);
                    }
                }
                if batch.is_empty() {
                    continue;
                }
                let has_images = prompts_have_images(&batch);
                messages.push(coalesced_user_text_message(workspace, &batch));
                result.appended = true;
                if has_images {
                    result.image_message_idxs.push(messages.len() - 1);
                }
            }
        }
    }
    result
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinishClass {
    Clean,
    Truncated,
    Blocked,
    Unknown,
}

pub(crate) fn normalize_finish_reason(stop_reason: &str) -> FinishClass {
    match stop_reason.to_ascii_lowercase().as_str() {
        "end_turn" | "stop" | "completed" | "end_of_turn" | "eos" => FinishClass::Clean,
        "max_tokens" | "max_output_tokens" | "length" | "model_length" => FinishClass::Truncated,
        "refusal" | "safety" | "recitation" | "content_filter" | "blocklist"
        | "prohibited_content" | "spii" | "image_safety" => FinishClass::Blocked,
        _ => FinishClass::Unknown,
    }
}

pub(crate) fn declined_partway_error(stop_reason: Option<&str>) -> Option<&'static str> {
    (stop_reason.map(normalize_finish_reason) == Some(FinishClass::Blocked)).then_some(
        "The model stopped partway: the provider's safety classifier withheld the rest of the \
         response. The text above is incomplete. Rephrase the request or start a new thread; \
         retrying the same prompt will be refused again.",
    )
}

pub(crate) struct EmptyCompletionClass {
    pub is_error: bool,
    pub hint: &'static str,
}

/// INVARIANT: Decide whether an empty completion (no text, no tool calls) is a genuine
/// failure or benign intentional silence — uniformly across providers and
/// thread types. The classification keys off the *cause* of the emptiness, not
/// what launched the thread:
///
/// - **Truncated / Blocked** (normalized finish reason): content was lost or
///   withheld → error.
/// - **Dropped output**: `output_tokens > 16` while the model did not think
///   (`model_thought` false), or `unknown_sse_dropped > 0`. The provider billed
///   real output but nothing reached the engine: a known block carried an
///   unrecognised shape, or an unknown SSE block slipped past the parser.
///   `model_thought` must count thinking BLOCKS, not only thinking text:
///   Claude returns empty thinking text unless asked to show it. → error.
/// - **Unknown** finish reason with nothing salvageable (`null`,
///   `stop_sequence`, a future reason): fail-safe → error.
/// - **Clean** stop with nothing dropped: the model ended its turn and chose to
///   emit no text → benign.
///
/// Threshold `16` is well above any realistic structural-overhead floor for
/// `usage.output_tokens` on a no-op clean stop — keeps true silence out of the
/// parser-miss branch.
pub(crate) fn classify_empty_completion(
    stop_reason: &str,
    output_tokens: u32,
    model_thought: bool,
    unknown_sse_dropped: u32,
) -> EmptyCompletionClass {
    let error = |hint: &'static str| EmptyCompletionClass {
        is_error: true,
        hint,
    };
    let class = normalize_finish_reason(stop_reason);
    match class {
        FinishClass::Truncated => return error(" — output truncated by token budget"),
        FinishClass::Blocked => {
            return error(" — the model declined to respond (provider safety classifier withheld the output). Rephrase the request or start a new thread; retrying the same prompt will be refused again")
        }
        FinishClass::Clean | FinishClass::Unknown => {}
    }
    // INVARIANT: Dropped output: tokens billed but nothing surfaced. Either a known block
    // carried an unexpected payload shape (couldn't classify) or an unknown
    // SSE block was dropped (`unknown_sse_dropped`, incremented in
    // `vertex::process_sse_data`). Surfacing this prevents the misleading
    // "model decided no action was needed" that masked real Vertex SSE drift.
    if output_tokens > 16 && !model_thought {
        if unknown_sse_dropped > 0 {
            return error(" — engine dropped unknown SSE shapes; provider stream may have changed (see [Vertex] WARNING logs for the exact types)");
        }
        return error(" — model generated output the engine couldn't classify (likely a stream-shape change in the provider)");
    }
    if unknown_sse_dropped > 0 {
        return error(" — engine dropped unknown SSE shapes; provider stream may have changed (see [Vertex] WARNING logs for the exact types)");
    }
    if class == FinishClass::Unknown {
        return error("");
    }
    let hint = if model_thought {
        " — model thought but produced no text or tool call"
    } else {
        " — model ended its turn without producing text"
    };
    EmptyCompletionClass {
        is_error: false,
        hint,
    }
}

pub(crate) fn effective_flush_text<'a>(
    cleaned: Option<&'a str>,
    raw: &'a str,
    content: Option<&'a str>,
) -> &'a str {
    if let Some(c) = cleaned {
        return c;
    }
    if !raw.is_empty() {
        return raw;
    }
    content.unwrap_or("")
}

pub(crate) fn derive_call_key(tool_name: &str, args: &serde_json::Value) -> String {
    if tool_name == tn::RUN_BASH {
        if let Some(cmd) = args.get("command").and_then(|v| v.as_str()) {
            if let Some(first_token) = cmd.split_whitespace().next() {
                return first_token.to_string();
            }
        }
        return tn::RUN_BASH.to_string();
    }

    if tool_name == tn::RUN_PYTHON || tool_name == tn::RUN_PYTHON_BACKGROUND {
        if let Some(code) = args.get("code").and_then(|v| v.as_str()) {
            if let Some(key) = python_call_key(code) {
                return key;
            }
        }
        return tool_name.to_string();
    }

    let base = args
        .get("path")
        .or_else(|| args.get("url"))
        .or_else(|| args.get("query"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    if tool_name == tn::READ_FILE {
        let start = args.get("start_line").and_then(|v| v.as_u64());
        let count = args.get("line_count").and_then(|v| v.as_u64());
        let offset = args.get("offset").and_then(|v| v.as_u64());
        if start.is_some() || count.is_some() || offset.is_some() {
            let fmt_opt = |n: Option<u64>| n.map(|v| v.to_string()).unwrap_or_default();
            return format!(
                "{}|s={}|c={}|o={}",
                base,
                fmt_opt(start),
                fmt_opt(count),
                fmt_opt(offset),
            );
        }
    }

    base.to_string()
}

pub(crate) fn python_call_key(code: &str) -> Option<String> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    const MAX_KEY_CHARS: usize = 80;
    for raw in code.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with("import ") || line.starts_with("from ") {
            continue;
        }
        let take = line
            .char_indices()
            .nth(MAX_KEY_CHARS)
            .map(|(i, _)| i)
            .unwrap_or(line.len());
        if take == line.len() {
            return Some(line.to_string());
        }
        let mut hasher = DefaultHasher::new();
        line.hash(&mut hasher);
        let h = hasher.finish();
        return Some(format!(
            "{}#{:08x}",
            &line[..take],
            (h & 0xFFFF_FFFF) as u32
        ));
    }
    None
}

pub(crate) struct SentinelMatch {
    pub label: &'static str,
    pub event: super::super::thread_events::ThreadEvent,
    pub redacted_text: Option<String>,
}

fn preview_id(payload: &str, key: &str) -> Option<uuid::Uuid> {
    serde_json::from_str::<serde_json::Value>(payload).ok()?[key]
        .as_str()?
        .parse()
        .ok()
}

pub(crate) fn match_sentinel(
    tool_name: &str,
    args: &serde_json::Value,
    text: &str,
) -> Option<SentinelMatch> {
    use super::super::thread_events::ThreadEvent;
    use super::super::tools::credentials::CREDENTIAL_REQUEST_PREFIX;
    use super::super::tools::plugins::{
        PLUGIN_INSTALL_REQUEST_PREFIX, PLUGIN_UNINSTALL_REQUEST_PREFIX,
    };
    use crate::llm::tool_names as tn;

    let tool = crate::engine::tool_arguments::dispatch_name(tool_name, args).ok()?;

    type SentinelEntry = (
        &'static str,
        &'static [&'static str],
        &'static str,
        fn(String) -> Option<ThreadEvent>,
        Option<&'static str>,
    );
    let entries: &[SentinelEntry] = &[
        (
            CREDENTIAL_REQUEST_PREFIX,
            &[
                tn::REQUEST_CREDENTIAL,
                tn::CONNECT_OAUTH_ACCOUNT,
                tn::CONFIGURE_EMAIL,
            ],
            "[AgenticLoop] CredentialRequested",
            |payload| {
                Some(ThreadEvent::CredentialRequested {
                    request_id: uuid::Uuid::new_v4(),
                    payload,
                })
            },
            Some("Credential form sent to the user. It stays open in this thread until they save or cancel it. Do not chat-ask for the same value: the form resolves the request."),
        ),
        (
            PLUGIN_INSTALL_REQUEST_PREFIX,
            &[tn::INSTALL_PLUGIN, tn::UPDATE_PLUGIN],
            "[AgenticLoop] PluginInstallRequested",
            |payload| {
                Some(ThreadEvent::PluginInstallRequested {
                    request_id: preview_id(&payload, "install_id")?,
                    payload,
                })
            },
            Some("Install panel sent to the user. It stays open in this thread until they click Install (or Update) or Cancel. Do not chat-ask about overwrites and do not claim the install succeeded: the panel resolves it and the next user message will tell you the outcome."),
        ),
        (
            PLUGIN_UNINSTALL_REQUEST_PREFIX,
            &[tn::UNINSTALL_PLUGIN],
            "[AgenticLoop] PluginUninstallRequested",
            |payload| {
                Some(ThreadEvent::PluginUninstallRequested {
                    request_id: preview_id(&payload, "uninstall_id")?,
                    payload,
                })
            },
            Some("Uninstall panel sent to the user. It stays open in this thread until they click Uninstall or Cancel. Do not chat-ask which files to delete and do not claim files were removed: the panel resolves it and the next user message will tell you the outcome."),
        ),
        (
            "[EMAIL_CONFIRM]",
            &[tn::SEND_EMAIL],
            "[AgenticLoop] EmailConfirmRequested",
            |payload| {
                Some(ThreadEvent::EmailConfirmRequested {
                    request_id: uuid::Uuid::new_v4(),
                    payload,
                })
            },
            None,
        ),
    ];

    for &(prefix, producers, label, ctor, redacted) in entries {
        if !producers.contains(&tool) || !text.starts_with(prefix) {
            continue;
        }
        let after = &text[prefix.len()..];
        let rel_start = after.find('{')?;
        let payload = after[rel_start..].to_string();
        let event = ctor(payload.clone())?;
        let mut redacted_text = redacted.map(str::to_string);
        if prefix == CREDENTIAL_REQUEST_PREFIX {
            let parsed = serde_json::from_str::<serde_json::Value>(&payload).ok();
            if let Some(service) = parsed
                .as_ref()
                .and_then(|v| v["service"].as_str())
                .filter(|s| !s.is_empty())
            {
                let widening = parsed
                    .as_ref()
                    .and_then(|v| v["adding_base_urls"].as_array())
                    .is_some_and(|hosts| !hosts.is_empty());
                if let Some(t) = redacted_text.as_mut() {
                    t.push_str(&if widening {
                        format!(
                            " It widens the existing \"{service}\" credential to reach another \
                             host. The stored secret is unchanged, so the user only presses Save."
                        )
                    } else {
                        format!(" It saves the credential as \"{service}\".")
                    });
                }
            }
            if let (ThreadEvent::CredentialRequested { request_id, .. }, Some(t)) =
                (&event, redacted_text.as_mut())
            {
                t.push_str(&format!(
                    " Nothing re-opens this thread when they save it. So before ending your turn, \
                     call `{}` on `FormRequestResolved` with the condition `request_id` = \
                     \"{request_id}\". Its `outcome` is `completed` (saved), `canceled`, \
                     `superseded` (a user message or a newer form replaced it, so say nothing more) \
                     or `expired`. Never ask the user to type or tap that they saved it.",
                    tn::AWAIT_EVENT
                ));
            }
        }
        return Some(SentinelMatch {
            label,
            event,
            redacted_text,
        });
    }
    None
}

pub(crate) fn should_flush(text: &str) -> bool {
    if text.ends_with("\n\n") {
        return true;
    }
    if text.ends_with("```\n") {
        return true;
    }
    if text.ends_with('\n') {
        if let Some(last_line) = text.lines().last() {
            if last_line.starts_with('#') {
                return true;
            }
        }
    }
    if text.ends_with("\n---\n") || text.ends_with("\n***\n") {
        return true;
    }
    false
}

pub(crate) fn parse_app_capture_marker(s: &str) -> Option<(&str, &str)> {
    let rest = s.strip_prefix("[APP_CAPTURE:")?;
    let end_bracket = rest.find("]\n")?;
    Some((&rest[..end_bracket], &rest[end_bracket + 2..]))
}

pub(crate) fn format_capture_result(screenshot_b64: &str, lead: &str, dom: &str) -> String {
    if screenshot_b64.is_empty() {
        dom.to_string()
    } else if lead.is_empty() {
        format!("[APP_CAPTURE:{}]\nDOM snapshot:\n{}", screenshot_b64, dom)
    } else {
        format!(
            "[APP_CAPTURE:{}]\n{}\nDOM snapshot:\n{}",
            screenshot_b64, lead, dom
        )
    }
}

pub(crate) fn strip_app_capture_marker(s: &str) -> Option<String> {
    let (screenshot_b64, dom) = parse_app_capture_marker(s)?;
    let approx_bytes = (screenshot_b64.len() * 3) / 4;
    Some(format!(
        "[screenshot {}, {} omitted, not embedded in event]\n{}",
        sniff_image_media_type(screenshot_b64),
        crate::core::format_byte_size(approx_bytes),
        dom,
    ))
}

pub(crate) fn sniff_image_media_type(b64: &str) -> &'static str {
    use base64::Engine;
    let prefix_len = 16.min(b64.len() & !3);
    base64::engine::general_purpose::STANDARD
        .decode(&b64.as_bytes()[..prefix_len])
        .ok()
        .as_deref()
        .and_then(crate::core::blobs::sniff_image_mime)
        .unwrap_or("image/png")
}

pub(crate) struct ToolResultSplit {
    pub llm_text: String,
    event_stub: Option<String>,
    pub images: Vec<String>,
}

impl ToolResultSplit {
    fn shared(text: String) -> Self {
        Self {
            llm_text: text,
            event_stub: None,
            images: Vec::new(),
        }
    }

    pub(crate) fn event_text(&self) -> &str {
        self.event_stub.as_deref().unwrap_or(&self.llm_text)
    }

    pub(crate) fn redact(&mut self, redacted: String) {
        self.llm_text = redacted;
        self.event_stub = None;
    }
}

pub(crate) fn split_tool_result(result: &str) -> ToolResultSplit {
    if let Some(rest) = result.strip_prefix("[GENERATED_IMAGE:") {
        let Some(end_bracket) = rest.find("]\n") else {
            return ToolResultSplit::shared(result.to_string());
        };
        return ToolResultSplit {
            llm_text: rest[end_bracket + 2..].to_string(),
            event_stub: None,
            images: vec![rest[..end_bracket].to_string()],
        };
    }
    if let Some(stub) = crate::engine::tools::files::strip_image_content_marker(result) {
        return ToolResultSplit {
            event_stub: Some(stub),
            ..ToolResultSplit::shared(result.to_string())
        };
    }
    if let Some(stub) = strip_app_capture_marker(result) {
        return ToolResultSplit {
            event_stub: Some(stub),
            ..ToolResultSplit::shared(result.to_string())
        };
    }
    ToolResultSplit::shared(result.to_string())
}

pub(crate) fn take_answer_images(tool_name: &str, result: String) -> (String, Vec<String>) {
    if tool_name != crate::llm::tool_names::ASK_USER_QUESTION {
        return (result, Vec::new());
    }
    match crate::engine::agent_question::parse_answer_images(&result) {
        Some((hashes, text)) => (text.to_string(), hashes),
        None => (result, Vec::new()),
    }
}

pub(crate) fn load_attached_images(
    workspace: &std::path::Path,
    hashes: &[String],
) -> Vec<crate::engine::types::ChatImage> {
    hashes
        .iter()
        .filter_map(|hash| {
            let loaded = crate::core::blobs::read_blob_as_base64(workspace, hash);
            if loaded.is_none() {
                log_legacy!("[AgentLoop] Attached image {hash} is no longer in the blob store");
            }
            loaded
        })
        .map(|(base64, mime_type)| {
            crate::engine::types::ChatImage { base64, mime_type }.fit_for_llm()
        })
        .collect()
}

pub(crate) struct ToolOutput {
    pub tool_use_id: String,
    pub text: String,
    pub event_id: Option<uuid::Uuid>,
    pub images: Vec<crate::engine::types::ChatImage>,
}

pub(crate) fn build_tool_result_blocks(
    tool_outputs: &[ToolOutput],
    instruction: &str,
) -> Vec<ContentBlock> {
    let mut result_blocks: Vec<ContentBlock> = Vec::new();
    let mut trailing_blocks: Vec<ContentBlock> = Vec::new();
    for ToolOutput {
        tool_use_id,
        text: result,
        event_id,
        images,
    } in tool_outputs
    {
        trailing_blocks.extend(images.iter().map(|image| ContentBlock::Image {
            source_type: "base64".to_string(),
            media_type: image.mime_type.clone(),
            data: image.base64.clone(),
        }));
        if let Some((screenshot_b64, dom_text)) = parse_app_capture_marker(result) {
            result_blocks.push(ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: with_event_address(dom_text.to_string(), event_id.as_ref()),
            });
            let fitted = crate::engine::types::ChatImage {
                base64: screenshot_b64.to_string(),
                mime_type: sniff_image_media_type(screenshot_b64).to_string(),
            }
            .fit_for_llm();
            trailing_blocks.push(ContentBlock::Image {
                source_type: "base64".to_string(),
                media_type: fitted.mime_type,
                data: fitted.base64,
            });
            continue;
        }
        if let Some((media_type, image_b64)) =
            crate::engine::tools::files::parse_image_content_marker(result)
        {
            result_blocks.push(ContentBlock::ToolResult {
                tool_use_id: tool_use_id.clone(),
                content: with_event_address(
                    crate::engine::tools::files::EXPLICIT_IMAGE_RESULT_TEXT.to_string(),
                    event_id.as_ref(),
                ),
            });
            trailing_blocks.push(ContentBlock::Image {
                source_type: "base64".to_string(),
                media_type: media_type.to_string(),
                data: image_b64.to_string(),
            });
            continue;
        }
        result_blocks.push(ContentBlock::ToolResult {
            tool_use_id: tool_use_id.clone(),
            content: with_event_address(result.clone(), event_id.as_ref()),
        });
    }
    result_blocks.append(&mut trailing_blocks);
    result_blocks.push(ContentBlock::Text {
        text: instruction.to_string(),
    });
    result_blocks
}

pub(crate) fn is_bad_image_description(desc: &str) -> bool {
    let lower = desc.to_lowercase();
    lower.contains("i do not see any image")
        || lower.contains("i don't see any image")
        || lower.contains("no image attached")
        || lower.contains("no images attached")
        || lower.contains("please provide the image")
        || lower.contains("i cannot see")
        || lower.contains("no image was provided")
        || lower.contains("no image provided")
}

pub(crate) const MAX_QUESTION_REASK: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuestionReaskCause {
    CallRejected,
    LeakedAsText,
    AskedInProse,
}

impl QuestionReaskCause {
    pub(crate) fn instruction(self) -> &'static str {
        match self {
            Self::CallRejected => {
                "Your previous `ask_user_question` call was rejected because a question object \
                 had no `question` text. Do NOT \
                 answer in prose or inline the options as a typed-reply menu: the user needs \
                 the clickable question card. Re-call `ask_user_question` now with the full \
                 question text filled in on every question object."
            }
            Self::LeakedAsText => {
                "You typed an `<ask_user_question>` tag as ordinary text, so no tool call was \
                 made and the user got no clickable card. The tag is not parsed out of your \
                 response. Do NOT type it again, and do NOT inline the options as a typed-reply \
                 menu. INVOKE `ask_user_question` AS A TOOL CALL now, with the full question \
                 text and 2-4 options."
            }
            Self::AskedInProse => {
                "You ended your turn with a question typed as prose, so the user got no \
                 clickable card. Prose does not park the thread: it reads as finished, nothing \
                 lights the Blocked badge, and nobody is told you are waiting. Re-issue \
                 the question now as an `ask_user_question` TOOL CALL with 2-4 options, and do \
                 NOT inline them as a typed-reply menu. Reserve plaintext for a genuinely \
                 open-ended question (e.g. \"what should I name this?\") where pre-baked options \
                 would be guesses. If yours is one, say in one line why options would be \
                 guesses, and call no tool. The user never sees that line: your draft above \
                 already reached them and stays as your answer. You are not asked a second time."
            }
        }
    }

    pub(crate) fn retry_reason(self) -> &'static str {
        match self {
            Self::CallRejected => "ask_user_question had no question text, forcing re-ask",
            Self::LeakedAsText => "ask_user_question was typed as text, forcing re-ask",
            Self::AskedInProse => "the reply ended on a question with no card, forcing re-ask",
        }
    }
}

pub(crate) const MAX_PROSE_QUESTION_NUDGE: usize = 1;

pub(crate) fn reply_ends_in_a_question(text: &str) -> bool {
    text.trim_end().ends_with('?')
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct QuestionReaskInputs {
    pub(crate) ask_failed_last_iter: bool,
    pub(crate) leaked_as_text: bool,
    pub(crate) ends_in_a_question: bool,
    pub(crate) human_can_answer: bool,
    pub(crate) reask_forced: usize,
    pub(crate) prose_nudges_forced: usize,
}

pub(crate) fn question_reask_cause(inputs: QuestionReaskInputs) -> Option<QuestionReaskCause> {
    if inputs.reask_forced < MAX_QUESTION_REASK {
        if inputs.ask_failed_last_iter {
            return Some(QuestionReaskCause::CallRejected);
        }
        if inputs.leaked_as_text {
            return Some(QuestionReaskCause::LeakedAsText);
        }
    }
    asked_in_prose(inputs).then_some(QuestionReaskCause::AskedInProse)
}

fn asked_in_prose(inputs: QuestionReaskInputs) -> bool {
    inputs.ends_in_a_question
        && inputs.human_can_answer
        && inputs.reask_forced == 0
        && inputs.prose_nudges_forced < MAX_PROSE_QUESTION_NUDGE
}

pub(crate) const TOOL_RESULTS_INSTRUCTION: &str = "Results above. The user never sees tool \
     results, only what you write, so what they need from these must reach them in your words \
     before you ask them anything or finish. Do not repeat what you already told them. Proceed \
     to your next action or final answer.";

pub(crate) fn instruction_after_round(
    instruction: &str,
    progress_notes: &[String],
    raised_a_card: bool,
) -> String {
    if progress_notes.is_empty() || raised_a_card {
        return instruction.to_string();
    }
    let seen = progress_notes
        .iter()
        .map(|note| format!("\"{note}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "Part of what you wrote before these tool calls reached the user only as this summary: \
         {seen}. The full text behind it never reached them. If it held anything they need, \
         such as steps, a draft, a link or a picture, write it out in full in your reply, or in \
         a card's `message`.\n\n{instruction}"
    )
}

pub(crate) const MAX_TODO_WAKE_NUDGE: usize = 1;

/// INVARIANT: The forcing instruction for a turn that would end with open todo work and no
/// wake. `open_items` is stated because it is the fact the model is about to
/// contradict, and a count is cheaper to act on than "some".
///
/// The three options are deliberately equal in weight. Arming, settling and
/// handing back are each correct in their own case. A nudge leaning on one
/// would teach the model to reach for it reflexively. Leaning on `todo_write`
/// in particular would buy silence by clearing the evidence.
///
/// It supplies no sentence for the model to repeat at the user. That is the
/// mistake `APPLY_VERIFY_DEV_ADDENDUM` made, whose ready-made negative sentence
/// got quoted back as a finding with the check behind it never run. See
/// `docs/plans/2026-08-10-the-agent-checks-state-instead-of-assuming-it.md`.
/// `curated` is true under *self-curated context mode*, which withdraws
/// `todo_write` whole. Naming a tool the model was never shown offers it an
/// action it cannot take, so option two names the heading it writes instead.
pub(crate) fn todo_wake_nudge_instruction(open_items: usize, curated: bool) -> String {
    let update_the_list = if curated {
        "rewrite the `[TODO]` heading of your working understanding, marking what is done as \
         `- [x]` and dropping what you are no longer doing"
    } else {
        "call `todo_write` to mark what is done as completed, and to drop what you are no longer \
         doing"
    };
    format!(
        "STOP, do not finish this turn yet. Your todo list still has {open_items} unfinished \
         item(s), and NOTHING will re-open this thread: you hold no event-wait subscription, no \
         background task is running, and no child thread still owes you a report. No wake is \
         scheduled, whatever your answer says. Do one \
         of these three now, then finish: (1) call `await_event` if that work is waiting on \
         something happening in Lucidos, so the thread re-opens when it does; (2) \
         {update_the_list}; (3) hand back to the user, deciding for yourself how to say that you \
         are not watching for anything. Whichever you choose, do not tell the user you are \
         watching, monitoring, or will report back, unless you armed a subscription in this turn."
    )
}

pub(crate) fn should_nudge_unwatched_turn(
    open_items: Option<usize>,
    covered: bool,
    nudges_forced: usize,
) -> bool {
    matches!(open_items, Some(n) if n > 0) && !covered && nudges_forced < MAX_TODO_WAKE_NUDGE
}

pub(crate) struct FinalAnswerBesideDecision<'a> {
    pub(crate) response: &'a crate::llm::provider::LlmResponse,
    pub(crate) had_errors: bool,
    pub(crate) thread_id: Uuid,
    pub(crate) injections_pending: bool,
    pub(crate) cancelled: bool,
    pub(crate) question: QuestionReaskInputs,
    pub(crate) wake_nudges_forced: usize,
}

pub(crate) fn calls_only_turn_end_decisions(calls: &[crate::llm::provider::ToolCall]) -> bool {
    !calls.is_empty()
        && calls.iter().all(|c| {
            [tn::REQUEST_READ, tn::WAKE_PARENT, tn::EXPECT_WAKE].contains(&c.name.as_str())
        })
}

#[easy_ext::ext(FinalAnswerDecisionExt)]
pub(crate) impl LucidosEngine {
    async fn final_answer_beside_decision(
        &self,
        input: FinalAnswerBesideDecision<'_>,
    ) -> Option<String> {
        if input.had_errors
            || input.injections_pending
            || input.cancelled
            || !input.response.progress_notes.is_empty()
            || !calls_only_turn_end_decisions(&input.response.tool_calls)
        {
            return None;
        }
        let answer = input
            .response
            .content
            .as_deref()
            .map(|c| self.clean_response(c))
            .filter(|c| !c.is_empty())?;
        let question = QuestionReaskInputs {
            ends_in_a_question: reply_ends_in_a_question(&answer),
            ..input.question
        };
        if question_reask_cause(question).is_some() {
            return None;
        }
        let (open, covered) = self.wake_check_facts(input.thread_id).await;
        if should_nudge_unwatched_turn(open, covered, input.wake_nudges_forced) {
            return None;
        }
        Some(answer)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BreakerAction {
    None,
    Warn,
    Break,
}

pub(crate) fn next_failure_streak(
    is_repeat: bool,
    prev_call_failed: bool,
    prev_streak: usize,
) -> usize {
    if is_repeat && prev_call_failed {
        prev_streak + 1
    } else {
        1
    }
}

pub(crate) fn generic_breaker_action(failure_streak: usize) -> BreakerAction {
    if failure_streak >= 5 {
        BreakerAction::Break
    } else if failure_streak >= 3 {
        BreakerAction::Warn
    } else {
        BreakerAction::None
    }
}

pub(crate) fn terminal_result(
    response: String,
    images: Vec<String>,
    request_id: Uuid,
    thread_id: Uuid,
    proposed_change: bool,
) -> super::super::types::ProcessResult {
    super::super::types::ProcessResult {
        response,
        steps: vec![],
        images,
        request_id,
        thread_id,
        proposed_change,
        auto_apply: false,
        orphaned_injections: vec![],
    }
}

pub(crate) const NON_TOOL_ROUND_SLACK: usize = 100;

pub(crate) const NOTED_CARRY_ON: &str =
    "Your working understanding is updated. Carry on with the work.";

/// INVARIANT: Whether a reply that made no tool call was bookkeeping rather than an
/// answer.
///
/// The model writes its document beside its next action, but the design record
/// measured 82% of notes written alone in a round. A lone write would otherwise
/// end the turn with an empty answer, abandoning the task.
///
/// So the span is spliced out first, and what is left decides. Nothing left is
/// bookkeeping and the turn continues. Something left is the answer.
pub(crate) fn reply_was_bookkeeping_alone(wrote_document: bool, visible: Option<&str>) -> bool {
    wrote_document && visible.is_none_or(str::is_empty)
}

pub(crate) fn tool_call_cap_message(max_tool_calls: usize) -> String {
    format!(
        "[ENGINE-LIMIT] Per-turn limit of {} tool calls reached. Send any message to continue \
         from here, or raise the limit in [Settings](settings) under Models, Chat & triggers, \
         Max tool calls.",
        max_tool_calls
    )
}

pub(crate) fn round_backstop_message(rounds: usize) -> String {
    format!(
        "[ENGINE-LIMIT] Turn ended after {} steps without reaching the tool-call limit, which \
         means it was looping without getting anywhere. Send any message to continue from here.",
        rounds
    )
}

pub(crate) async fn emit_iteration_cap_response_generated(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    meta: &crate::engine::thread_events::EventMeta,
    images: Vec<String>,
    effective_model: Option<String>,
    effective_effort: Option<String>,
    msg: String,
) -> String {
    bus.emit_or_log(
        crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: crate::engine::thread_events::ThreadEvent::ResponseGenerated {
                text: msg.clone(),
                images,
                model: effective_model,
                reasoning_effort: effective_effort,
            },
            meta: meta.authored_by(crate::engine::thread_events::AgentParticipant::LucidosAgent),
        },
        "[AgenticLoop] ResponseGenerated (iteration cap)",
    )
    .await;
    msg
}

pub(crate) async fn emit_prompt_injected_event(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    base_meta: &crate::engine::thread_events::EventMeta,
    prompt: &super::super::InjectedPrompt,
) -> bool {
    let mut inject_meta = base_meta.clone();
    inject_meta.event_id = None;
    let emitted = bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: crate::engine::thread_events::ThreadEvent::PromptInjected {
                text: prompt.text.clone(),
                mode: prompt.mode,
                origin: prompt.origin.clone(),
                injected_message_id: prompt.event_id,
                delivered_event_id: None,
            },
            meta: inject_meta,
        })
        .await;
    match emitted {
        Ok(_) => {
            if let Some(message) = prompt.event_id {
                log_record_to_inject(bus.pool(), thread_id, message).await;
            }
            true
        }
        Err(e) if crate::engine::consumption_claim::is_removed_refusal(&*e) => {
            crate::log_legacy!(
                "[Inject] Skipped a queued prompt the user took back on thread {thread_id}"
            );
            false
        }
        Err(e) => {
            crate::log_legacy!("[AgenticLoop] PromptInjected emit failed: {e}");
            true
        }
    }
}

async fn log_record_to_inject(pool: &sqlx::PgPool, thread_id: Uuid, message: Uuid) {
    let waited: Result<Option<i64>, sqlx::Error> = sqlx::query_scalar(
        "SELECT (EXTRACT(EPOCH FROM now() - created) * 1000)::bigint FROM events \
         WHERE id = $1 AND thread_id = $2 AND event_type = 'MessageReceived'",
    )
    .bind(message)
    .bind(thread_id)
    .fetch_optional(pool)
    .await;
    match waited {
        Ok(Some(ms)) => crate::log_legacy!("[Inject] Thread {thread_id}: message {message} reached the agent {ms} ms after it was recorded"),
        Ok(None) => {}
        Err(e) => crate::log_legacy!("[Inject] Thread {thread_id}: could not read when message {message} was recorded: {e}"),
    }
}

pub(crate) const MAX_PINNED_EXPLICIT_IMAGES: usize = 8;

pub(crate) fn push_explicit_image_pin(pins: &mut Vec<usize>, idx: usize) {
    pins.push(idx);
    if pins.len() > MAX_PINNED_EXPLICIT_IMAGES {
        pins.remove(0);
    }
}

pub(crate) fn holds_answer_images(tool_outputs: &[ToolOutput]) -> bool {
    tool_outputs.iter().any(|output| !output.images.is_empty())
}

pub(crate) fn holds_explicitly_requested_image(blocks: &[ContentBlock]) -> bool {
    blocks.iter().any(|b| {
        matches!(
            b,
            ContentBlock::ToolResult { content, .. }
                if content.starts_with(crate::engine::tools::files::EXPLICIT_IMAGE_RESULT_TEXT)
        )
    })
}

pub(crate) async fn emit_image_descriptions(
    event_store: &crate::core::EventStore,
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    origin_id: Uuid,
    channel: Option<crate::engine::thread_events::EventChannel>,
    handle: Option<tokio::task::JoinHandle<Option<(String, String)>>>,
) {
    let Some((desc, model)) = (match handle {
        Some(h) => match h.await {
            Ok(opt) => opt.filter(|(d, _)| !is_bad_image_description(d)),
            Err(_) => None,
        },
        None => None,
    }) else {
        return;
    };

    let hashes = match event_store.get_event_by_id(origin_id).await {
        Ok(Some(row)) => row
            .payload
            .get("user_image_hashes")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|h| h.as_str().map(str::to_string))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
        Ok(None) => {
            crate::log_legacy!(
                "[AgentLoop] ImageDescribed: origin event {} not found, skipping emit",
                origin_id
            );
            Vec::new()
        }
        Err(e) => {
            crate::log_legacy!(
                "[AgentLoop] ImageDescribed: failed to load origin event {}: {}",
                origin_id,
                e
            );
            Vec::new()
        }
    };

    for hash in hashes {
        bus.emit_or_log(
            crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::ImageDescribed {
                    source_event_id: origin_id,
                    hash,
                    description: desc.clone(),
                    model: model.clone(),
                },
                meta: crate::engine::thread_events::EventMeta {
                    channel,
                    ..crate::engine::thread_events::EventMeta::NONE
                },
            },
            "[AgentLoop] ImageDescribed",
        )
        .await;
    }
}

pub(crate) async fn ensure_terminator_emitted(
    bus: &crate::engine::event_bus::EventBus,
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    request_event_id: Uuid,
    channel: Option<crate::engine::thread_events::EventChannel>,
) {
    if crate::engine::thread_events::has_terminator_for(pool, thread_id, request_event_id).await {
        return;
    }

    crate::log_legacy!(
        "[AgenticLoop] WARNING: loop exited without emitting a terminator for request {} on thread {} — emitting defensive ResponseAborted",
        request_event_id,
        thread_id
    );
    crate::engine::thread_events::emit_response_aborted(
        bus,
        thread_id,
        crate::engine::thread_events::AbortCause::ProcessKilled,
        String::new(),
        vec![],
        None,
        None,
        crate::engine::thread_events::EventMeta {
            request_event_id: Some(request_event_id),
            channel,
            ..crate::engine::thread_events::EventMeta::NONE
        },
        "[AgenticLoop] ResponseAborted (defensive — no terminator emitted)",
    )
    .await;
}

pub(crate) struct ContextCaptureSeed<'a> {
    pub sections: &'a [crate::engine::ContextSection],
    pub model: &'a str,
}

/// INVARIANT: The rows one round's `ContextCaptured` persists.
///
/// `capture_body` is the `capture_context` preference as read for THIS round,
/// never for the turn: a question card can hold a turn open for hours.
pub(crate) fn round_capture_sections(
    seed: &[crate::engine::ContextSection],
    tail: impl IntoIterator<Item = crate::engine::ContextSection>,
    capture_body: bool,
) -> Vec<crate::engine::ContextSection> {
    seed.iter()
        .cloned()
        .chain(tail)
        .map(|mut section| {
            if !capture_body {
                section.content = None;
            }
            section
        })
        .collect()
}

pub(crate) struct TurnTools {
    defs: Vec<ToolDefinition>,
    names: Vec<String>,
    defs_chars: usize,
    mcp_generation: u64,
    mcp_char_ceiling: usize,
    mcp_dropped_notice: Option<String>,
    mcp_servers: std::collections::BTreeSet<String>,
}

impl TurnTools {
    pub(crate) fn new(
        engine_defs: Vec<ToolDefinition>,
        surface: crate::mcp::McpToolSurface,
        mcp_char_ceiling: usize,
    ) -> Self {
        let mut tools = Self {
            defs: engine_defs,
            names: Vec::new(),
            defs_chars: 0,
            mcp_generation: 0,
            mcp_char_ceiling,
            mcp_dropped_notice: None,
            mcp_servers: std::collections::BTreeSet::new(),
        };
        tools.refresh_mcp(surface);
        tools.remeasure();
        tools
    }

    pub(crate) fn defs(&self) -> &[ToolDefinition] {
        &self.defs
    }

    pub(crate) fn names(&self) -> &[String] {
        &self.names
    }

    pub(crate) fn defs_chars(&self) -> usize {
        self.defs_chars
    }

    pub(crate) fn mcp_generation(&self) -> u64 {
        self.mcp_generation
    }

    pub(crate) fn mcp_char_ceiling(&self) -> usize {
        self.mcp_char_ceiling
    }

    pub(crate) fn mcp_dropped_notice(&self) -> Option<&str> {
        self.mcp_dropped_notice.as_deref()
    }

    pub(crate) fn mcp_server_ids(&self) -> &std::collections::BTreeSet<String> {
        &self.mcp_servers
    }

    pub(crate) fn refresh_mcp(&mut self, surface: crate::mcp::McpToolSurface) -> bool {
        self.mcp_generation = surface.generation;
        self.mcp_servers = surface.servers.clone();
        let is_mcp =
            |t: &ToolDefinition| crate::mcp::McpManager::parse_mcp_tool_name(&t.name).is_some();
        let head_len = self.defs.iter().take_while(|t| !is_mcp(t)).count();
        let notice = surface.dropped_notice();
        if self.defs[head_len..] == surface.tools[..] && self.mcp_dropped_notice == notice {
            return false;
        }
        self.defs.truncate(head_len);
        self.defs.extend(surface.tools);
        self.mcp_dropped_notice = notice;
        self.remeasure();
        true
    }

    fn remeasure(&mut self) {
        self.names = self.defs.iter().map(|t| t.name.clone()).collect();
        self.defs_chars = crate::engine::context::tool_definitions_chars(&self.defs);
    }
}

pub(crate) fn mcp_surface_correction(
    before: &std::collections::BTreeSet<String>,
    after: &std::collections::BTreeSet<String>,
) -> Option<String> {
    let gained: Vec<&str> = after.difference(before).map(String::as_str).collect();
    let lost: Vec<&str> = before.difference(after).map(String::as_str).collect();

    let mut parts: Vec<String> = Vec::new();
    if !gained.is_empty() {
        parts.push(format!(
            "enabled in this thread and callable now: {}",
            gained.join(", ")
        ));
    }
    if !lost.is_empty() {
        parts.push(format!(
            "no longer offered in this thread: {}",
            lost.join(", ")
        ));
    }
    (!parts.is_empty()).then(|| {
        format!(
            "[MCP UPDATE] {}. This corrects the MCP server list above.",
            parts.join("; ")
        )
    })
}

pub(crate) fn mcp_dropped_correction(before: Option<&str>, after: Option<&str>) -> Option<String> {
    match (before, after) {
        (before, Some(after)) if before != Some(after) => Some(after.to_string()),
        (Some(_), None) => Some(
            "[MCP UPDATE] Every MCP tool is sent now. This corrects the tools-not-sent line above."
                .to_string(),
        ),
        _ => None,
    }
}

const CONVERSATION_PERSIST_MAX: usize = 8_000;

pub(crate) fn conversation_section(
    messages: &[Message],
    bundled_total: usize,
    context_chars: usize,
) -> crate::engine::ContextSection {
    let serialized = serialize_messages_for_capture(messages);
    let content_chars = serialized.chars().count();
    let cap = crate::engine::eval_capture::body_cap(CONVERSATION_PERSIST_MAX);
    let content = Some(match cap {
        Some(cap) if serialized.len() > cap => {
            crate::engine::context::truncate_head_tail(&serialized, cap)
        }
        _ => serialized,
    });
    crate::engine::ContextSection {
        name: "Conversation".to_string(),
        content,
        budget_delta_chars: context_chars.saturating_sub(bundled_total),
        content_chars: Some(content_chars),
        role: crate::engine::ContextRole::User,
        group: None,
    }
}

pub(crate) fn serialize_messages_for_capture(messages: &[Message]) -> String {
    let mut out = String::new();
    for msg in messages {
        out.push_str(&format!("[{}]\n", msg.role));
        match &msg.content {
            MessageContent::Text(s) => out.push_str(s),
            MessageContent::Blocks(blocks) => {
                for block in blocks {
                    match block {
                        ContentBlock::Text { text }
                        | ContentBlock::EngineTail { text }
                        | ContentBlock::MemoryView { text }
                        | ContentBlock::StableContext { text } => out.push_str(text),
                        ContentBlock::ToolUse {
                            name, input, id, ..
                        } => {
                            out.push_str(&format!(
                                "\n[tool_use {} id={} input={}]",
                                name, id, input
                            ));
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            ..
                        } => {
                            out.push_str(&format!(
                                "\n[tool_result id={} content={}]",
                                tool_use_id, content
                            ));
                        }
                        ContentBlock::Image { .. } => out.push_str("\n[image]"),
                    }
                }
            }
        }
        out.push_str("\n\n");
    }
    out
}

pub(super) fn note_failure(
    failed: &mut std::collections::HashSet<String>,
    tool_called_event_id: Option<uuid::Uuid>,
    mode: crate::engine::chat::process::context_mode::ContextMode,
) {
    if let Some(handle) = tool_called_event_id.filter(|_| mode.is_on()) {
        failed.insert(crate::engine::chat::process::context_mode::event_address(
            handle,
        ));
    }
}

pub(crate) async fn read_working_understanding(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: uuid::Uuid,
    raw_text: &str,
    end: wu::ReplyEnd,
    live_document: &mut wu::WorkingUnderstanding,
    live_todo: &mut Vec<crate::engine::thread_events::TodoItem>,
    live_todo_notes: &Option<String>,
) -> (wu::ParsedReply, wu::Applied) {
    let mut parsed = wu::parse_message(wu::ASSISTANT_ROLE, raw_text);
    if end == wu::ReplyEnd::Truncated {
        parsed.drop_unclosed();
    }
    let (next_document, applied) = wu::apply_spans(live_document, &parsed);
    if parsed.wrote_something() {
        *live_document = next_document;
        bus.emit_or_log(
            crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::WorkingUnderstandingWritten {
                    document: live_document.to_document(),
                },
                meta: crate::engine::thread_events::EventMeta::NONE,
            },
            "[AgenticLoop] WorkingUnderstandingWritten",
        )
        .await;
    }
    if let Some(items) = applied.todo.clone().filter(|items| items != live_todo) {
        live_todo.clone_from(&items);
        bus.emit_or_log(
            crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::TodoListWritten {
                    items,
                    notes: live_todo_notes.clone(),
                },
                meta: crate::engine::thread_events::EventMeta::NONE,
            },
            "[AgenticLoop] TodoListWritten (working understanding)",
        )
        .await;
    }
    (parsed, applied)
}
