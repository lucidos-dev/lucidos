//! Codex `exec --json` JSONL parser.
//!
//! `codex exec --json` prints one JSON object per line. Top-level event
//! types: `thread.started`, `turn.started`, `turn.completed`, `turn.failed`,
//! `item.started` / `item.updated` / `item.completed`, and `error`. Item
//! types: `agent_message`, `reasoning`, `command_execution`, `file_change`,
//! `mcp_tool_call`, `web_search`, `todo_list`, `error`.
//!
//! Parsing is split in two pure layers so both are unit-testable without a
//! process: [`parse_codex_line`] decodes one line into a [`CodexLine`], and
//! [`TurnTracker::map_line`] folds lines into the canonical [`AgentEvent`]s
//! the engine consumes. The driver in `codex.rs` owns the process and the
//! per-turn lifecycle (duration measurement, synthesizing a `Result` when
//! the child dies without a turn terminal).

use super::agent_runtime::AgentEvent;

/// One decoded line of `codex exec --json` output.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum CodexLine {
    ThreadStarted {
        thread_id: String,
    },
    TurnStarted,
    TurnCompleted {
        input_tokens: u64,
        cached_input_tokens: u64,
        output_tokens: u64,
    },
    TurnFailed {
        message: String,
    },
    /// Top-level `error` event. May be transient ("Reconnecting… 1/5") —
    /// the tracker records it but only the driver decides whether it ends
    /// the turn (it does only when the process dies without a terminal).
    StreamError {
        message: String,
    },
    Item {
        phase: ItemPhase,
        id: String,
        kind: ItemKind,
    },
    /// Unrecognized or non-JSON line — logged by the caller, otherwise ignored.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ItemPhase {
    Started,
    Updated,
    Completed,
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum ItemKind {
    AgentMessage {
        text: String,
    },
    Reasoning {
        text: String,
    },
    CommandExecution {
        command: String,
        aggregated_output: String,
        exit_code: Option<i64>,
        status: String,
    },
    FileChange {
        changes: serde_json::Value,
        status: String,
    },
    McpToolCall {
        server: String,
        tool: String,
        arguments: serde_json::Value,
        result: serde_json::Value,
        error: Option<String>,
        status: String,
    },
    WebSearch {
        query: String,
    },
    TodoList {
        items: serde_json::Value,
    },
    /// In-stream `error` item (e.g. "command output truncated").
    Error {
        message: String,
    },
    /// Item type this parser doesn't know — forwarded as `Other` so a new
    /// Codex item type degrades to a log line, not a wedged turn.
    Unknown {
        item_type: String,
    },
}

fn str_field(v: &serde_json::Value, key: &str) -> String {
    v.get(key)
        .and_then(|x| x.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Decode one JSONL line. Never fails — undecodable input maps to
/// [`CodexLine::Other`] so a protocol surprise can't kill the stream loop.
pub(super) fn parse_codex_line(line: &str) -> CodexLine {
    let line = line.trim();
    if line.is_empty() {
        return CodexLine::Other;
    }
    let val: serde_json::Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return CodexLine::Other,
    };
    let event_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match event_type {
        "thread.started" => CodexLine::ThreadStarted {
            thread_id: str_field(&val, "thread_id"),
        },
        "turn.started" => CodexLine::TurnStarted,
        "turn.completed" => {
            let usage = val.get("usage").cloned().unwrap_or(serde_json::Value::Null);
            let u64_field = |key: &str| usage.get(key).and_then(|v| v.as_u64()).unwrap_or(0);
            CodexLine::TurnCompleted {
                input_tokens: u64_field("input_tokens"),
                cached_input_tokens: u64_field("cached_input_tokens"),
                output_tokens: u64_field("output_tokens"),
            }
        }
        "turn.failed" => CodexLine::TurnFailed {
            message: val
                .get("error")
                .map(|e| str_field(e, "message"))
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| "turn failed".to_string()),
        },
        "error" => CodexLine::StreamError {
            message: str_field(&val, "message"),
        },
        "item.started" | "item.updated" | "item.completed" => {
            let phase = match event_type {
                "item.started" => ItemPhase::Started,
                "item.updated" => ItemPhase::Updated,
                _ => ItemPhase::Completed,
            };
            let Some(item) = val.get("item") else {
                return CodexLine::Other;
            };
            let id = str_field(item, "id");
            let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let kind = match item_type {
                "agent_message" => ItemKind::AgentMessage {
                    text: str_field(item, "text"),
                },
                "reasoning" => ItemKind::Reasoning {
                    text: str_field(item, "text"),
                },
                "command_execution" => ItemKind::CommandExecution {
                    command: str_field(item, "command"),
                    aggregated_output: str_field(item, "aggregated_output"),
                    exit_code: item.get("exit_code").and_then(|v| v.as_i64()),
                    status: str_field(item, "status"),
                },
                "file_change" => ItemKind::FileChange {
                    changes: item
                        .get("changes")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                    status: str_field(item, "status"),
                },
                "mcp_tool_call" => ItemKind::McpToolCall {
                    server: str_field(item, "server"),
                    tool: str_field(item, "tool"),
                    arguments: item
                        .get("arguments")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                    result: item
                        .get("result")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                    error: item
                        .get("error")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    status: str_field(item, "status"),
                },
                "web_search" => ItemKind::WebSearch {
                    query: str_field(item, "query"),
                },
                "todo_list" => ItemKind::TodoList {
                    items: item
                        .get("items")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null),
                },
                "error" => ItemKind::Error {
                    message: str_field(item, "message"),
                },
                other => ItemKind::Unknown {
                    item_type: other.to_string(),
                },
            };
            CodexLine::Item { phase, id, kind }
        }
        _ => CodexLine::Other,
    }
}

