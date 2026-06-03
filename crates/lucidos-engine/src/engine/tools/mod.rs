mod apps;
pub(crate) mod bash;
pub(crate) mod bash_background;
mod browser;
mod bulk_limits;
pub(crate) mod credentials;
mod email;
pub(crate) mod files;
mod http;
pub(crate) mod image;
mod import;
mod mcp;
mod memory;
pub(crate) mod plugins;
mod preferences;
mod proxy;
mod python;
pub(crate) mod scheduler;
pub(crate) mod search;
pub(crate) mod todo;
mod web;

use super::LucidosEngine;
use crate::llm::tool_names as tn;

/// Result of a tool dispatch: `Ok(text)` = success, `Err(text)` = failure.
/// In both cases `text` is what the LLM sees as the tool result; the typed
/// tag is what the agentic loop persists into `ToolResult.success`. Routing
/// failure through `Err` instead of inferring it from a `result.starts_with(
/// "Error:")` prefix keeps the success bit honest when a tool's error string
/// happens to start with `Error reading…` / `Error executing…` / etc. (the
/// pre-typed dispatch silently stamped those as `success: true`).
pub(crate) type ToolOutcome = Result<String, String>;

/// Lift a raw `String` tool result into a `ToolOutcome` using the legacy
/// "starts with `Error:`" convention. Single source for the legacy lift —
/// `to_outcome`, the plugin-tool branch, and the special-tool / read-cache
/// sites in the agentic loops all route through here so the convention can
/// be retired in one place once every tool internally returns typed `Err`.
pub(crate) fn lift_legacy_string(s: String) -> ToolOutcome {
    if s.starts_with("Error:") {
        Err(s)
    } else {
        Ok(s)
    }
}

/// Convert a `Result<String, Box<dyn Error + Send + Sync>>`-returning helper
/// into a `ToolOutcome`. The Err arm is rendered with the canonical
/// `Error: <e>` prefix so the LLM still sees a familiar failure shape; the
/// Ok arm goes through `lift_legacy_string` so legacy `Ok("Error: …")`
/// in-band errors land as typed `Err` until every internal site is migrated.
fn to_outcome(r: Result<String, Box<dyn std::error::Error + Send + Sync>>) -> ToolOutcome {
    match r {
        Ok(s) => lift_legacy_string(s),
        Err(e) => Err(format!("Error: {}", e)),
    }
}

