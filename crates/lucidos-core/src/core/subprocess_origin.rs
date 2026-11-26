use crate::core::hex;
use std::sync::OnceLock;
use uuid::Uuid;

/// INVARIANT: Header carrying the originating browser/device id on mutating endpoints
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

pub const HEADER_DEVICE_LABEL: &str = "x-lucidos-device-label";

pub const HEADER_AGENT_ORIGIN_TOKEN: &str = "x-lucidos-agent-origin-token";

/// INVARIANT: Header carrying the workspace the caller believes it is talking to: the
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

pub const ENV_AGENT_ORIGIN_TOKEN: &str = "LUCIDOS_AGENT_ORIGIN_TOKEN";

/// INVARIANT: Env-var name for the spawning thread id. Same engine/CLI mirror rule as
/// [`ENV_AGENT_ORIGIN_TOKEN`].
///
/// This is **not** how the engine learns the source thread: that comes from
/// the token itself (see the module docs). The variable stays because plenty
/// of subprocess-side code reads it for its own purposes: the Python runtime
/// shim, the Codex MCP child config, and `lucidos spawn-thread`'s
/// `caller_thread_id` default.
pub const ENV_SOURCE_THREAD_ID: &str = "LUCIDOS_THREAD_ID";

const ABSENT_SEGMENT: &str = "-";

const FIELD_SEPARATOR: char = '@';

static AGENT_ORIGIN_SECRET: OnceLock<String> = OnceLock::new();

pub fn init_agent_origin_secret(secret: String) {
    let _ = AGENT_ORIGIN_SECRET.set(secret);
}

fn origin_mac(prefix: &str) -> Option<hmac::Hmac<sha2::Sha256>> {
    use hmac::Mac;
    let secret = AGENT_ORIGIN_SECRET.get()?;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC accepts a key of any length");
    mac.update(prefix.as_bytes());
    Some(mac)
}

pub fn mint_agent_origin_token(
    thread_id: Option<Uuid>,
    chain_depth: u32,
    emitting_trigger_id: Option<&str>,
) -> Option<String> {
    mint_origin_token(thread_id, chain_depth, None, emitting_trigger_id)
}

fn mint_origin_token(
    thread_id: Option<Uuid>,
    chain_depth: u32,
    event_hook: Option<&str>,
    emitting_trigger_id: Option<&str>,
) -> Option<String> {
    use hmac::Mac;
    let thread = match thread_id {
        Some(tid) => tid.to_string(),
        None => ABSENT_SEGMENT.to_string(),
    };
    let hook = match event_hook {
        Some(slug) if !is_header_safe_field(slug) || slug == ABSENT_SEGMENT => {
            log_legacy!("[Origin] Event hook slug is not header-safe, minting no token: {slug:?}");
            return None;
        }
        Some(slug) => slug.to_string(),
        None => ABSENT_SEGMENT.to_string(),
    };
    let trigger = match emitting_trigger_id.filter(|id| is_header_safe_field(id)) {
        Some(id) => id.to_string(),
        None => {
            if let Some(dropped) = emitting_trigger_id {
                log_legacy!(
                    "[Origin] Trigger id is not header-safe, minting with no claim: {dropped:?}"
                );
            }
            ABSENT_SEGMENT.to_string()
        }
    };
    let prefix =
        [thread, chain_depth.to_string(), hook, trigger].join(&FIELD_SEPARATOR.to_string());
    let mac = hex::hex_lower(&origin_mac(&prefix)?.finalize().into_bytes());
    Some(format!("{prefix}.{mac}"))
}

fn is_header_safe_field(field: &str) -> bool {
    !field.is_empty() && field.bytes().all(|b| (b'!'..=b'~').contains(&b))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubprocessOrigin {
    NotSubprocess,
    Subprocess {
        source_thread_id: Option<Uuid>,
        chain_depth: u32,
        /// INVARIANT: The trigger whose fire this subprocess is, when it is one. Read off
        /// the signed prefix, so it is authenticated. See the module docs.
        ///
        /// A caller holding no minted token claims nothing, and suppresses
        /// nobody. That is the fail-open direction, and it is why the claim
        /// rides the token instead of a header a forger could set.
        emitting_trigger_id: Option<String>,
        event_hook: Option<String>,
    },
}

pub fn subprocess_origin(headers: &axum::http::HeaderMap) -> SubprocessOrigin {
    let Some(presented) = headers
        .get(HEADER_AGENT_ORIGIN_TOKEN)
        .and_then(|v| v.to_str().ok())
    else {
        return SubprocessOrigin::NotSubprocess;
    };
    // INVARIANT: `rsplit_once` rather than `split_once`: the MAC is hex and carries no
    // dot, so the LAST dot is the separator even when a trigger id carries one.
    let Some((prefix, presented_mac)) = presented.rsplit_once('.') else {
        return SubprocessOrigin::NotSubprocess;
    };
    let (Some(mac), Some(presented_bytes)) = (origin_mac(prefix), hex::hex_decode(presented_mac))
    else {
        return SubprocessOrigin::NotSubprocess;
    };
    if hmac::Mac::verify_slice(mac, &presented_bytes).is_err() {
        return SubprocessOrigin::NotSubprocess;
    }
    let mut fields = prefix.splitn(4, FIELD_SEPARATOR);
    let (Some(thread), Some(depth), Some(hook), Some(trigger)) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return SubprocessOrigin::NotSubprocess;
    };
    let Ok(chain_depth) = depth.parse::<u32>() else {
        return SubprocessOrigin::NotSubprocess;
    };
    let claim = |field: &str| (field != ABSENT_SEGMENT).then(|| field.to_string());
    let source_thread_id = match thread {
        ABSENT_SEGMENT => None,
        thread => match Uuid::parse_str(thread) {
            Ok(tid) => Some(tid),
            Err(_) => return SubprocessOrigin::NotSubprocess,
        },
    };
    SubprocessOrigin::Subprocess {
        source_thread_id,
        chain_depth,
        emitting_trigger_id: claim(trigger),
        event_hook: claim(hook),
    }
}

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

pub fn event_hook_origin_env_vars(chain_depth: u32, slug: &str) -> Vec<(&'static str, String)> {
    mint_origin_token(None, chain_depth, Some(slug), None)
        .map(|token| (ENV_AGENT_ORIGIN_TOKEN, token))
        .into_iter()
        .collect()
}

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
            // INVARIANT: The workspace gateway sets `LUCIDOS_BIND_LOOPBACK` when it spawns
            // the engine. Its subprocesses use the engine's plain HTTP loopback
            // port.
            let behind_gateway = std::env::var("LUCIDOS_BIND_LOOPBACK")
                .is_ok_and(|v| crate::core::prefs::env_switch_is_on(&v));
            if behind_gateway {
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

pub fn build_watch_pid(static_dir: &std::path::Path) -> Option<u32> {
    lucidos_owner_lock::build_watch::live_pid(static_dir.parent()?)
}
