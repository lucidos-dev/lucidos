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
//! 1. **Subprocess origin** (`X-Lucidos-Agent-Origin-Token` header carries a
//!    thread-bound origin token that verifies) → `Api { mode: Agent,
//!    source_thread_id }`, the thread the token was minted for. A
//!    Lucidos-spawned subprocess (CC, run_bash, run_python, scheduled script,
//!    `lucidos` CLI) calling back into the engine is an agent action, never the
//!    user — stamp honestly so the timeline shows "Lucidos Agent" instead of
//!    "You" and the source thread id flows into the route popover.
//! 2. `caller` set → `Workspace` (other Lucidos workspace; carries optional
//!    thread/event id from the request body)
//! 3. `device_id` present     → `Device` (label looked up by caller from the
//!    `devices` table)
//! 4. **Machine-local token** (`X-Lucidos-Local-Token` verifies against the
//!    mode 0600 file the gateway minted) → `Api { mode: Engine }`, the engine's
//!    own machinery. The build-watch and the release scripts reach the engine
//!    this way: they belong to no thread, so they hold no origin token, and
//!    they are not a person either. See ADR 0169.
//! 5. else                    → `None`. A caller presenting no credential has
//!    said nothing about itself, and there is no honest actor to build. It is
//!    NOT the user: stamping one made dropping a credential buy more than
//!    presenting it. [`require_user_actor`] turns this into a refusal at any
//!    handler that must know who is acting. See ADR 0169.
//!
//! Mode = `Agent` or `Engine`:
//! - With `caller` set → `Workspace` (cross-workspace agent/engine call)
//! - Otherwise → `ThreadLink` when `parent_thread_id` is set, else `None`
//!   (callers must construct `MessageOrigin::Engine { reason }` directly).
//!
//! Caller fields are user-controllable — treat as a display hint only,
//! never for authorization. The subprocess token is process-local secret state
//! (env-injected) and IS authoritative for "did this request come from a
//! Lucidos-spawned process".
//!
//! ## The token is thread-bound, so `source_thread_id` is authenticated
//!
//! It did not used to be. The engine minted one token per startup and handed
//! the same value to every subprocess, then read the thread id off a separate
//! `X-Lucidos-Source-Thread-Id` header that nothing verified, so the token
//! proved "a Lucidos subprocess sent this" and nothing about *which thread*.
//! Any subprocess could claim any source thread id, which made every gate that
//! reads `source_thread_id` (`api::chat::subprocess_chat_legitimate`,
//! `api::internal::coding_agent_diff_refresh`,
//! `engine::cc_permission::resolve_attend_mode`) an accounting boundary rather
//! than an authorization one.
//!
//! Now each spawn gets its own **thread-bound origin token**, minted by
//! [`mint_agent_origin_token`] and shaped `"<thread>@<depth>@<trigger>.<mac>"`,
//! where the MAC is over the whole prefix under a per-startup secret. A `-`
//! stands in for a thread or a trigger the spawn does not carry.
//! [`subprocess_origin`] recomputes and verifies it, and the **prefix is the
//! source thread id**. There is no second input left to disagree with the
//! token, so there is no claim left to forge: a subprocess can only present the
//! token it was handed, and that token names exactly one thread. The old header
//! is gone.
//!
//! ## The prefix also carries the event-trigger chain depth
//!
//! A script trigger's `lucidos events emit` arrives on an axum request task,
//! which shares nothing with the fire that spawned the script. Without a carry
//! the emit stamps depth 0, restarting the chain, and a script trigger
//! subscribed to the event its own script emits never stops.
//!
//! The depth rides the same signed prefix, so it is authenticated rather than
//! claimed: a caller cannot declare a lower depth to escape the cap. The token
//! value is opaque to every client. The CLI, the Python shim and a coding-agent
//! session all copy the env var into the header without reading it. So widening
//! it needs no change outside this file.
//!
//! A token that does not verify is [`SubprocessOrigin::NotSubprocess`], never a
//! subprocess with weaker attribution. The failure is a downgrade in
//! attribution, never an upgrade in reach.
//!
//! ## The prefix also carries the emitting trigger
//!
//! The third field is `emitting_trigger_id`: which trigger's fire this
//! subprocess *is*. `ACTIVE_TRIGGER_ID` marks a fire's emits, and that
//! task-local stops at a `fork`. So a trigger's script emitting through the
//! `lucidos` CLI could still wake the trigger that ran it. Signing the claim
//! authenticates it the way the source thread id already is. An app through
//! the SDK holds no minted token, so it can state no claim at all.
//!
//! The claim covers the fire, never what the fire hands off. A coding-agent
//! spawn is where the two fields part: it carries the depth and passes `None`
//! for the trigger. ADR 0137 holds that rule and its reasoning, ADR 0138 the
//! depth.

