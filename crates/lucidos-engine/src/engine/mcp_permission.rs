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
//! Two pre-authorization short-circuits skip the card entirely (no event):
//!   * the server's `auto_approve` flag (an independent, coarser server-level
//!     trust toggle — settable via `PUT /api/v1/mcp/auto-approve`), and
//!   * a **non-interactive trigger thread** (`meta.channel == Trigger`): a
//!     trigger has no human to prompt, so it auto-approves silently rather than
//!     wedge waiting for a click.
//!
//! Restart-safe by the same shape as the command lane — a request left
//! unresolved across a restart is cleared by
//! [`recover_orphan_mcp_permission_requests`].

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use serde_json::Value;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::engine::cc_permission::{DedupKey, PermissionState};
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

/// Persisted allowlist file under `~/.lucidos/` — the MCP counterpart of
/// `cc-allowed-tools` (CC) and `agent-allowed-commands` (chat command guard).
/// Separate file because MCP patterns (`Mcp(<server>:<tool>)`) are neither CC
/// `--allowedTools` patterns nor bash/python command patterns.
const MCP_ALLOWED_TOOLS_FILE: &str = "mcp-allowed-tools";
const MCP_ALLOWED_TOOLS_HEADER: &str = "# Lucidos Agent MCP allowlist — one pattern per line. Lines starting with '#' are ignored.\n# Patterns: Mcp(<server>:<tool>) e.g. Mcp(slack:channels_list) · Mcp(<server>:*) (any tool on the server).\n# <server> is the MCP server's registry id. Delete a line to revoke that grant.\n";

/// Compiled-in default allowlist — empty so the feature ships dark; users build
/// their list via the per-prompt "Always allow" buttons (which append to the
/// file) or by editing `mcp-allowed-tools` directly.
pub const DEFAULT_MCP_ALLOWED_TOOLS: &[&str] = &[];

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
pub fn derive_mcp_allow_pattern(
    server_id: &str,
    tool: &str,
    scope: AllowScope,
) -> Option<String> {
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

/// Patterns from `<user_dir>/mcp-allowed-tools` (one per line, blanks and `#`
/// comments ignored). Empty on a missing file, a `None` user_dir, or an IO
/// error — a read failure must never auto-allow, so it degrades to "no
/// patterns" (the user re-approves once rather than the gate silently opening).
pub fn mcp_allowed_tools_patterns(user_dir: Option<&Path>) -> Vec<String> {
    let Some(dir) = user_dir else {
        return DEFAULT_MCP_ALLOWED_TOOLS
            .iter()
            .map(|s| s.to_string())
            .collect();
    };
    let path = dir.join(MCP_ALLOWED_TOOLS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(contents) => contents
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| l.to_string())
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT_MCP_ALLOWED_TOOLS
            .iter()
            .map(|s| s.to_string())
            .collect(),
        Err(e) => {
            crate::log!(
                "[McpPermission] Failed to read {}: {} — treating as no allowlist",
                path.display(),
                e
            );
            Vec::new()
        }
    }
}

/// Append `pattern` to `<user_dir>/mcp-allowed-tools` if not already present.
/// Creates the file (with the header comment) if it doesn't exist. Atomic write
/// via tmp + rename. No-op when `user_dir` is `None`.
pub fn append_mcp_allowed_tool(
    user_dir: Option<&Path>,
    pattern: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let Some(dir) = user_dir else {
        return Ok(());
    };
    let path = dir.join(MCP_ALLOWED_TOOLS_FILE);
    let existing = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => MCP_ALLOWED_TOOLS_HEADER.to_string(),
        Err(e) => return Err(e.into()),
    };
    if existing
        .lines()
        .map(str::trim)
        .any(|l| !l.is_empty() && !l.starts_with('#') && l == pattern)
    {
        return Ok(());
    }
    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str(pattern);
    next.push('\n');
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &next)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Read the raw contents of `<user_dir>/mcp-allowed-tools` for a settings UI. A
/// missing file returns the seeded header (mirrors the command-guard allowlist),
/// so the editor always opens with the instructional comment even before the
/// first "Always allow" grant.
pub fn read_mcp_allowed_tools_file(
    user_dir: &Path,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let path = user_dir.join(MCP_ALLOWED_TOOLS_FILE);
    match std::fs::read_to_string(&path) {
        Ok(s) => Ok(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Ok(MCP_ALLOWED_TOOLS_HEADER.to_string())
        }
        Err(e) => Err(e.into()),
    }
}

