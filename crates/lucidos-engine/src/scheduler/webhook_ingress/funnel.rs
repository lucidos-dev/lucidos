//! INVARIANT: Where the public ingress is, read from `tailscale serve status --json`.
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

pub const SERVE_STATUS_ARGS: &[&str] = &["serve", "status", "--json"];

const SERVE_STATUS_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicIngress {
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FunnelState {
    Serving(PublicIngress),
    NotServed,
    Unknown,
}

pub async fn public_ingress(hook_port: u16) -> FunnelState {
    // Stdout alone carries JSON. Version-skew warnings go to stderr.
    let binary = lucidos_tailscale::tailscale_binary();
    let child = tokio::process::Command::new(&binary)
        .args(SERVE_STATUS_ARGS)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();

    let output = match tokio::time::timeout(SERVE_STATUS_TIMEOUT, child).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => return FunnelState::NotServed,
        Ok(Err(e)) => {
            log_legacy!("[WebhookIngress] tailscale serve status did not run: {e}");
            return FunnelState::Unknown;
        }
        Err(_) => {
            log_legacy!("[WebhookIngress] tailscale serve status timed out");
            return FunnelState::Unknown;
        }
    };
    if !output.status.success() {
        log_legacy!(
            "[WebhookIngress] tailscale serve status exited {}",
            output.status
        );
        return FunnelState::Unknown;
    }
    parse_serve_status(&String::from_utf8_lossy(&output.stdout), hook_port)
}

/// INVARIANT: Find the funnel entry that carries this hook port.
///
/// Both maps are keyed by the same `host:port` string, so the intersection is a
/// lookup rather than a match. The lowest such port wins, so two funnels on one
/// hook port pick the same one every cycle.
pub fn parse_serve_status(json: &str, hook_port: u16) -> FunnelState {
    let Ok(root) = serde_json::from_str::<serde_json::Value>(json) else {
        return FunnelState::Unknown;
    };
    if !root.is_object() && !root.is_null() {
        return FunnelState::Unknown;
    }
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

fn split_host_port(key: &str) -> Option<(String, u16)> {
    let (host, port) = key.rsplit_once(':')?;
    if host.is_empty() {
        return None;
    }
    Some((host.to_string(), port.parse().ok()?))
}

/// INVARIANT: Does this handler forward to the hook socket?
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
