//! MCP permission lane (chat) — the chat mirror of the command-guard
//! permission lane ([`crate::engine::command_permission`]) for **MCP server
//! tool calls** made by the Lucidos Agent.
//!
//! When the agentic loop is about to call an MCP server tool (`mcp__<server>__<tool>`),
//! it asks the user for permission unless the call is pre-authorized. Like the
//! command guard, the loop runs in-process: it blocks directly on the
//! [`PermissionEntry`](crate::engine::cc_permission::PermissionEntry)'s broadcast
//! channel and resumes when `POST /api/v1/mcp-permission/consent` sends the
//! answer. The shared dedup / session-allow mechanism lives in
//! [`crate::engine::cc_permission`] ([`PermissionState`]); the MCP-flavored
//! allow-pattern derivation, the persisted `mcp-allowed-tools` allowlist, the
//! reason constants, the superseded / orphan-recovery sweeps, and the
//! in-process block orchestrator are here.
//!
//! The card is skipped in two cases:
//!   * the server's `auto_approve` flag (an independent, coarser server-level
//!     trust toggle, settable via `PUT /api/v1/mcp/auto-approve`), and
//!   * a **trigger thread** (`meta.channel == Trigger`), which has nobody to
//!     prompt. There a call runs silently only when the trigger lists the
//!     server in `mcp_servers`. Any other call is refused, and the user is
//!     notified (ADR 0430).
//!
//! Restart-safe by the same shape as the command lane — a request left
//! unresolved across a restart is cleared by
//! [`recover_orphan_mcp_permission_requests`].

use std::collections::HashSet;
use std::sync::Mutex;

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::core::grants::{self, GrantFile};
use crate::engine::cc_permission::{DedupKey, PermissionAnswer, PermissionState};
use crate::engine::claude_code::AllowScope;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventChannel, EventMeta, MessageOrigin, ThreadEvent};
use crate::engine::LucidosEngine;

/// Reason on a resolution emitted because the user clicked Deny.
pub const DENIAL_REASON: &str = "User denied";
/// Reason on a resolution the engine emits because the user typed a new message
/// instead of answering the card.
pub const SUPERSEDED_REASON: &str = "Superseded by a new message";
/// Reason on a resolution the engine emits because the chat turn was canceled
/// (Stop button) while the card was on screen.
pub const CANCELED_REASON: &str = "Canceled by user";

// ---------------------------------------------------------------------------
// Allow-pattern derivation (reuses `AllowScope`)
// ---------------------------------------------------------------------------

/// The pattern to STORE when the user grants an "Always allow" / "Allow for
/// this thread" click, by scope:
///
///   * `Narrow`  → `Mcp(<server>:<tool>)` — this specific tool on this server.
///   * `Broad`   → `Mcp(<server>:*)`      — any tool on this server.
///   * `Session` → `Mcp(<server>:<tool>)` — per-tool, recorded in-memory per
///     thread (not the file).
///
/// Always derivable for a non-empty `(server_id, tool)`; returns `None` only
/// when either is empty (a malformed MCP tool name).
pub fn derive_mcp_allow_pattern(server_id: &str, tool: &str, scope: AllowScope) -> Option<String> {
    if server_id.is_empty() || tool.is_empty() {
        return None;
    }
    Some(match scope {
        AllowScope::Broad => format!("Mcp({server_id}:*)"),
        AllowScope::Narrow | AllowScope::Session => format!("Mcp({server_id}:{tool})"),
    })
}

/// Whether the granted pattern set covers `(server_id, tool)` — the auto-allow
/// (skip the card) check. `allowed` answers "is this exact pattern granted?"
/// over the union of the session set and the persisted allowlist.
///
/// A broad `Mcp(server:*)` grant covers every tool on that server; otherwise
/// the exact `Mcp(server:tool)` must be granted.
pub fn mcp_is_allowed(server_id: &str, tool: &str, allowed: impl Fn(&str) -> bool) -> bool {
    if server_id.is_empty() || tool.is_empty() {
        return false;
    }
    allowed(&format!("Mcp({server_id}:*)")) || allowed(&format!("Mcp({server_id}:{tool})"))
}

// ---------------------------------------------------------------------------
// Persisted allowlist file (`mcp-allowed-tools`)
// ---------------------------------------------------------------------------

