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
fn to_outcome(
    r: Result<String, Box<dyn std::error::Error + Send + Sync>>,
) -> ToolOutcome {
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
            tn::RELOAD_PROXY_MODULES => {
                to_outcome(self.execute_reload_proxy_modules_tool().await)
            }
            tn::IMPORT_FILE | tn::GIT_CLONE => {
                to_outcome(self.execute_import_tool(name, args, extraction_ctx).await)
            }
            tn::CREATE_TRIGGER
            | tn::LIST_TRIGGERS
            | tn::UPDATE_TRIGGER
            | tn::DELETE_TRIGGER
            | tn::PAUSE_TRIGGER
            | tn::RESUME_TRIGGER => {
                to_outcome(self.execute_scheduler_tool(name, args).await)
            }
            tn::SET_LANGUAGE | tn::SET_TIMEZONE | tn::ENABLE_PUSH_NOTIFICATIONS => {
                to_outcome(self.execute_preferences_tool(name, args, device_id).await)
            }
            tn::WEB_SEARCH | tn::FETCH_NEWS => {
                to_outcome(self.execute_web_tool(name, args).await)
            }
            tn::REQUEST_CREDENTIAL | tn::CONNECT_OAUTH_ACCOUNT => {
                to_outcome(self.execute_credential_tool(name, args).await)
            }
            tn::CREATE_APP | tn::LIST_APPS | tn::LOAD_KNOWHOW => {
                to_outcome(self.execute_app_tool(name, args).await)
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
            tn::RUN_PYTHON => to_outcome(self.execute_python_tool(args).await),
            tn::RUN_BASH => to_outcome(self.execute_bash_tool(args).await),
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
            tn::DISMISS_FROM_CONTEXT => {
                self.execute_dismiss_from_context(args, thread_id).await
            }
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

        // When this trigger fired in response to a thread-scoped event (e.g.
        // `UserQuestionAsked`), the originating thread lives in a task-local
        // set by `handle_domain_event`. Prefer it as the deep-link target —
        // otherwise the push would point at the trigger LLM's own thread,
        // which the user has no reason to open.
        let link_thread = crate::scheduler::user_tasks::ORIGIN_THREAD_ID
            .try_with(|t| *t)
            .unwrap_or(thread_id);

        let notification_id = uuid::Uuid::new_v4();
        if let Err(e) = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::System(
                crate::engine::event_bus::SystemEvent::NotificationCreated {
                    id: notification_id.to_string(),
                    title: title.to_string(),
                    message: message.to_string(),
                    task_id: None,
                    app_id: app_id.clone(),
                    thread_id: Some(link_thread.to_string()),
                },
            ))
            .await
        {
            return Err(format!("Error: failed to create notification: {}", e));
        }

        // `link_thread` does double duty: suppress pushes to devices already
        // viewing the thread, and deep-link recipients straight to it on tap.
        crate::scheduler::push::send_push_to_all_with_app(
            &self.pool,
            title,
            message,
            Some(notification_id),
            app_id.as_deref(),
            Some(link_thread),
        )
        .await;

        Ok("Notification sent.".to_string())
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
        match self.emit_domain_event(event_type, payload).await {
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
            .unwrap_or(100)
            .clamp(1, 1000);

        match self
            .event_store
            .query_events(event_type, since, until, limit)
            .await
        {
            Ok(events) => serde_json::to_string_pretty(&events)
                .map_err(|e| format!("Error: failed to serialise events: {}", e)),
            Err(e) => Err(format!("Error: failed to query events: {}", e)),
        }
    }
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