impl LucidosEngine {
    /// Execute a tool call and return its outcome.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn execute_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
        extraction_ctx: &str,
        request_id: uuid::Uuid,
        device_id: Option<&str>,
        cancel_token: &tokio_util::sync::CancellationToken,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        match name {
            tn::READ_FILE
            | tn::WRITE_FILE
            | tn::EDIT_FILE
            | tn::LIST_FILES
            | tn::GLOB_FILES
            | tn::GREP_FILES
            | tn::COPY_FILE
            | tn::DELETE_FILE => to_outcome(self.execute_file_tool(name, args).await),
            tn::BROWSER_OPEN
            | tn::BROWSER_EXTRACT
            | tn::BROWSER_CLICK
            | tn::BROWSER_TYPE
            | tn::BROWSER_EVAL
            | tn::BROWSER_SCREENSHOT
            | tn::BROWSER_CLOSE
            | tn::BROWSER_FORGET_LOGIN
            | tn::BROWSER_CLEAR_DATA => {
                to_outcome(self.execute_browser_tool(name, args, request_id).await)
            }
            tn::SEND_EMAIL
            | tn::READ_EMAILS
            | tn::READ_EMAIL
            | tn::CONFIGURE_EMAIL
            | tn::SAVE_EMAIL_ATTACHMENT => {
                to_outcome(self.execute_email_tool(name, args, request_id).await)
            }
            tn::HTTP_REQUEST => to_outcome(self.execute_http_tool(args).await),
            tn::PROXY_REQUEST => to_outcome(self.execute_proxy_tool(args).await),
            tn::RELOAD_PROXY_MODULES => to_outcome(self.execute_reload_proxy_modules_tool().await),
            tn::IMPORT_FILE | tn::GIT_CLONE => {
                to_outcome(self.execute_import_tool(name, args, extraction_ctx).await)
            }
            tn::CREATE_TRIGGER
            | tn::LIST_TRIGGERS
            | tn::UPDATE_TRIGGER
            | tn::DELETE_TRIGGER
            | tn::PAUSE_TRIGGER
            | tn::RESUME_TRIGGER
            | tn::LIST_TRIGGER_GROUPS
            | tn::CREATE_TRIGGER_GROUP
            | tn::RENAME_TRIGGER_GROUP
            | tn::REORDER_TRIGGER_GROUPS
            | tn::DELETE_TRIGGER_GROUP => to_outcome(self.execute_scheduler_tool(name, args).await),
            tn::SET_LANGUAGE | tn::SET_TIMEZONE | tn::ENABLE_PUSH_NOTIFICATIONS => {
                to_outcome(self.execute_preferences_tool(name, args, device_id).await)
            }
            tn::WEB_SEARCH | tn::FETCH_NEWS => to_outcome(self.execute_web_tool(name, args).await),
            tn::REQUEST_CREDENTIAL | tn::CONNECT_OAUTH_ACCOUNT => {
                to_outcome(self.execute_credential_tool(name, args).await)
            }
            tn::CREATE_APP | tn::LIST_APPS | tn::LOAD_KNOWHOW => {
                to_outcome(self.execute_app_tool(name, args, thread_id).await)
            }
            tn::EXECUTE_INTENT => to_outcome(
                Box::pin(self.handle_execute_intent(
                    args,
                    extraction_ctx,
                    request_id,
                    device_id,
                    cancel_token,
                    thread_id,
                ))
                .await,
            ),
            tn::RUN_PYTHON => to_outcome(self.execute_python_tool(args, thread_id).await),
            tn::RUN_PYTHON_BACKGROUND => {
                self.execute_python_background_tool(args, thread_id).await
            }
            tn::RUN_BASH => to_outcome(self.execute_bash_tool(args, thread_id).await),
            tn::RUN_BASH_BACKGROUND => self.execute_bash_background_tool(args, thread_id).await,
            tn::BASH_OUTPUT => self.execute_bash_output_tool(args, thread_id).await,
            tn::BASH_KILL => self.execute_bash_kill_tool(args).await,
            tn::CORRECT_MEMORY => to_outcome(self.execute_memory_tool(args).await),
            tn::GENERATE_IMAGE => to_outcome(self.execute_generate_image(args, thread_id).await),
            tn::SAVE_THREAD_IMAGE => {
                to_outcome(self.execute_save_thread_image(args, thread_id).await)
            }
            tn::NAVIGATE_UI => self.execute_navigate_ui(args, thread_id).await,
            tn::SEND_NOTIFICATION => self.execute_send_notification(args, thread_id).await,
            tn::READ_NOTIFICATIONS => self.execute_read_notifications(args).await,
            tn::EMIT_EVENT => self.execute_emit_event(args).await,
            tn::QUERY_EVENTS => self.execute_query_events(args).await,
            tn::COUNT_EVENTS => self.execute_count_events(args).await,
            tn::LIST_THREADS => self.execute_list_threads(args).await,
            tn::COUNT_THREADS => self.execute_count_threads(args).await,
            tn::LIST_CHANGES => self.execute_list_changes().await,
            tn::APPLY_CHANGE => self.execute_apply_change(args, thread_id).await,
            tn::DISMISS_FROM_CONTEXT => self.execute_dismiss_from_context(args, thread_id).await,
            tn::TODO_WRITE => self.execute_todo_write(args, thread_id).await,
            tn::MANAGE_REPOSITORIES => self.execute_manage_repositories(args).await,
            tn::INSTALL_PLUGIN
            | tn::CHECK_PLUGIN_UPDATES
            | tn::UPDATE_PLUGIN
            | tn::UNINSTALL_PLUGIN => self.execute_plugin_tool(name, args).await,
            tn::SETUP_MCP_SERVER
            | tn::LIST_MCP_SERVERS
            | tn::START_MCP_SERVER
            | tn::STOP_MCP_SERVER
            | tn::REMOVE_MCP_SERVER => {
                to_outcome(self.execute_mcp_management_tool(name, args).await)
            }
            _ if name.starts_with("mcp__") => {
                // Safety fallback — MCP tools are handled by handle_special_tool() before reaching here
                Err(format!(
                    "Error: MCP tool '{}' must be routed through handle_special_tool()",
                    name
                ))
            }
            _ => Err(format!("Error: Unknown tool: {}", name)),
        }
    }

    /// Thin wrapper that delegates to the standalone
    /// [`todo::todo_write_impl`] so tests can drive the validation branches
    /// without booting a full engine.
    async fn execute_todo_write(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        todo::todo_write_impl(&self.event_bus, args, thread_id).await
    }

    async fn execute_navigate_ui(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        let target = match args.get("target").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => return Err("Error: target is required".to_string()),
        };
        if let Err(e) = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::NavigationRequested {
                    payload: serde_json::to_string(args).unwrap_or_default(),
                },
                meta: crate::engine::thread_events::EventMeta::NONE,
            })
            .await
        {
            return Err(format!("Error: failed to emit NavigationRequested: {}", e));
        }

        // Return contextual help so the LLM knows what the UI offers
        let settings_view = args.get("settings_view").and_then(|v| v.as_str());
        if target == "settings" && settings_view == Some("backup") {
            return Ok("Navigated to Backup & Restore settings. The UI shows:\n\
                - Connected backup providers (e.g. Google Drive)\n\
                - A list of available cloud backups with dates\n\
                - Buttons to create a new backup or restore from an existing one\n\
                - The user's encryption key (needed for restore)\n\
                Tell the user they can pick a backup from the list and restore it directly — \
                no manual downloading or uploading needed."
                .to_string());
        }

        if target == "url" {
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
            if url.is_empty() {
                return Err("Error: url is required when target is 'url'".to_string());
            }
            return Ok(format!(
                "Opened {} in the internal browser panel. The user can now see this page.",
                url
            ));
        }

        Ok(format!("Navigated to {}", target))
    }

    async fn execute_send_notification(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        use crate::scheduler::notifications::Tap;

        let title = match args.get("title").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => return Err("Error: title is required".to_string()),
        };
        let message = match args.get("message").and_then(|v| v.as_str()) {
            Some(m) if !m.is_empty() => m,
            _ => return Err("Error: message is required".to_string()),
        };

        // The notification popover compares `notification.app_id` against the
        // apps list's `id` (the app dir). Only stamp it when the LLM explicitly
        // passes one — never auto-stamp from the trigger's owning app, since
        // most reminders/nudges/summaries shouldn't deep-link even when their
        // trigger lives inside an app dir for organizational reasons.
        let app_id: Option<String> = args
            .get("app_id")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        // Treat missing / null / empty-string the same way: default to Modal.
        // Some LLM providers emit `"tap": null` or `"tap": ""` for unset
        // optionals; both should land on the safe modal default rather than
        // surface a deserialize error to the LLM. The structured `{kind, to?}`
        // object is the only accepted positive shape — `Tap::Deserialize`
        // strictly rejects the legacy bare-string form ("modal" / "open_app" /
        // "open_thread" / "none") and the LLM is documented against the
        // structured shape.
        let tap: Tap = match args.get("tap") {
            None | Some(serde_json::Value::Null) => Tap::Modal,
            Some(serde_json::Value::String(s)) if s.is_empty() => Tap::Modal,
            Some(v) => serde_json::from_value(v.clone()).map_err(|e| {
                format!(
                    "Error: invalid tap {} — expected an object like \
                     {{\"kind\":\"modal\"}}, {{\"kind\":\"none\"}}, or \
                     {{\"kind\":\"navigate\",\"to\":{{\"target\":\"app\",\"app_id\":\"...\"}}}}. \
                     Parse error: {}",
                    v, e
                )
            })?,
        };

        // Optional event_id deep-link target — the LLM passes the row id of
        // the event the user should jump straight to (e.g. the
        // UserQuestionAsked from the triggering event payload). Validated as
        // a UUID; empty/null/missing all mean "no event anchor".
        let link_event: Option<uuid::Uuid> =
            crate::api::parse_optional_uuid_trimmed(args.get("event_id").and_then(|v| v.as_str()))
                .map_err(|raw| format!("Error: event_id is not a valid UUID: {}", raw))?;

        // When this trigger fired in response to a thread-scoped event (e.g.
        // `UserQuestionAsked`), the originating thread lives in a task-local
        // set by `handle_domain_event`. Prefer it as the deep-link target —
        // otherwise the push would point at the trigger LLM's own thread,
        // which the user has no reason to open.
        let link_thread = crate::scheduler::user_tasks::ORIGIN_THREAD_ID
            .try_with(|t| *t)
            .unwrap_or(thread_id);

        match self
            .create_notification(
                title,
                message,
                app_id.as_deref(),
                Some(link_thread),
                link_event,
                tap,
                None,
            )
            .await
        {
            Ok(_) => Ok("Notification sent.".to_string()),
            Err(e) => Err(format!("Error: {}", e)),
        }
    }

    /// Persist a notification to the inbox AND fan it out as a web push.
    ///
    /// Shared between the `send_notification` LLM tool and the
    /// `POST /api/v1/notifications` HTTP route. Both surfaces produce the
    /// same `NotificationCreated` event and the same `send_push_to_all_with_app`
    /// fanout so a script-driven push is indistinguishable from an LLM-driven
    /// one to the recipient.
    ///
    /// `link_thread_id` is the push-payload `thread_id` for tap deep-links.
    /// The "is the user viewing this thread?" suppression is now made live
    /// by the page's PresenceCheck pong (see
    /// `system-knowhow/notifications.md` §3), not by a persisted
    /// projection. The LLM tool path passes the originating thread (via
    /// `ORIGIN_THREAD_ID`, falling back to its own thread). The HTTP path
    /// passes whatever the caller explicitly opted into — typically `None`,
    /// since scripts rarely have a thread context.
    ///
    /// Callers are expected to pre-validate at their boundary (HTTP returns
    /// 400 for empty title/message; the LLM tool returns the same as a tool
    /// error). The `trim().is_empty()` guards here are belt-and-braces so a
    /// future caller can't accidentally publish a blank notification.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_notification(
        &self,
        title: &str,
        message: &str,
        app_id: Option<&str>,
        link_thread_id: Option<uuid::Uuid>,
        link_event_id: Option<uuid::Uuid>,
        tap: crate::scheduler::notifications::Tap,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) -> Result<uuid::Uuid, Box<dyn std::error::Error + Send + Sync>> {
        if title.trim().is_empty() {
            return Err("title is required".into());
        }
        if message.trim().is_empty() {
            return Err("message is required".into());
        }

        let notification_id = uuid::Uuid::new_v4();
        // `tap` is non-Copy (it owns the `NavigateUi.to` strings) so we keep
        // one copy for the emit and another for the spawned push fan-out.
        let tap_for_push = tap.clone();
        self.event_bus
            .emit(crate::engine::event_bus::BusEvent::System(
                crate::engine::event_bus::SystemEvent::NotificationCreated {
                    id: notification_id.to_string(),
                    title: title.to_string(),
                    message: message.to_string(),
                    task_id: None,
                    app_id: app_id.map(str::to_string),
                    thread_id: link_thread_id.map(|t| t.to_string()),
                    event_id: link_event_id.map(|e| e.to_string()),
                    tap,
                    actor,
                },
            ))
            .await
            .map_err(|e| format!("failed to create notification: {}", e))?;

        // Spawned so the create call doesn't block on the PresenceCheck
        // deadline + N web push round-trips — the caller already has the
        // notification id and the SSE NotificationCreated event has
        // fanned out.
        let engine = self.clone_arc();
        let title_owned = title.to_string();
        let message_owned = message.to_string();
        let app_id_owned = app_id.map(str::to_string);
        tokio::spawn(async move {
            crate::scheduler::push::send_push_to_all_with_app(
                &engine,
                &title_owned,
                &message_owned,
                Some(notification_id),
                app_id_owned.as_deref(),
                link_thread_id,
                link_event_id,
                tap_for_push,
            )
            .await;
        });

        Ok(notification_id)
    }

    async fn execute_read_notifications(&self, args: &serde_json::Value) -> ToolOutcome {
        let filter = args
            .get("filter")
            .and_then(|v| v.as_str())
            .unwrap_or("unread");
        let limit = args
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(20)
            .clamp(1, 50);

        let notifications = match crate::scheduler::NotificationStore::get_filtered(
            &self.pool, filter, limit, None,
        )
        .await
        {
            Ok(n) => n,
            Err(e) => return Err(format!("Error: failed to read notifications: {}", e)),
        };

        if notifications.is_empty() {
            return Ok(format!("No {} notifications.", filter));
        }

        let items: Vec<serde_json::Value> = notifications
            .iter()
            .map(|n| {
                serde_json::json!({
                    "id": n.id.to_string(),
                    "title": n.title,
                    "message": n.message,
                    "read": n.read,
                    "created_at": n.created_at.to_rfc3339(),
                    "task_id": n.task_id.map(|id| id.to_string()),
                })
            })
            .collect();

        serde_json::to_string_pretty(&items)
            .map_err(|e| format!("Error: failed to serialise notifications: {}", e))
    }

    async fn execute_emit_event(&self, args: &serde_json::Value) -> ToolOutcome {
        let event_type = match args.get("event_type").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => return Err("Error: event_type is required".to_string()),
        };
        let payload = args
            .get("payload")
            .cloned()
            .unwrap_or(serde_json::json!({}));
        // The agent itself is the actor; attribution flows via the
        // surrounding `MessageReceived` / `ToolCalled` events.
        match self.emit_domain_event(event_type, payload, None).await {
            Ok(id) => Ok(format!("Event {} emitted (id: {})", event_type, id)),
            Err(e) => Err(format!("Error: failed to emit event: {}", e)),
        }
    }

    /// Drop a prior `ToolCalled` (and its matching `ToolResult`) or
    /// `ChildThreadCompleted` from the agent's future resume context. Emits a
    /// `ContextDismissed` event the resume helper honours on every subsequent
    /// assembly. Validates that the referenced event exists in *this* thread
    /// and is one of the dismissible event types — cross-thread dismissals
    /// would let one agent prune another's history, and dismissing arbitrary
    /// events (e.g. ResponseGenerated) would corrupt history rendering.
    async fn execute_dismiss_from_context(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        dismiss_from_context_impl(&self.pool, &self.event_bus, args, thread_id).await
    }

    async fn execute_manage_repositories(&self, args: &serde_json::Value) -> ToolOutcome {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return Err("Error: 'action' is required (add, list, remove)".to_string()),
        };

        match action {
            "list" => match crate::core::repositories::RepositoryStore::list(&self.pool).await {
                Ok(repos) if repos.is_empty() => Ok("No repositories registered.".to_string()),
                Ok(repos) => {
                    let mut out = format!("{} registered repositories:\n", repos.len());
                    for r in &repos {
                        out.push_str(&format!("- **{}** — `{}`", r.name, r.path));
                        if let Some(ref desc) = r.description {
                            out.push_str(&format!(" ({})", desc));
                        }
                        out.push('\n');
                    }
                    Ok(out)
                }
                Err(e) => Err(format!("Error: failed to list repositories: {}", e)),
            },
            "add" => {
                let name = match args.get("name").and_then(|v| v.as_str()) {
                    Some(n) if !n.is_empty() => n,
                    _ => return Err("Error: 'name' is required for 'add' action".to_string()),
                };
                let path = match args.get("path").and_then(|v| v.as_str()) {
                    Some(p) if !p.is_empty() => p,
                    _ => return Err("Error: 'path' is required for 'add' action".to_string()),
                };

                // Expand ~/
                let expanded = if let Some(rest) = path.strip_prefix("~/") {
                    if let Ok(home) = std::env::var("HOME") {
                        format!("{}/{}", home, rest)
                    } else {
                        path.to_string()
                    }
                } else {
                    path.to_string()
                };

                // Validate path exists and is a git repo
                if !std::path::Path::new(&expanded).exists() {
                    return Err(format!("Error: path does not exist: {}", expanded));
                }

                let git_check = tokio::process::Command::new("git")
                    .args(["rev-parse", "--git-dir"])
                    .current_dir(&expanded)
                    .output()
                    .await;
                match git_check {
                    Ok(o) if !o.status.success() => {
                        return Err(format!("Error: not a git repository: {}", expanded));
                    }
                    Err(e) => return Err(format!("Error: failed to check git repo: {}", e)),
                    _ => {}
                }

                let desc = args.get("description").and_then(|v| v.as_str());
                match crate::core::repositories::RepositoryStore::add(
                    &self.pool, name, &expanded, desc,
                )
                .await
                {
                    Ok(repo) => Ok(format!(
                        "Repository '{}' registered at `{}`",
                        repo.name, repo.path
                    )),
                    Err(e) => Err(format!("Error: failed to add repository: {}", e)),
                }
            }
            "remove" => {
                let name = match args.get("name").and_then(|v| v.as_str()) {
                    Some(n) if !n.is_empty() => n,
                    _ => return Err("Error: 'name' is required for 'remove' action".to_string()),
                };

                match crate::core::repositories::RepositoryStore::get_by_name(&self.pool, name).await {
                    Ok(Some(repo)) => {
                        match crate::core::repositories::RepositoryStore::remove(
                            &self.pool, repo.id,
                        ).await {
                            Ok(true) => Ok(format!("Repository '{}' removed", name)),
                            Ok(false) => Err(format!(
                                "Error: repository '{}' not found at remove time",
                                name
                            )),
                            Err(e) => Err(format!("Error: failed to remove repository: {}", e)),
                        }
                    }
                    Ok(None) => Err(format!(
                        "Error: no repository found with name '{}'. Use action 'list' to see registered repos.",
                        name
                    )),
                    Err(e) => Err(format!("Error: failed to look up repository: {}", e)),
                }
            }
            other => Err(format!(
                "Error: unknown action '{}'. Use 'add', 'list', or 'remove'.",
                other
            )),
        }
    }

    async fn execute_query_events(&self, args: &serde_json::Value) -> ToolOutcome {
        let event_type = args.get("event_type").and_then(|v| v.as_str());
        let since = args
            .get("since")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));
        let until = args
            .get("until")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));
        let limit = args
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(query_events_limit::DEFAULT)
            .clamp(query_events_limit::MIN, query_events_limit::MAX);
        let byte_limit = args
            .get("byte_limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(query_events_byte_budget::DEFAULT)
            .clamp(query_events_byte_budget::MIN, query_events_byte_budget::MAX);

        match self
            .event_store
            .query_events(event_type, since, until, limit)
            .await
        {
            Ok(events) => Ok(build_query_events_response(&events, byte_limit)),
            Err(e) => Err(format!("Error: failed to query events: {}", e)),
        }
    }

    /// LLM tool: per-`event_type` count + byte total over the same time
    /// filters as `query_events`. Mirrors `count_threads` shape-wise.
    /// Without an `event_type` filter, returns the per-type breakdown sorted
    /// by count desc — the "what's noisy this week" view that a sweep recipe
    /// (workspace-learning, workspace-audit) should call before drilling.
    async fn execute_count_events(&self, args: &serde_json::Value) -> ToolOutcome {
        let event_type = args.get("event_type").and_then(|v| v.as_str());
        let since = args
            .get("since")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));
        let until = args
            .get("until")
            .and_then(|v| v.as_str())
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc));

        if let Some(et) = event_type {
            match self.event_store.count_events(Some(et), since, until).await {
                Ok((count, byte_total)) => Ok(serde_json::json!({
                    "count": count,
                    "byte_total": byte_total,
                })
                .to_string()),
                Err(e) => Err(format!("Error: failed to count events: {}", e)),
            }
        } else {
            match self.event_store.count_events_by_type(since, until).await {
                Ok(rows) => {
                    let total_count: i64 = rows.iter().map(|(_, c, _)| *c).sum();
                    let total_byte_total: i64 = rows.iter().map(|(_, _, b)| *b).sum();
                    let by_type: Vec<serde_json::Value> = rows
                        .into_iter()
                        .map(|(et, count, byte_total)| {
                            serde_json::json!({
                                "event_type": et,
                                "count": count,
                                "byte_total": byte_total,
                            })
                        })
                        .collect();
                    Ok(serde_json::json!({
                        "by_type": by_type,
                        "total_count": total_count,
                        "total_byte_total": total_byte_total,
                    })
                    .to_string())
                }
                Err(e) => Err(format!("Error: failed to count events: {}", e)),
            }
        }
    }

    /// LLM tool: list thread summaries for the workspace. Mirrors
    /// `GET /api/v1/threads/list` and `lucidos threads list`.
    async fn execute_list_threads(&self, args: &serde_json::Value) -> ToolOutcome {
        let active = args.get("active").and_then(|v| v.as_bool());
        let sources = parse_source_arg(args.get("source"));
        let limit = args
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(100)
            .clamp(1, 1000);
        match self
            .event_store
            .list_thread_summaries(crate::core::store::ThreadSummaryFilters {
                active,
                sources: sources.as_deref(),
                limit,
            })
            .await
        {
            // Compact JSON — every tool that returns a JSON array does the
            // same (see `query_events`, `count_threads`). Pretty-printing
            // would inflate output tokens ~30% with no parsing benefit for
            // the LLM.
            Ok(summaries) => serde_json::to_string(&summaries)
                .map_err(|e| format!("Error: failed to serialise thread summaries: {}", e)),
            Err(e) => Err(format!("Error: failed to list thread summaries: {}", e)),
        }
    }

    /// LLM tool: count thread summaries matching the same filters as
    /// `list_threads`. Returns `{ "count": N }`.
    async fn execute_count_threads(&self, args: &serde_json::Value) -> ToolOutcome {
        let active = args.get("active").and_then(|v| v.as_bool());
        let sources = parse_source_arg(args.get("source"));
        match self
            .event_store
            .count_thread_summaries(crate::core::store::ThreadSummaryFilters {
                active,
                sources: sources.as_deref(),
                limit: 0,
            })
            .await
        {
            Ok(count) => Ok(serde_json::json!({ "count": count }).to_string()),
            Err(e) => Err(format!("Error: failed to count thread summaries: {}", e)),
        }
    }

    /// LLM tool: list pending + recently-applied *changes* (coding-agent
    /// branches awaiting Apply). The in-thread mirror of `GET /api/v1/changes`
    /// and `lucidos changes list` — calls the same projection reads in-process
    /// rather than shelling out to the CLI. Read-only.
    async fn execute_list_changes(&self) -> ToolOutcome {
        let proj = self.changes();
        let mut pending = proj
            .list_pending()
            .await
            .map_err(|e| format!("Error: failed to list pending changes: {}", e))?;
        // A small applied window gives the LLM enough recent history to confirm
        // a just-applied change without flooding the context with the full log.
        let mut applied = proj
            .list_recently_applied(10, None)
            .await
            .map_err(|e| format!("Error: failed to list applied changes: {}", e))?;
        crate::core::changes::enrich_thread_titles(self.pool(), &mut pending)
            .await
            .map_err(|e| format!("Error: failed to enrich pending change titles: {}", e))?;
        crate::core::changes::enrich_thread_titles(self.pool(), &mut applied)
            .await
            .map_err(|e| format!("Error: failed to enrich applied change titles: {}", e))?;
        let total_pending = pending.len();
        // Compact JSON — same convention as list_threads / query_events.
        serde_json::to_string(&serde_json::json!({
            "pending": pending,
            "applied": applied,
            "total_pending": total_pending,
        }))
        .map_err(|e| format!("Error: failed to serialise changes: {}", e))
    }

    /// LLM tool: apply a pending *change* — merge the coding-agent branch into
    /// main, exactly as the Apply button does. Calls the shared
    /// `LucidosEngine::apply_change` pipeline in-process (which handles the
    /// /harden gate, restart gating, conflict recovery, `ChangeApplied` emit,
    /// and projection broadcast) — no apply logic is duplicated here.
    async fn execute_apply_change(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        let change_id = parse_apply_change_id(args)?;
        // The agent in THIS thread drove the apply. `ThreadLink { mode: Agent }`
        // is the in-process analog of the CLI's `Api { mode: Agent,
        // source_thread_id }`: it stamps an Agent attribution on the
        // `ChangeApplied` event (emitted on the *proposing* thread's timeline)
        // that deep-links back to the thread whose agent applied it — so the
        // route popover never mislabels an agent apply as "You". `direction:
        // Parent` because the dominant flow is a chat thread applying the change
        // of a coding-agent thread it spawned (the proposing thread is the
        // child; this thread is its parent).
        let actor = crate::engine::thread_events::MessageOrigin::ThreadLink {
            thread_id,
            title: None,
            spawning_event_id: None,
            mode: crate::engine::thread_events::ActorMode::Agent,
            direction: crate::engine::thread_events::ThreadDirection::Parent,
        };
        // `apply_change` takes `&Arc<Self>`; the tool handler only has `&self`.
        let engine = self.clone_arc();
        match engine.apply_change(change_id, Some(actor)).await {
            // Echo the typed ApplyResult verbatim so the LLM sees status,
            // SHAs, restart_required, and any conflict/review thread ids.
            Ok(result) => serde_json::to_string(&result)
                .map_err(|e| format!("Error: failed to serialise apply result: {}", e)),
            Err(e) => Err(format!("Error: failed to apply change: {}", e)),
        }
    }
}

