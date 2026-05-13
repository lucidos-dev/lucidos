//! Engine-side tool dispatch: `handle_special_tool` covers tools that need
//! more than `execute_tool()` can give them (capture/refresh app UI,
//! run_thread, resume_thread, run_claude, …) and `run_intent_loop` runs
//! the intent sub-loop for the open-ended verbs.

use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::{build_intent_tools, derive_call_key};
use crate::engine::context::trim_context_if_needed;
use crate::engine::event_bus::BusEvent;
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
use crate::engine::LucidosEngine;
use crate::llm::tool_names as tn;
use crate::llm::{ContentBlock, Message, MessageContent};

/// How a thread spawned by `run_thread` / `run_claude` relates to the
/// thread that issued the spawn. Drives both the spawn linkage (parent /
/// spawning event ids) and the result-text branch.
///
/// `Sub` (default) preserves today's callback semantics: the spawning
/// thread's `notify_parent_if_child` fires when the spawned thread reaches
/// a terminal event, and `active_children_count` is incremented while the
/// spawned thread is in flight.
///
/// `Top` drops the linkage entirely — no callback, no count bump. The
/// spawned thread appears as an independent top-level thread in the list.
/// Use when the spawn is for the user to follow, not for the spawning
/// thread to resume on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Relation {
    Sub,
    Top,
}

/// Parse the optional `relation` argument from a tool call. Defaults to
/// [`Relation::Sub`] when absent so existing prompts keep working unchanged.
/// Returns `Err` for unknown values or non-string types so the LLM sees the
/// contract violation instead of silently getting `Sub`.
pub(crate) fn parse_relation(args: &serde_json::Value) -> Result<Relation, String> {
    match args.get("relation") {
        None | Some(serde_json::Value::Null) => Ok(Relation::Sub),
        Some(serde_json::Value::String(s)) => match s.as_str() {
            "sub" => Ok(Relation::Sub),
            "top" => Ok(Relation::Top),
            other => Err(format!(
                "relation must be \"sub\" or \"top\", got {:?}",
                other
            )),
        },
        Some(other) => Err(format!(
            "relation must be \"sub\" or \"top\", got {}",
            other
        )),
    }
}

impl Relation {
    /// Resolve the `(parent_thread_id, spawning_event_id)` pair to pass
    /// into the spawn helpers. `Sub` carries the spawning thread + tool
    /// call event through; `Top` drops both so the spawned thread has no
    /// parent linkage and `notify_parent_if_child` early-returns.
    pub(crate) fn spawn_linkage(
        self,
        spawning_thread_id: Uuid,
        tool_called_event_id: Option<Uuid>,
    ) -> (Option<Uuid>, Option<Uuid>) {
        match self {
            Self::Sub => (Some(spawning_thread_id), tool_called_event_id),
            Self::Top => (None, None),
        }
    }

    /// Tool-result text returned for `run_thread`. The LLM reads this to
    /// decide whether to expect a callback, so each branch must state the
    /// callback contract explicitly.
    pub(crate) fn run_thread_success_text(self, child_thread_id: Uuid) -> String {
        match self {
            Self::Sub => format!(
                "Thread started (thread_id: {0}). When the child thread finishes, \
                 this conversation will automatically resume with its results — \
                 you don't need to check on it. Continue with other work or tell \
                 the user you've delegated this subtask: [Open thread](thread:{0})",
                child_thread_id
            ),
            Self::Top => format!(
                "Top-level thread started (thread_id: {0}). It runs independently \
                 and will NOT report back to this conversation. Tell the user you've \
                 started a separate thread for them to follow: [Open thread](thread:{0})",
                child_thread_id
            ),
        }
    }

    /// Tool-result text returned for `run_claude`. Mirrors the
    /// callback-contract guarantee of `run_thread_success_text` so the LLM
    /// sees both spawn tools as a uniform pair.
    pub(crate) fn run_claude_success_text(self, cc_thread_id: Uuid) -> String {
        match self {
            Self::Sub => format!(
                "Claude Code session started in a new thread (thread_id: {0}). \
                 Tell the user you've started a new thread to work on this and include \
                 a link: [Open thread](thread:{0})",
                cc_thread_id
            ),
            Self::Top => format!(
                "Top-level Claude Code session started (thread_id: {0}). It runs \
                 independently and will NOT report back to this conversation. \
                 Include the link in your reply so the user can follow it: \
                 [Open thread](thread:{0})",
                cc_thread_id
            ),
        }
    }
}