use super::hex;
use crate::engine::thread_events::{ActorMode, MessageOrigin, ThreadDirection};
use sqlx::PgPool;
use std::sync::OnceLock;
use uuid::Uuid;

/// Header carrying the originating browser/device id on mutating endpoints
/// that don't accept a request body (apply/discard/revert, pin/unpin, etc.).
/// Frontend's `json()` helper sets this from `getDeviceId()`.
///
/// How much it is worth depends on who is asking. Through the *workspace
/// gateway* it is the gateway's own value: `enforce` strips the client's copy
/// and re-injects the device it authenticated. Straight to the engine's
/// loopback port it is whatever the caller typed. That second case is the
/// ADR 0050 posture, and the reason nothing here reads it as authorization.
pub const HEADER_DEVICE_ID: &str = "x-lucidos-device-id";

/// Header forwarded by the `lucidos` CLI carrying the thread-bound origin
/// token; verified against [`AGENT_ORIGIN_SECRET`] by [`subprocess_origin`].
pub const HEADER_AGENT_ORIGIN_TOKEN: &str = "x-lucidos-agent-origin-token";

/// Header carrying the workspace the caller believes it is talking to: the
/// *target workspace assertion*. Verified by `api::target_workspace`, which
/// refuses the request with 409 when it names a workspace other than the one
/// this engine serves.
///
/// Distinct from the `caller_workspace` body field, and the pair is easy to
/// confuse: `caller_workspace` says who is CALLING (a display hint, never
/// authorization), this says who the caller thinks it is CALLING. Same
/// CLI-mirror rule as [`HEADER_AGENT_ORIGIN_TOKEN`]: the literal is duplicated
/// in `crates/lucidos-cli/src/http.rs` and the two must be renamed in lockstep.
pub const HEADER_TARGET_WORKSPACE: &str = "x-lucidos-target-workspace";

/// Env-var name for the thread-bound origin token. Engine sets it; CLI
/// forwards it as [`HEADER_AGENT_ORIGIN_TOKEN`]. The CLI mirrors this literal
/// because it can't depend on the engine crate (no `lucidos-common`); rename
/// the engine side and the CLI's `crates/lucidos-cli/src/http.rs` constant must
/// follow in lockstep. The *value* is opaque to every client (all of them copy
/// the env var into the header without inspecting it), so its shape can change
/// without touching the CLI, the Python shim, or a coding-agent session.
pub const ENV_AGENT_ORIGIN_TOKEN: &str = "LUCIDOS_AGENT_ORIGIN_TOKEN";

/// Env-var name for the spawning thread id. Same engine/CLI mirror rule as
/// [`ENV_AGENT_ORIGIN_TOKEN`].
///
/// This is **not** how the engine learns the source thread: that comes from
/// the token itself (see the module docs). The variable stays because plenty
/// of subprocess-side code reads it for its own purposes: the Python runtime
/// shim, the Codex MCP child config, and `lucidos spawn-thread`'s
/// `caller_thread_id` default.
pub const ENV_SOURCE_THREAD_ID: &str = "LUCIDOS_THREAD_ID";

/// Prefix field standing in for a claim this subprocess does not carry. That
/// is no thread context (a scheduled script), or no emitting trigger (any
/// spawn outside a fire). Not a valid uuid, so it cannot collide with a real
/// thread id. It is signed with the rest of the prefix, so an absent claim
/// cannot be edited into a present one.
const ABSENT_SEGMENT: &str = "-";

