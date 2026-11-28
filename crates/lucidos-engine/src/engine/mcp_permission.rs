use std::sync::Mutex;

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::core::grants::GrantFile;
use crate::engine::card_queue;
use crate::engine::cc_permission::{DedupKey, PermissionAnswer, PermissionState};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::permission::tool_grants::{self, AnsweredBy};
use crate::engine::thread_events::AllowScope;
use crate::engine::thread_events::{EventChannel, EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::LucidosEngine;

pub const DENIAL_REASON: &str = "User denied";
pub const SUPERSEDED_REASON: &str = "Superseded by a new message";
pub const CANCELED_REASON: &str = "Canceled by user";

pub fn derive_mcp_allow_pattern(server_id: &str, tool: &str, scope: AllowScope) -> Option<String> {
    if server_id.is_empty() || tool.is_empty() {
        return None;
    }
    Some(match scope {
        AllowScope::Broad => format!("Mcp({server_id}:*)"),
        AllowScope::Narrow | AllowScope::Session => format!("Mcp({server_id}:{tool})"),
    })
}

pub fn mcp_is_allowed(server_id: &str, tool: &str, allowed: impl Fn(&str) -> bool) -> bool {
    if server_id.is_empty() || tool.is_empty() {
        return false;
    }
    allowed(&format!("Mcp({server_id}:*)")) || allowed(&format!("Mcp({server_id}:{tool})"))
}

#[allow(clippy::too_many_arguments)]
pub async fn record_mcp_allow_grant(
    engine: &LucidosEngine,
    thread_id: Uuid,
    request_id: &str,
    server_id: &str,
    tool: &str,
    scope: AllowScope,
    answered_by: AnsweredBy,
) {
    let Some(pattern) = derive_mcp_allow_pattern(server_id, tool, scope) else {
        return;
    };
    if let Err(e) = tool_grants::record_card_allow(
        &engine.event_bus,
        GrantFile::McpTools,
        thread_id,
        request_id,
        pattern.clone(),
        scope,
        answered_by,
    )
    .await
    {
        crate::log_legacy!("[McpPermission] Failed to record allow pattern {pattern:?}: {e}");
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum McpGate {
    Proceed,
    Ask,
    Refuse,
}

/// INVARIANT: The MCP gate decision.
///   * `auto_approve_flag`: the server's coarse server-level trust toggle,
///     which proceeds on every channel.
///   * a **trigger** thread proceeds only when `listed_by_trigger`: its
///     definition names the server in `mcp_servers`. An MCP card in a trigger
///     would deadlock the unattended run, so anything else is refused.
///
/// The agentic loop serves the Lucidos Agent (chat + triggers), so `channel` is
/// either `Some(Trigger)` (a scheduled trigger) or `None` (an ordinary chat
/// turn, since the loop does not stamp `Chat` on its per-turn meta). We gate on
/// "is this a trigger?" so the common `None` chat case reaches the ask lane.
/// The file/session allowlist is consulted separately (inside
/// `ask_mcp_permission`) on the `Ask` path.
pub fn mcp_gate(
    auto_approve_flag: bool,
    channel: Option<EventChannel>,
    listed_by_trigger: bool,
) -> McpGate {
    match (auto_approve_flag, channel) {
        (true, _) => McpGate::Proceed,
        (false, Some(EventChannel::Trigger)) if listed_by_trigger => McpGate::Proceed,
        (false, Some(EventChannel::Trigger)) => McpGate::Refuse,
        (false, _) => McpGate::Ask,
    }
}

pub fn unlisted_trigger_refusal(tool: &str, server_id: &str) -> String {
    format!(
        "Error: The MCP tool '{tool}' was NOT run. This trigger does not list MCP server \
         '{server_id}' in its mcp_servers, so its fires may not call that server unattended. \
         Do not call '{server_id}' again in this fire. The user has been notified, and can \
         add the server to the trigger's mcp_servers."
    )
}

fn unlisted_call_notification(trigger_name: &str, server_name: &str) -> (String, String) {
    (
        format!("{trigger_name}: MCP server '{server_name}' was refused"),
        format!(
            "This trigger called '{server_name}' without listing it in its MCP servers, so \
             the call did not run. Add the server to the trigger if its fires need it."
        ),
    )
}

/// INVARIANT: The tool result fed back to the LLM when the user denies an MCP tool call.
/// The `Error:` prefix is load-bearing: `handle_special_tool` returns this as a
/// legacy `Some(String)` that `tools::lift_legacy_string` only maps to a failed
/// (`is_error = true`) ToolResult when it starts with `Error:` — so a denied
/// call counts as a tool failure (circuit-breaker streak) and reads as refused
/// to the model, matching the command-guard lane's typed `Err`.
pub fn denial_refusal(tool: &str, server_name: &str) -> String {
    format!(
        "Error: Refused by the user — permission to call the MCP tool '{tool}' on '{server_name}' \
         was denied, so it was NOT run. Do not retry it. Either explain what you intended and ask \
         the user how they'd like to proceed, or choose a safe alternative."
    )
}

fn canceled_refusal(tool: &str) -> String {
    format!("Error: The MCP tool '{tool}' was not run — the request was canceled before you got permission.")
}

fn withdrawn_refusal(tool: &str) -> String {
    format!(
        "Error: The MCP tool '{tool}' was NOT run: the permission card was withdrawn before anyone \
         answered it, usually because the user sent a new message. This is not a denial. Read the \
         user's latest message before deciding whether to retry."
    )
}

fn ask_from_answer(
    answer: Result<PermissionAnswer, tokio::sync::broadcast::error::RecvError>,
    tool: &str,
    server_name: &str,
) -> McpAsk {
    match answer {
        Ok(PermissionAnswer::Allowed) => McpAsk::Proceed,
        Ok(PermissionAnswer::Denied) => McpAsk::Refuse(denial_refusal(tool, server_name)),
        Ok(PermissionAnswer::Refused(reason)) => McpAsk::Refuse(reason),
        Ok(PermissionAnswer::Withdrawn(_)) | Err(_) => McpAsk::Refuse(withdrawn_refusal(tool)),
    }
}

pub async fn resolve_pending_mcp_permissions_as_superseded(
    pool: &sqlx::PgPool,
    event_bus: &EventBus,
    pending: &Mutex<PermissionState>,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
) {
    let rows: Vec<(Option<String>,)> = match sqlx::query_as(
        "SELECT e.payload->>'request_id' \
         FROM events e \
         WHERE e.event_type = 'McpPermissionRequested' \
           AND e.thread_id = $1 \
           AND NOT EXISTS ( \
             SELECT 1 FROM events r \
             WHERE r.event_type = 'McpPermissionResolved' \
               AND r.payload->>'request_id' = e.payload->>'request_id' \
           )",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            crate::log_legacy!(
                "[McpPermission] unresolved-permission query failed for {}: {}",
                thread_id,
                e
            );
            return;
        }
    };

    for (request_id,) in rows {
        let Some(request_id) = request_id.filter(|s| !s.is_empty()) else {
            continue;
        };
        // INVARIANT: Best-effort unblock of a still-waiting loop. The std Mutex is released
        // before the `.await` below — never hold it across an await.
        {
            let mut state = pending.lock().unwrap();
            if let Some(entry) = state.take(&request_id) {
                let _ = entry
                    .tx
                    .send(PermissionAnswer::Withdrawn(SUPERSEDED_REASON));
            }
        }
        emit_mcp_permission_resolved(
            event_bus,
            thread_id,
            request_id,
            false,
            Some(SUPERSEDED_REASON.to_string()),
            None,
            EventMeta::with_actor(actor.clone()),
            "[McpPermission] McpPermissionResolved (superseded)",
        )
        .await;
    }
}

pub async fn recover_orphan_mcp_permission_requests(pool: &sqlx::PgPool, event_bus: &EventBus) {
    let rows: Vec<(Uuid, String)> = match sqlx::query_as(
        "SELECT e.thread_id, e.payload->>'request_id' AS request_id \
         FROM events e \
         WHERE e.event_type = 'McpPermissionRequested' \
           AND e.thread_id IS NOT NULL \
           AND NOT EXISTS ( \
             SELECT 1 FROM events r \
             WHERE r.event_type = 'McpPermissionResolved' \
               AND r.payload->>'request_id' = e.payload->>'request_id' \
           )",
    )
    .fetch_all(pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            crate::log_legacy!("[Recovery] orphan MCP permission query failed: {}", e);
            return;
        }
    };

    if rows.is_empty() {
        return;
    }

    crate::log_legacy!(
        "[Recovery] Auto-resolving {} orphan MCP permission request(s) (chat turn gone after restart)",
        rows.len()
    );

    for (thread_id, request_id) in rows {
        emit_mcp_permission_resolved(
            event_bus,
            thread_id,
            request_id,
            false,
            Some(crate::engine::cc_permission::RESTART_CLOSED_REASON.to_string()),
            None,
            EventMeta::NONE,
            "[Recovery] McpPermissionResolved (orphan)",
        )
        .await;
    }
}

pub async fn resolve_mcp_permission(
    engine: &LucidosEngine,
    request_id: String,
    allowed: bool,
    persist_scope: Option<AllowScope>,
    answered_by: AnsweredBy,
    log_context: &str,
) -> bool {
    let entry = {
        let mut pending = engine.pending_mcp_permission.lock().unwrap();
        pending.take(&request_id)
    };
    let Some(entry) = entry else {
        return false;
    };
    let _ = entry.tx.send(PermissionAnswer::from_decision(allowed));

    let reason = if allowed {
        None
    } else {
        Some(DENIAL_REASON.to_string())
    };
    let persist_scope = persist_scope.filter(|_| allowed);
    if let Some(scope) = persist_scope {
        if let Some((server_id, tool)) =
            crate::mcp::McpManager::parse_mcp_tool_name(&entry.tool_name)
        {
            record_mcp_allow_grant(
                engine,
                entry.thread_id,
                &request_id,
                &server_id,
                &tool,
                scope,
                answered_by.clone(),
            )
            .await;
        }
    }
    emit_mcp_permission_resolved(
        &engine.event_bus,
        entry.thread_id,
        request_id,
        allowed,
        reason,
        persist_scope,
        EventMeta::with_actor(answered_by.actor),
        log_context,
    )
    .await;
    true
}

#[allow(clippy::too_many_arguments)]
pub async fn emit_mcp_permission_resolved(
    event_bus: &EventBus,
    thread_id: Uuid,
    request_id: String,
    allowed: bool,
    reason: Option<String>,
    persist_scope: Option<AllowScope>,
    meta: EventMeta,
    log_context: &str,
) {
    event_bus
        .emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: ThreadEvent::McpPermissionResolved {
                    request_id,
                    allowed,
                    reason,
                    persist_scope,
                },
                meta,
            },
            log_context,
        )
        .await;
}

