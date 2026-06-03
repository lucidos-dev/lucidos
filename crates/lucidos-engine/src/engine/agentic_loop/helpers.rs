//! Free helper functions and small types extracted from the agentic-loop
//! driver. Re-exported from `agentic_loop`'s mod.rs so existing
//! `agentic_loop::X` and `super::X` paths keep resolving.

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use crate::llm::{get_default_tools, get_notification_tool, get_read_notifications_tool};
use crate::llm::{ContentBlock, Message, MessageContent};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::super::LucidosEngine;

/// Build a fresh `EventMeta` for a `ResponseCanceled` emit by copying the
/// request meta and stamping the actor that the chat-cancel handler
/// recorded on the per-thread handle. Drains the actor slot via
/// `take_cancel_actor` so a follow-up request on the same thread can't
/// inherit a stale device. When no actor was stamped (engine-internal
/// cancels — shutdown, restart), the request meta passes through unchanged
/// and `meta.actor` stays `None`.
///
/// Defensive: only fills `actor` if `base.actor` is `None`. Today the
/// request meta is always built from `EventMeta::NONE` so `base.actor` is
/// `None` at every call site, but if a future refactor starts propagating
/// the originating message's actor into the request meta, this guard
/// prevents the cancel-clicker from silently overwriting the legitimate
/// message author.
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

/// Race a tool execution future against the per-thread cancel token. On
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

/// Hint suffix for the empty-completion `ResponseFailed` error message. The
/// LLM call returned with no text and no tool calls — the user always sees a
/// failure, but the *reason* needs to be honest:
///
/// - `refusal`: the provider's streaming safety classifier stopped the turn
///   and withheld the content the model had begun generating, so
///   `output_tokens` can be non-zero while nothing reaches the engine. This
///   is a definitive provider signal, not a parser miss — checked before the
///   `output_tokens` heuristic below so a refusal isn't misattributed to a
///   stream-shape change (the misleading message the 2026-06-02 report saw).
/// - `output_tokens > 16` with `thinking_chars == 0`: the provider billed
///   real output but nothing reached the engine. Either a known content
///   block carried an unrecognised payload shape or an unknown block / delta
///   type slipped past the parser (the latter increments
///   `unknown_sse_dropped` in `vertex::process_sse_data`). Surfacing this
///   prevents the misleading "model decided no action was needed" message
///   that masked the real Vertex SSE drift seen on 2026-05-18.
/// - `thinking_chars > 0`: the model thought (we got thinking blocks) but
///   chose not to emit text or call a tool. Distinct from a parser miss.
/// - `max_tokens`: the model hit the token budget before producing any
///   visible block.
/// - `end_turn` with no other signal: genuinely intentional silence.
///
/// Threshold `16` is well above any realistic structural-overhead floor for
/// `usage.output_tokens` on a no-op `end_turn` — keeps true silence out of
/// the parser-miss branch.
pub(crate) fn empty_completion_hint(
    stop_reason: &str,
    output_tokens: u32,
    thinking_chars: usize,
    unknown_sse_dropped: u32,
) -> &'static str {
    // Truncation wins regardless of what was captured — the user needs to
    // know the budget was hit before they wonder about parser drift.
    if stop_reason == "max_tokens" {
        return " — output truncated by token budget";
    }
    // Refusal is a definitive provider signal, not a parser miss: the safety
    // classifier stopped the turn and withheld already-generated content, so
    // output_tokens can be non-zero while nothing reaches the engine. Check it
    // before the output_tokens heuristic, which would otherwise blame the
    // parser / a stream-shape change.
    if stop_reason == "refusal" {
        return " — the model declined to respond (provider safety classifier withheld the output). Rephrase the request or start a new thread; retrying the same prompt will be refused again";
    }
    if output_tokens > 16 && thinking_chars == 0 {
        if unknown_sse_dropped > 0 {
            return " — engine dropped unknown SSE shapes; provider stream may have changed (see [Vertex] WARNING logs for the exact types)";
        }
        return " — model generated output the engine couldn't classify (likely a stream-shape change in the provider)";
    }
    if thinking_chars > 0 {
        return " — model thought but produced no text or tool call";
    }
    match stop_reason {
        "end_turn" => " — model decided no action was needed",
        _ => "",
    }
}

/// Build the tool list for intent sub-loops.
/// Notification tools must be included explicitly — they're not in get_default_tools().
pub(crate) fn build_intent_tools() -> Vec<ToolDefinition> {
    let mut tools: Vec<_> = get_default_tools()
        .into_iter()
        .filter(|t| t.name != tn::EXECUTE_INTENT)
        .collect();
    tools.push(get_notification_tool());
    tools.push(get_read_notifications_tool());
    tools
}