/// Map Codex item status (`completed` / `failed` / `in_progress`) to the
/// `AgentEvent::ToolResult.status` convention CC established
/// (`success` / `error`).
fn tool_status(codex_status: &str) -> &'static str {
    if codex_status == "failed" {
        "error"
    } else {
        "success"
    }
}

/// Folds decoded lines into canonical [`AgentEvent`]s across one turn.
///
/// Owned by the driver for the lifetime of a session (so `Init` is emitted
/// once and the session id survives across turns); per-turn accumulation is
/// reset by [`TurnTracker::begin_turn`].
#[derive(Debug, Default)]
pub(super) struct TurnTracker {
    /// Codex thread id — the engine's resume handle for this session.
    pub session_id: Option<String>,
    init_emitted: bool,
    /// Assistant text accumulated this turn — becomes `Result.text` so the
    /// engine's empty-result (stale-resume) detection sees what streamed.
    agent_texts: Vec<String>,
    /// Last todo_list payload emitted, for dedup across started/updated/completed.
    last_todo_items: Option<serde_json::Value>,
    /// Last in-stream error message — the driver uses it to synthesize a
    /// failed `Result` when the process dies without a turn terminal.
    pub last_error: Option<String>,
    /// True once `turn.completed` / `turn.failed` was seen for the current
    /// turn — tells the driver the child's exit is a clean turn boundary.
    pub turn_terminal_seen: bool,
    /// Tool-use ids emitted without a matching result yet. Codex abandons
    /// in-flight items when a turn fails or is interrupted (openai/codex
    /// #14691) — the engine's `tools_in_flight` counter would then stay
    /// positive and permanently disarm its hang watchdog. Drained into
    /// synthetic closing `ToolResult`s at every turn boundary.
    open_tool_ids: Vec<String>,
}

impl TurnTracker {
    /// Tracker for a session that may resume an existing Codex thread.
    pub fn new(session_id: Option<String>) -> Self {
        Self {
            session_id,
            ..Self::default()
        }
    }

    /// Reset per-turn accumulation. Session-level state (session id, whether
    /// Init went out) survives.
    pub fn begin_turn(&mut self) {
        self.agent_texts.clear();
        self.last_error = None;
        self.turn_terminal_seen = false;
        self.open_tool_ids.clear();
    }

    /// Assistant text accumulated this turn, joined the way the engine
    /// renders consecutive messages.
    pub fn turn_text(&self) -> String {
        self.agent_texts.join("\n\n")
    }

    /// Close every tool call still in flight with an error `ToolResult`.
    /// Called at each turn boundary (clean terminal, synthesized failure,
    /// interrupt) so the engine's paired tool counter re-arms its watchdog.
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

    /// Fold one decoded line into zero or more canonical events.
    /// `turn_duration_ms` is the driver-measured elapsed time for the turn —
    /// Codex doesn't report one, unlike CC's `result.duration_ms`.
    pub fn map_line(&mut self, line: CodexLine, turn_duration_ms: u64) -> Vec<AgentEvent> {
        match line {
            CodexLine::ThreadStarted { thread_id } => {
                self.session_id = Some(thread_id.clone());
                if self.init_emitted {
                    // Resumed turns re-announce the same thread id — the
                    // engine's Init handler would just re-store it.
                    return Vec::new();
                }
                self.init_emitted = true;
                vec![AgentEvent::Init {
                    session_id: thread_id,
                    // The JSONL stream doesn't echo the model; the engine
                    // falls back to the model it requested at spawn.
                    model: None,
                    slash_commands: Vec::new(),
                    skills: Vec::new(),
                }]
            }
            CodexLine::TurnStarted => Vec::new(),
            CodexLine::TurnCompleted {
                input_tokens,
                cached_input_tokens,
                output_tokens,
            } => {
                self.turn_terminal_seen = true;
                let clamp = |v: u64| crate::llm::clamp_provider_token_count(v, "Codex");
                // Codex's `input_tokens` is the TOTAL prompt size with
                // `cached_input_tokens` counting the cached portion within it
                // — unlike Anthropic, where input_tokens excludes the cache.
                // `AgentEvent::Usage.input_tokens` carries the UNCACHED
                // portion (the CC convention the consumer re-totals), so
                // subtract the cached share here.
                let total_input = clamp(input_tokens);
                let cached = clamp(cached_input_tokens).min(total_input);
                let mut events = self.close_open_tools();
                if total_input > 0 || output_tokens > 0 {
                    events.push(AgentEvent::Usage {
                        model: None,
                        input_tokens: total_input - cached,
                        output_tokens: clamp(output_tokens),
                        cache_read_tokens: cached,
                        cache_creation_tokens: 0,
                    });
                }
                events.push(AgentEvent::Result {
                    text: self.turn_text(),
                    duration_ms: turn_duration_ms,
                    error: None,
                });
                events
            }
            CodexLine::TurnFailed { message } => {
                self.turn_terminal_seen = true;
                let mut events = self.close_open_tools();
                events.push(AgentEvent::Result {
                    text: self.turn_text(),
                    duration_ms: turn_duration_ms,
                    error: Some(message),
                });
                events
            }
            CodexLine::StreamError { message } => {
                // Possibly transient (reconnects) — record, don't terminate.
                // The driver promotes it to a failed Result only when the
                // process exits without a turn terminal.
                self.last_error = Some(message);
                Vec::new()
            }
            CodexLine::Item { phase, id, kind } => self.map_item(phase, id, kind),
            CodexLine::Other => Vec::new(),
        }
    }

