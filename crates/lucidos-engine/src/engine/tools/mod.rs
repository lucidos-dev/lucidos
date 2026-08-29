mod apps;
pub(crate) mod bash;
pub(crate) mod bash_background;
pub(crate) mod bash_background_recovery;
pub(crate) mod browser;
mod bulk_limits;
mod capabilities;
pub(crate) mod credentials;
mod email;
mod env_vars;
pub(crate) mod files;
mod grouped;
mod http;
pub(crate) mod image;
mod import;
mod mcp;
mod memory;
mod models;
mod notifications;
pub(crate) mod plugins;
mod preferences;
mod proxy;
mod python;
pub(crate) mod repo_files;
mod repositories;
pub(crate) mod scheduler;
pub(crate) mod search;
pub(crate) mod todo;
mod web;

pub(crate) use capabilities::TurnCapabilities;

use super::LucidosEngine;
use crate::engine::thread_lifecycle::ThreadStatus;
use crate::engine::thread_queue::{CapacityPolicy, OverflowPolicy};
use crate::llm::tool_names as tn;

/// Result of a tool dispatch: `Ok(text)` = success, `Err(text)` = failure.
/// In both cases `text` is what the LLM sees as the tool result; the typed
/// tag is what the agentic loop persists into `ToolResult.success`. Routing
/// failure through `Err` instead of inferring it from a `result.starts_with(
/// "Error:")` prefix keeps the success bit honest when a tool's error string
/// happens to start with `Error reading…` / `Error executing…` / etc. (the
/// pre-typed dispatch silently stamped those as `success: true`).
pub(crate) type ToolOutcome = Result<String, String>;

/// What the agent is told after landing on Settings → System → Backup.
///
/// A tool result describing a screen is a promise about that screen, and this
/// one had rotted into the opposite: it advertised a cloud-backup list and an
/// in-app Restore button (both gone, restore moved to the workspace picker) and
/// implied the page connects the provider account. Settings → Accounts owns
/// that, and a 2026-08-05 session sent a user hunting for accounts on the Backup
/// page because two sources (this string and `system-knowhow/backups.md`) said
/// so. Kept as a named const so `backup_navigation_names_the_accounts_page`
/// can pin it.
const BACKUP_SETTINGS_NAVIGATED: &str = "Navigated to Settings → System → Backup. The UI shows:\n\
     - A health card: last run outcome, last cloud backup + age, staleness warning\n\
     - The provider dropdown (Google Drive / Dropbox), and a red line linking to \
       Settings → Accounts when the selected provider has no connected account\n\
     - 'Back up now', the schedule dropdown, and the retention dropdown\n\
     - Show/generate the encryption key (required to restore, cannot be recovered)\n\
     This page does NOT connect the provider account, and has no account UI at all: \
     that is Settings → Accounts, and it is the only place to do it. Do not tell the \
     user to connect an account here. Restore is not in the app either, it happens \
     from the workspace picker.";

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