/// Separates the three prefix fields: thread, chain depth, emitting trigger.
///
/// Not `.`, which [`subprocess_origin`] already splits the MAC on, and not a
/// character a uuid can contain, so the fields can never be ambiguous.
const FIELD_SEPARATOR: char = '@';

/// Per-startup HMAC key. `OnceLock::set` is first-writer-wins, which matches
/// the production invariant (engine startup writes once); test harnesses
/// that reboot the engine in the same process keep the first secret.
static AGENT_ORIGIN_SECRET: OnceLock<String> = OnceLock::new();

/// Install the per-engine-startup secret. Idempotent.
pub fn init_agent_origin_secret(secret: String) {
    let _ = AGENT_ORIGIN_SECRET.set(secret);
}

/// A `Hmac<Sha256>` primed with the installed secret and `prefix`; `None`
/// before [`init_agent_origin_secret`] is called (the no-token path leaves
/// device/api resolution unchanged).
///
/// The single derivation site. Minting finalizes it to hex and verification
/// hands it the presented bytes, so the two cannot drift into computing
/// different things.
fn origin_mac(prefix: &str) -> Option<hmac::Hmac<sha2::Sha256>> {
    use hmac::Mac;
    let secret = AGENT_ORIGIN_SECRET.get()?;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts a key of any length");
    mac.update(prefix.as_bytes());
    Some(mac)
}

/// Mint a thread-bound origin token for a subprocess spawned on behalf of
/// `thread_id` (or with no thread context), carrying `emitting_trigger_id` when
/// the spawn IS a trigger's fire. `None` before the secret is installed.
///
/// The **only** place a token is constructed. Every subprocess surface reaches
/// it through [`subprocess_origin_env_vars`], so a new one cannot ship with an
/// unbound token.
///
/// Pass the trigger only where the marker should travel. A spawn that starts a
/// new thread passes `None`, see the module docs.
pub fn mint_agent_origin_token(
    thread_id: Option<Uuid>,
    chain_depth: u32,
    emitting_trigger_id: Option<&str>,
) -> Option<String> {
    use hmac::Mac;
    let thread = match thread_id {
        Some(tid) => tid.to_string(),
        None => ABSENT_SEGMENT.to_string(),
    };
    // The token ends up in an HTTP header, so a field that is not visible
    // ASCII would break the request. Drop the claim instead: fail open.
    let trigger = match emitting_trigger_id.filter(|id| is_header_safe_field(id)) {
        Some(id) => id.to_string(),
        None => {
            // Say so. A silently dropped claim looks exactly like a trigger
            // that woke itself for some other reason.
            if let Some(dropped) = emitting_trigger_id {
                log!("[Origin] Trigger id is not header-safe, minting with no claim: {dropped:?}");
            }
            ABSENT_SEGMENT.to_string()
        }
    };
    let prefix = format!("{thread}{FIELD_SEPARATOR}{chain_depth}{FIELD_SEPARATOR}{trigger}");
    let mac = hex::hex_lower(&origin_mac(&prefix)?.finalize().into_bytes());
    Some(format!("{prefix}.{mac}"))
}

/// Non-empty, and every byte visible ASCII with no space. A trigger id is a
/// uuid today, so this only guards against a future id shape.
fn is_header_safe_field(field: &str) -> bool {
    !field.is_empty() && field.bytes().all(|b| (b'!'..=b'~').contains(&b))
}