/// Parse the required `change_id` UUID arg for the `apply_change` tool. Pure
/// so the validation branches are unit-testable without booting an engine
/// (same pattern as `dismiss_from_context_impl`). Missing / null / empty /
/// whitespace-only all collapse to "required"; a non-UUID string is rejected
/// before the heavyweight merge pipeline runs.
pub(crate) fn parse_apply_change_id(args: &serde_json::Value) -> Result<uuid::Uuid, String> {
    let raw = match args.get("change_id").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => s.trim(),
        _ => return Err("Error: change_id is required".to_string()),
    };
    uuid::Uuid::parse_str(raw)
        .map_err(|_| format!("Error: change_id is not a valid UUID: {}", raw))
}

/// Byte-budget bounds for the `query_events` LLM tool. The default cap
/// (128 KB of compact JSON) keeps a single tool result well under the
/// model's per-turn token budget — a busy `ToolResult` query in a real
/// workspace can easily return 2 MB+ (~500k tokens), which blows the
/// prompt cap on the next turn. Even at the default, a recipe that
/// chains 8 sweep calls in one turn accumulates ~1 MB of tool-result
/// content. The MAX (512 KB) is the rare-case ceiling — the LLM may
/// override via `byte_limit` within these bounds, but should narrow the
/// query (aggregate_id, tighter `since/until`) before bumping the cap.
///
/// History: bounds tightened from {DEFAULT=256K, MAX=1M} after a weekly
/// workspace-learning trigger sent 1.54M tokens to a 1M-cap Opus API
/// and crashed with `prompt is too long`. Eight `query_events` calls at
/// the old 256K default totalled ~2MB of tool results in the LLM
/// context, and `chars/4` estimation undercounted by ~2.4×.
pub(crate) mod query_events_byte_budget {
    pub const DEFAULT: i64 = 128 * 1024;
    pub const MIN: i64 = 1024;
    pub const MAX: i64 = 512 * 1024;
}

