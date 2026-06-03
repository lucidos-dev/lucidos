//! Resolve the actor (`MessageOrigin`) for an inbound HTTP request.
//!
//! Every mutating endpoint that emits an event (apply/discard/revert a change,
//! create/edit a thread, run a trigger, etc.) MUST stamp the produced event with
//! an actor so the timeline can show *who* initiated each system action.
//!
//! Cross-workspace caller info travels in the request **body** (the `caller_*`
//! fields on `ChatRequest`), not in headers. Handlers parse and validate those
//! fields up front, then bundle them into a `CallerOrigin` and pass it here.
//!
//! Resolution order (mode = `Human`):
//! 1. **Subprocess origin** (`X-Lucidos-Agent-Origin-Token` header matches the
//!    per-engine-startup token) → `Api { mode: Agent, source_thread_id }`. A
//!    Lucidos-spawned subprocess (CC, run_bash, run_python, scheduled script,
//!    `lucidos` CLI) calling back into the engine is an agent action, never the
//!    user — stamp honestly so the timeline shows "Lucidos Agent" instead of
//!    "You" and the source thread id flows into the route popover.
//! 2. `caller` set → `Workspace` (other Lucidos workspace; carries optional
//!    thread/event id from the request body)
//! 3. `device_id` present     → `Device` (label looked up by caller from the
//!    `devices` table)
//! 4. else                    → `Api { mode: Human }` (external API client;
//!    carries `User-Agent`)
//!
//! Mode = `Agent` or `Engine`:
//! - With `caller` set → `Workspace` (cross-workspace agent/engine call)
//! - Otherwise → `ThreadLink` when `parent_thread_id` is set, else `None`
//!   (callers must construct `MessageOrigin::Engine { reason }` directly).
//!
//! Caller fields are user-controllable — treat as a display hint only,
//! never for authorization. The subprocess token is process-local secret state
//! (env-injected) and IS authoritative for "did this request come from a
//! Lucidos-spawned process". `source_thread_id` from `X-Lucidos-Source-Thread-Id`
//! is display attribution only: any subprocess can claim any source thread id.

use crate::engine::thread_events::{ActorMode, MessageOrigin, ThreadDirection};
use sqlx::PgPool;
use std::sync::OnceLock;
use uuid::Uuid;

/// Header carrying the originating browser/device id on mutating endpoints
/// that don't accept a request body (apply/discard/revert, pin/unpin, etc.).
/// Frontend's `json()` helper sets this from `getDeviceId()`.
pub const HEADER_DEVICE_ID: &str = "x-lucidos-device-id";

/// Header forwarded by the `lucidos` CLI carrying the per-engine-startup
/// token; matched against [`AGENT_ORIGIN_TOKEN`] by [`subprocess_origin`].
pub const HEADER_AGENT_ORIGIN_TOKEN: &str = "x-lucidos-agent-origin-token";

/// Header carrying the spawning thread id when a subprocess calls back into
/// the engine. Only trusted when paired with a valid
/// `HEADER_AGENT_ORIGIN_TOKEN`; surfaced as `MessageOrigin::Api.source_thread_id`.
pub const HEADER_SOURCE_THREAD_ID: &str = "x-lucidos-source-thread-id";

/// Env-var name for the subprocess-origin token. Engine sets it; CLI forwards
/// it as [`HEADER_AGENT_ORIGIN_TOKEN`]. The CLI mirrors this literal because
/// it can't depend on the engine crate (no `lucidos-common`); rename the
/// engine side and the CLI's `crates/lucidos-cli/src/http.rs` constant must
/// follow in lockstep.
pub const ENV_AGENT_ORIGIN_TOKEN: &str = "LUCIDOS_AGENT_ORIGIN_TOKEN";

/// Env-var name for the spawning thread id. Same engine/CLI mirror rule as
/// [`ENV_AGENT_ORIGIN_TOKEN`].
pub const ENV_SOURCE_THREAD_ID: &str = "LUCIDOS_THREAD_ID";

/// Per-startup secret. `OnceLock::set` is first-writer-wins, which matches
/// the production invariant (engine startup writes once); test harnesses
/// that reboot the engine in the same process keep the first token.
static AGENT_ORIGIN_TOKEN: OnceLock<String> = OnceLock::new();

/// Install the per-engine-startup token. Idempotent.
pub fn init_agent_origin_token(token: String) {
    let _ = AGENT_ORIGIN_TOKEN.set(token);
}

/// Read the installed token; `None` before [`init_agent_origin_token`] is
/// called (no-token path leaves device/api resolution unchanged).
pub fn agent_origin_token() -> Option<&'static str> {
    AGENT_ORIGIN_TOKEN.get().map(String::as_str)
}