pub enum McpAsk {
    Proceed,
    Refuse(String),
}

pub(crate) struct UnlistedCallNotice {
    pub title: String,
    pub message: String,
    pub tap: crate::scheduler::notifications::Tap,
}

/// INVARIANT: The notice for a refused unlisted call in `thread_id`, or `None` when this
/// thread already notified about this server. The agent is told to stop using
/// the server, but a retry must not stack a second notification on the first.
/// A failed read counts as not yet notified: one notice too many beats none.
pub(crate) async fn unlisted_call_notice(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    trigger: Option<&crate::triggers::TriggerConfig>,
    server_name: &str,
) -> Option<UnlistedCallNotice> {
    use crate::scheduler::notifications::{NavigateTarget, NavigateUi, Tap};
    let trigger_name = trigger.map_or("A trigger", |t| t.name.as_str());
    let (title, message) = unlisted_call_notification(trigger_name, server_name);
    let already: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM notifications WHERE thread_id = $1 AND title = $2)",
    )
    .bind(thread_id)
    .bind(&title)
    .fetch_one(pool)
    .await
    .unwrap_or(false);
    if already {
        return None;
    }
    let tap = match trigger {
        Some(t) => Tap::Navigate {
            to: Box::new(NavigateUi {
                target: NavigateTarget::Trigger,
                id: Some(t.id.clone()),
                ..Default::default()
            }),
        },
        None => Tap::Modal,
    };
    Some(UnlistedCallNotice {
        title,
        message,
        tap,
    })
}