/// The actor to stamp on a `SystemEvent` a tool call mutated state through:
/// the agent running in THIS thread did it, not the user directly.
/// `ThreadLink { mode: Agent }` is the in-process analog of the CLI's
/// `Api { mode: Agent, source_thread_id }`, deep-linking back to the thread
/// whose agent acted so the route popover never mislabels it as "You".
/// `direction: Parent` because the dominant flow is a chat thread acting on
/// behalf of work it spawned.
///
/// One definition shared by every agent-tool emit site (repositories, plugins,
/// change apply, Thread Queue policy) so the attribution can't drift per tool.
pub(crate) fn agent_tool_actor(
    thread_id: uuid::Uuid,
) -> crate::engine::thread_events::MessageOrigin {
    crate::engine::thread_events::MessageOrigin::ThreadLink {
        thread_id,
        title: None,
        spawning_event_id: None,
        mode: crate::engine::thread_events::ActorMode::Agent,
        direction: crate::engine::thread_events::ThreadDirection::Parent,
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
        // Phase 5 grouped manifest tools delegate to their flat-alias handlers:
        // resolve `action` to the legacy flat tool name (validated against the
        // capability parity manifest) and fall through to that name's arm below.
        // The flat arms stay wired as back-compat aliases for cached prompts /
        // in-flight threads. Domains with bespoke handling (notifications /
        // preferences / triggers / trigger_groups) keep their own arms below.
        let name: &str = match name {
            tn::MCP
            | tn::PLUGINS
            | tn::EVENTS
            | tn::CHANGES
            | tn::THREADS
            | tn::THREAD_QUEUE
            | tn::MEMORY => grouped::grouped_legacy_name(name, args)?,
            other => other,
        };
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
            // Grouped manifest tools (consolidated surface the model sees).
            tn::TRIGGERS | tn::TRIGGER_GROUPS => self.execute_scheduler_grouped(name, args).await,
            tn::PREFERENCES => self.execute_preferences_grouped(args, device_id).await,
            // Flat per-verb names: back-compat aliases that still dispatch to the
            // same handlers (consolidated into the grouped tools above, but kept
            // so cached prompts/in-flight threads don't break).
            tn::CREATE_TRIGGER
            | tn::LIST_TRIGGERS
            | tn::UPDATE_TRIGGER
            | tn::DELETE_TRIGGER
            | tn::PAUSE_TRIGGER
            | tn::RESUME_TRIGGER
            | tn::RUN_TRIGGER
            | tn::LIST_TRIGGER_GROUPS
            | tn::CREATE_TRIGGER_GROUP
            | tn::RENAME_TRIGGER_GROUP
            | tn::REORDER_TRIGGER_GROUPS
            | tn::DELETE_TRIGGER_GROUP => to_outcome(self.execute_scheduler_tool(name, args).await),
            tn::SET_PREFERENCE | tn::GET_PREFERENCES => {
                to_outcome(self.execute_preferences_tool(name, args, device_id).await)
            }
            tn::GET_BACKUP_STATUS => to_outcome(self.execute_get_backup_status().await),
            // Grouped env-var tool (list/set/delete). `set_environment_variable`
            // stays wired as a back-compat alias → dispatched as action "set".
            tn::ENV_VARS | tn::SET_ENVIRONMENT_VARIABLE => self.execute_env_vars(name, args).await,
            tn::MANAGE_MODELS => to_outcome(self.execute_manage_models(args).await),
            tn::WEB_SEARCH | tn::FETCH_NEWS => to_outcome(self.execute_web_tool(name, args).await),
            tn::REQUEST_CREDENTIAL | tn::CONNECT_OAUTH_ACCOUNT => to_outcome(
                self.execute_credential_tool(name, args, thread_id, device_id)
                    .await,
            ),
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
            tn::RUN_PYTHON_BACKGROUND => self.execute_python_background_tool(args, thread_id).await,
            tn::RUN_BASH => to_outcome(self.execute_bash_tool(args, thread_id).await),
            tn::RUN_BASH_BACKGROUND => self.execute_bash_background_tool(args, thread_id).await,
            tn::BASH_OUTPUT => self.execute_bash_output_tool(args, thread_id).await,
            tn::BASH_KILL => self.execute_bash_kill_tool(args).await,
            tn::CORRECT_MEMORY => to_outcome(self.execute_memory_tool(args).await),
            tn::CORRECT_MEMORY_BY_ID => to_outcome(self.execute_correct_memory_by_id(args).await),
            tn::SEARCH_MEMORY => to_outcome(self.execute_search_memory(args).await),
            tn::MEMORY_SOURCE => to_outcome(self.execute_memory_source(args).await),
            tn::GENERATE_IMAGE => to_outcome(self.execute_generate_image(args, thread_id).await),
            tn::SAVE_THREAD_IMAGE => {
                to_outcome(self.execute_save_thread_image(args, thread_id).await)
            }
            tn::VIEW_IMAGE => to_outcome(self.execute_view_image(args, thread_id).await),
            tn::NAVIGATE_UI => self.execute_navigate_ui(args, thread_id, device_id).await,
            tn::SEND_NOTIFICATION => self.execute_send_notification(args, thread_id).await,
            tn::NOTIFICATIONS | tn::READ_NOTIFICATIONS => {
                self.execute_notifications(name, args).await
            }
            tn::EMIT_EVENT => self.execute_emit_event(args).await,
            tn::QUERY_EVENTS => self.execute_query_events(args, thread_id).await,
            tn::COUNT_EVENTS => self.execute_count_events(args).await,
            tn::LIST_EVENT_TYPES => self.execute_list_event_types().await,
            tn::FOLLOW_UP_CHILD_THREAD => {
                self.execute_follow_up_child_thread(args, thread_id).await
            }
            tn::LIST_THREADS => self.execute_list_threads(args, thread_id).await,
            tn::COUNT_THREADS => self.execute_count_threads(args, thread_id).await,
            tn::SEARCH_THREADS => to_outcome(self.execute_search_threads(args).await),
            tn::LIST_CHANGES => self.execute_list_changes().await,
            tn::APPLY_CHANGE => self.execute_apply_change(args, thread_id).await,
            tn::LIST_THREAD_QUEUE => self.execute_list_thread_queue().await,
            tn::UPDATE_THREAD_QUEUE_POLICY => {
                self.execute_update_thread_queue_policy(args, thread_id)
                    .await
            }
            tn::TODO_WRITE => self.execute_todo_write(args, thread_id).await,
            tn::MANAGE_REPOSITORIES => self.execute_manage_repositories(args, thread_id).await,
            tn::INSTALL_PLUGIN
            | tn::REGISTER_PLUGIN_MARKETPLACE
            | tn::CHECK_PLUGIN_UPDATES
            | tn::UPDATE_PLUGIN
            | tn::UNINSTALL_PLUGIN => self.execute_plugin_tool(name, args, thread_id).await,
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

    /// Resolve the turn's originating device into a `MessageOrigin` for
    /// device-scoping an emitted event.
    ///
    /// The device that sent the prompt that triggered this turn, so the frontend
    /// can act only on the screen the user is actually at. A turn with no device
    /// (trigger / scheduled / background) yields `None`, which the frontend
    /// reads as "unscoped" and falls back to its focused-thread / offer
    /// behaviour.
    pub(crate) async fn turn_device_actor(
        &self,
        device_id: Option<&str>,
    ) -> Option<crate::engine::thread_events::MessageOrigin> {
        let did = device_id?;
        let label = crate::core::DeviceStore::display_name(&self.pool, did)
            .await
            .unwrap_or_else(|| crate::core::devices::resolve_device_name(None, did));
        Some(crate::engine::thread_events::MessageOrigin::Device {
            device_id: did.to_string(),
            label,
        })
    }

    /// Ask the frontend to navigate. The one emitter of `NavigationRequested`
    /// from a tool call, so every navigate is device-scoped the same way.
    ///
    /// Two callers: `navigate_ui` (the agent moving the UI) and the OAuth flow
    /// (handing the authorization URL to whichever browser the user configured,
    /// rather than the engine shelling out to one).
    pub(crate) async fn request_navigation(
        &self,
        payload: &serde_json::Value,
        thread_id: uuid::Uuid,
        device_id: Option<&str>,
    ) -> Result<(), String> {
        let actor = self.turn_device_actor(device_id).await;
        self.event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::NavigationRequested {
                    payload: serde_json::to_string(payload).unwrap_or_default(),
                },
                meta: crate::engine::thread_events::EventMeta::with_actor(actor),
            })
            .await
            .map(|_| ())
            .map_err(|e| format!("failed to emit NavigationRequested: {}", e))
    }

    async fn execute_navigate_ui(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
        device_id: Option<&str>,
    ) -> ToolOutcome {
        let target = match args.get("target").and_then(|v| v.as_str()) {
            Some(t) if !t.is_empty() => t,
            _ => return Err("Error: target is required".to_string()),
        };
        log!(
            "[Navigate] navigate_ui thread={} target={} app_id={:?} id={:?} device={:?}",
            thread_id,
            target,
            args.get("app_id").and_then(|v| v.as_str()),
            args.get("id").and_then(|v| v.as_str()),
            device_id
        );
        if let Err(e) = self.request_navigation(args, thread_id, device_id).await {
            return Err(format!("Error: {}", e));
        }

        // Return contextual help so the LLM knows what the UI offers
        let settings_view = args.get("settings_view").and_then(|v| v.as_str());
        if target == "settings" && settings_view == Some("models") {
            return Ok("Navigated to Settings → Models. The UI shows:\n\
                - The active Chat & triggers model (the model picker) and reasoning effort\n\
                - Image generation and background-task models (title, image description, memory)\n\
                - Providers (Anthropic, OpenAI, OpenRouter, xAI, local) and the model registry\n\
                Tell the user they can change the active model from the picker here. To switch \
                it for them instead, use set_preference(key='chat_model'); to add a model to the \
                picker, use manage_models."
                .to_string());
        }
        if target == "settings" && settings_view == Some("backup") {
            return Ok(BACKUP_SETTINGS_NAVIGATED.to_string());
        }
        if target == "settings" && settings_view == Some("environment-variables") {
            return Ok(
                "Navigated to Settings → System → Environment variables. The UI shows:\n\
                - The user's environment variables as NAME = value rows\n\
                - Buttons to add, edit, or delete a variable\n\
                These are non-secret values injected into every subprocess Lucidos spawns \
                (run_bash, run_python, scheduled scripts, coding agents), which pick a change up \
                on the next spawn with no restart. The engine loads the same store into its own \
                process environment once at startup, so a variable the engine itself reads \
                changes only after an engine restart. For secrets like API keys, the user should \
                use credentials instead."
                    .to_string(),
            );
        }

        if target == "url" {
            let url = args.get("url").and_then(|v| v.as_str()).unwrap_or("");
            if url.is_empty() {
                return Err("Error: url is required when target is 'url'".to_string());
            }
            // Two hedges, both earned by an agent over-claiming. It does NOT
            // name a surface: where the URL lands is the client's decision
            // (`openUrl`), the in-app panel only when the user has that
            // preference on in the desktop app, otherwise their system browser
            // or a new tab, and naming one of the three had the agent telling
            // users to look at a panel that was never going to open. And it
            // does NOT claim the page opened: all this call did was emit the
            // request. A browser can still refuse it (a blocked popup, which is
            // exactly what a navigate from a chat turn hits, since the client
            // handles it with no user activation), in which case the client
            // offers the user an Open button instead.
            return Ok(format!(
                "Sent a request to the user's device to open {}. Emitted is not the \
                 same as open: the client may be unable to open it (a blocked popup, \
                 for one) and offer the user an Open button instead, so tell them you \
                 have sent them the page rather than stating it is already on their \
                 screen. It opens wherever they have configured links to open (the \
                 in-app browser panel, their system browser, or a new tab), so refer \
                 to it as their browser rather than naming one.",
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
        use crate::scheduler::notifications::{default_tap, Tap};

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

        // Optional event_id deep-link target. The LLM passes the row id of the
        // event the user should jump straight to (e.g. the UserQuestionAsked
        // from the triggering event payload). Validated as a UUID; empty, null
        // and missing all mean "no event anchor".
        let link_event: Option<uuid::Uuid> =
            crate::api::parse_optional_uuid_trimmed(args.get("event_id").and_then(|v| v.as_str()))
                .map_err(|raw| format!("Error: event_id is not a valid UUID: {}", raw))?;

        // When this trigger fired in response to a thread-scoped event (e.g.
        // `UserQuestionAsked`), the originating thread lives in a task-local
        // set by `handle_domain_event`. Prefer it as the deep-link target.
        // Otherwise the push would point at the trigger LLM's own thread, which
        // the user has no reason to open.
        let link_thread = crate::scheduler::user_tasks::ORIGIN_THREAD_ID
            .try_with(|t| *t)
            .unwrap_or(thread_id);

        // Missing, null and empty-string all mean the same thing: use
        // `default_tap`. It navigates to the source event when this
        // notification names one, and opens the card otherwise. Some LLM
        // providers emit `"tap": null` or `"tap": ""` for an unset optional, so
        // both take that default rather than erroring. The structured
        // `{kind, to?}` object is the only accepted positive shape.
        // `Tap::Deserialize` strictly rejects the legacy bare-string form
        // ("modal" / "open_app" / "open_thread" / "none"), and the LLM is
        // documented against the structured shape.
        let tap: Tap = match args.get("tap") {
            None | Some(serde_json::Value::Null) => default_tap(Some(link_thread), link_event),
            Some(serde_json::Value::String(s)) if s.is_empty() => {
                default_tap(Some(link_thread), link_event)
            }
            Some(v) => serde_json::from_value(v.clone()).map_err(|e| {
                format!(
                    "Error: invalid tap {}: expected an object like \
                     {{\"kind\":\"modal\"}} or \
                     {{\"kind\":\"navigate\",\"to\":{{\"target\":\"app\",\"app_id\":\"...\"}}}}. \
                     Parse error: {}",
                    v, e
                )
            })?,
        };

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
        //
        // The tool call runs on the fire's own task, so the ambient marker is
        // this fire. Passing it keeps what `EventBus::emit` used to read for
        // free, now that the emit states its owner (ADR 0137).
        let emitting_trigger_id = crate::scheduler::user_tasks::current_trigger_id();
        match self
            .emit_domain_event(event_type, payload, None, emitting_trigger_id)
            .await
        {
            Ok(id) => Ok(format!("Event {} emitted (id: {})", event_type, id)),
            Err(e) => Err(format!("Error: failed to emit event: {}", e)),
        }
    }

    /// Thin wrapper over [`repositories::manage_repositories_impl`], which owns
    /// the add/list/remove branches plus their `Repository{Added,Removed}`
    /// emits. Same split as [`query_events_impl`]: the free function takes the
    /// pool + bus so tests can drive it without booting the engine.
    async fn execute_manage_repositories(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        repositories::manage_repositories_impl(&self.pool, &self.event_bus, args, thread_id).await
    }

    /// Thin wrapper over [`query_events_impl`], which owns the arg parsing and
    /// the dereference branch. The free function takes the store, so tests can
    /// drive every refusal against a real Postgres without booting the engine.
    ///
    /// `thread_id` is the caller's own, ambient from `execute_tool`. It resolves
    /// the `current` alias, and the model cannot set it.
    async fn execute_query_events(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        query_events_impl(&self.event_store, args, thread_id).await
    }

    /// LLM tool: per-`event_type` count + byte total over the same time
    /// filters as `query_events`. Mirrors `count_threads` shape-wise.
    /// Without an `event_type` filter, returns the per-type breakdown sorted
    /// by count desc — the "what's noisy this week" view that a sweep recipe
    /// (workspace-learning, workspace-audit) should call before drilling.
    async fn execute_count_events(&self, args: &serde_json::Value) -> ToolOutcome {
        let event_type = args.get("event_type").and_then(|v| v.as_str());
        let since = parse_time_filter(args, "since")?;
        let until = parse_time_filter(args, "until")?;

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

    /// LLM tool: the event names this workspace can subscribe to.
    ///
    /// **The answer the refusals point at.** `engine` is a closed set, checked
    /// hard: a name that merely resembles one of these is refused with a
    /// suggestion. `workspace` is open, holding this workspace's own domain
    /// events. A name in neither list is accepted with a warning.
    ///
    /// Drawn from the trigger surface, the wider of the two, so it names the
    /// one family a wait may not watch.
    async fn execute_list_event_types(&self) -> ToolOutcome {
        use crate::core::event_subscription::{event_type_catalog, SubscriptionSurface};
        let catalog = event_type_catalog(&self.event_store, SubscriptionSurface::Trigger)
            .await
            .map_err(|e| format!("Error: {e}"))?;
        Ok(serde_json::json!({
            "engine": catalog.engine,
            "workspace": catalog.workspace,
            "retired": crate::engine::thread_events::ThreadEvent::LEGACY_TYPE_NAME_ALIASES,
            "note": "Subscribe by exact name. 'engine' is closed, so a near miss on one \
                     is refused rather than armed. 'workspace' holds this workspace's own \
                     domain events; a name in neither list is accepted, with a warning, \
                     for a domain event you are about to start emitting. 'retired' names \
                     still read back in history but nothing emits them again. A wait \
                     cannot watch the EventWait* family, a trigger can.",
        })
        .to_string())
    }

    /// LLM tool: redirect a child thread this thread already spawned.
    ///
    /// `caller_thread_id` is `execute_tool`'s ambient thread, never an
    /// argument. That is what makes the authorization ladder in
    /// `chat::child_follow_up` a real boundary here rather than an accounting
    /// one: the model can pick which child to address, but it cannot pick who
    /// it is.
    async fn execute_follow_up_child_thread(
        &self,
        args: &serde_json::Value,
        caller_thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        use crate::engine::chat::child_follow_up::ChildFollowUpError;

        let raw_id = args
            .get("thread_id")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                "Error: follow_up_child_thread needs a thread_id. It is the child's uuid, from \
                 the run_thread / run_coding_agent result, a completion card, or the threads \
                 tool's 'list' action with my_children: true."
                    .to_string()
            })?;
        let child_thread_id: uuid::Uuid = raw_id.parse().map_err(|_| {
            format!(
                "Error: '{raw_id}' is not a thread id. follow_up_child_thread addresses a child \
                 by uuid, never by title: titles are not unique, and a fuzzy match would \
                 silently deliver to the wrong child. List your own children with the threads \
                 tool's 'list' action and my_children: true."
            )
        })?;
        let message = args
            .get("message")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                "Error: follow_up_child_thread needs a non-empty message. It lands in the \
                 child's conversation as a message from you."
                    .to_string()
            })?;

        // Absent means not urgent; a present non-boolean is an error rather
        // than a silent `false`. See `FollowUpUrgency::from_tool_arg`.
        let urgency = crate::engine::FollowUpUrgency::from_tool_arg(args.get("urgent"))
            .map_err(|e| format!("Error: follow_up_child_thread's {e}"))?;

        let Some(engine) = self.try_clone_arc() else {
            return Err(
                "Error: follow_up_child_thread is unavailable on this engine instance.".to_string(),
            );
        };
        match engine
            .follow_up_child_thread(
                Some(caller_thread_id),
                child_thread_id,
                message,
                None,
                None,
                None,
                urgency,
            )
            .await
        {
            // Names the child by TITLE, never by uuid, so the model's prose
            // stays uuid-free by default (a uuid means nothing to the user:
            // no screen is labelled with one).
            Ok(ack) => Ok(format!(
                "Sent to \"{}\". {}",
                ack.child_title,
                ack.delivered_to.describe()
            )),
            // Each refusal tells the model what to do instead, and none of them
            // leaks whose child a thread is beyond "not yours".
            Err(e @ ChildFollowUpError::NotYourChild(_)) => Err(format!(
                "Error: {e} List your own children with the threads tool's 'list' action and \
                 my_children: true, then address one of those."
            )),
            Err(e @ ChildFollowUpError::UnknownChild(_)) => Err(format!(
                "Error: {e} Check the id against the threads tool's 'list' action with \
                 my_children: true."
            )),
            Err(e @ ChildFollowUpError::ChildDiscarded(_)) => Err(format!(
                "Error: {e} Spawn a fresh thread with run_thread or run_coding_agent instead."
            )),
            Err(e @ ChildFollowUpError::SelfTarget(_)) => Err(format!("Error: {e}")),
            Err(e) => Err(format!("Error: {e}")),
        }
    }

    /// LLM tool: list thread summaries for the workspace. Mirrors
    /// `GET /api/v1/threads/list` and `lucidos threads list`.
    ///
    /// `caller_thread_id` is `execute_tool`'s ambient thread, which the model
    /// cannot set. It is what `my_children: true` resolves to.
    async fn execute_list_threads(
        &self,
        args: &serde_json::Value,
        caller_thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        let statuses = parse_status_arg(args)?;
        let status = status_filter_arg(args, &statuses);
        let sources = parse_source_arg(args.get("source"));
        let parent = parent_filter_arg(args, caller_thread_id);
        let limit = args
            .get("limit")
            .and_then(|v| v.as_i64())
            .unwrap_or(100)
            .clamp(1, 1000);
        match self
            .event_store
            .list_thread_summaries(crate::core::store::ThreadSummaryFilters {
                status,
                sources: sources.as_deref(),
                parent,
                limit,
            })
            .await
        {
            // Compact JSON: every tool that returns a JSON array does the
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
    async fn execute_count_threads(
        &self,
        args: &serde_json::Value,
        caller_thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        let statuses = parse_status_arg(args)?;
        let status = status_filter_arg(args, &statuses);
        let sources = parse_source_arg(args.get("source"));
        let parent = parent_filter_arg(args, caller_thread_id);
        match self
            .event_store
            .count_thread_summaries(crate::core::store::ThreadSummaryFilters {
                status,
                sources: sources.as_deref(),
                parent,
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
        // The agent in THIS thread drove the apply, so the `ChangeApplied`
        // event (emitted on the *proposing* thread's timeline) deep-links back
        // here. `direction: Parent` fits the dominant flow: a chat thread
        // applying the change of a coding-agent thread it spawned (the
        // proposing thread is the child; this thread is its parent).
        let actor = agent_tool_actor(thread_id);
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

    /// LLM tool: list the Thread Queue plus the active capacity policy. Shares
    /// `ThreadQueue::snapshot` with `GET /api/v1/thread-queue`, so the tool and
    /// the panel return identical entries — including the in-memory
    /// user-initiated occupants (`kind: "user-chat"`) the tool previously
    /// omitted, which is why it reported an empty pool while the panel showed
    /// running user-chat rows.
    async fn execute_list_thread_queue(&self) -> ToolOutcome {
        let snapshot = self
            .thread_queue
            .snapshot()
            .await
            .map_err(|e| format!("Error: failed to list Thread Queue: {}", e))?;
        serde_json::to_string(&snapshot)
            .map_err(|e| format!("Error: failed to serialise Thread Queue: {}", e))
    }

    /// LLM tool: partially update the Thread Queue capacity policy. Unlike
    /// the HTTP panel endpoint, omitted fields are merged with the live policy
    /// rather than with code defaults, which is the safe shape for natural
    /// requests such as "double capacity".
    async fn execute_update_thread_queue_policy(
        &self,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        let previous = self.thread_queue.policy().await;
        let policy = merge_thread_queue_policy_patch(previous.clone(), args)?;
        let actor = agent_tool_actor(thread_id);
        let engine = self.clone_arc();
        engine
            .thread_queue
            .set_policy(policy.clone(), Some(actor))
            .await
            .map_err(|e| format!("Error: failed to update Thread Queue policy: {}", e))?;
        serde_json::to_string(&serde_json::json!({
            "previous_policy": previous,
            "policy": policy,
        }))
        .map_err(|e| format!("Error: failed to serialise Thread Queue policy: {}", e))
    }
}

/// Apply a partial Thread Queue policy patch to a starting policy. Missing
/// fields keep their existing value; present fields must have the same JSON
/// type as the wire policy. Pure so validation is testable without an engine.
pub(crate) fn merge_thread_queue_policy_patch(
    mut policy: CapacityPolicy,
    args: &serde_json::Value,
) -> Result<CapacityPolicy, String> {
    let obj = args
        .as_object()
        .ok_or_else(|| "Error: update_thread_queue_policy expects an object".to_string())?;
    if obj.is_empty() {
        return Err("Error: at least one Thread Queue policy field is required".to_string());
    }
    for field in obj.keys() {
        if !is_thread_queue_policy_field(field) {
            return Err(format!(
                "Error: unknown Thread Queue policy field `{}`",
                field
            ));
        }
    }

    apply_usize_policy_field(
        args,
        "max_concurrent_total",
        &mut policy.max_concurrent_total,
    )?;
    apply_usize_policy_field(
        args,
        "max_concurrent_event_trigger",
        &mut policy.max_concurrent_event_trigger,
    )?;
    apply_usize_policy_field(args, "max_concurrent_cron", &mut policy.max_concurrent_cron)?;
    apply_usize_policy_field(
        args,
        "max_concurrent_sub_thread",
        &mut policy.max_concurrent_sub_thread,
    )?;
    apply_usize_policy_field(
        args,
        "max_concurrent_coding_agent",
        &mut policy.max_concurrent_coding_agent,
    )?;
    apply_usize_policy_field(
        args,
        "max_concurrent_per_trigger",
        &mut policy.max_concurrent_per_trigger,
    )?;
    apply_usize_policy_field(
        args,
        "max_queued_per_trigger",
        &mut policy.max_queued_per_trigger,
    )?;
    apply_usize_policy_field(args, "reserved_background", &mut policy.reserved_background)?;

    if let Some(value) = args.get("max_event_trigger_depth") {
        policy.max_event_trigger_depth =
            serde_json::from_value::<u32>(value.clone()).map_err(|_| {
                "Error: max_event_trigger_depth must be an unsigned integer".to_string()
            })?;
    }

    if let Some(value) = args.get("overflow") {
        policy.overflow =
            serde_json::from_value::<OverflowPolicy>(value.clone()).map_err(|_| {
                "Error: overflow must be one of `drop-oldest` or `pause-trigger`".to_string()
            })?;
    }

    if policy.max_queued_per_trigger == 0 {
        return Err("Error: max_queued_per_trigger must be at least 1".to_string());
    }
    // 0 would cap every chain at its first hop, so no event trigger would ever
    // fire. That is a config that silently switches triggers off.
    if policy.max_event_trigger_depth == 0 {
        return Err("Error: max_event_trigger_depth must be at least 1".to_string());
    }
    Ok(policy)
}

fn apply_usize_policy_field(
    args: &serde_json::Value,
    field: &str,
    target: &mut usize,
) -> Result<(), String> {
    let Some(value) = args.get(field) else {
        return Ok(());
    };
    *target = serde_json::from_value::<usize>(value.clone())
        .map_err(|_| format!("Error: {field} must be an unsigned integer"))?;
    Ok(())
}

fn is_thread_queue_policy_field(field: &str) -> bool {
    matches!(
        field,
        "max_concurrent_total"
            | "max_concurrent_event_trigger"
            | "max_concurrent_cron"
            | "max_concurrent_sub_thread"
            | "max_concurrent_coding_agent"
            | "max_concurrent_per_trigger"
            | "max_queued_per_trigger"
            | "reserved_background"
            | "max_event_trigger_depth"
            | "overflow"
    )
}

/// Parse the required `change_id` UUID arg for the `apply_change` tool. Pure
/// so the validation branches are unit-testable without booting an engine
/// (same pattern as `query_events_impl`). Missing / null / empty /
/// whitespace-only all collapse to "required"; a non-UUID string is rejected
/// before the heavyweight merge pipeline runs.
pub(crate) fn parse_apply_change_id(args: &serde_json::Value) -> Result<uuid::Uuid, String> {
    let raw = match args.get("change_id").and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => s.trim(),
        _ => return Err("Error: change_id is required".to_string()),
    };
    uuid::Uuid::parse_str(raw).map_err(|_| format!("Error: change_id is not a valid UUID: {}", raw))
}

/// Parse an optional RFC3339 time filter (`since` / `until`) for the event
/// query tools.
///
/// A present-but-unparseable value is a hard error, never a silent `None`:
/// dropping the bound turns a windowed query into an all-time one, which the
/// model then reports to the user as the window. `2026-07-01` (no time, no
/// offset) is a very common model shape and does NOT parse as RFC3339, so this
/// path is hit routinely rather than exotically.
pub(crate) fn parse_time_filter(
    args: &serde_json::Value,
    key: &str,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, String> {
    let Some(raw) = args
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    else {
        return Ok(None);
    };
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|dt| Some(dt.with_timezone(&chrono::Utc)))
        .map_err(|e| {
            format!(
                "Error: `{}` must be an RFC3339 timestamp (e.g. 2026-07-01T00:00:00Z), got '{}': {}",
                key, raw, e
            )
        })
}

/// Default + inclusive `[min, max]` bounds for an optional numeric tool
/// argument. `apply(None)` yields the default; `apply(Some(v))` clamps `v` into
/// range. Unifies the `query_events` limit/byte-budget pair (and is reusable by
/// any other tool that wants the same "default-when-absent, then clamp" shape).
#[derive(Clone, Copy)]
pub(crate) struct ClampBounds<T> {
    pub default: T,
    pub min: T,
    pub max: T,
}

impl<T: Ord + Copy> ClampBounds<T> {
    pub(crate) fn apply(&self, value: Option<T>) -> T {
        value.unwrap_or(self.default).clamp(self.min, self.max)
    }
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
pub(crate) const QUERY_EVENTS_BYTE_BUDGET: ClampBounds<i64> = ClampBounds {
    default: 128 * 1024,
    min: 1024,
    max: 512 * 1024,
};

/// Row-count bounds for the `query_events` LLM tool. The default of 50
/// matches the `workspace-learning` recipe's "sampling, not enumeration"
/// rule. The MAX of 200 leaves room for a deliberate full-window pull
/// on a small event type (e.g. `EngineSupervisorRespawned` over a year)
/// without enabling the abuse pattern that crashed the May 25 trigger
/// (single calls at `limit: 300/500` for high-byte-per-row types).
pub(crate) const QUERY_EVENTS_LIMIT: ClampBounds<i64> = ClampBounds {
    default: 50,
    min: 1,
    max: 200,
};

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
                serde_json::Value::String(QUERY_EVENTS_TRUNCATION_HINT.into()),
            );
        }
    }
    wrapper.to_string()
}