/// Outcome of subprocess-origin detection. The two-variant shape forces
/// callers to spell out "is this a subprocess" rather than overload an
/// `Option<Option<Uuid>>` whose inner `None` is "subprocess without source
/// thread" — readers don't have to translate nested `Option`s back to prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubprocessOrigin {
    /// Request did not present a valid origin token. Fall through to the
    /// regular device/api resolution.
    NotSubprocess,
    /// Request came from a Lucidos-spawned subprocess. `source_thread_id`
    /// is the spawning thread when known (CC always; Lucidos LLM tools when
    /// the tool invocation has thread context).
    Subprocess { source_thread_id: Option<Uuid> },
}

impl SubprocessOrigin {
    /// Source thread when this is a subprocess, else `None`. Lets callers
    /// shovel the value into `MessageOrigin::Api.source_thread_id` directly.
    pub fn source_thread_id(self) -> Option<Uuid> {
        match self {
            Self::Subprocess { source_thread_id } => source_thread_id,
            Self::NotSubprocess => None,
        }
    }
}

/// Detect a request from a Lucidos-spawned subprocess by comparing the
/// `HEADER_AGENT_ORIGIN_TOKEN` header to the engine-wide token. O(1)
/// lock-free on the no-token fast path (one `OnceLock::get` + one
/// `HeaderMap::get`).
pub fn subprocess_origin(headers: &axum::http::HeaderMap) -> SubprocessOrigin {
    let Some(expected) = AGENT_ORIGIN_TOKEN.get().map(String::as_str) else {
        return SubprocessOrigin::NotSubprocess;
    };
    let Some(presented) = headers
        .get(HEADER_AGENT_ORIGIN_TOKEN)
        .and_then(|v| v.to_str().ok())
    else {
        return SubprocessOrigin::NotSubprocess;
    };
    if presented != expected {
        return SubprocessOrigin::NotSubprocess;
    }
    let source_thread_id = headers
        .get(HEADER_SOURCE_THREAD_ID)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| Uuid::parse_str(s).ok());
    SubprocessOrigin::Subprocess { source_thread_id }
}

/// Build the env vars every Lucidos-spawned subprocess receives so the
/// engine recognises its HTTP callbacks. Single source of truth — both
/// `engine::engine_impl::build_script_env_vars` and
/// `runtime::claude_code::build_command` call this so a future subprocess
/// surface (MCP child process, future signer host, …) cannot silently ship
/// without origin attribution — which is exactly how the original incident
/// would re-grow.
///
/// `thread_id = None` produces just the token (scheduled scripts with no
/// thread context); `Some(tid)` adds the source-thread-id env so callbacks
/// can deep-link back to the spawning thread.
pub fn subprocess_origin_env_vars(thread_id: Option<Uuid>) -> Vec<(&'static str, String)> {
    let mut vars = Vec::with_capacity(2);
    if let Some(token) = agent_origin_token() {
        vars.push((ENV_AGENT_ORIGIN_TOKEN, token.to_string()));
    }
    if let Some(tid) = thread_id {
        vars.push((ENV_SOURCE_THREAD_ID, tid.to_string()));
    }
    vars
}