impl LucidosEngine {
    pub async fn trigger_of_thread(
        &self,
        thread_id: Uuid,
    ) -> Option<crate::triggers::TriggerConfig> {
        let trigger_id: Option<String> = match sqlx::query_scalar(
            "SELECT trigger_id FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(&self.pool)
        .await
        {
            Ok(row) => row.flatten(),
            Err(e) => {
                crate::log_legacy!(
                    "[McpPermission] Failed to read the trigger of thread {}: {}",
                    thread_id,
                    e
                );
                None
            }
        };
        let trigger_id = trigger_id?;
        self.trigger_configs.read().ok()?.get(&trigger_id).cloned()
    }

    pub async fn notify_unlisted_trigger_mcp_call(
        &self,
        thread_id: Uuid,
        trigger: Option<&crate::triggers::TriggerConfig>,
        server_name: &str,
    ) {
        let Some(notice) = unlisted_call_notice(&self.pool, thread_id, trigger, server_name).await
        else {
            return;
        };
        if let Err(e) = self
            .create_notification(
                &notice.title,
                &notice.message,
                None,
                Some(thread_id),
                None,
                notice.tap,
                None,
            )
            .await
        {
            crate::log_legacy!(
                "[McpPermission] Failed to notify about a refused MCP call in thread {}: {}",
                thread_id,
                e
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn ask_mcp_permission(
        &self,
        server_id: &str,
        server_name: &str,
        tool: &str,
        namespaced_tool: &str,
        args: &Value,
        arguments_summary: &str,
        thread_id: Uuid,
        tool_use_id: &str,
        meta: &EventMeta,
        cancel_token: &CancellationToken,
    ) -> McpAsk {
        let granted =
            tool_grants::granted_lines(&self.event_bus, GrantFile::McpTools, Some(thread_id)).await;
        if mcp_is_allowed(server_id, tool, |p| granted.contains(p)) {
            return McpAsk::Proceed;
        }

        let canonical_input = serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string());
        let dedup_key: DedupKey = (thread_id, namespaced_tool.to_string(), canonical_input);
        let (request_id, mut rx, is_canonical) = {
            let mut pending = self.pending_mcp_permission.lock().unwrap();
            pending.register_or_attach(
                dedup_key,
                thread_id,
                namespaced_tool.to_string(),
                args.clone(),
                crate::engine::thread_events::GrantOffer::default(),
            )
        };

        if is_canonical {
            let request = ThreadEvent::McpPermissionRequested {
                request_id: request_id.clone(),
                tool_use_id: tool_use_id.to_string(),
                server_id: server_id.to_string(),
                server_name: server_name.to_string(),
                tool_name: tool.to_string(),
                arguments_summary: arguments_summary.to_string(),
            };
            let posted = card_queue::post_permission_prompt(
                &self.event_bus,
                thread_id,
                request,
                meta.clone(),
                Some(cancel_token),
            )
            .await;
            match posted {
                Ok(card_queue::PromptPost::Posted) => {}
                Ok(card_queue::PromptPost::Canceled) => {
                    self.pending_mcp_permission
                        .lock()
                        .unwrap()
                        .take(&request_id);
                    return McpAsk::Refuse(canceled_refusal(tool));
                }
                Err(e) => {
                    crate::log_legacy!("[McpPermission] McpPermissionRequested emit failed: {e}")
                }
            }
        }

        // INVARIANT: Block until the user answers (no timeout — the user is the rate-
        // limiter, same as CC + the command guard) or the turn is canceled. The
        // paired `McpPermissionResolved` for an Allow/Deny click is emitted by
        // the consent endpoint; only the cancel branch resolves it here.
        let answer = tokio::select! {
            biased;
            _ = cancel_token.cancelled() => {
                if is_canonical {
                    let had_entry = {
                        let mut pending = self.pending_mcp_permission.lock().unwrap();
                        pending.take(&request_id).is_some()
                    };
                    if had_entry {
                        emit_mcp_permission_resolved(
                            &self.event_bus,
                            thread_id,
                            request_id,
                            false,
                            Some(CANCELED_REASON.to_string()),
                            None,
                            meta.clone(),
                            "[McpPermission] McpPermissionResolved (canceled)",
                        )
                        .await;
                    }
                }
                return McpAsk::Refuse(canceled_refusal(tool));
            }
            res = rx.recv() => res,
        };
        ask_from_answer(answer, tool, server_name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed_in<'a>(grants: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |p| grants.contains(&p)
    }

    #[test]
    fn only_a_human_deny_reads_as_a_denial() {
        use tokio::sync::broadcast::error::RecvError;
        let refusal = |answer| match ask_from_answer(answer, "channels_list", "Slack") {
            McpAsk::Proceed => None,
            McpAsk::Refuse(text) => Some(text),
        };
        assert_eq!(refusal(Ok(PermissionAnswer::Allowed)), None);
        assert_eq!(
            refusal(Ok(PermissionAnswer::Denied)),
            Some(denial_refusal("channels_list", "Slack"))
        );
        for nobody_decided in [
            Ok(PermissionAnswer::Withdrawn(SUPERSEDED_REASON)),
            Err(RecvError::Closed),
        ] {
            assert_eq!(
                refusal(nobody_decided),
                Some(withdrawn_refusal("channels_list"))
            );
        }
    }

    #[test]
    fn derive_pattern_by_scope() {
        assert_eq!(
            derive_mcp_allow_pattern("slack", "channels_list", AllowScope::Narrow),
            Some("Mcp(slack:channels_list)".to_string())
        );
        assert_eq!(
            derive_mcp_allow_pattern("slack", "channels_list", AllowScope::Broad),
            Some("Mcp(slack:*)".to_string())
        );
        assert_eq!(
            derive_mcp_allow_pattern("slack", "channels_list", AllowScope::Session),
            Some("Mcp(slack:channels_list)".to_string())
        );
        assert_eq!(derive_mcp_allow_pattern("", "t", AllowScope::Narrow), None);
        assert_eq!(derive_mcp_allow_pattern("s", "", AllowScope::Narrow), None);
    }

    #[test]
    fn broad_grant_covers_every_tool_narrow_only_the_exact() {
        assert!(mcp_is_allowed(
            "slack",
            "channels_list",
            allowed_in(&["Mcp(slack:*)"])
        ));
        assert!(mcp_is_allowed(
            "slack",
            "post_message",
            allowed_in(&["Mcp(slack:*)"])
        ));
        assert!(mcp_is_allowed(
            "slack",
            "channels_list",
            allowed_in(&["Mcp(slack:channels_list)"])
        ));
        assert!(!mcp_is_allowed(
            "slack",
            "post_message",
            allowed_in(&["Mcp(slack:channels_list)"])
        ));
        assert!(!mcp_is_allowed(
            "slack",
            "channels_list",
            allowed_in(&["Mcp(github:*)"])
        ));
        assert!(!mcp_is_allowed("", "t", allowed_in(&["Mcp(:*)"])));
    }

    #[test]
    fn a_trigger_thread_runs_only_a_listed_or_auto_approved_server() {
        let trigger = Some(EventChannel::Trigger);
        assert_eq!(mcp_gate(false, trigger, true), McpGate::Proceed, "listed");
        assert_eq!(
            mcp_gate(true, trigger, false),
            McpGate::Proceed,
            "auto-approve"
        );
        assert_eq!(mcp_gate(false, trigger, false), McpGate::Refuse, "neither");

        assert_eq!(mcp_gate(true, None, false), McpGate::Proceed);
        assert_eq!(
            mcp_gate(true, Some(EventChannel::Chat), false),
            McpGate::Proceed
        );
        assert_eq!(mcp_gate(false, None, false), McpGate::Ask);
        assert_eq!(mcp_gate(false, None, true), McpGate::Ask);
        assert_eq!(
            mcp_gate(false, Some(EventChannel::Chat), false),
            McpGate::Ask
        );
    }

    #[tokio::test]
    async fn a_refused_server_is_notified_once_per_thread() {
        use crate::engine::event_bus::SystemEvent;
        use crate::scheduler::notifications::{NavigateTarget, Tap};
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let mut trigger =
            crate::triggers::TriggerConfig::from_created_payload(&serde_json::json!({
                "trigger_id": "t-watch", "name": "Hourly watch", "schedule": ["0 0 * * * *"],
                "timezone": "UTC", "run": { "type": "intent", "intent": "watch" },
            }))
            .unwrap();
        trigger.mcp_servers = vec!["jira".to_string()];

        let notice = unlisted_call_notice(&pool, thread_id, Some(&trigger), "Slack")
            .await
            .expect("the first refusal notifies");
        match &notice.tap {
            Tap::Navigate { to } => {
                assert_eq!(to.target, NavigateTarget::Trigger);
                assert_eq!(to.id.as_deref(), Some("t-watch"));
            }
            other => panic!("the notification must open the trigger: {other:?}"),
        }
        bus.emit(BusEvent::System(SystemEvent::NotificationCreated {
            id: Uuid::new_v4().to_string(),
            title: notice.title.clone(),
            message: notice.message.clone(),
            task_id: None,
            app_id: None,
            thread_id: Some(thread_id.to_string()),
            event_id: None,
            tap: notice.tap,
            actor: None,
            audience: None,
            push_group: None,
        }))
        .await
        .unwrap();

        assert!(
            unlisted_call_notice(&pool, thread_id, Some(&trigger), "Slack")
                .await
                .is_none()
        );
        assert!(
            unlisted_call_notice(&pool, thread_id, Some(&trigger), "GitHub")
                .await
                .is_some(),
            "another server is another notice"
        );
        assert!(
            unlisted_call_notice(&pool, Uuid::new_v4(), Some(&trigger), "Slack")
                .await
                .is_some(),
            "another fire is another notice"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[test]
    fn a_refused_unlisted_call_says_what_to_do_on_both_sides() {
        let refusal = unlisted_trigger_refusal("mcp__slack__post", "slack");
        assert!(refusal.starts_with("Error:"), "{refusal}");
        assert!(refusal.contains("NOT run"), "{refusal}");
        assert!(refusal.contains("again in this fire"), "{refusal}");
        assert!(refusal.contains("mcp_servers"), "{refusal}");

        let (title, message) = unlisted_call_notification("Hourly watch", "Slack");
        assert!(
            title.contains("Hourly watch") && title.contains("Slack"),
            "{title}"
        );
        assert!(message.contains("MCP servers"), "{message}");
        for text in [&refusal, &title, &message] {
            assert!(!text.contains('\u{2014}'), "{text}");
        }
    }

    #[tokio::test]
    async fn a_standing_broad_grant_auto_allows_a_fresh_tool_on_that_server() {
        use crate::test_support::{seed_coding_thread, setup_test_db, teardown_test_db};
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = seed_coding_thread(&bus, None).await;
        let pattern = derive_mcp_allow_pattern("github", "list_repos", AllowScope::Broad).unwrap();
        tool_grants::record_card_allow(
            &bus,
            GrantFile::McpTools,
            thread_id,
            "req",
            pattern,
            AllowScope::Broad,
            AnsweredBy::tap(None),
        )
        .await
        .unwrap();

        let granted = tool_grants::granted_lines(&bus, GrantFile::McpTools, None).await;
        assert!(mcp_is_allowed("github", "create_issue", |p| granted.contains(p)));
        assert!(!mcp_is_allowed("slack", "post_message", |p| granted.contains(p)));
        teardown_test_db(&db).await;
    }
}