/// Outcome of subprocess-origin detection. The two-variant shape forces
/// callers to spell out "is this a subprocess" rather than overload an
/// `Option<Option<Uuid>>` whose inner `None` is "subprocess without source
/// thread" — readers don't have to translate nested `Option`s back to prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubprocessOrigin {
    /// Request did not present a valid origin token. Fall through to the
    /// regular device/api resolution.
    NotSubprocess,
    /// Request came from a Lucidos-spawned subprocess. `source_thread_id`
    /// is the spawning thread when known (CC always; Lucidos LLM tools when
    /// the tool invocation has thread context).
    Subprocess {
        source_thread_id: Option<Uuid>,
        /// Event-trigger chain depth of the work that spawned this subprocess,
        /// so an emit it makes stays on that chain. 0 for anything outside a
        /// trigger fire, which is most subprocesses.
        chain_depth: u32,
        /// The trigger whose fire this subprocess is, when it is one. Read off
        /// the signed prefix, so it is authenticated. See the module docs.
        ///
        /// A caller holding no minted token claims nothing, and suppresses
        /// nobody. That is the fail-open direction, and it is why the claim
        /// rides the token instead of a header a forger could set.
        emitting_trigger_id: Option<String>,
    },
}

/// Detect a request from a Lucidos-spawned subprocess by verifying the
/// thread-bound origin token in `HEADER_AGENT_ORIGIN_TOKEN`. Lock-free and
/// allocation-free on the overwhelmingly common path (a browser request with
/// no such header): one `HeaderMap::get` and out, without touching the secret.
/// A request that does present a token costs one split, one hex decode and one
/// HMAC.
///
/// The token's own prefix is the source thread, so nothing about the request
/// besides the token influences the answer. Anything that fails to verify is
/// `NotSubprocess`: an unrecognised caller falls through to the ordinary
/// device/api resolution, which is the right answer for a forger (it holds no
/// Lucidos-issued credential, so it is an external API caller).
pub fn subprocess_origin(headers: &axum::http::HeaderMap) -> SubprocessOrigin {
    let Some(presented) = headers
        .get(HEADER_AGENT_ORIGIN_TOKEN)
        .and_then(|v| v.to_str().ok())
    else {
        return SubprocessOrigin::NotSubprocess;
    };
    // `rsplit_once` rather than `split_once`: the MAC is hex and carries no
    // dot, so the LAST dot is the separator even when a trigger id carries one.
    let Some((prefix, presented_mac)) = presented.rsplit_once('.') else {
        return SubprocessOrigin::NotSubprocess;
    };
    let (Some(mac), Some(presented_bytes)) = (origin_mac(prefix), hex::hex_decode(presented_mac))
    else {
        return SubprocessOrigin::NotSubprocess;
    };
    // `verify_slice` is the library's constant-time comparison, which is why
    // the presented MAC is decoded to bytes rather than compared as hex text.
    if hmac::Mac::verify_slice(mac, &presented_bytes).is_err() {
        return SubprocessOrigin::NotSubprocess;
    }
    // Every field is inside the MAC, so what follows splits authenticated text
    // rather than a claim. The trigger is last, so `splitn` keeps a trigger id
    // holding a separator intact.
    //
    // The secret is per-startup, so a verified prefix was minted by this
    // process and always has all three fields. Each malformed arm below is
    // therefore unreachable, and refusing is the right answer if it ever is.
    let mut fields = prefix.splitn(3, FIELD_SEPARATOR);
    let (Some(thread), Some(depth), Some(trigger)) = (fields.next(), fields.next(), fields.next())
    else {
        return SubprocessOrigin::NotSubprocess;
    };
    let Ok(chain_depth) = depth.parse::<u32>() else {
        return SubprocessOrigin::NotSubprocess;
    };
    let emitting_trigger_id = (trigger != ABSENT_SEGMENT).then(|| trigger.to_string());
    if thread == ABSENT_SEGMENT {
        return SubprocessOrigin::Subprocess {
            source_thread_id: None,
            chain_depth,
            emitting_trigger_id,
        };
    }
    // A verified thread field other than the sentinel was minted from a `Uuid`.
    match Uuid::parse_str(thread) {
        Ok(tid) => SubprocessOrigin::Subprocess {
            source_thread_id: Some(tid),
            chain_depth,
            emitting_trigger_id,
        },
        Err(_) => SubprocessOrigin::NotSubprocess,
    }
}

