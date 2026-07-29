//! Codex `app-server` JSON-RPC protocol mapper.
//!
//! `codex app-server` speaks line-delimited JSON-RPC 2.0 over stdio (verified
//! against codex-cli 0.139.0 — `codex app-server generate-json-schema` is the
//! schema source). Three inbound frame shapes: responses to our requests
//! (`{"id", "result"|"error"}`), server notifications (`{"method", "params"}`,
//! no id), and server→client REQUESTS (`{"id", "method", "params"}` — the
//! approval prompts we must answer).
//!
//! Mirrors the two-pure-layer split of `codex_parse.rs`:
//! [`parse_app_server_line`] decodes one line into an [`AppServerLine`];
//! [`AppServerTracker`] folds notifications into the canonical
//! [`AgentEvent`]s. The driver in `codex_app_server.rs` owns the process,
//! the handshake state machine, and the approval round-trips.

use super::agent_runtime::AgentEvent;
use std::collections::HashMap;

/// One decoded line of `codex app-server` stdout.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum AppServerLine {
    /// Response to a request WE sent. `error` carries the JSON-RPC error's
    /// `message` when the request failed.
    Response {
        id: u64,
        result: serde_json::Value,
        error: Option<String>,
    },
    /// Server notification (no id) — the streaming surface.
    Notification {
        method: String,
        params: serde_json::Value,
    },
    /// Server→client request (id + method) — approvals. Must be answered or
    /// codex blocks the item forever.
    ServerRequest {
        id: serde_json::Value,
        method: String,
        params: serde_json::Value,
    },
    /// Unrecognized or non-JSON line — logged by the caller, otherwise ignored.
    Other,
}

/// Decode one stdout line. Never fails — undecodable input maps to
/// [`AppServerLine::Other`] so a protocol surprise can't kill the read loop.
pub(super) fn parse_app_server_line(line: &str) -> AppServerLine {
    let line = line.trim();
    if line.is_empty() {
        return AppServerLine::Other;
    }
    let val: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return AppServerLine::Other,
    };
    let id = val.get("id");
    let method = val.get("method").and_then(|m| m.as_str());
    match (id, method) {
        (Some(id), Some(method)) => AppServerLine::ServerRequest {
            id: id.clone(),
            method: method.to_string(),
            params: val
                .get("params")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        },
        (None, Some(method)) => AppServerLine::Notification {
            method: method.to_string(),
            params: val
                .get("params")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        },
        (Some(id), None) => {
            // Responses to our requests — we only ever send integer ids.
            let Some(id) = id.as_u64() else {
                return AppServerLine::Other;
            };
            let error = val.get("error").map(|e| {
                e.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown JSON-RPC error")
                    .to_string()
            });
            AppServerLine::Response {
                id,
                result: val
                    .get("result")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
                error,
            }
        }
        (None, None) => AppServerLine::Other,
    }
}

/// An approval request decoded from a server→client request. The driver
/// forwards these to the engine's permission bridge
/// (`RunningAgent::permission_rx`) and replies with the user's decision.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ApprovalRequest {
    /// The Codex item id — becomes `CodingAgentPermissionRequest.tool_use_id`
    /// and pairs the card with the item's `CodingAgentToolCalled`.
    pub item_id: String,
    /// Backend-shaped tool name (`command_execution` / `file_change`) —
    /// same vocabulary the Codex `CodingAgentToolCalled` events use.
    pub tool_name: String,
    /// Card input — feeds the dedup key and the summary line.
    pub input: serde_json::Value,
}

/// Decode the two approval methods. Returns `None` for any other server
/// request (the driver replies with a JSON-RPC error so codex doesn't hang).
pub(super) fn parse_approval_request(
    method: &str,
    params: &serde_json::Value,
) -> Option<ApprovalRequest> {
    let item_id = params
        .get("itemId")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    match method {
        "item/commandExecution/requestApproval" => {
            let mut input = serde_json::Map::new();
            if let Some(cmd) = params.get("command").and_then(|v| v.as_str()) {
                input.insert("command".to_string(), cmd.into());
            }
            if let Some(cwd) = params.get("cwd").and_then(|v| v.as_str()) {
                input.insert("cwd".to_string(), cwd.into());
            }
            if let Some(reason) = params.get("reason").and_then(|v| v.as_str()) {
                input.insert("reason".to_string(), reason.into());
            }
            Some(ApprovalRequest {
                item_id,
                tool_name: "command_execution".to_string(),
                input: serde_json::Value::Object(input),
            })
        }
        "item/fileChange/requestApproval" => {
            let mut input = serde_json::Map::new();
            if let Some(reason) = params.get("reason").and_then(|v| v.as_str()) {
                input.insert("reason".to_string(), reason.into());
            }
            if let Some(root) = params.get("grantRoot").and_then(|v| v.as_str()) {
                input.insert("grant_root".to_string(), root.into());
            }
            // The item id keeps the engine's dedup key unique per approval:
            // two concurrent file-change requests without reason/grantRoot
            // would otherwise collapse onto one card, and a single click
            // would approve a change the user never saw described.
            input.insert("item_id".to_string(), item_id.clone().into());
            Some(ApprovalRequest {
                item_id,
                tool_name: "file_change".to_string(),
                input: serde_json::Value::Object(input),
            })
        }
        _ => None,
    }
}

