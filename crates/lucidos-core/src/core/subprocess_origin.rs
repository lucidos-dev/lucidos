use crate::core::hex;
use std::sync::OnceLock;
use uuid::Uuid;

/// Header carrying the originating browser/device id on mutating endpoints
/// that don't accept a request body (apply/discard/revert, pin/unpin, etc.).
/// Frontend's `json()` helper sets this from `getDeviceId()`.
///
/// How much it is worth depends on who is asking. Through the *workspace
/// gateway* it is the gateway's own value: `enforce` strips the client's copy
/// and re-injects the device it authenticated, with a gateway stamp
/// (`api::gateway_stamp`). Straight to the engine's loopback port it is
/// whatever the caller typed, and it carries no valid stamp. Only a stamped id
/// builds a `Device` actor.
pub const HEADER_DEVICE_ID: &str = "x-lucidos-device-id";

/// Header carrying the pairing label of the device in [`HEADER_DEVICE_ID`],
/// percent-encoded. The *workspace gateway* writes both, from the device it
/// authenticated. Display metadata only, like the id on a loopback caller.
pub const HEADER_DEVICE_LABEL: &str = "x-lucidos-device-label";

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

/// The engine sets this variable and the CLI forwards it as
/// [`HEADER_AGENT_ORIGIN_TOKEN`]. Keep its name synchronized with the CLI
/// constant in `http.rs`. Clients forward the opaque value without inspecting
/// it.
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

/// The per-startup HMAC key is set once. A test harness that reboots the engine
/// within one process keeps the first key.
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

/// Mint an origin token for the given thread context. Include
/// `emitting_trigger_id` when spawning a trigger fire. Returns `None` before
/// key installation.
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
                log_legacy!(
                    "[Origin] Trigger id is not header-safe, minting with no claim: {dropped:?}"
                );
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

/// Distinguishes ordinary callers from authenticated subprocesses, whose token
/// may or may not carry a thread.
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
/// The verified token supplies the source thread. A failed verification returns
/// `NotSubprocess` and uses ordinary device or API resolution.
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

/// Build the environment that attributes subprocess HTTP callbacks. Script and
/// agent spawns share this helper.
///
/// The token authenticates only the thread it was minted for.
/// `MessageOrigin::Api.source_thread_id` therefore comes from the token.
///
/// A threadless spawn gets a threadless token. A thread-bound spawn also gets
/// `LUCIDOS_THREAD_ID` for local consumers such as the Python shim and CLI.
///
/// `emitting_trigger_id` travels only with the trigger fire. `chain_depth` also
/// travels with work the fire hands off.
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

/// Tell subprocesses which processes and port belong to their host. Stale-port
/// cleanup uses these values to protect the engine and frontend.
/// - `LUCIDOS_HOST_PID` identifies the engine.
/// - `LUCIDOS_FRONTEND_PID` identifies the build watcher while it holds the
///   lock beside `LUCIDOS_STATIC_DIR`.
/// - `LUCIDOS_API_PORT` forwards the engine's configured port when present.
/// - `LUCIDOS_API_BASE_URL` gives gateway-spawned subprocesses the engine's
///   loopback HTTP URL (ADR 0014). The gateway port in `.lucidos/ports` would
///   require the workspace slug.
pub fn host_protection_env_vars() -> Vec<(&'static str, String)> {
    let mut vars: Vec<(&'static str, String)> = Vec::with_capacity(4);
    vars.push(("LUCIDOS_HOST_PID", std::process::id().to_string()));
    if let Some(pid) = std::env::var_os("LUCIDOS_STATIC_DIR")
        .and_then(|dir| build_watch_pid(std::path::Path::new(&dir)))
    {
        vars.push(("LUCIDOS_FRONTEND_PID", pid.to_string()));
    }
    if let Ok(api_port) = std::env::var("LUCIDOS_API_PORT") {
        if !api_port.is_empty() {
            // The workspace gateway sets `LUCIDOS_BIND_LOOPBACK` when it spawns
            // the engine. Its subprocesses use the engine's plain HTTP loopback
            // port.
            let behind_gateway = std::env::var("LUCIDOS_BIND_LOOPBACK")
                .is_ok_and(|v| crate::core::prefs::env_switch_is_on(&v));
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

/// The pid of the build-watch republishing `static_dir`, while one runs. Parsed
/// as a `u32`, so a junk pidfile cannot inject a newline into the env var and
/// silently disable the hook's match.
pub fn build_watch_pid(static_dir: &std::path::Path) -> Option<u32> {
    lucidos_owner_lock::build_watch::live_pid(static_dir.parent()?)
}