/// Derive the "target" key for the consecutive-tool-call circuit breaker.
///
/// Most tools have a meaningful target argument (`path`, `url`, `query`).
/// `run_bash` is special: its argument is `command`, and bucketing by the
/// bare tool name would mean three unrelated shell calls (e.g. `git status`
/// → `git add` → `git commit`) trip the guard. Bucketing by the first
/// whitespace-delimited token of `command` keeps the original "stop the LLM
/// from spamming the exact same call" intent (same prefix still counts) while
/// letting unrelated commands run in sequence.
///
/// `run_python` and `run_python_background` mirror that idea: bucket by the
/// first non-blank, non-comment, non-import line of `code` (truncated to 80
/// chars). Different scripts have different first actionable lines; identical
/// retry-storms (most famously `time.sleep(N)` polling) share theirs, so the
/// generic 3-strike guard fires. Falls back to the tool name when the code
/// has no actionable line so the guard still fires on three pure-import or
/// pure-comment calls in a row.
///
/// `read_file` is similarly special: its window args (`start_line`,
/// `line_count`, `offset`) are how an LLM legitimately pages through one big
/// file. Bucketing the page-1, page-2, page-3 reads under the same `path`
/// key would trip the read_file breaker on the third page even though each
/// call returns new content. When any window arg is present, suffix the key
/// so each (path, window) combination is its own bucket.
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

/// First non-blank, non-comment, non-import line of a Python script —
/// the bucket key the consecutive-call circuit breaker uses for
/// `run_python` / `run_python_background`. Returns `None` when the code
/// is empty / pure-import / pure-comment so the caller falls back to
/// the bare tool name (three pure-import calls in a row still trip the
/// guard under the tool name, which is fine — they're a retry storm).
///
/// Truncated to 80 chars for readability in logs / STOP messages, with
/// an 8-hex-char hash suffix of the FULL line so two long scripts
/// diverging past char 80 (e.g. `df = pd.read_csv('/long/path/.../foo_A.csv')`
/// vs `..._B.csv`) bucket separately. Case-preserved because Python is
/// case-sensitive (`time.sleep` vs `Time.sleep` mean different things).
pub(crate) fn python_call_key(code: &str) -> Option<String> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    const MAX_KEY_CHARS: usize = 80;
    for raw in code.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // Skip imports — different scripts share these but their
        // actionable lines diverge. Catches `import x`, `from x import y`,
        // and the rare `import x as y` form.
        if line.starts_with("import ") || line.starts_with("from ") {
            continue;
        }
        let take = line
            .char_indices()
            .nth(MAX_KEY_CHARS)
            .map(|(i, _)| i)
            .unwrap_or(line.len());
        if take == line.len() {
            // Line fits — no hash suffix needed.
            return Some(line.to_string());
        }
        // Line longer than 80 chars — append a hash of the full line so
        // two retries that share the same prefix but diverge past
        // char 80 don't collide.
        let mut hasher = DefaultHasher::new();
        line.hash(&mut hasher);
        let h = hasher.finish();
        return Some(format!("{}#{:08x}", &line[..take], (h & 0xFFFF_FFFF) as u32));
    }
    None
}

/// One sentinel-prefixed tool result, parsed: which transient `ThreadEvent`
/// to broadcast to the frontend, and (when not `None`) what to substitute
/// into the LLM-visible tool result so the model cannot read the raw JSON
/// and act on it as if the call returned synchronously. `redacted_text:
/// None` means "emit the event but leave the tool result alone" — the
/// EmailConfirm tool already has a description that explains the modal
/// flow correctly, so its raw payload is harmless to the model.
pub(crate) struct SentinelMatch {
    pub label: &'static str,
    pub event: super::super::thread_events::ThreadEvent,
    pub redacted_text: Option<String>,
}