/// Build the env vars every Lucidos-spawned subprocess receives so that
/// kill-stale-by-port code reachable from inside the subprocess
/// (`scripts/lib/ports.sh`, ad-hoc shell scripts, the pre-kill Bash hook)
/// can refuse to signal the host engine or its sibling frontend.
///
/// Single source of truth for the same reason `subprocess_origin_env_vars`
/// is: both `runtime::claude_code::build_command` and
/// `engine::engine_impl::build_script_env_vars` call this so a future
/// subprocess surface (MCP child, signer host, scheduled script, …)
/// cannot ship without the guard — which is exactly how the original
/// incident grew (a test invoked from a Claude Code subprocess freed up "stale"
/// ports and took down its own host).
///
///   * `LUCIDOS_HOST_PID`     — always set; the engine's own pid.
///   * `LUCIDOS_FRONTEND_PID` — set only when `<workspace>/.lucidos/frontend.pid`
///     exists and is non-blank (web-dev mode; Tauri / production-bundled
///     installs have no separate Vite process).
///   * `LUCIDOS_API_PORT`     — re-exported when the engine itself was given
///     one, so the pre-kill hook can block `lsof -ti :<port> | xargs kill`
///     patterns targeting the engine port.
pub fn host_protection_env_vars(workspace_path: &std::path::Path) -> Vec<(&'static str, String)> {
    let mut vars: Vec<(&'static str, String)> = Vec::with_capacity(3);
    vars.push(("LUCIDOS_HOST_PID", std::process::id().to_string()));
    let frontend_pid_path = workspace_path.join(".lucidos/frontend.pid");
    if let Ok(contents) = std::fs::read_to_string(&frontend_pid_path) {
        // Validate as u32: a multi-line pidfile (or other junk) would
        // otherwise inject an embedded newline into the env var and break the
        // hook's regex match — silently disabling the guard for that pid.
        if let Ok(pid) = contents.trim().parse::<u32>() {
            vars.push(("LUCIDOS_FRONTEND_PID", pid.to_string()));
        }
    }
    if let Ok(api_port) = std::env::var("LUCIDOS_API_PORT") {
        if !api_port.is_empty() {
            vars.push(("LUCIDOS_API_PORT", api_port));
        }
    }
    vars
}

/// Cross-workspace origin info, extracted from request body `caller_*` fields.
/// `Some(_)` means this is a cross-workspace POST; `None` means same-workspace.
/// Mutual exclusion vs `parent_thread_id` is enforced upstream by
/// `validate_mode_and_spawn`, so this doesn't need to defend against both.
#[derive(Debug, Clone)]
pub struct CallerOrigin {
    pub workspace: String,
    pub thread_id: Option<Uuid>,
    pub event_id: Option<Uuid>,
    /// Upstream actor mode (Human/Agent/Engine of the calling workspace).
    pub mode: ActorMode,
}

// Aggregates eight unrelated inputs (headers + mode + device + parent-thread + caller);
// a struct wrapper would just shift the parameters one level deeper at every call site.
#[allow(clippy::too_many_arguments)]
pub fn build_message_origin(
    headers: &axum::http::HeaderMap,
    mode: ActorMode,
    device_id: Option<&str>,
    device_label: Option<String>,
    parent_thread_id: Option<Uuid>,
    parent_thread_title: Option<String>,
    spawning_event_id: Option<Uuid>,
    caller: Option<CallerOrigin>,
) -> Option<MessageOrigin> {
    let user_agent = header_str(headers, "user-agent");
    // Subprocess origin overrides device/api resolution so an agent action
    // never stamps as Human. Cross-workspace `caller` still wins (different
    // channel, different actor model).
    if caller.is_none() {
        if let SubprocessOrigin::Subprocess { source_thread_id } = subprocess_origin(headers) {
            return Some(MessageOrigin::Api {
                user_agent,
                mode: ActorMode::Agent,
                source_thread_id,
            });
        }
    }
    if let Some(c) = caller {
        return Some(MessageOrigin::Workspace {
            workspace: c.workspace,
            thread_id: c.thread_id,
            event_id: c.event_id,
            user_agent,
            mode: c.mode,
        });
    }
    match mode {
        ActorMode::Human => {
            if let Some(id) = device_id {
                Some(MessageOrigin::Device {
                    device_id: id.to_string(),
                    label: device_label
                        .unwrap_or_else(|| crate::core::devices::resolve_device_name(None, id)),
                })
            } else {
                Some(MessageOrigin::Api {
                    user_agent,
                    mode: ActorMode::Human,
                    source_thread_id: None,
                })
            }
        }
        ActorMode::Agent | ActorMode::Engine => {
            // No caller → must be same-workspace parent-thread spawn.
            parent_thread_id.map(|id| MessageOrigin::ThreadLink {
                thread_id: id,
                title: parent_thread_title,
                spawning_event_id,
                mode,
                direction: ThreadDirection::Parent,
            })
        }
    }
}

/// Convenience: build an actor for a `User`-initiated mutating endpoint that
/// has no parent-thread context (apply/discard/revert, settings writes, etc.).
///
/// `device_id` and `device_label` are optional explicit overrides — when both
/// are `None`, the header `x-lucidos-device-id` (if present) supplies the id
/// and the resulting `Device` actor uses the `device-<short>` fallback label.
/// Callers that have access to the `devices` table should prefer
/// `user_actor_resolved` so the popover shows the stored device name.
pub fn user_actor(
    headers: &axum::http::HeaderMap,
    device_id: Option<&str>,
    device_label: Option<String>,
) -> Option<MessageOrigin> {
    let header_did = if device_id.is_none() {
        header_str(headers, HEADER_DEVICE_ID)
    } else {
        None
    };
    let effective_did = device_id.or(header_did.as_deref());
    build_message_origin(
        headers,
        ActorMode::Human,
        effective_did,
        device_label,
        None,
        None,
        None,
        None,
    )
}

/// Like `user_actor` but enriches the device origin with the stored device
/// label from the `devices` table, so the popover renders "Chrome on Mac" (or
/// the `device-<short>` fallback) instead of an opaque id. Use this — not
/// `user_actor` directly — at every mutating HTTP handler.
///
/// `device_id_override` lets handlers that receive the device id in the
/// request body (e.g. per-device preferences) supply it explicitly; otherwise
/// the `x-lucidos-device-id` header is used.
pub async fn user_actor_resolved(
    headers: &axum::http::HeaderMap,
    pool: &PgPool,
    device_id_override: Option<&str>,
) -> Option<MessageOrigin> {
    let header_did = header_str(headers, HEADER_DEVICE_ID);
    let effective_did = device_id_override.or(header_did.as_deref());
    let device_label = match effective_did {
        Some(d) => crate::core::DeviceStore::display_name(pool, d).await,
        None => None,
    };
    user_actor(headers, device_id_override, device_label)
}

fn header_str(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

#[cfg(test)]
#[path = "actor_tests.rs"]
mod tests;