/// Map an app-server item status (`inProgress` / `completed` / `failed` /
/// `declined`) to the `AgentEvent::ToolResult.status` convention
/// (`success` / `error`). `declined` = the user denied the approval — the
/// item never ran, which is an error from the tool's perspective.
fn tool_status(status: &str) -> &'static str {
    match status {
        "failed" | "declined" => "error",
        _ => "success",
    }
}

fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Folds app-server notifications into canonical [`AgentEvent`]s across one
/// session. Same contract as the exec driver's `TurnTracker`: `Init` once,
/// one `Result` per turn terminal, abandoned tool calls closed at every turn
/// boundary so the engine's paired-tool watchdog counter re-arms.
#[derive(Debug, Default)]
pub(super) struct AppServerTracker {
    /// Codex thread id — the engine's resume handle.
    pub session_id: Option<String>,
    init_emitted: bool,
    /// Turn id of the in-flight turn — the `turn/interrupt` target.
    pub current_turn_id: Option<String>,
    /// Assistant text accumulated this turn — becomes `Result.text`.
    agent_texts: Vec<String>,
    /// Per-item text already emitted via `item/agentMessage/delta`, so the
    /// final `item/completed` only emits the (normally empty) remainder
    /// instead of duplicating the whole message.
    streamed: HashMap<String, String>,
    /// Last error notification — promoted to a failed `Result` only when the
    /// process dies without a turn terminal.
    pub last_error: Option<String>,
    /// True once the current turn saw its `turn/completed`.
    pub turn_terminal_seen: bool,
    /// Tool-use ids emitted without a matching result yet.
    open_tool_ids: Vec<String>,
    /// Approval-gated command/file items whose visible `ToolUse` must wait
    /// until the user accepts the permission card.
    approval_gates: HashMap<String, ApprovalGate>,
    /// Last plan snapshot emitted from `turn/plan/updated`, for dedup —
    /// mirrors the exec tracker's `last_todo_items`. Deliberately NOT reset
    /// in [`begin_turn`]: the plan persists across turns, so a new turn's
    /// first update carrying the unchanged list must not re-emit a card.
    last_plan_items: Option<serde_json::Value>,
    /// Monotonic id source for the synthesized plan tool pairs —
    /// `turn/plan/updated` is a turn-level notification with no item id.
    plan_seq: u64,
}

#[derive(Debug)]
enum ApprovalGate {
    Pending { tool: Option<DeferredToolUse> },
    Accepted,
    Declined,
}

#[derive(Debug)]
struct DeferredToolUse {
    name: String,
    input: serde_json::Value,
    id: String,
}

impl AppServerTracker {
    pub fn new(session_id: Option<String>) -> Self {
        Self {
            session_id,
            ..Self::default()
        }
    }

    /// Reset per-turn accumulation. Session-level state survives.
    pub fn begin_turn(&mut self) {
        self.agent_texts.clear();
        self.streamed.clear();
        self.last_error = None;
        self.turn_terminal_seen = false;
        self.current_turn_id = None;
        self.open_tool_ids.clear();
        self.approval_gates.clear();
    }

    pub fn turn_text(&self) -> String {
        self.agent_texts.join("\n\n")
    }

    /// Record the thread id and emit `Init` exactly once. Fed from the
    /// `thread/start` / `thread/resume` response (primary) and the
    /// `thread/started` notification (defensive duplicate).
    pub fn note_thread_started(
        &mut self,
        thread_id: String,
        model: Option<String>,
    ) -> Vec<AgentEvent> {
        self.session_id = Some(thread_id.clone());
        if self.init_emitted {
            return Vec::new();
        }
        self.init_emitted = true;
        vec![AgentEvent::Init {
            session_id: thread_id,
            model,
            slash_commands: Vec::new(),
            skills: Vec::new(),
        }]
    }