/// The narrowing arguments [`QUERY_EVENTS_TRUNCATION_HINT`] tells the model to
/// reach for. Test-only: it is the claim
/// `truncation_hint_names_only_real_query_arguments` checks in both directions,
/// that each name really is a property of the `events` domain's `query`
/// operation, and that the hint really does mention it.
#[cfg(test)]
const QUERY_EVENTS_HINT_FILTERS: &[&str] = &["event_type", "since", "until"];

/// What `query_events` says when its result didn't fit `byte_limit`.
///
/// It must only ever name arguments the tool accepts. It previously said
/// "Narrow by aggregate_id", and `aggregate_id` is a real COLUMN on the
/// `events` table but not an argument of this tool, so the advice sent the
/// model into a retry with an ignored parameter and an identical truncated
/// result.
const QUERY_EVENTS_TRUNCATION_HINT: &str =
    "result truncated to fit byte_limit. Narrow with event_type, shorten the \
     time window with since/until, or call count_events first to size the sweep \
     before drilling. Do not retry with a larger byte_limit unless you have \
     already narrowed the query.";

/// Resolve the parent filter for `list_threads` / `count_threads`.
///
/// The wire shape is a boolean-shaped `my_children`, NOT a `parent` uuid the
/// model supplies and not a `"self"` sentinel that means something different on
/// each surface. The caller's own thread id is ambient
/// (`execute_tool`'s `thread_id`), so a model asking for "my children" cannot
/// name a thread that is not its own: impossible states made impossible rather
/// than validated after the fact.
///
/// The HTTP surface takes a literal `parent` uuid instead, because it has no
/// ambient caller to resolve.
fn parent_filter_arg(args: &serde_json::Value, caller_thread_id: uuid::Uuid) -> Option<uuid::Uuid> {
    args.get("my_children")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        .then_some(caller_thread_id)
}