/// Inspect a tool result for any front-end confirm-flow sentinel and, if one
/// matches, return the transient event to emit + the LLM-facing replacement
/// text (or `None` to leave the LLM's view of the result unchanged).
pub(crate) fn match_sentinel(text: &str) -> Option<SentinelMatch> {
    use super::super::thread_events::ThreadEvent;
    use super::super::tools::credentials::CREDENTIAL_REQUEST_PREFIX;
    use super::super::tools::plugins::{
        PLUGIN_INSTALL_REQUEST_PREFIX, PLUGIN_UNINSTALL_REQUEST_PREFIX,
    };

    type SentinelEntry = (
        &'static str,
        &'static str,
        fn(String) -> ThreadEvent,
        Option<&'static str>,
    );
    let entries: &[SentinelEntry] = &[
        (
            CREDENTIAL_REQUEST_PREFIX,
            "[AgenticLoop] CredentialPromptRequested",
            |payload| ThreadEvent::CredentialPromptRequested { payload },
            Some("Credential request modal shown to the user. Wait for them to enter the credential or cancel — do not chat-ask for the same value; the modal resolves the request."),
        ),
        (
            PLUGIN_INSTALL_REQUEST_PREFIX,
            "[AgenticLoop] PluginInstallRequested",
            |payload| ThreadEvent::PluginInstallRequested { payload },
            Some("Install panel shown to the user. Wait for them to click Confirm or Cancel — do not chat-ask about overwrites and do not claim the install succeeded; the panel resolves it and the next user message will tell you the outcome."),
        ),
        (
            PLUGIN_UNINSTALL_REQUEST_PREFIX,
            "[AgenticLoop] PluginUninstallRequested",
            |payload| ThreadEvent::PluginUninstallRequested { payload },
            Some("Uninstall panel shown to the user. Wait for them to click Confirm or Cancel — do not chat-ask which files to delete and do not claim files were removed; the panel resolves it and the next user message will tell you the outcome."),
        ),
        (
            "[EMAIL_CONFIRM]",
            "[AgenticLoop] EmailConfirmRequested",
            |payload| ThreadEvent::EmailConfirmRequested { payload },
            None,
        ),
    ];

    for &(prefix, label, ctor, redacted) in entries {
        if !text.starts_with(prefix) {
            continue;
        }
        let after = &text[prefix.len()..];
        let rel_start = after.find('{')?;
        let payload = after[rel_start..].to_string();
        return Some(SentinelMatch {
            label,
            event: ctor(payload),
            redacted_text: redacted.map(str::to_string),
        });
    }
    None
}

/// Check if buffered text has reached a renderable boundary.
pub(crate) fn should_flush(text: &str) -> bool {
    // Paragraph break
    if text.ends_with("\n\n") {
        return true;
    }
    // Code fence close
    if text.ends_with("```\n") {
        return true;
    }
    // Heading completed (ends with newline after a heading line)
    if text.ends_with('\n') {
        if let Some(last_line) = text.lines().last() {
            if last_line.starts_with('#') {
                return true;
            }
        }
    }
    // Horizontal rule
    if text.ends_with("\n---\n") || text.ends_with("\n***\n") {
        return true;
    }
    false
}

/// Parse a `[APP_CAPTURE:<screenshot_b64>]\n<dom>` sentinel produced by the SDK
/// frontend `capture_app` tool. Returns `(screenshot_b64, dom_text)` if matched.
pub(crate) fn parse_app_capture_marker(s: &str) -> Option<(&str, &str)> {
    let rest = s.strip_prefix("[APP_CAPTURE:")?;
    let end_bracket = rest.find("]\n")?;
    Some((&rest[..end_bracket], &rest[end_bracket + 2..]))
}

/// Inverse of [`parse_app_capture_marker`]: format a tool-result string for
/// the agentic loop to feed back to Claude.
///
/// When the SDK couldn't take a screenshot (html2canvas can't render every
/// CSS feature — `oklab(...)` colors, for one), it returns `screenshot = ""`
/// and stuffs the failure into `dom`. Wrapping that in `[APP_CAPTURE:]`
/// (empty marker) makes the parser hand the loop an empty base64, which
/// becomes a `ContentBlock::Image { data: "" }` and gets a 400
/// `image cannot be empty` from Anthropic. Drop the marker entirely in that
/// case so the result falls through to the plain-text path; the LLM still
/// reads the `dom` (which carries the error) and can decide how to proceed.
pub(crate) fn format_capture_result(screenshot_b64: &str, dom: &str) -> String {
    if screenshot_b64.is_empty() {
        dom.to_string()
    } else {
        format!("[APP_CAPTURE:{}]\nDOM snapshot:\n{}", screenshot_b64, dom)
    }
}

/// Sniff a recognized image mime from a base64-encoded image. Decodes only the
/// leading bytes (enough for any magic header in `core::blobs::sniff_image_mime`)
/// so the multi-MB capture body isn't decoded twice. Falls back to PNG when the
/// header doesn't match the allowlist — keeps prior behaviour for unrecognised
/// inputs; the Anthropic API rejects the message either way.
pub(crate) fn sniff_image_media_type(b64: &str) -> &'static str {
    use base64::Engine;
    // 16 base64 chars decode to exactly 12 bytes — enough for HEIC's `ftyp` brand
    // at offset 8-11, the longest header `sniff_image_mime` inspects. Round down
    // to a multiple of 4 so the strict decoder accepts the partial slice.
    let prefix_len = 16.min(b64.len() & !3);
    base64::engine::general_purpose::STANDARD
        .decode(&b64.as_bytes()[..prefix_len])
        .ok()
        .as_deref()
        .and_then(crate::core::blobs::sniff_image_mime)
        .unwrap_or("image/png")
}