/// Row-count bounds for the `query_events` LLM tool. The default of 50
/// matches the `workspace-learning` recipe's "sampling, not enumeration"
/// rule. The MAX of 200 leaves room for a deliberate full-window pull
/// on a small event type (e.g. `EngineSupervisorRespawned` over a year)
/// without enabling the abuse pattern that crashed the May 25 trigger
/// (single calls at `limit: 300/500` for high-byte-per-row types).
pub(crate) mod query_events_limit {
    pub const DEFAULT: i64 = 50;
    pub const MIN: i64 = 1;
    pub const MAX: i64 = 200;
}

/// Serialise events to compact JSON, stopping when the next event would
/// push the cumulative size over `byte_limit`. Always returns a wrapper
/// `{events, total_matching, returned, byte_size, truncated, hint?}` so
/// the LLM can see whether it got the full result and how to narrow if
/// not.
///
/// Guarantee: even on `Vec` size > 0 with `byte_limit < first_event_size`
/// the response is still valid — `events` will be empty and `truncated`
/// will be true, telling the LLM to bump `byte_limit` or narrow the
/// query.
pub(crate) fn build_query_events_response(
    events: &[crate::core::EventRow],
    byte_limit: i64,
) -> String {
    let mut included: Vec<serde_json::Value> = Vec::new();
    let mut running: i64 = 0;
    for row in events {
        let Ok(val) = serde_json::to_value(row) else {
            continue;
        };
        let bytes = serde_json::to_string(&val).map(|s| s.len()).unwrap_or(0) as i64;
        let next = running.saturating_add(bytes);
        if next > byte_limit {
            // Stop on the first event that wouldn't fit. If it's the first
            // event overall, we still return an empty list with truncated=true
            // so the LLM knows to bump byte_limit or narrow the query.
            break;
        }
        included.push(val);
        running = next;
    }

    let total_matching = events.len();
    let returned = included.len();
    let truncated = returned < total_matching;
    let mut wrapper = serde_json::json!({
        "events": included,
        "total_matching": total_matching,
        "returned": returned,
        "byte_size": running,
        "truncated": truncated,
    });
    if truncated {
        if let Some(obj) = wrapper.as_object_mut() {
            obj.insert(
                "hint".into(),
                serde_json::Value::String(
                    "result truncated to fit byte_limit. Narrow by aggregate_id, \
                     shorten the time window with since/until, or call count_events \
                     first to size the sweep before drilling. Do not retry with a \
                     larger byte_limit unless you have already narrowed the query."
                        .into(),
                ),
            );
        }
    }
    wrapper.to_string()
}