impl LucidosEngine {
    /// Handle tool calls that need special processing outside of `execute_tool`.
    /// Returns `Some(result)` if the tool was handled, `None` if it should fall through
    /// to `execute_tool()`.
    pub(super) async fn handle_special_tool(
        &self,
        tool_name: &str,
        tool_args: &serde_json::Value,
        thread_id: Uuid,
        user_images: Option<&[crate::api::ChatImage]>,
        device_id: Option<&str>,
        // Event_id of the just-emitted ToolCalled event. Forwarded as
        // `spawning_event_id` for spawn-style tools (run_thread, run_claude)
        // so the new thread can be traced back to the exact tool call that
        // started it.
        tool_called_event_id: Option<Uuid>,
    ) -> Option<String> {
        if tool_name == tn::REFRESH_FILE {
            let path = tool_args["path"].as_str().unwrap_or("");
            self.event_bus
                .emit_or_log(
                    BusEvent::Thread {
                        thread_id,
                        event: ThreadEvent::RefreshFile {
                            path: path.to_string(),
                        },
                        meta: EventMeta::NONE,
                    },
                    "[AgenticLoop] RefreshFile",
                )
                .await;
            Some(format!("File preview refreshed for {}", path))
        } else if tool_name == tn::CAPTURE_APP || tool_name == tn::REFRESH_APP {
            let app_id = tool_args
                .get("app_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            if tool_name == tn::REFRESH_APP {
                self.event_bus
                    .emit_or_log(
                        BusEvent::Thread {
                            thread_id,
                            event: ThreadEvent::RefreshAppUI {
                                app_id: app_id.clone(),
                            },
                            meta: EventMeta::NONE,
                        },
                        "[AgenticLoop] RefreshAppUI (refresh_app tool)",
                    )
                    .await;
            }

            let skip_capture = tool_args
                .get("skip_capture")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if skip_capture {
                Some("App UI refreshed.".to_string())
            } else {
                if tool_name == tn::REFRESH_APP {
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }

                let request_id = Uuid::new_v4().to_string();
                let (tx, rx) = tokio::sync::oneshot::channel();
                {
                    let mut captures = self.pending_captures.lock().unwrap();
                    captures.insert(request_id.clone(), tx);
                }

                self.event_bus
                    .emit_or_log(
                        BusEvent::Thread {
                            thread_id,
                            event: ThreadEvent::CaptureAppUI {
                                app_id: app_id.clone(),
                                request_id: request_id.clone(),
                            },
                            meta: EventMeta::NONE,
                        },
                        "[AgenticLoop] CaptureAppUI",
                    )
                    .await;

                Some(
                    match tokio::time::timeout(std::time::Duration::from_secs(10), rx).await {
                        Ok(Ok(capture)) => {
                            format!(
                                "[APP_CAPTURE:{}]\nDOM snapshot:\n{}",
                                capture.screenshot, capture.dom
                            )
                        }
                        Ok(Err(_)) => {
                            "Error: Capture failed — the frontend channel was dropped.".to_string()
                        }
                        Err(_) => {
                            let mut captures = self.pending_captures.lock().unwrap();
                            captures.remove(&request_id);
                            "Error: Capture timed out (10s) — the frontend could not open or capture the app UI.".to_string()
                        }
                    },
                )
            }
        } else if tool_name == tn::RUN_CLAUDE {
            let prompt = tool_args["prompt"].as_str().unwrap_or("");
            if prompt.is_empty() {
                Some("Error: prompt is required".to_string())
            } else {
                let relation = match parse_relation(tool_args) {
                    Ok(r) => r,
                    Err(e) => return Some(format!("Error: {}", e)),
                };
                let (parent_thread_id, spawning_event_id) =
                    relation.spawn_linkage(thread_id, tool_called_event_id);
                let repo_id = match tool_args["repo"].as_str().filter(|s| !s.is_empty()) {
                    Some(repo_param) => match crate::core::repositories::RepositoryStore::resolve(
                        &self.pool, repo_param,
                    )
                    .await
                    {
                        Ok(Some(repo)) => Some(repo.id.to_string()),
                        Ok(None) => {
                            return Some(format!("Error: Repository '{}' not found. Use manage_repositories with action 'list' to see registered repositories.", repo_param));
                        }
                        Err(e) => {
                            return Some(format!("Error: Failed to look up repository: {}", e));
                        }
                    },
                    None => None,
                };

                // Absent `images` → forward the current message's images (default).
                // Present (even empty) → caller has explicitly chosen the selection.
                let resolved_images: Option<Vec<crate::api::ChatImage>> = match tool_args
                    .get("images")
                {
                    Some(serde_json::Value::Array(arr)) => {
                        let refs: Vec<String> = arr
                            .iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect();
                        if refs.len() != arr.len() {
                            return Some("Error: `images` entries must be strings".to_string());
                        }
                        let events = match self
                            .event_store
                            .get_thread_events(&thread_id.to_string())
                            .await
                        {
                            Ok(evts) => evts,
                            Err(e) => {
                                return Some(format!("Error: Failed to load thread events: {}", e))
                            }
                        };
                        match crate::engine::tools::image::resolve_thread_image_refs(&self.workspace_path, &events, &refs)
                        {
                            Ok(imgs) => Some(imgs),
                            Err(e) => return Some(format!("Error: {}", e)),
                        }
                    }
                    _ => None,
                };

                let caller_title = tool_args["title"].as_str();
                let images = resolved_images
                    .as_deref()
                    .or(user_images)
                    .map(<[crate::api::ChatImage]>::to_vec);
                Some(
                    match self
                        .spawn_agent_thread(crate::engine::claude_code::SpawnAgentThreadParams {
                            prompt: prompt.to_string(),
                            user_images: images,
                            device_id: device_id.map(str::to_string),
                            parent_thread_id,
                            spawning_event_id,
                            repo_id,
                            caller_title: caller_title.map(str::to_string),
                        })
                        .await
                    {
                        Ok(cc_thread_id) => relation.run_claude_success_text(cc_thread_id),
                        Err(e) => format!("Error: Failed to start Claude Code: {}", e),
                    },
                )
            }
        } else if tool_name == tn::RUN_THREAD {
            let prompt = tool_args["prompt"].as_str().unwrap_or("");
            if prompt.is_empty() {
                Some("Error: prompt is required".to_string())
            } else {
                let relation = match parse_relation(tool_args) {
                    Ok(r) => r,
                    Err(e) => return Some(format!("Error: {}", e)),
                };
                let (parent_thread_id, spawning_event_id) =
                    relation.spawn_linkage(thread_id, tool_called_event_id);
                Some(
                    match LucidosEngine::check_thread_recursion_guard(&self.pool, thread_id).await {
                        Err(guard_err) => format!("Error: {}", guard_err),
                        Ok(_) => {
                            let child_thread_id = uuid::Uuid::new_v4();
                            // Without this the spawned loop defaults to LUCIDOS_MODEL
                            // instead of the user's chat-mode preference.
                            let (chat_model, chat_effort) =
                                crate::core::PreferenceStore::user_chat_settings(&self.pool).await;
                            // Eager emit BEFORE the background spawn: a Sub child's
                            // active_children_count must increment before the parent
                            // can complete this turn, otherwise ResponseGenerated wins
                            // the race and the parent flips to "review" before the
                            // child is on the projection. origin=None so
                            // make_message_received synthesizes a ParentThread origin
                            // from mode + parent_thread_id — emits None for a Top
                            // spawn since parent_thread_id is None there.
                            let eager_result = self
                                .event_bus
                                .emit(BusEvent::Thread {
                                    thread_id: child_thread_id,
                                    event: crate::engine::chat::make_message_received(
                                        &self.workspace_path,
                                        prompt,
                                        None,
                                        None,
                                        None,
                                        parent_thread_id,
                                        spawning_event_id,
                                        ActorMode::Agent,
                                        chat_model.as_deref(),
                                        chat_effort.as_deref(),
                                        None,
                                    ),
                                    meta: EventMeta {
                                        channel: Some(
                                            EventChannel::Chat,
                                        ),
                                        ..EventMeta::NONE
                                    },
                                })
                                .await;
                            match eager_result {
                                Ok(Some(emit)) => {
                                    let origin_id = emit.event_id;
                                    let caller_title = tool_args["title"].as_str();
                                    match self.spawn_thread(
                                        prompt,
                                        parent_thread_id,
                                        spawning_event_id,
                                        child_thread_id,
                                        Some(origin_id),
                                        caller_title,
                                        chat_model,
                                        chat_effort,
                                    ) {
                                        Ok(cid) => relation.run_thread_success_text(cid),
                                        Err(e) => format!("Error starting thread: {}", e),
                                    }
                                }
                                Ok(None) => {
                                    "Error: MessageReceived emit returned no result".to_string()
                                }
                                Err(e) => format!("Error starting thread: {}", e),
                            }
                        }
                    },
                )
            }
        } else if let Some((server_id, mcp_tool_name)) =
            crate::mcp::McpManager::parse_mcp_tool_name(tool_name)
        {
            let (auto_approve, server_name) = {
                let statuses = self.mcp_manager.list_servers().await.unwrap_or_default();
                let server = statuses.iter().find(|s| s.id == server_id);
                (
                    server.map(|s| s.auto_approve).unwrap_or(false),
                    server
                        .map(|s| s.name.clone())
                        .unwrap_or_else(|| server_id.clone()),
                )
            };

            let approved = if auto_approve {
                true
            } else {
                let consent_request_id = uuid::Uuid::new_v4().to_string();
                let (tx, rx) = tokio::sync::oneshot::channel();
                {
                    let mut pending = self.pending_mcp_consent.lock().unwrap();
                    pending.insert(consent_request_id.clone(), tx);
                }

                let args_summary = serde_json::to_string_pretty(tool_args)
                    .unwrap_or_else(|_| tool_args.to_string());
                let args_summary = if args_summary.len() > 500 {
                    format!(
                        "{}...",
                        &args_summary[..args_summary.floor_char_boundary(500)]
                    )
                } else {
                    args_summary
                };

                self.event_bus
                    .emit_or_log(
                        BusEvent::Thread {
                            thread_id,
                            event: ThreadEvent::McpConsentRequest {
                                data: serde_json::json!({
                                    "request_id": consent_request_id,
                                    "server_name": server_name,
                                    "tool_name": mcp_tool_name,
                                    "arguments_summary": args_summary,
                                })
                                .to_string(),
                            },
                            meta: EventMeta::NONE,
                        },
                        "[AgenticLoop] McpConsentRequest",
                    )
                    .await;

                match tokio::time::timeout(std::time::Duration::from_secs(30), rx).await {
                    Ok(Ok(allowed)) => allowed,
                    Ok(Err(_)) => false,
                    Err(_) => {
                        let mut pending = self.pending_mcp_consent.lock().unwrap();
                        pending.remove(&consent_request_id);
                        false
                    }
                }
            };

            Some(if approved {
                match self
                    .mcp_manager
                    .call_tool(&server_id, &mcp_tool_name, tool_args.clone())
                    .await
                {
                    Ok((result, _, _)) => result,
                    Err(e) => format!("Error: MCP tool call failed: {}", e),
                }
            } else {
                format!(
                    "Error: User denied MCP tool call '{}' on '{}'",
                    mcp_tool_name, server_name
                )
            })
        } else {
            None
        }
    }