/// Detect image descriptions that indicate the model couldn't see the image.
/// These are error responses, not actual descriptions — using them would poison
/// the LLM context with false information (e.g. "the image shows an error message").
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

/// Hard cap on per-turn LLM tool-call iterations. The engine returns a
/// `[ENGINE-LIMIT]`-tagged response when the cap is hit — the chat agent
/// cannot otherwise observe its own tool-call count, so any "tool-call
/// cap" claim without this prefix is a hallucination. The chat system
/// prompt tells the model the same.
pub(crate) const MAX_ITERATIONS: usize = 500;

/// Without this emit the frontend would never see a terminator and the
/// thread would show "running" forever after hitting the cap.
pub(crate) async fn emit_iteration_cap_response_generated(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    meta: &crate::engine::thread_events::EventMeta,
    images: Vec<String>,
    effective_model: Option<String>,
    effective_effort: Option<String>,
) -> String {
    let msg = format!(
        "[ENGINE-LIMIT] Per-turn limit of {} tool calls reached. Send any message to continue from here.",
        MAX_ITERATIONS
    );
    bus.emit_or_log(
        crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: crate::engine::thread_events::ThreadEvent::ResponseGenerated {
                text: msg.clone(),
                images,
                model: effective_model,
                reasoning_effort: effective_effort,
            },
            meta: meta.clone(),
        },
        "[AgenticLoop] ResponseGenerated (iteration cap)",
    )
    .await;
    msg
}

/// Critical invariant: the persisted event MUST get a fresh primary key,
/// NOT `prompt.event_id`. The chat fast-path (`chat::process`) emits
/// `MessageReceived` with the client-provided UUID before injecting,
/// so reusing that UUID here causes an `events_pkey` duplicate-key
/// error and the event is silently dropped under `emit_or_log`. The
/// frontend correlates pending messages via `MessageReceived.id`;
/// `UserPromptInjected` is a separate engine-side acknowledgment whose
/// link back to the request is carried by `meta.request_event_id`.
pub(crate) async fn emit_user_prompt_injected_event(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    base_meta: &crate::engine::thread_events::EventMeta,
    prompt: &super::super::InjectedPrompt,
) {
    let mut inject_meta = base_meta.clone();
    inject_meta.event_id = None;
    bus.emit_or_log(
        crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: crate::engine::thread_events::ThreadEvent::UserPromptInjected {
                text: prompt.text.clone(),
                mode: prompt.mode,
                origin: prompt.origin.clone(),
                injected_message_id: prompt.event_id,
            },
            meta: inject_meta,
        },
        "[AgenticLoop] UserPromptInjected",
    )
    .await;
}

/// Defensive post-loop guard: emits `ResponseAborted` if no terminator
/// landed for `request_event_id`. Catches future regressions in the
/// loop's many return paths — every existing path emits explicitly, but
/// the SQL check is the safety net. Scoped to `request_event_id` so a
/// previous exchange's terminator doesn't mask a current zombie one.
///
/// Skip the SQL when callers can prove a terminator was emitted (success
/// path) — `chat::process` does this via the `terminator_emitted` flag
/// threaded through `run_agentic_loop`. Without that fast path this
/// query runs on every chat turn against a `payload->>'request_event_id'`
/// expression that has no functional index, walking every event in the
/// thread on long-lived conversations.
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

    crate::log!(
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

/// Static parts of a ContextCaptured event built once by `chat::process`
/// before the loop starts: section list (system + memory + history + …),
/// the tool list, and the model id. The loop appends a dynamic
/// `Conversation` section per iteration sized from the current `messages`.
///
/// `capture_body` mirrors `PreferenceStore::capture_context` — read once at
/// build time and threaded through so the loop can fill the dynamic
/// `Conversation` section's body when the user has the preference on.
/// Without this, the body stayed `None` even with capture on, and the
/// modal misleadingly showed "Body not captured (capture_context off)".
pub(crate) struct ContextCaptureSeed<'a> {
    pub sections: &'a [crate::engine::ContextSection],
    pub tools: &'a [String],
    pub model: &'a str,
    pub capture_body: bool,
}

/// Render the in-flight `messages` array as a compact text body for the
/// `Conversation` section's persisted content. Each message is prefixed with
/// its role; tool calls/results are summarized inline so the dump stays
/// readable rather than dumping raw JSON. Caller truncates with
/// `truncate_head_tail` when over the persistence cap.
pub(crate) fn serialize_messages_for_capture(messages: &[Message]) -> String {
    let mut out = String::new();
    for msg in messages {
        out.push_str(&format!("[{}]\n", msg.role));
        match &msg.content {
            MessageContent::Text(s) => out.push_str(s),
            MessageContent::Blocks(blocks) => {
                for block in blocks {
                    match block {
                        ContentBlock::Text { text } => out.push_str(text),
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