/// Parse the `source` arg accepted by `list_threads` / `count_threads`.
/// The LLM may emit a comma-separated string (`"chat,trigger"`) or a JSON
/// array of strings (`["chat", "trigger"]`). Empty results collapse to
/// `None` so the store helper's "no filter" branch fires. `coding-agent` is
/// the public filter name; rows are still persisted with the legacy
/// `claude_code` source.
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
    let out: Vec<String> = out.into_iter().map(canonical_source_filter_value).collect();
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Parse the `status` arg accepted by `list_threads` / `count_threads`, and
/// refuse it alongside `active`.
///
/// Accepts the array the schema advertises or a comma-separated string, the
/// same two shapes `parse_source_arg` takes, because a model that has seen one
/// of these tools will reach for either. Unlike `parse_source_arg` this returns
/// a `Result`: a status value the model invented must come back as a tool error
/// it can correct, not as a filter that silently matches nothing.
fn parse_status_arg(args: &serde_json::Value) -> Result<Vec<ThreadStatus>, String> {
    let Some(raw) = args.get("status").filter(|v| !v.is_null()) else {
        return Ok(Vec::new());
    };
    if args.get("active").and_then(|v| v.as_bool()).is_some() {
        return Err(format!(
            "Error: pass either active or status, not both. active is the union \
             (running, waiting_for_user_answer); status names exactly the statuses \
             you want, out of {}. For 'is the workspace busy?' use status: \
             [\"running\"], since a thread awaiting a user answer is blocked on the \
             human, not working.",
            crate::core::store::status_value_list()
        ));
    }
    let values: Vec<String> = if let Some(s) = raw.as_str() {
        s.split(',').map(str::to_string).collect()
    } else if let Some(arr) = raw.as_array() {
        arr.iter()
            .map(|v| match v.as_str() {
                Some(s) => s.to_string(),
                // Not silently skipped: a non-string item is a malformed call,
                // and dropping it would answer a narrower question than asked.
                None => v.to_string(),
            })
            .collect()
    } else {
        return Err(format!(
            "Error: status takes a list of statuses (or a comma-separated string), \
             one or more of {}.",
            crate::core::store::status_value_list()
        ));
    };
    crate::core::store::parse_status_filter_values(&values).map_err(|e| format!("Error: {e}"))
}