    /// Run an isolated agentic loop for intent execution.
    /// This is a simplified version of run_agentic_loop with its own system prompt,
    /// no conversation history, and all tools except execute_intent (no recursion).
    /// Returns only the final response text.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn run_intent_loop(
        &self,
        system_prompt: &str,
        task: &str,
        request_id: Uuid,
        extraction_ctx: &str,
        device_id: Option<&str>,
        cancel_token: &CancellationToken,
        thread_id: Uuid,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let tools = build_intent_tools();

        let mut messages = vec![Message {
            role: "user".to_string(),
            content: MessageContent::Text(format!("Task: {}", task)),
        }];

        const MAX_INTENT_ITERATIONS: usize = 100;
        let mut iterations = 0;
        let mut last_tool_call: Option<(String, String)> = None;
        let mut consecutive_same_call = 0;

        loop {
            iterations += 1;

            if cancel_token.is_cancelled() {
                return Ok("Intent execution canceled.".to_string());
            }

            if iterations > MAX_INTENT_ITERATIONS {
                return Ok("Intent execution reached iteration limit.".to_string());
            }

            // Trim context if needed
            let message_budget = 400_000; // ~100k tokens budget for intent sub-loops
            trim_context_if_needed(&mut messages, message_budget);

            // Call LLM with no streaming (sub-loop doesn't stream text to frontend)
            let response = self
                .llm
                .chat(
                    messages.clone(),
                    tools.clone(),
                    None, // no model override — use default
                    Some(system_prompt),
                    None, // no token streaming callback
                    None, // no reasoning effort override
                )
                .await?;

            // No tool calls → final answer
            if response.tool_calls.is_empty() {
                return Ok(response.content.unwrap_or_else(|| "Done.".to_string()));
            }

            // Build assistant message with tool_use blocks
            let mut assistant_blocks: Vec<ContentBlock> = Vec::new();
            if let Some(ref text) = response.content {
                assistant_blocks.push(ContentBlock::Text { text: text.clone() });
            }
            for tc in &response.tool_calls {
                assistant_blocks.push(ContentBlock::ToolUse {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    input: tc.arguments.clone(),
                });
            }
            messages.push(Message {
                role: "assistant".to_string(),
                content: MessageContent::Blocks(assistant_blocks),
            });

            // Circuit breaker for consecutive duplicate calls
            if response.tool_calls.len() == 1 {
                let tc = &response.tool_calls[0];
                let call_key = derive_call_key(&tc.name, &tc.arguments);
                let current_call = (tc.name.clone(), call_key);
                if Some(&current_call) == last_tool_call.as_ref() {
                    consecutive_same_call += 1;
                } else {
                    consecutive_same_call = 1;
                    last_tool_call = Some(current_call);
                }

                if consecutive_same_call >= 4 {
                    // Force break — add tool results and stop
                    let result_blocks: Vec<ContentBlock> = response
                        .tool_calls
                        .iter()
                        .map(|tc| ContentBlock::ToolResult {
                            tool_use_id: tc.id.clone(),
                            content: "STOP: Repeated tool call. Give your final answer now."
                                .to_string(),
                        })
                        .collect();
                    messages.push(Message {
                        role: "user".to_string(),
                        content: MessageContent::Blocks(result_blocks),
                    });
                    continue;
                }
            } else {
                consecutive_same_call = 0;
                last_tool_call = None;
            }

            // Execute each tool call
            let mut result_blocks: Vec<ContentBlock> = Vec::new();
            for tc in &response.tool_calls {
                // Emit ToolCalled via bus. Capture the event_id so spawn-style tools
                // can record it as the spawning_event_id of the new thread.
                let mut redacted_args = tc.arguments.clone();
                crate::core::redact_postgres_secrets_in_json(&mut redacted_args);
                let tool_called_event_id = self
                    .event_bus
                    .emit_for_id(BusEvent::Thread {
                        thread_id,
                        event: ThreadEvent::ToolCalled {
                            name: tc.name.clone(),
                            description: self.describe_tool(&tc.name, &tc.arguments),
                            args: redacted_args,
                        },
                        meta: EventMeta::NONE,
                    })
                    .await;

                let outcome: crate::engine::tools::ToolOutcome = if let Some(r) = self
                    .handle_special_tool(
                        &tc.name,
                        &tc.arguments,
                        thread_id,
                        None,
                        device_id,
                        tool_called_event_id,
                    )
                    .await
                {
                    crate::engine::tools::lift_legacy_string(r)
                } else {
                    super::run_tool_with_cancel(
                        self.execute_tool(
                            &tc.name,
                            &tc.arguments,
                            extraction_ctx,
                            request_id,
                            device_id,
                            cancel_token,
                            thread_id,
                        ),
                        cancel_token,
                    )
                    .await
                };

                let (result, is_error) = match outcome {
                    Ok(text) => (text, false),
                    Err(text) => (text, true),
                };
                self.event_bus
                    .emit_or_log(
                        BusEvent::Thread {
                            thread_id,
                            event: ThreadEvent::ToolResult {
                                name: tc.name.clone(),
                                result: crate::core::sanitize_for_jsonb(&result),
                                images: vec![],
                                success: !is_error,
                            },
                            meta: EventMeta::NONE,
                        },
                        "[IntentLoop] ToolResult",
                    )
                    .await;

                result_blocks.push(ContentBlock::ToolResult {
                    tool_use_id: tc.id.clone(),
                    content: result,
                });
            }

            messages.push(Message {
                role: "user".to_string(),
                content: MessageContent::Blocks(result_blocks),
            });
        }
    }
}