    /// Close every tool call still in flight with an error `ToolResult` —
    /// same watchdog-re-arm contract as the exec tracker.
    pub fn close_open_tools(&mut self) -> Vec<AgentEvent> {
        std::mem::take(&mut self.open_tool_ids)
            .into_iter()
            .map(|id| AgentEvent::ToolResult {
                output: "(abandoned — turn ended before the tool finished)".to_string(),
                status: "error".to_string(),
                id,
            })
            .collect()
    }

    /// Called when Codex asks Lucidos for approval before it runs a command or
    /// file-change item. The later `item/started` notification is still not
    /// real execution while this gate is pending, so the visible tool step is
    /// deferred until [`note_approval_resolved`] reports acceptance.
    pub fn note_approval_request(&mut self, approval: &ApprovalRequest) {
        self.approval_gates
            .entry(approval.item_id.clone())
            .or_insert(ApprovalGate::Pending { tool: None });
    }

    /// Called after the user answered the permission card and the JSON-RPC
    /// response has been sent back to Codex. If the item already announced
    /// `item/started`, acceptance flushes that deferred tool step now.
    pub fn note_approval_resolved(&mut self, item_id: &str, allowed: bool) -> Vec<AgentEvent> {
        if !allowed {
            self.approval_gates
                .insert(item_id.to_string(), ApprovalGate::Declined);
            return Vec::new();
        }

        match self.approval_gates.remove(item_id) {
            Some(ApprovalGate::Pending { tool: Some(tool) }) => self.emit_tool_use(tool),
            Some(ApprovalGate::Pending { tool: None }) | None => {
                self.approval_gates
                    .insert(item_id.to_string(), ApprovalGate::Accepted);
                Vec::new()
            }
            Some(ApprovalGate::Accepted | ApprovalGate::Declined) => Vec::new(),
        }
    }

    fn emit_tool_use(&mut self, tool: DeferredToolUse) -> Vec<AgentEvent> {
        self.open_tool_ids.push(tool.id.clone());
        vec![AgentEvent::ToolUse {
            name: tool.name,
            input: tool.input,
            id: tool.id,
        }]
    }

    fn maybe_emit_tool_use(&mut self, tool: DeferredToolUse) -> Vec<AgentEvent> {
        match self.approval_gates.remove(&tool.id) {
            Some(ApprovalGate::Pending { .. }) => {
                self.approval_gates
                    .insert(tool.id.clone(), ApprovalGate::Pending { tool: Some(tool) });
                Vec::new()
            }
            Some(ApprovalGate::Accepted) | None => self.emit_tool_use(tool),
            Some(ApprovalGate::Declined) => {
                self.approval_gates.insert(tool.id, ApprovalGate::Declined);
                Vec::new()
            }
        }
    }

    fn should_drop_declined_result(&mut self, id: &str) -> bool {
        if !matches!(self.approval_gates.get(id), Some(ApprovalGate::Declined)) {
            return false;
        }
        self.approval_gates.remove(id);
        !self.open_tool_ids.iter().any(|open| open == id)
    }