/// Resolve the parsed `status` list and the `active` boolean into the single
/// store filter. Kept separate from [`parse_status_arg`] because
/// `StatusFilter::OneOf` borrows the parsed vector.
fn status_filter_arg<'a>(
    args: &serde_json::Value,
    statuses: &'a [ThreadStatus],
) -> crate::core::store::StatusFilter<'a> {
    if !statuses.is_empty() {
        crate::core::store::StatusFilter::OneOf(statuses)
    } else if let Some(want) = args.get("active").and_then(|v| v.as_bool()) {
        crate::core::store::StatusFilter::Active(want)
    } else {
        crate::core::store::StatusFilter::Any
    }
}

fn canonical_source_filter_value(value: String) -> String {
    match value.as_str() {
        "coding-agent" => "claude_code".to_string(),
        _ => value,
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

/// One refusal for a malformed event address, shared by every tool that takes
/// one. It names where the agent saw the address, because the commonest cause
/// is a paraphrase rather than a copy.
fn bad_event_address(got: impl std::fmt::Display) -> String {
    format!(
        "event_id '{got}' is not an event address. Pass the `evt-<32 hex>` \
         form a tool result states, or a bare uuid."
    )
}

/// What the model writes for "the thread I am in", instead of an id it has to
/// go and look up. Matched case-insensitively, after a trim.
///
/// The LLM tool is the only surface that takes them. `/api/v1/events/query` and
/// the SDK have no ambient caller to resolve, and an alias there would name
/// whichever thread the engine happened to be serving.
const CURRENT_THREAD_ALIASES: [&str; 2] = ["current", "this"];

/// Read core for the `events` tool's `query` action. Factored out of the
/// `LucidosEngine` impl so unit tests can drive every refusal branch against
/// a real Postgres pool without booting the full engine.
///
/// `caller_thread_id` is the thread the tool call runs in, ambient from
/// `execute_tool`. It is what the `current` alias resolves to, so the model
/// cannot point the alias at somebody else's conversation.
pub(crate) async fn query_events_impl(
    event_store: &crate::core::store::EventStore,
    args: &serde_json::Value,
    caller_thread_id: uuid::Uuid,
) -> ToolOutcome {
    let event_type = args.get("event_type").and_then(|v| v.as_str());
    let since = parse_time_filter(args, "since")?;
    let until = parse_time_filter(args, "until")?;
    let limit = QUERY_EVENTS_LIMIT.apply(args.get("limit").and_then(|v| v.as_i64()));
    let byte_limit =
        QUERY_EVENTS_BYTE_BUDGET.apply(args.get("byte_limit").and_then(|v| v.as_i64()));
    // Refused rather than ignored. This is the read half of "we talked
    // about this": the model finds a thread with `threads` 'search', then
    // asks for its messages here. Silently widening a malformed id to EVERY
    // thread would hand it another conversation, with no way to tell.
    // Matched on the VALUE, not on `as_str()`. Filtering to strings first
    // would send a `thread_id` of `["<uuid>"]` or `{...}` down the absent
    // arm, which widens the query to every thread.
    // The alias is the one string that is not an id, and it resolves to the
    // caller's own thread. Anything else still has to parse as a uuid.
    let thread_id = match args.get("thread_id") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(raw)) => {
            let raw = raw.trim();
            if CURRENT_THREAD_ALIASES
                .iter()
                .any(|alias| raw.eq_ignore_ascii_case(alias))
            {
                Some(caller_thread_id)
            } else {
                match uuid::Uuid::parse_str(raw) {
                    Ok(id) => Some(id),
                    Err(_) => {
                        return Err(format!(
                            "Error: thread_id '{raw}' is not a uuid. Pass 'current' for \
                             this thread. For another thread, copy its id from the \
                             `threads` tool's 'search' or 'list' result."
                        ))
                    }
                }
            }
        }
        Some(other) => {
            return Err(format!(
                "Error: thread_id must be a uuid string or 'current', got {other}. Pass \
                 'current' for this thread. For another thread, copy its id from the \
                 `threads` tool's 'search' or 'list' result."
            ))
        }
    };

    // Dereference half of a noted pointer (ADR 0085). Matched on the VALUE
    // for the same reason `thread_id` is. A non-string is a malformed
    // address. Reading it as absent would silently turn a lookup of one
    // named row into a newest-first window over everything.
    let event_id = match args.get("event_id") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(raw)) => {
            match crate::core::store::parse_event_address(raw) {
                Some(id) => Some(id),
                None => return Err(format!("Error: {}", bad_event_address(raw))),
            }
        }
        Some(other) => return Err(format!("Error: {}", bad_event_address(other))),
    };

    let mut events = event_store
        .query_events(
            crate::core::store::EventQueryFilters {
                event_type,
                since,
                until,
                thread_id,
                event_id,
                ..Default::default()
            },
            limit,
        )
        .await
        .map_err(|e| format!("Error: failed to query events: {}", e))?;

    // A dereference that resolves to nothing is a failure, not an empty
    // window. Said plainly, so a mistyped or hallucinated address never
    // reads to the agent as "that event no longer exists".
    if let Some(id) = event_id.filter(|_| events.is_empty()) {
        return Err(format!(
            "Error: no event has id {id}. Check the address, and drop any \
             other filter that could exclude it (event_type, thread_id, \
             since, until)."
        ));
    }

    // Dereferencing a tool call returns the PAIR, call then result.
    //
    // The address names the call, because that is the form the panel
    // prints and resumed blocks carry. What the boundary
    // dropped, though, is the result. The arguments alone resolve the pointer
    // to the half the agent still remembers. Nothing on this surface gets from
    // a call id to its result either, since there is no payload filter. That
    // is a recovery tool that does not recover, which ADR 0085 Decision 5
    // rules out.
    if let Some(call) = events.first().filter(|_| event_id.is_some()) {
        if call.event_type == "ToolCalled" {
            match event_store
                .tool_result_for_call(call.id, call.thread_id)
                .await
            {
                // An orphan call has no result to add. Returning the call
                // alone is the honest answer, not an error.
                Ok(None) => {}
                Ok(Some(result)) => events.push(result),
                Err(e) => return Err(format!("Error: failed to read the tool result: {}", e)),
            }
        }
    }
    Ok(build_query_events_response(&events, byte_limit))
}

#[cfg(test)]
#[path = "tools_tests.rs"]
mod tests;