/// Build the env vars every Lucidos-spawned subprocess receives so the
/// engine recognises its HTTP callbacks. Single source of truth — both
/// `engine::engine_impl::build_script_env_vars` and
/// `runtime::claude_code::build_command` call this so a future subprocess
/// surface (MCP child process, future signer host, …) cannot silently ship
/// without origin attribution — which is exactly how the original incident
/// would re-grow.
///
/// The token is **bound to `thread_id`**: a subprocess spawned for thread A
/// receives a token that authenticates as A and as nothing else, which is what
/// makes `MessageOrigin::Api.source_thread_id` an authenticated fact rather
/// than a claim. This is the only caller of [`mint_agent_origin_token`], so a
/// new subprocess surface cannot ship with an unbound token.
///
/// `thread_id = None` mints the thread-less token (scheduled scripts with no
/// thread context) and emits no `LUCIDOS_THREAD_ID`; `Some(tid)` also emits it,
/// for the subprocess-side consumers that read it directly (the Python shim,
/// the Codex MCP child config, `lucidos spawn-thread`). The engine itself never
/// reads it back off a request.
///
/// `emitting_trigger_id` travels only where the spawn IS a trigger's fire, and
/// never where the fire hands work off. `chain_depth` is the opposite: it
/// reaches handed-off work too. Each caller states both, and the module docs
/// give the rule.
pub fn subprocess_origin_env_vars(
    thread_id: Option<Uuid>,
    chain_depth: u32,
    emitting_trigger_id: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut vars = Vec::with_capacity(2);
    if let Some(token) = mint_agent_origin_token(thread_id, chain_depth, emitting_trigger_id) {
        vars.push((ENV_AGENT_ORIGIN_TOKEN, token));
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
///   * `LUCIDOS_API_BASE_URL` — set only under the workspace gateway (ADR 0014),
///     where the engine binds a LOOPBACK port and serves plain HTTP while the
///     gateway terminates TLS and routes the workspace under `/<slug>/`. A
///     subprocess that built its URL from `.lucidos/ports` (the gateway port,
///     no `/<slug>/` prefix) would never reach this engine (the gateway resolves
///     the first path segment as a workspace slug), so we hand it the exact
///     loopback base URL to reach the engine directly.
///     Absent in legacy / Tauri / production (engine on the user-facing port —
///     the ports file resolves it), keyed off `LUCIDOS_BIND_LOOPBACK`, which
///     only the gateway sets.
pub fn host_protection_env_vars(workspace_path: &std::path::Path) -> Vec<(&'static str, String)> {
    let mut vars: Vec<(&'static str, String)> = Vec::with_capacity(4);
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
            // Under the gateway the engine is reachable by same-host
            // subprocesses only on its loopback port over plain HTTP — see the
            // doc comment above. `LUCIDOS_BIND_LOOPBACK` is set solely by the
            // gateway when it spawns the engine, so this override never fires in
            // the legacy / Tauri / production single-engine model.
            let behind_gateway = std::env::var("LUCIDOS_BIND_LOOPBACK")
                .map(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
                .unwrap_or(false);
            if behind_gateway {
                // Scheme via `net_config::tls_scheme` (never hardcoded): a
                // fronted engine serves plain HTTP today (the gateway strips
                // `LUCIDOS_TLS_*`), and this resolves to exactly what THIS
                // process serves either way.
                let scheme = crate::net_config::tls_scheme();
                vars.push((
                    "LUCIDOS_API_BASE_URL",
                    format!("{scheme}://127.0.0.1:{api_port}"),
                ));
            }
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
        if let SubprocessOrigin::Subprocess {
            source_thread_id, ..
        } = subprocess_origin(headers)
        {
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
            } else if super::local_auth::is_local_process(headers) {
                // A device wins above, because it names a person and this names
                // a machine. The order is load-bearing rather than theoretical:
                // the gateway injects BOTH on every request it proxies, its own
                // token to prove the hop and the device it authenticated. Read
                // the other way round, every action from a phone would be
                // recorded as the engine's own machinery.
                Some(MessageOrigin::Api {
                    user_agent,
                    mode: ActorMode::Engine,
                    source_thread_id: None,
                })
            } else {
                // Nobody. Not the user: a caller presenting no credential has
                // said nothing about itself, and naming it human is the
                // inversion ADR 0169 removes. `require_user_actor` turns this
                // into a refusal for a handler that must know who is acting.
                None
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

/// What a caller is told when it presented no identity at all.
///
/// It names the four credentials rather than the one this route wanted,
/// because the caller does not know which class the engine put it in. Each is
/// something the right caller already holds, so the message is a route back
/// rather than a wall.
pub const UNIDENTIFIED_CALLER: &str =
    "This request carries no identity, so the engine cannot record who is acting. \
     Present one of four credentials: the thread-bound origin token a Lucidos-spawned \
     subprocess is given, the machine-local token in ~/.lucidos/local-token, an \
     x-lucidos-device-id header for a registered device, or a caller_workspace body \
     field for a call from another workspace.";

/// Is this device id evidence, or a header somebody typed?
///
/// [`user_actor_resolved`] stamps ANY non-empty id as a `Device` actor, falling
/// back to a `device-<short>` label. That is right for attribution and wrong
/// for a gate, and `display_name` cannot tell the two apart: its `None` means
/// absent OR a database error.
///
/// A database error counts as registered. This sits on the user's own action
/// path, so failing closed would refuse real work on a blip. Same trade and
/// same probe as the chat path's device gate, which is why `is_registered`
/// returns the error instead of swallowing it.
async fn device_is_evidence(pool: &PgPool, device_id: &str) -> bool {
    match crate::core::DeviceStore::is_registered(pool, device_id).await {
        Ok(exists) => exists,
        Err(e) => {
            crate::log!(
                "[Actor] device lookup for '{}' failed ({}); treating it as registered so a \
                 database blip cannot refuse real work",
                device_id,
                e
            );
            true
        }
    }
}

/// [`user_actor_resolved`], refusing rather than returning nobody.
///
/// The one gate every mutating handler asks. A second spelling of it is how
/// four routes drifted apart before ADR 0169, so callers take this and never
/// re-derive the check.
///
/// **A device id must name a registered device.** Attribution accepts any id
/// and a gate cannot, or the evidence ADR 0168 clause 4 asks for would be one
/// header anyone can type. An id that names nothing is suppressed rather than
/// refused outright, so the caller falls through to its other credentials.
///
/// 401 rather than 403: the caller may retry with a credential, and nothing
/// here says its identity would be insufficient. That is a later question, and
/// ADR 0168 clause 4 owns it.
pub(crate) async fn require_user_actor(
    headers: &axum::http::HeaderMap,
    pool: &PgPool,
    device_id_override: Option<&str>,
) -> Result<MessageOrigin, super::error::ApiError> {
    // Blank-filter EACH source before falling back, never the winner
    // afterwards, or a blank override swallows a real header. Same order and
    // same rule as `require_human_mode_is_attributed`.
    let claimed = device_id_override
        .filter(|v| !v.trim().is_empty())
        .map(str::to_string)
        .or_else(|| header_str(headers, HEADER_DEVICE_ID).filter(|v| !v.trim().is_empty()));
    let device_id = match claimed.as_deref() {
        Some(id) if device_is_evidence(pool, id).await => Some(id),
        _ => None,
    };
    let device_label = match device_id {
        Some(id) => crate::core::DeviceStore::display_name(pool, id).await,
        None => None,
    };
    build_message_origin(
        headers,
        ActorMode::Human,
        device_id,
        device_label,
        None,
        None,
        None,
        None,
    )
    .ok_or_else(|| {
        super::error::ApiError::new(axum::http::StatusCode::UNAUTHORIZED, UNIDENTIFIED_CALLER)
    })
}

/// [`require_user_actor`], shaped for a handler that returns a bare `Response`.
///
/// The same refusal, rendered rather than propagated. A handler that has not
/// adopted `ApiError` still gates BEFORE it mutates. Leaving the check to its
/// emit would run it after the write had landed.
pub(crate) async fn require_user_actor_response(
    headers: &axum::http::HeaderMap,
    pool: &PgPool,
) -> Result<MessageOrigin, axum::response::Response> {
    use axum::response::IntoResponse as _;
    require_user_actor(headers, pool, None)
        .await
        .map_err(|e| e.into_response())
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
