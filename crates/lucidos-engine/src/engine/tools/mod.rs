mod apps;
mod bash;
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
mod plugins;
mod preferences;
mod proxy;
mod python;
pub(crate) mod scheduler;
pub(crate) mod search;
mod web;

use super::LucidosEngine;
use crate::llm::tool_names as tn;

impl LucidosEngine {
    /// Execute a tool call and return the result string.
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
    ) -> String {
        match name {
            tn::READ_FILE
            | tn::WRITE_FILE
            | tn::EDIT_FILE
            | tn::LIST_FILES
            | tn::GLOB_FILES
            | tn::GREP_FILES
            | tn::COPY_FILE
            | tn::DELETE_FILE => self
                .execute_file_tool(name, args)
                .await
                .unwrap_or_else(|e| format!("Error: {}", e)),
            tn::BROWSER_OPEN
            | tn::BROWSER_EXTRACT
            | tn::BROWSER_CLICK
            | tn::BROWSER_TYPE
            | tn::BROWSER_EVAL
            | tn::BROWSER_SCREENSHOT
            | tn::BROWSER_CLOSE
            | tn::BROWSER_FORGET_LOGIN
            | tn::BROWSER_CLEAR_DATA => self
                .execute_browser_tool(name, args, request_id)
                .await
                .unwrap_or_else(|e| format!("Error: {}", e)),
            tn::SEND_EMAIL
            | tn::READ_EMAILS
            | tn::READ_EMAIL
            | tn::CONFIGURE_EMAIL
            | tn::SAVE_EMAIL_ATTACHMENT => self
                .execute_email_tool(name, args, request_id)
                .await
                .unwrap_or_else(|e| format!("Error: {}", e)),
            tn::HTTP_REQUEST => self
                .execute_http_tool(args)
                .await
                .unwrap_or_else(|e| format!("Error: {}", e)),
            tn::PROXY_REQUEST => self
                .execute_proxy_tool(args)
                .await
                .unwrap_or_else(|e| format!("Error: {}", e)),
            tn::IMPORT_FILE | tn::GIT_CLONE => self
                .execute_import_tool(name, args, extraction_ctx)
                .await
                .unwrap_or_else(|e| format!("Error: {}", e)),
            tn::CREATE_TRIGGER
            | tn::LIST_TRIGGERS
            | tn::UPDATE_TRIGGER
            | tn::DELETE_TRIGGER
            | tn::PAUSE_TRIGGER
            | tn::RESUME_TRIGGER => self
                .execute_scheduler_tool(name, args)
                .await
                .unwrap_or_else(|e| format!("Error: {}", e)),
            tn::SET_LANGUAGE | tn::SET_TIMEZONE | tn::ENABLE_PUSH_NOTIFICATIONS => self
                .execute_preferences_tool(name, args, device_id)
                .await
                .unwrap_or_else(|e| format!("Error: {}", e)),
            tn::WEB_SEARCH | tn::FETCH_NEWS => self
                .execute_web_tool(name, args)
                .await
                .unwrap_or_else(|e| format!("Error: {}", e)),
            tn::REQUEST_CREDENTIAL | tn::CONNECT_OAUTH_ACCOUNT => self
                .execute_credential_tool(name, args)
                .await
                .unwrap_or_else(|e| format!("Error: {}", e)),
            tn::CREATE_APP | tn::LIST_APPS | tn::LOAD_KNOWHOW => self
                .execute_app_tool(name, args)
                .await
                .unwrap_or_else(|e| format!("Error: {}", e)),
            tn::EXECUTE_INTENT => Box::pin(self.handle_execute_intent(
                args,
                extraction_ctx,
                request_id,
                device_id,
                cancel_token,
                thread_id,
            ))
            .await
            .unwrap_or_else(|e| format!("Error: {}", e)),
            tn::RUN_PYTHON => self
                .execute_python_tool(args)
                .await
                .unwrap_or_else(|e| format!("Error: {}", e)),
            tn::RUN_BASH => self
                .execute_bash_tool(args)
                .await
                .unwrap_or_else(|e| format!("Error: {}", e)),
            tn::CORRECT_MEMORY => self
                .execute_memory_tool(args)
                .await
                .unwrap_or_else(|e| format!("Error: {}", e)),
            tn::GENERATE_IMAGE => self
                .execute_generate_image(args, thread_id)
                .await
                .unwrap_or_else(|e| format!("Error: {}", e)),
            tn::SAVE_THREAD_IMAGE => self
                .execute_save_thread_image(args, thread_id)
                .await
                .unwrap_or_else(|e| format!("Error: {}", e)),
            tn::NAVIGATE_UI => self.execute_navigate_ui(args, thread_id),
            tn::SEND_NOTIFICATION => self.execute_send_notification(args, thread_id).await,
            tn::READ_NOTIFICATIONS => self.execute_read_notifications(args).await,
            tn::EMIT_EVENT => self.execute_emit_event(args).await,
            tn::QUERY_EVENTS => self.execute_query_events(args).await,
            tn::MANAGE_REPOSITORIES => self.execute_manage_repositories(args).await,
            tn::INSTALL_PLUGIN
            | tn::CHECK_PLUGIN_UPDATES
            | tn::UPDATE_PLUGIN
            | tn::UNINSTALL_PLUGIN => self.execute_plugin_tool(name, args).await,
            tn::SETUP_MCP_SERVER
            | tn::LIST_MCP_SERVERS
            | tn::START_MCP_SERVER
            | tn::STOP_MCP_SERVER
            | tn::REMOVE_MCP_SERVER => self
                .execute_mcp_management_tool(name, args)
                .await
                .unwrap_or_else(|e| format!("Error: {}", e)),
            _ if name.starts_with("mcp__") => {
                // Safety fallback — MCP tools are handled by handle_special_tool() before reaching here
                format!(
                    "Error: MCP tool '{}' must be routed through handle_special_tool()",
                    name
                )
            }
            _ => format!("Unknown tool: {}", name),
        }
    }

    fn execute_navigate_ui(&self, args: &serde_json::Value, thread_id: uuid::Uuid) -> String {
        let target = match args.get("target").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => return "Error: target is required".to_string(),
        };
        let _ = self
            .event_bus
            .sender()
            .send(crate::engine::event_bus::EmittedEvent {
                event_id: uuid::Uuid::new_v4(),
                seq: None,
                created: chrono::Utc::now(),
                typed: crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::NavigationRequested {
                        payload: serde_json::to_string(args).unwrap_or_default(),
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                aggregate: None,
            });

        // Return contextual help so the LLM knows what the UI offers
        let settings_view = args.get("settings_view").and_then(|v| v.as_str());
        if target == "settings" && settings_view == Some("backup") {
            return "Navigated to Backup & Restore settings. The UI shows:\n\
                - Connected backup providers (e.g. Google Drive)\n\
                - A list of available cloud backups with dates\n\
                - Buttons to create a new backup or restore from an existing one\n\
                - The user's encryption key (needed for restore)\n\
                Tell the user they can pick a backup from the list and restore it directly — \
                no manual downloading or uploading needed."
                .to_string();
        }

        if target == "url" {
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
            if url.is_empty() {
                return "Error: url is required when target is 'url'".to_string();
            }
            return format!(
                "Opened {} in the internal browser panel. The user can now see this page.",
                url
            );
        }

        format!("Navigated to {}", target)
    }

    async fn execute_send_notification(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> String {
        let title = match args.get("title").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => return "Error: title is required".to_string(),
        };
        let message = match args.get("message").and_then(|v| v.as_str()) {
            Some(m) if !m.is_empty() => m,
            _ => return "Error: message is required".to_string(),
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
                },
            ))
            .await
        {
            return format!("Error creating notification: {}", e);
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

        "Notification sent.".to_string()
    }

    async fn execute_read_notifications(&self, args: &serde_json::Value) -> String {
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
            Err(e) => return format!("Error reading notifications: {}", e),
        };

        if notifications.is_empty() {
            return format!("No {} notifications.", filter);
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
            .unwrap_or_else(|e| format!("Error serializing notifications: {}", e))
    }

    async fn execute_emit_event(&self, args: &serde_json::Value) -> String {
        let event_type = match args.get("event_type").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => return "Error: event_type is required".to_string(),
        };
        let payload = args
            .get("payload")
            .cloned()
            .unwrap_or(serde_json::json!({}));
        match self.emit_domain_event(event_type, payload).await {
            Ok(id) => format!("Event {} emitted (id: {})", event_type, id),
            Err(e) => format!("Error emitting event: {}", e),
        }
    }

    async fn execute_manage_repositories(&self, args: &serde_json::Value) -> String {
        let action = match args.get("action").and_then(|v| v.as_str()) {
            Some(a) => a,
            None => return "Error: 'action' is required (add, list, remove)".to_string(),
        };

        match action {
            "list" => match crate::core::repositories::RepositoryStore::list(&self.pool).await {
                Ok(repos) if repos.is_empty() => "No repositories registered.".to_string(),
                Ok(repos) => {
                    let mut out = format!("{} registered repositories:\n", repos.len());
                    for r in &repos {
                        out.push_str(&format!("- **{}** — `{}`", r.name, r.path));
                        if let Some(ref desc) = r.description {
                            out.push_str(&format!(" ({})", desc));
                        }
                        out.push('\n');
                    }
                    out
                }
                Err(e) => format!("Error listing repositories: {}", e),
            },
            "add" => {
                let name = match args.get("name").and_then(|v| v.as_str()) {
                    Some(n) if !n.is_empty() => n,
                    _ => return "Error: 'name' is required for 'add' action".to_string(),
                };
                let path = match args.get("path").and_then(|v| v.as_str()) {
                    Some(p) if !p.is_empty() => p,
                    _ => return "Error: 'path' is required for 'add' action".to_string(),
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
                    return format!("Error: path does not exist: {}", expanded);
                }

                let git_check = tokio::process::Command::new("git")
                    .args(["rev-parse", "--git-dir"])
                    .current_dir(&expanded)
                    .output()
                    .await;
                match git_check {
                    Ok(o) if !o.status.success() => {
                        return format!("Error: not a git repository: {}", expanded);
                    }
                    Err(e) => return format!("Error checking git repo: {}", e),
                    _ => {}
                }

                let desc = args.get("description").and_then(|v| v.as_str());
                match crate::core::repositories::RepositoryStore::add(
                    &self.pool, name, &expanded, desc,
                )
                .await
                {
                    Ok(repo) => {
                        format!("Repository '{}' registered at `{}`", repo.name, repo.path)
                    }
                    Err(e) => format!("Error adding repository: {}", e),
                }
            }
            "remove" => {
                let name = match args.get("name").and_then(|v| v.as_str()) {
                    Some(n) if !n.is_empty() => n,
                    _ => return "Error: 'name' is required for 'remove' action".to_string(),
                };

                match crate::core::repositories::RepositoryStore::get_by_name(&self.pool, name).await {
                    Ok(Some(repo)) => {
                        match crate::core::repositories::RepositoryStore::remove(
                            &self.pool, repo.id,
                        ).await {
                            Ok(true) => format!("Repository '{}' removed", name),
                            Ok(false) => format!("Repository '{}' not found", name),
                            Err(e) => format!("Error removing repository: {}", e),
                        }
                    }
                    Ok(None) => format!(
                        "No repository found with name '{}'. Use action 'list' to see registered repos.",
                        name
                    ),
                    Err(e) => format!("Error looking up repository: {}", e),
                }
            }
            other => format!(
                "Unknown action '{}'. Use 'add', 'list', or 'remove'.",
                other
            ),
        }
    }

    async fn execute_query_events(&self, args: &serde_json::Value) -> String {
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
                .unwrap_or_else(|e| format!("Error serializing events: {}", e)),
            Err(e) => format!("Error querying events: {}", e),
        }
    }
}