/// Parse the `source` arg accepted by `list_threads` / `count_threads`.
/// The LLM may emit a comma-separated string (`"chat,trigger"`) or a JSON
/// array of strings (`["chat", "trigger"]`). Empty results collapse to
/// `None` so the store helper's "no filter" branch fires.
fn parse_source_arg(raw: Option<&serde_json::Value>) -> Option<Vec<String>> {
    let v = raw?;
    let out: Vec<String> = if let Some(s) = v.as_str() {
        split_csv(s)
    } else if let Some(arr) = v.as_array() {
        arr.iter()
            .filter_map(|x| x.as_str())
            .map(|s| s.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect()
    } else {
        return None;
    };
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Split a comma-separated string, trimming each part and dropping
/// empties. Same semantics as `api::threads::parse_csv` — kept in this
/// module so the LLM-tool path doesn't pull a `pub(super)` symbol across
/// crate-internal layers.
fn split_csv(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

/// Validation core for the `dismiss_from_context` tool, factored out of the
/// `LucidosEngine` impl so unit tests can exercise the parsing / event-type /
/// thread-scope branches against a real Postgres pool without booting the
/// full engine. The handler on `LucidosEngine` is now a thin wrapper.
///
/// Accepts the event_id as either:
/// - bare UUID (hyphenated `xxxxxxxx-xxxx-...` or simple `xxxxxxxx...`), or
/// - the `evt-<uuid>` form rendered as `tool_use_id` in resumed tool blocks
///   (see [`synthesize_tool_use_id`](crate::core::store::messages)).
pub(crate) async fn dismiss_from_context_impl(
    pool: &sqlx::PgPool,
    event_bus: &crate::engine::event_bus::EventBus,
    args: &serde_json::Value,
    thread_id: uuid::Uuid,
) -> ToolOutcome {
    // Errors flow through the typed `Err` arm — the agentic loop persists
    // ToolResult.success from the Result tag, not from a string prefix.
    let event_id_str = match args.get("event_id").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => s.trim(),
        _ => return Err("Error: event_id is required".to_string()),
    };
    // Accept the `evt-<uuid>` form the LLM sees as tool_use_id in resumed
    // tool blocks; `Uuid::parse_str` then handles either hyphenated or
    // simple-form UUID after the prefix is stripped.
    let stripped = event_id_str.strip_prefix("evt-").unwrap_or(event_id_str);
    let event_id = match uuid::Uuid::parse_str(stripped) {
        Ok(u) => u,
        Err(_) => {
            return Err(
                "Error: event_id must be a UUID (or 'evt-<uuid>' as shown in tool blocks)"
                    .to_string(),
            );
        }
    };
    // Lookup must scope to (event_id, thread_id) AND restrict event_type
    // to the dismissible set — otherwise a typo or hallucinated id silently
    // succeeds, leaving phantom ContextDismissed rows the resume helper
    // would happily honour.
    let row: Option<(String,)> = match sqlx::query_as(
        "SELECT event_type FROM events \
         WHERE id = $1 AND aggregate_id = $2 \
         AND event_type IN ('ToolCalled', 'ChildThreadCompleted')",
    )
    .bind(event_id)
    .bind(thread_id.to_string())
    .fetch_optional(pool)
    .await
    {
        Ok(opt) => opt,
        Err(e) => {
            crate::log!(
                "[DismissFromContext] DB lookup failed for thread={} event={}: {}",
                thread_id,
                event_id,
                e
            );
            return Err(format!("Error: failed to look up event: {}", e));
        }
    };
    if row.is_none() {
        return Err("Error: event id not found or not dismissible".to_string());
    }

    if let Err(e) = event_bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: crate::engine::thread_events::ThreadEvent::ContextDismissed {
                dismissed_event_id: event_id,
            },
            meta: crate::engine::thread_events::EventMeta::NONE,
        })
        .await
    {
        return Err(format!("Error: failed to emit ContextDismissed: {}", e));
    }
    Ok(format!(
        "Dismissed event {} from future resume context.",
        event_id
    ))
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