/// Atomically overwrite `<user_dir>/mcp-allowed-tools` with `contents`. The gate
/// reads the file fresh on each prompt (see [`mcp_allowed_tools_patterns`]), so
/// an edit takes effect on the next gated MCP call — no restart.
pub fn write_mcp_allowed_tools_file(
    user_dir: &Path,
    contents: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    std::fs::create_dir_all(user_dir)?;
    let path = user_dir.join(MCP_ALLOWED_TOOLS_FILE);
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Record a granted "Always allow" by scope: `Session` into the in-memory
/// per-thread allow set; `Narrow` / `Broad` into the persisted allowlist file.
/// No-op for scopes whose pattern doesn't derive. Mirrors
/// `command_permission::record_command_allow_grant`.
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
            if let Err(e) = append_mcp_allowed_tool(engine.user_dir(), &pattern) {
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

/// Whether an MCP tool call needs a permission card. Pure decision, mirroring
/// the command guard's `action_for_lane` channel gate.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum McpGate {
    /// Pre-authorized — run the tool with no card.
    Proceed,
    /// Interactive chat with no blanket grant — pause and ask the user.
    Ask,
}

/// The MCP gate decision from the two card-skipping short-circuits:
///   * `auto_approve_flag` — the server's coarse server-level trust toggle.
///   * a non-interactive **trigger** thread — no human to prompt, so it
///     auto-approves silently (the user's explicit choice; an MCP card in a
///     trigger would deadlock the unattended run).
///
/// The agentic loop serves the Lucidos Agent (chat + triggers), so `channel` is
/// either `Some(Trigger)` (a scheduled trigger) or `None` (an ordinary chat
/// turn — the loop does not stamp `Chat` on its per-turn meta). We gate on "is
/// this a trigger?" so the common `None` chat case correctly reaches the ask
/// lane. The file/session allowlist is consulted separately (inside
/// `ask_mcp_permission`) on the `Ask` path.
pub(crate) fn mcp_gate(auto_approve_flag: bool, channel: Option<EventChannel>) -> McpGate {
    if auto_approve_flag || channel == Some(EventChannel::Trigger) {
        McpGate::Proceed
    } else {
        McpGate::Ask
    }
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

// ---------------------------------------------------------------------------
// Superseded / orphan recovery (mirror command_permission)
// ---------------------------------------------------------------------------

/// Resolve every unresolved `McpPermissionRequested` on `thread_id` as denied,
/// because the user typed a new message instead of clicking a button. Fans a
/// `false` to any in-process waiter and emits `McpPermissionResolved` so the
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
                let _ = entry.tx.send(false);
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

impl LucidosEngine {
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
        let persisted = mcp_allowed_tools_patterns(self.user_dir());
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
        let allowed = tokio::select! {
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
            res = rx.recv() => res.unwrap_or(false),
        };

        if allowed {
            McpAsk::Proceed
        } else {
            McpAsk::Refuse(denial_refusal(tool, server_name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed_in<'a>(grants: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        move |p| grants.contains(&p)
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
        assert!(mcp_is_allowed("slack", "channels_list", allowed_in(&["Mcp(slack:*)"])));
        assert!(mcp_is_allowed("slack", "post_message", allowed_in(&["Mcp(slack:*)"])));
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

    #[test]
    fn gate_proceeds_on_auto_approve_or_trigger_else_asks() {
        // Trigger channel → silent auto-approve (Invariant I1).
        assert_eq!(
            mcp_gate(false, Some(EventChannel::Trigger)),
            McpGate::Proceed
        );
        // Server auto_approve flag → proceed on any channel.
        assert_eq!(mcp_gate(true, None), McpGate::Proceed);
        assert_eq!(mcp_gate(true, Some(EventChannel::Chat)), McpGate::Proceed);
        // Ordinary chat turn (channel None) with no flag → ask.
        assert_eq!(mcp_gate(false, None), McpGate::Ask);
        assert_eq!(mcp_gate(false, Some(EventChannel::Chat)), McpGate::Ask);
    }

    #[test]
    fn allowlist_roundtrip_append_then_read() {
        let dir = tempfile::tempdir().unwrap();
        let p = Some(dir.path());
        assert!(mcp_allowed_tools_patterns(p).is_empty());
        append_mcp_allowed_tool(p, "Mcp(slack:channels_list)").unwrap();
        append_mcp_allowed_tool(p, "Mcp(github:*)").unwrap();
        // Duplicate append is a no-op.
        append_mcp_allowed_tool(p, "Mcp(slack:channels_list)").unwrap();
        let patterns = mcp_allowed_tools_patterns(p);
        assert!(patterns.contains(&"Mcp(slack:channels_list)".to_string()));
        assert!(patterns.contains(&"Mcp(github:*)".to_string()));
        assert_eq!(
            patterns
                .iter()
                .filter(|p| *p == "Mcp(slack:channels_list)")
                .count(),
            1,
            "duplicate must not be appended twice"
        );
        // A persisted broad grant auto-allows a fresh tool on that server.
        assert!(mcp_is_allowed("github", "create_issue", |pat| patterns
            .iter()
            .any(|x| x == pat)));
    }

    #[test]
    fn allowlist_missing_file_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(mcp_allowed_tools_patterns(Some(dir.path())).is_empty());
        assert!(mcp_allowed_tools_patterns(None).is_empty());
    }

    #[test]
    fn whole_file_read_missing_returns_header() {
        let dir = tempfile::tempdir().unwrap();
        let contents = read_mcp_allowed_tools_file(dir.path()).unwrap();
        assert_eq!(contents, MCP_ALLOWED_TOOLS_HEADER);
    }

    #[test]
    fn whole_file_write_then_read_and_parse() {
        let dir = tempfile::tempdir().unwrap();
        let body = format!("{MCP_ALLOWED_TOOLS_HEADER}Mcp(slack:channels_list)\nMcp(github:*)\n");
        write_mcp_allowed_tools_file(dir.path(), &body).unwrap();
        assert_eq!(read_mcp_allowed_tools_file(dir.path()).unwrap(), body);
        let patterns = mcp_allowed_tools_patterns(Some(dir.path()));
        assert_eq!(
            patterns,
            vec!["Mcp(slack:channels_list)".to_string(), "Mcp(github:*)".to_string()]
        );
    }
}