/// Record a granted "Always allow" by scope: `Session` into the in-memory
/// per-thread allow set; `Narrow` / `Broad` into the persisted allowlist file
/// ([`GrantFile::McpTools`]). No-op for scopes whose pattern doesn't derive.
/// Mirrors `command_permission::record_command_allow_grant`.
pub fn record_mcp_allow_grant(
    engine: &LucidosEngine,
    thread_id: Uuid,
    server_id: &str,
    tool: &str,
    scope: AllowScope,
) {
    let Some(pattern) = derive_mcp_allow_pattern(server_id, tool, scope) else {
        return;
    };
    match scope {
        AllowScope::Session => {
            let mut pending = engine.pending_mcp_permission.lock().unwrap();
            pending.allow_session(thread_id, pattern);
        }
        AllowScope::Narrow | AllowScope::Broad => {
            if let Err(e) = grants::append(&engine.grants_dir(), GrantFile::McpTools, &pattern) {
                crate::log!(
                    "[McpPermission] Failed to persist allow pattern {:?}: {}",
                    pattern,
                    e
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pre-authorization gate (pure — unit-tested)
// ---------------------------------------------------------------------------

/// Whether an MCP tool call runs, asks, or is refused. Pure decision, mirroring
/// the command guard's `action_for_lane` channel gate.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum McpGate {
    /// Pre-authorized: run the tool with no card.
    Proceed,
    /// Interactive chat with no blanket grant: pause and ask the user.
    Ask,
    /// A trigger thread calling a server its trigger does not list. Nobody is
    /// there to ask, so the call is refused and the user is notified.
    Refuse,
}

/// The MCP gate decision.
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
pub(crate) fn mcp_gate(
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

/// The tool result fed back to the LLM when a trigger thread calls a server
/// its trigger does not list. `Error:`-prefixed for the same reason as
/// [`denial_refusal`].
pub(crate) fn unlisted_trigger_refusal(tool: &str, server_id: &str) -> String {
    format!(
        "Error: The MCP tool '{tool}' was NOT run. This trigger does not list MCP server \
         '{server_id}' in its mcp_servers, so its fires may not call that server unattended. \
         Do not call '{server_id}' again in this fire. The user has been notified, and can \
         add the server to the trigger's mcp_servers."
    )
}

/// The notification for a refused unlisted call: it names the trigger and
/// the server, and says how to allow it. Returns the title and the message.
fn unlisted_call_notification(trigger_name: &str, server_name: &str) -> (String, String) {
    (
        format!("{trigger_name}: MCP server '{server_name}' was refused"),
        format!(
            "This trigger called '{server_name}' without listing it in its MCP servers, so \
             the call did not run. Add the server to the trigger if its fires need it."
        ),
    )
}

// ---------------------------------------------------------------------------
// LLM-facing refusal messages
// ---------------------------------------------------------------------------

/// The tool result fed back to the LLM when the user denies an MCP tool call.
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

/// The tool result fed back to the LLM when the chat turn was canceled while the
/// permission card was on screen. `Error:`-prefixed for the same reason as
/// [`denial_refusal`].
fn canceled_refusal(tool: &str) -> String {
    format!("Error: The MCP tool '{tool}' was not run — the request was canceled before you got permission.")
}

/// The tool result fed back to the LLM when the card ended with nobody
/// answering it. `Error:`-prefixed for the same reason as [`denial_refusal`].
fn withdrawn_refusal(tool: &str) -> String {
    format!(
        "Error: The MCP tool '{tool}' was NOT run: the permission card was withdrawn before anyone \
         answered it, usually because the user sent a new message. This is not a denial. Read the \
         user's latest message before deciding whether to retry."
    )
}

/// What the agentic loop feeds the model once the card ends. Only a human's
/// Deny reads as a denial.
fn ask_from_answer(
    answer: Result<PermissionAnswer, tokio::sync::broadcast::error::RecvError>,
    tool: &str,
    server_name: &str,
) -> McpAsk {
    match answer {
        Ok(PermissionAnswer::Allowed) => McpAsk::Proceed,
        Ok(PermissionAnswer::Denied) => McpAsk::Refuse(denial_refusal(tool, server_name)),
        Ok(PermissionAnswer::Withdrawn(_)) | Err(_) => McpAsk::Refuse(withdrawn_refusal(tool)),
    }
}

// ---------------------------------------------------------------------------
// Superseded / orphan recovery (mirror command_permission)
// ---------------------------------------------------------------------------

/// Resolve every unresolved `McpPermissionRequested` on `thread_id` as denied,
/// because the user typed a new message instead of clicking a button. Fans a
/// `Withdrawn` to any in-process waiter and emits `McpPermissionResolved` so the
/// card stops dangling and the thread status flips back to `running`. Mirrors
/// `command_permission::resolve_pending_command_permissions_as_superseded`.
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
            crate::log!(
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
        // Best-effort unblock of a still-waiting loop. The std Mutex is released
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

/// Re-emit `McpPermissionResolved` for every persisted `McpPermissionRequested`
/// with no paired resolution. The in-memory `pending_mcp_permission` is gone
/// after a restart, so any loop that was blocked on a card is dead; emitting the
/// resolution clears the card buttons and the projection flips status from
/// `waiting_for_user_answer` to `running` (the orphan running→idle reset then
/// settles it). Mirrors `command_permission::recover_orphan_command_permission_requests`.
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
            crate::log!("[Recovery] orphan MCP permission query failed: {}", e);
            return;
        }
    };

    if rows.is_empty() {
        return;
    }

    crate::log!(
        "[Recovery] Auto-resolving {} orphan MCP permission request(s) (chat turn gone after restart)",
        rows.len()
    );

    for (thread_id, request_id) in rows {
        emit_mcp_permission_resolved(
            event_bus,
            thread_id,
            request_id,
            false,
            Some("Chat turn ended before answering — request expired".to_string()),
            None,
            EventMeta::NONE,
            "[Recovery] McpPermissionResolved (orphan)",
        )
        .await;
    }
}

/// Answer one chat MCP permission card, whoever answered it.
///
/// This lane's half of the contract `command_permission::resolve_command_permission`
/// documents: the single in-process path, three effects in order, and `false`
/// for a request id nothing is waiting on.
pub async fn resolve_mcp_permission(
    engine: &LucidosEngine,
    request_id: String,
    allowed: bool,
    persist_scope: Option<AllowScope>,
    actor: Option<MessageOrigin>,
    log_context: &str,
) -> bool {
    let entry = {
        let mut pending = engine.pending_mcp_permission.lock().unwrap();
        pending.take(&request_id)
    };
    let Some(entry) = entry else {
        return false;
    };
    // Wake the blocked loop, and every deduped waiter on the same broadcast.
    let _ = entry.tx.send(PermissionAnswer::from_decision(allowed));

    let reason = if allowed {
        None
    } else {
        Some(DENIAL_REASON.to_string())
    };
    // A scope on a denial grants nothing: the click said no.
    let persist_scope = persist_scope.filter(|_| allowed);
    if let Some(scope) = persist_scope {
        // `entry.tool_name` is the namespaced `mcp__<server>__<tool>`, so the
        // stored pattern needs the pair back. A name that will not parse
        // records nothing.
        if let Some((server_id, tool)) =
            crate::mcp::McpManager::parse_mcp_tool_name(&entry.tool_name)
        {
            record_mcp_allow_grant(engine, entry.thread_id, &server_id, &tool, scope);
        }
    }
    emit_mcp_permission_resolved(
        &engine.event_bus,
        entry.thread_id,
        request_id,
        allowed,
        reason,
        persist_scope,
        EventMeta::with_actor(actor),
        log_context,
    )
    .await;
    true
}

/// Emit an `McpPermissionResolved` via the bus. Shared by the consent endpoint,
/// the superseded sweep, the orphan-recovery sweep, and the cancel branch.
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

// ---------------------------------------------------------------------------
// In-process block orchestrator
// ---------------------------------------------------------------------------

/// Outcome of `ask_mcp_permission` — what the agentic loop relays back as the
/// MCP tool's result.
pub(crate) enum McpAsk {
    /// Run the MCP tool.
    Proceed,
    /// Blocked — feed `refusal` back to the LLM as the tool result.
    Refuse(String),
}

/// The notification for a refused unlisted MCP call, as it will be sent.
pub(crate) struct UnlistedCallNotice {
    pub title: String,
    pub message: String,
    pub tap: crate::scheduler::notifications::Tap,
}

/// The notice for a refused unlisted call in `thread_id`, or `None` when this
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
    /// The trigger whose fire `thread_id` is, read off the in-memory registry.
    /// `None` for a chat thread, and for a trigger deleted since it fired.
    pub(crate) async fn trigger_of_thread(
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
                crate::log!(
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

    /// Tell the user a trigger thread's MCP call was refused. See
    /// [`unlisted_call_notice`] for the once-per-thread-and-server rule.
    pub(crate) async fn notify_unlisted_trigger_mcp_call(
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
            crate::log!(
                "[McpPermission] Failed to notify about a refused MCP call in thread {}: {}",
                thread_id,
                e
            );
        }
    }

    /// Auto-allow when a session or persisted allowlist pattern already covers
    /// `(server_id, tool)`; otherwise emit an `McpPermissionRequested`, block the
    /// loop until the user resolves it (or the turn is canceled), and translate
    /// the answer into an [`McpAsk`]. Mirrors
    /// `command_permission::ask_command_permission`, but keyed on
    /// `(server_id, tool)` rather than a command head.
    ///
    /// `namespaced_tool` (`mcp__<server>__<tool>`) is used for the dedup key +
    /// stored on the entry so the consent endpoint can recover `(server, tool)`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn ask_mcp_permission(
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
        // Auto-allow: prior "Allow for this thread" (session) and persisted
        // "Always allow" (allowlist file) grants are unioned.
        let session_patterns: HashSet<String> = {
            let pending = self.pending_mcp_permission.lock().unwrap();
            pending
                .session_allows
                .get(&thread_id)
                .cloned()
                .unwrap_or_default()
        };
        let persisted = grants::patterns(&self.grants_dir(), GrantFile::McpTools);
        if mcp_is_allowed(server_id, tool, |p| {
            session_patterns.contains(p) || persisted.iter().any(|x| x == p)
        }) {
            return McpAsk::Proceed;
        }

        // Register (deduping identical concurrent requests) and emit the card.
        // The entry's `tool_name` is the namespaced name so the consent endpoint
        // recovers `(server, tool)`; the dedup key keys on it + the args.
        let canonical_input = serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string());
        let dedup_key: DedupKey = (thread_id, namespaced_tool.to_string(), canonical_input);
        let (request_id, mut rx, is_canonical) = {
            let mut pending = self.pending_mcp_permission.lock().unwrap();
            pending.register_or_attach(
                dedup_key,
                thread_id,
                namespaced_tool.to_string(),
                args.clone(),
                crate::engine::claude_code::GrantOffer::default(),
            )
        };

        if is_canonical {
            self.event_bus
                .emit_or_log(
                    BusEvent::Thread {
                        thread_id,
                        event: ThreadEvent::McpPermissionRequested {
                            request_id: request_id.clone(),
                            tool_use_id: tool_use_id.to_string(),
                            server_id: server_id.to_string(),
                            server_name: server_name.to_string(),
                            tool_name: tool.to_string(),
                            arguments_summary: arguments_summary.to_string(),
                        },
                        meta: meta.clone(),
                    },
                    "[McpPermission] McpPermissionRequested",
                )
                .await;
        }

        // Block until the user answers (no timeout — the user is the rate-
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

    /// Only a human's Deny tells the model the user refused. A superseded card
    /// and a closed channel used to fold into the same "do not retry".
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
        // Malformed → None.
        assert_eq!(derive_mcp_allow_pattern("", "t", AllowScope::Narrow), None);
        assert_eq!(derive_mcp_allow_pattern("s", "", AllowScope::Narrow), None);
    }

    #[test]
    fn broad_grant_covers_every_tool_narrow_only_the_exact() {
        // A broad grant covers any tool on the server.
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
        // A narrow grant covers only the exact tool.
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
        // A grant for another server does not cross over.
        assert!(!mcp_is_allowed(
            "slack",
            "channels_list",
            allowed_in(&["Mcp(github:*)"])
        ));
        // Malformed never auto-allows.
        assert!(!mcp_is_allowed("", "t", allowed_in(&["Mcp(:*)"])));
    }

    /// The invariant "trigger consent" over its three cases: a trigger thread
    /// calls a server silently only when the trigger lists it or the server
    /// has auto-approve set. Neither is refused, never asked.
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

        // Chat keeps its card: the listing is a trigger's, and means nothing here.
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

    /// One notification per thread and server, linking to the trigger. A
    /// second refused call in the same fire finds the first and sends none.
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

    /// The refusal tells the agent to stop using the server in this fire, and
    /// the notification names the trigger and the server.
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

    /// The file and the gate agree: a broad grant persisted through
    /// `core::grants` auto-allows a tool nobody has seen before. The file
    /// mechanics themselves are covered in `core::grants`.
    #[test]
    fn a_persisted_broad_grant_auto_allows_a_fresh_tool_on_that_server() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = grants::grants_dir(tmp.path());
        grants::append(&dir, GrantFile::McpTools, "Mcp(github:*)").unwrap();

        let patterns = grants::patterns(&dir, GrantFile::McpTools);
        assert!(mcp_is_allowed("github", "create_issue", |pat| patterns
            .iter()
            .any(|x| x == pat)));
        assert!(!mcp_is_allowed("slack", "post_message", |pat| patterns
            .iter()
            .any(|x| x == pat)));
    }
}