    /// Fold one notification into zero or more canonical events.
    /// `turn_duration_ms` is driver-measured elapsed time for the turn.
    pub fn map_notification(
        &mut self,
        method: &str,
        params: &serde_json::Value,
        turn_duration_ms: u64,
    ) -> Vec<AgentEvent> {
        match method {
            "thread/started" => {
                let thread_id = params
                    .get("thread")
                    .map(|t| str_field(t, "id"))
                    .unwrap_or_default();
                if thread_id.is_empty() {
                    return Vec::new();
                }
                self.note_thread_started(thread_id, None)
            }
            "turn/started" => {
                if let Some(turn) = params.get("turn") {
                    let id = str_field(turn, "id");
                    if !id.is_empty() {
                        self.current_turn_id = Some(id);
                    }
                }
                Vec::new()
            }
            "turn/completed" => {
                self.turn_terminal_seen = true;
                self.current_turn_id = None;
                let turn = params.get("turn").cloned().unwrap_or_default();
                let status = str_field(&turn, "status");
                let mut events = self.close_open_tools();
                let error = match status.as_str() {
                    "failed" => Some(
                        turn.get("error")
                            .map(|e| str_field(e, "message"))
                            .filter(|m| !m.is_empty())
                            .unwrap_or_else(|| "turn failed".to_string()),
                    ),
                    // `interrupted` is an error-free terminal: the engine's
                    // user_hit_stop latch classifies it as Canceled.
                    _ => None,
                };
                events.push(AgentEvent::Result {
                    text: self.turn_text(),
                    duration_ms: turn_duration_ms,
                    error,
                });
                events
            }
            "thread/tokenUsage/updated" => {
                let last = params
                    .get("tokenUsage")
                    .and_then(|u| u.get("last"))
                    .cloned()
                    .unwrap_or_default();
                let u64_field = |key: &str| last.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
                let clamp = |v: u64| crate::llm::clamp_provider_token_count(v, "Codex");
                // Same convention as the exec tracker: codex reports TOTAL
                // input with the cached share inside it; `Usage.input_tokens`
                // carries the uncached portion (Anthropic convention the
                // consumer re-totals).
                let total_input = clamp(u64_field("inputTokens"));
                let cached = clamp(u64_field("cachedInputTokens")).min(total_input);
                let output = clamp(u64_field("outputTokens"));
                if total_input == 0 && output == 0 {
                    return Vec::new();
                }
                vec![AgentEvent::Usage {
                    model: None,
                    input_tokens: total_input - cached,
                    output_tokens: output,
                    cache_read_tokens: cached,
                    cache_creation_tokens: 0,
                }]
            }
            "error" => {
                let message = params
                    .get("error")
                    .map(|e| str_field(e, "message"))
                    .unwrap_or_default();
                if !message.is_empty() {
                    self.last_error = Some(message);
                }
                Vec::new()
            }
            "item/agentMessage/delta" => {
                let delta = str_field(params, "delta");
                if delta.is_empty() {
                    return Vec::new();
                }
                let item_id = str_field(params, "itemId");
                self.streamed.entry(item_id).or_default().push_str(&delta);
                vec![AgentEvent::Message {
                    role: "assistant".to_string(),
                    text: delta,
                }]
            }
            // Streamed reasoning. Codex emits raw reasoning (`textDelta`) or a
            // reasoning summary (`summaryTextDelta`) depending on the model /
            // `model_reasoning_summary` config; both carry a `delta` string. We
            // surface either as a `Thought` so a long reasoning pass renders as a
            // live "Thinking" step instead of a silent gap. The completed
            // `reasoning` item is left in the catch-all below — the content already
            // streamed here, so re-emitting it on completion would duplicate.
            "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
                let delta = str_field(params, "delta");
                if delta.is_empty() {
                    return Vec::new();
                }
                vec![AgentEvent::Thought { text: delta }]
            }
            "item/started" | "item/completed" => {
                let completed = method == "item/completed";
                let Some(item) = params.get("item") else {
                    return Vec::new();
                };
                self.map_item(item, completed)
            }
            // The plan tool (codex's TodoWrite analog). Normalized to the
            // exec protocol's `todo_list` shape (`{items: [{text, completed}]}`
            // from `{plan: [{step, status}]}`) and emitted as the same
            // synthesized ToolUse/ToolResult pair per *distinct* list, so the
            // timeline shows live plan progress on the default protocol too —
            // previously these notifications were dropped and Codex plan usage
            // was invisible (exec-driver and CC-TodoWrite parity gap).
            "turn/plan/updated" => {
                let items: Vec<serde_json::Value> = params
                    .get("plan")
                    .and_then(|p| p.as_array())
                    .map(|steps| {
                        steps
                            .iter()
                            .map(|s| {
                                serde_json::json!({
                                    "text": str_field(s, "step"),
                                    "completed": s.get("status").and_then(|v| v.as_str())
                                        == Some("completed"),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if items.is_empty() {
                    return Vec::new();
                }
                let items = serde_json::Value::Array(items);
                if self.last_plan_items.as_ref() == Some(&items) {
                    return Vec::new();
                }
                self.last_plan_items = Some(items.clone());
                self.plan_seq += 1;
                let id = format!("plan_{}", self.plan_seq);
                vec![
                    AgentEvent::ToolUse {
                        name: "todo_list".to_string(),
                        input: serde_json::json!({ "items": items }),
                        id: id.clone(),
                    },
                    AgentEvent::ToolResult {
                        output: String::new(),
                        status: "success".to_string(),
                        id,
                    },
                ]
            }
            _ => Vec::new(),
        }
    }

    fn map_item(&mut self, item: &serde_json::Value, completed: bool) -> Vec<AgentEvent> {
        let id = str_field(item, "id");
        let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match item_type {
            "agentMessage" => {
                if !completed {
                    return Vec::new();
                }
                let text = str_field(item, "text");
                if text.is_empty() {
                    return Vec::new();
                }
                self.agent_texts.push(text.clone());
                // Deltas already streamed most (usually all) of this item —
                // emit only the remainder so the engine's text buffer doesn't
                // duplicate the message.
                let streamed = self.streamed.remove(&id).unwrap_or_default();
                let remainder = match text.strip_prefix(streamed.as_str()) {
                    Some(rest) => rest.to_string(),
                    // Deltas diverged from the final text (shouldn't happen;
                    // defensive) — prefer the streamed version already shown,
                    // emit nothing extra.
                    None if !streamed.is_empty() => String::new(),
                    None => text,
                };
                if remainder.is_empty() {
                    return Vec::new();
                }
                vec![AgentEvent::Message {
                    role: "assistant".to_string(),
                    text: remainder,
                }]
            }
            "commandExecution" => {
                if completed {
                    if self.should_drop_declined_result(&id) {
                        return Vec::new();
                    }
                    self.open_tool_ids.retain(|open| open != &id);
                    let aggregated = str_field(item, "aggregatedOutput");
                    let output = if aggregated.is_empty() {
                        format!(
                            "exit_code: {}",
                            item.get("exitCode")
                                .and_then(|v| v.as_i64())
                                .unwrap_or_default()
                        )
                    } else {
                        aggregated
                    };
                    vec![AgentEvent::ToolResult {
                        output,
                        status: tool_status(&str_field(item, "status")).to_string(),
                        id,
                    }]
                } else {
                    self.maybe_emit_tool_use(DeferredToolUse {
                        name: "command_execution".to_string(),
                        input: serde_json::json!({ "command": str_field(item, "command") }),
                        id,
                    })
                }
            }
            "fileChange" => {
                // Keep path + kind, drop the inline diffs — a multi-file
                // patch would balloon the persisted event.
                let changes: Vec<serde_json::Value> = item
                    .get("changes")
                    .and_then(|c| c.as_array())
                    .map(|arr| {
                        arr.iter()
                            .map(|ch| {
                                serde_json::json!({
                                    "path": str_field(ch, "path"),
                                    "kind": ch.get("kind").cloned().unwrap_or(serde_json::Value::Null),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if completed {
                    if self.should_drop_declined_result(&id) {
                        return Vec::new();
                    }
                    self.open_tool_ids.retain(|open| open != &id);
                    let status = str_field(item, "status");
                    vec![AgentEvent::ToolResult {
                        output: status.clone(),
                        status: tool_status(&status).to_string(),
                        id,
                    }]
                } else {
                    self.maybe_emit_tool_use(DeferredToolUse {
                        name: "file_change".to_string(),
                        input: serde_json::json!({ "changes": changes }),
                        id,
                    })
                }
            }
            "mcpToolCall" => {
                // Same mcp__<server>__<tool> naming the exec driver and CC use.
                let name = format!(
                    "mcp__{}__{}",
                    str_field(item, "server"),
                    str_field(item, "tool")
                );
                if completed {
                    self.open_tool_ids.retain(|open| open != &id);
                    let error = item
                        .get("error")
                        .filter(|e| !e.is_null())
                        .map(|e| str_field(e, "message"))
                        .filter(|m| !m.is_empty());
                    let output = error.unwrap_or_else(|| {
                        item.get("result")
                            .filter(|r| !r.is_null())
                            .map(|r| r.to_string())
                            .unwrap_or_default()
                    });
                    vec![AgentEvent::ToolResult {
                        output,
                        status: tool_status(&str_field(item, "status")).to_string(),
                        id,
                    }]
                } else {
                    self.open_tool_ids.push(id.clone());
                    vec![AgentEvent::ToolUse {
                        name,
                        input: item
                            .get("arguments")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                        id,
                    }]
                }
            }
            "webSearch" => {
                // Completed-only synthesized pair, mirroring the exec tracker.
                if !completed {
                    return Vec::new();
                }
                vec![
                    AgentEvent::ToolUse {
                        name: "web_search".to_string(),
                        input: serde_json::json!({ "query": str_field(item, "query") }),
                        id: id.clone(),
                    },
                    AgentEvent::ToolResult {
                        output: String::new(),
                        status: "success".to_string(),
                        id,
                    },
                ]
            }
            // userMessage echoes our own input; completed reasoning items
            // already streamed via `item/reasoning/*` deltas; plan progress
            // arrives via `turn/plan/updated` (mapped in `map_notification`),
            // not as items.
            _ => Vec::new(),
        }
    }
}