    fn map_item(&mut self, phase: ItemPhase, id: String, kind: ItemKind) -> Vec<AgentEvent> {
        match kind {
            ItemKind::AgentMessage { text } => {
                if phase != ItemPhase::Completed || text.is_empty() {
                    return Vec::new();
                }
                self.agent_texts.push(text.clone());
                vec![AgentEvent::Message {
                    role: "assistant".to_string(),
                    text,
                }]
            }
            // Reasoning summary — surfaced as a Thought so the timeline can show a
            // live "Thinking" step. Emitted only on completion (Codex exec sends
            // the full summary text on the completed item, not as deltas) and only
            // when non-empty, mirroring the AgentMessage guard above.
            ItemKind::Reasoning { text } => {
                if phase != ItemPhase::Completed || text.is_empty() {
                    return Vec::new();
                }
                vec![AgentEvent::Thought { text }]
            }
            ItemKind::CommandExecution {
                command,
                aggregated_output,
                exit_code,
                status,
            } => match phase {
                ItemPhase::Started => {
                    self.open_tool_ids.push(id.clone());
                    vec![AgentEvent::ToolUse {
                        name: "command_execution".to_string(),
                        input: serde_json::json!({ "command": command }),
                        id,
                    }]
                }
                ItemPhase::Updated => Vec::new(),
                ItemPhase::Completed => {
                    self.open_tool_ids.retain(|open| open != &id);
                    vec![AgentEvent::ToolResult {
                        output: if aggregated_output.is_empty() {
                            format!("exit_code: {}", exit_code.unwrap_or_default())
                        } else {
                            aggregated_output
                        },
                        status: tool_status(&status).to_string(),
                        id,
                    }]
                }
            },
            // Completed-only in the stream — synthesize the call/result pair
            // so the engine's in-flight tool counter stays balanced.
            ItemKind::FileChange { changes, status } => {
                if phase != ItemPhase::Completed {
                    return Vec::new();
                }
                vec![
                    AgentEvent::ToolUse {
                        name: "file_change".to_string(),
                        input: serde_json::json!({ "changes": changes }),
                        id: id.clone(),
                    },
                    AgentEvent::ToolResult {
                        output: status.clone(),
                        status: tool_status(&status).to_string(),
                        id,
                    },
                ]
            }
            ItemKind::McpToolCall {
                server,
                tool,
                arguments,
                result,
                error,
                status,
            } => {
                // Same mcp__<server>__<tool> naming CC uses for MCP tools.
                let name = format!("mcp__{}__{}", server, tool);
                match phase {
                    ItemPhase::Started => {
                        self.open_tool_ids.push(id.clone());
                        vec![AgentEvent::ToolUse {
                            name,
                            input: arguments,
                            id,
                        }]
                    }
                    ItemPhase::Updated => Vec::new(),
                    ItemPhase::Completed => {
                        self.open_tool_ids.retain(|open| open != &id);
                        vec![AgentEvent::ToolResult {
                            output: error.unwrap_or_else(|| result.to_string()),
                            status: tool_status(&status).to_string(),
                            id,
                        }]
                    }
                }
            }
            ItemKind::WebSearch { query } => {
                if phase != ItemPhase::Completed {
                    return Vec::new();
                }
                vec![
                    AgentEvent::ToolUse {
                        name: "web_search".to_string(),
                        input: serde_json::json!({ "query": query }),
                        id: id.clone(),
                    },
                    AgentEvent::ToolResult {
                        output: String::new(),
                        status: "success".to_string(),
                        id,
                    },
                ]
            }
            ItemKind::TodoList { items } => {
                // started/updated/completed all carry the full list — emit a
                // pair per *distinct* list so the timeline shows live plan
                // progress without duplicate cards.
                if self.last_todo_items.as_ref() == Some(&items) {
                    return Vec::new();
                }
                self.last_todo_items = Some(items.clone());
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
            ItemKind::Error { message } => {
                self.last_error = Some(message);
                Vec::new()
            }
            ItemKind::Unknown { item_type } => {
                crate::log!("[Codex] Unrecognized item type: {}", item_type);
                Vec::new()
            }
        }
    }
}
