//! Where the public ingress is, read from `tailscale serve status --json`.
//!
//! Read-only, always. The engine never re-arms a funnel and never runs a
//! mutating `tailscale` command, so an outage is reported rather than repaired.
//! `SERVE_STATUS_ARGS` is the whole argument vector, pinned by a test.
//!
//! Two maps have to agree before anything is probed. `AllowFunnel` says which
//! `host:port` is public. `Web[host:port].Handlers[path].Proxy` says what that
//! port forwards to. One hook port can be served on a funnel port and on a
//! tailnet-only port at once. Trusting either map alone then probes the wrong
//! one.
//!
//! Not serving and not knowing are told apart, because they call for opposite
//! answers. A funnel the user turned off retracts a standing outage. A daemon
//! that would not answer leaves one exactly as it was.

use std::process::Stdio;
use std::time::Duration;

/// The only `tailscale` invocation this engine makes.
pub const SERVE_STATUS_ARGS: &[&str] = &["serve", "status", "--json"];

/// How long the CLI gets. A wedged daemon must not hold the scheduler tick.
const SERVE_STATUS_TIMEOUT: Duration = Duration::from_secs(10);

/// The public front door of one webhook port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicIngress {
    /// The funnel hostname, used for TLS and SNI while the address is pinned.
    pub host: String,
    /// The public port, which is not the loopback port behind it.
    pub port: u16,
}

/// What the daemon says about one hook port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunnelState {
    /// A funnel carries the hook port, at this public front door.
    Serving(PublicIngress),
    /// The daemon answered, and no funnel carries the hook port.
    NotServed,
    /// The daemon could not be asked, so nothing is known.
    Unknown,
}

/// Ask the daemon where the funnel points.
///
/// `Unknown` covers no CLI, a non-zero exit, a timeout, and output that is not
/// the JSON we know.
pub async fn public_ingress(hook_port: u16) -> FunnelState {
    match serve_status_json().await {
        Some(json) => parse_serve_status(&json, hook_port),
        None => FunnelState::Unknown,
    }
}

/// Run the CLI and return its stdout.
///
/// Stdout only. A version-skew warning goes to stderr, so mixing the two would
/// turn clean JSON into something no parser accepts.
async fn serve_status_json() -> Option<String> {
    let binary = lucidos_tailscale::tailscale_binary();
    let child = tokio::process::Command::new(&binary)
        .args(SERVE_STATUS_ARGS)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    let output = match tokio::time::timeout(SERVE_STATUS_TIMEOUT, child).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => {
            log!("[WebhookIngress] tailscale serve status did not run: {e}");
            return None;
        }
        Err(_) => {
            log!("[WebhookIngress] tailscale serve status timed out");
            return None;
        }
    };
    if !output.status.success() {
        log!(
            "[WebhookIngress] tailscale serve status exited {}",
            output.status
        );
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Find the funnel entry that carries this hook port.
///
/// Both maps are keyed by the same `host:port` string, so the intersection is a
/// lookup rather than a match. The lowest such port wins, so two funnels on one
/// hook port pick the same one every cycle.
pub fn parse_serve_status(json: &str, hook_port: u16) -> FunnelState {
    let Ok(root) = serde_json::from_str::<serde_json::Value>(json) else {
        return FunnelState::Unknown;
    };
    // The CLI omits an empty map, and prints a bare `null` for an empty config.
    // A missing key is therefore the daemon saying no funnel is armed, which is
    // an answer. A key holding a shape we do not recognise is not.
    let allow_funnel = match root.get("AllowFunnel") {
        None | Some(serde_json::Value::Null) => return FunnelState::NotServed,
        Some(serde_json::Value::Object(entries)) => entries,
        Some(_) => return FunnelState::Unknown,
    };
    let web = root.get("Web").and_then(|w| w.as_object());

    let mut found: Option<PublicIngress> = None;
    for (key, allowed) in allow_funnel {
        if allowed != &serde_json::Value::Bool(true) {
            continue;
        }
        let Some((host, port)) = split_host_port(key) else {
            continue;
        };
        let Some(handlers) = web
            .and_then(|w| w.get(key))
            .and_then(|entry| entry.get("Handlers"))
            .and_then(|h| h.as_object())
        else {
            continue;
        };
        let serves_hook = handlers.values().any(|handler| {
            handler
                .get("Proxy")
                .and_then(|p| p.as_str())
                .is_some_and(|proxy| proxy_targets_port(proxy, hook_port))
        });
        if !serves_hook {
            continue;
        }
        let candidate = PublicIngress { host, port };
        if found.as_ref().is_none_or(|best| candidate.port < best.port) {
            found = Some(candidate);
        }
    }
    found.map_or(FunnelState::NotServed, FunnelState::Serving)
}

/// Split a `host:port` map key, keeping an IPv6 literal in one piece.
fn split_host_port(key: &str) -> Option<(String, u16)> {
    let (host, port) = key.rsplit_once(':')?;
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), port.parse().ok()?))
}

/// Does this handler forward to the hook socket?
///
/// The socket binds loopback only, so a proxy target anywhere else is some
/// other service that happens to share the port number.
fn proxy_targets_port(proxy: &str, hook_port: u16) -> bool {
    let authority = proxy
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(proxy);
    let authority = authority.split(['/', '?', '#']).next().unwrap_or(authority);
    let Some((host, port)) = split_host_port(authority) else {
        return false;
    };
    let loopback = matches!(host.as_str(), "127.0.0.1" | "localhost" | "[::1]" | "::1");
    loopback && port == hook_port
}

#[cfg(test)]
#[path = "funnel_tests.rs"]
mod tests;
