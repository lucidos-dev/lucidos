//! Network bind resolution for the engine.
//!
//! Resolves the address the API server binds to, with a security-first
//! precedence (highest wins):
//!
//!   1. `LUCIDOS_BIND_LOOPBACK` truthy → **loopback floor**. This is the
//!      behind-gateway / packaged signal: a fronted engine must never face the
//!      network regardless of any other env, config, or per-workspace setting.
//!      The gateway is the sole network door in that topology.
//!   2. `LUCIDOS_BIND_ADDR` parses to an `IpAddr` → bind exactly that address.
//!      Set but unparseable → log a warning, ignore it, continue (never panic,
//!      never silently widen the bind).
//!   3. `LUCIDOS_BIND_ALL` truthy → all interfaces (`[::]`, dual-stack).
//!   4. Machine + per-workspace config: if `~/.lucidos/network.toml`'s
//!      `[engine] inherit` is true (the default) the engine binds the gateway's
//!      `[gateway] bind`; otherwise it binds this workspace's own `network_bind`
//!      preference. Either value maps `"loopback"|"all"|<IP>` (invalid IP →
//!      warn + loopback).
//!   5. default → loopback.
//!
//! Env knobs are the highest-precedence override (below only the loopback floor)
//! so existing launch scripts / e2e keep working unchanged. A malformed config
//! always fails safe to **loopback**, never to all-interfaces.
//!
//! The matching machine-global half (the gateway's own bind + writing
//! `network.toml`) lives in `crates/lucidos-gateway/src/net_config.rs`. The two
//! are deliberately duplicated rather than shared — the gateway has no
//! dependency on this crate (ADR 0014 §1).

use serde::Deserialize;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;

/// The per-workspace engine bind, stored as the `network_bind` preference
/// (an `INTERNAL` key — set via Settings → Network access, never by the agent).
pub const NETWORK_BIND_PREF_KEY: &str = "network_bind";

/// The resolved bind scope for a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindChoice {
    /// Loopback only (`127.0.0.1`) — the safe default.
    Loopback,
    /// All interfaces (`[::]`, dual-stack).
    All,
    /// A single explicit address (e.g. a Tailscale `100.x` tailnet IP).
    Address(IpAddr),
}

/// The machine-global `~/.lucidos/network.toml`, parsed with safe defaults.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkToml {
    /// `[gateway] bind` — `loopback` | `all` | `<IP>`. `None` = unset → loopback.
    pub gateway_bind: Option<String>,
    /// `[engine] inherit` — true (default) = engines bind the gateway's bind;
    /// false = each engine reads its own `network_bind` preference.
    pub engine_inherit: bool,
}

impl Default for NetworkToml {
    fn default() -> Self {
        // Absent / unreadable / malformed file → safe defaults: gateway
        // loopback, engines inherit. Never widens the bind.
        NetworkToml {
            gateway_bind: None,
            engine_inherit: true,
        }
    }
}

#[derive(Deserialize, Default)]
struct RawNetworkToml {
    gateway: Option<RawGateway>,
    engine: Option<RawEngine>,
}

#[derive(Deserialize, Default)]
struct RawGateway {
    bind: Option<String>,
}

#[derive(Deserialize)]
struct RawEngine {
    #[serde(default = "default_true")]
    inherit: bool,
}

fn default_true() -> bool {
    true
}

/// `~/.lucidos/network.toml`. `None` only when `HOME` is unset.
pub fn network_toml_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".lucidos/network.toml"))
}

/// Read + parse the machine-global config. Any failure (missing file, unreadable,
/// malformed TOML) yields safe defaults — never a panic, never a widened bind.
pub fn read_network_toml() -> NetworkToml {
    match network_toml_path().and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(contents) => parse_network_toml(&contents),
        None => NetworkToml::default(),
    }
}

/// Pure parse of `network.toml` contents. Malformed → safe defaults + a warning.
pub fn parse_network_toml(contents: &str) -> NetworkToml {
    match toml::from_str::<RawNetworkToml>(contents) {
        Ok(raw) => NetworkToml {
            gateway_bind: raw.gateway.and_then(|g| g.bind),
            engine_inherit: raw.engine.map(|e| e.inherit).unwrap_or(true),
        },
        Err(e) => {
            crate::log!(
                "[NetConfig] malformed ~/.lucidos/network.toml ({e}); using safe defaults (loopback)"
            );
            NetworkToml::default()
        }
    }
}

/// Map a config string (`"loopback" | "all" | <IP literal>`) to a [`BindChoice`].
/// An unparseable value warns and fails safe to loopback.
pub fn parse_bind_value(value: &str) -> BindChoice {
    match value.trim().to_ascii_lowercase().as_str() {
        "loopback" | "" => BindChoice::Loopback,
        "all" => BindChoice::All,
        _ => match value.trim().parse::<IpAddr>() {
            Ok(ip) => BindChoice::Address(ip),
            Err(_) => {
                crate::log!(
                    "[NetConfig] invalid bind address '{value}' — falling back to loopback only"
                );
                BindChoice::Loopback
            }
        },
    }
}

fn truthy(value: &str) -> bool {
    matches!(value.trim(), "1" | "true" | "yes" | "on")
}

/// Validate a user-supplied bind value for the network-config endpoint. Accepts
/// `loopback`, `all` (case-insensitive), or a parseable IP literal. Returns a
/// message suitable to hand back to the client on rejection — the server-side
/// mirror of the pane's client-side validation, so garbage never reaches the
/// stored preference / config file.
pub fn validate_bind_input(value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "loopback" | "all" => Ok(()),
        _ => trimmed.parse::<IpAddr>().map(|_| ()).map_err(|_| {
            format!("'{value}' is not a valid bind — use 'loopback', 'all', or an IP address")
        }),
    }
}

/// Resolve the engine bind. See the module doc for the precedence. Pure — the
/// caller supplies env values, the parsed `network.toml`, and (when not
/// inheriting) this workspace's `network_bind` preference.
pub fn resolve_engine_bind(
    loopback_signal: bool,
    bind_addr_env: Option<&str>,
    bind_all_env: Option<&str>,
    inherit: bool,
    gateway_bind: Option<&str>,
    per_workspace_bind: Option<&str>,
) -> BindChoice {
    // 1. Behind-gateway / packaged floor — never face the network.
    if loopback_signal {
        return BindChoice::Loopback;
    }
    // 2. Explicit env address.
    if let Some(addr) = bind_addr_env {
        let trimmed = addr.trim();
        if !trimmed.is_empty() {
            match trimmed.parse::<IpAddr>() {
                Ok(ip) => return BindChoice::Address(ip),
                Err(_) => crate::log!(
                    "[NetConfig] invalid LUCIDOS_BIND_ADDR '{trimmed}' — ignoring, falling back"
                ),
            }
        }
    }
    // 3. All-interfaces env bool.
    if bind_all_env.map(truthy).unwrap_or(false) {
        return BindChoice::All;
    }
    // 4. Config: inherit the gateway bind, or this workspace's own bind.
    let configured = if inherit {
        gateway_bind
    } else {
        per_workspace_bind
    };
    match configured {
        Some(s) if !s.trim().is_empty() => parse_bind_value(s),
        // 5. Default.
        _ => BindChoice::Loopback,
    }
}

/// The primary socket address for a resolved choice. `All` uses `[::]`
/// (dual-stack: macOS defaults `IPV6_V6ONLY=0`, so it serves IPv4 too). For an
/// explicit `Address` this is the configured IP only — use [`bind_socket_addrs`]
/// to also retain loopback.
pub fn bind_socket_addr(choice: &BindChoice, port: u16) -> SocketAddr {
    match choice {
        BindChoice::Loopback => SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        BindChoice::All => SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)),
        BindChoice::Address(ip) => SocketAddr::from((*ip, port)),
    }
}

/// Every socket address the server must listen on for a resolved choice.
///
/// A specific `Address(ip)` binds a single socket to that one IP, which
/// **excludes loopback** — but the gateway always reaches this engine over
/// `127.0.0.1:<port>` (proxy + health probe), the dev scripts POST the control
/// API over loopback, and the engine's own Apply-restart callback hits the
/// gateway over loopback. So an `Address` ALSO binds IPv4 loopback; otherwise
/// every co-located, intra-host caller is refused. `Loopback` and `All` already
/// cover loopback (`[::]` accepts IPv4 loopback as v4-mapped), so they bind a
/// single socket. Never empty.
pub fn bind_socket_addrs(choice: &BindChoice, port: u16) -> Vec<SocketAddr> {
    let primary = bind_socket_addr(choice, port);
    match choice {
        BindChoice::Loopback | BindChoice::All => vec![primary],
        BindChoice::Address(_) => {
            let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
            if primary == loopback {
                vec![primary]
            } else {
                vec![primary, loopback]
            }
        }
    }
}

/// Human-readable scope for the startup log — reports the actual address chosen.
/// A specific address notes the retained loopback (see [`bind_socket_addrs`]).
pub fn bind_scope_label(choice: &BindChoice) -> String {
    match choice {
        BindChoice::Loopback => "loopback only".to_string(),
        BindChoice::All => "all interfaces, dual-stack".to_string(),
        BindChoice::Address(ip) => format!("address {ip} + loopback"),
    }
}

/// Is `ip` a Tailscale-range (CGNAT `100.64.0.0/10`) IPv4 literal?
fn is_tailnet_ipv4(ip: &str) -> bool {
    matches!(
        ip.trim().parse::<Ipv4Addr>(),
        Ok(v4) if v4.octets()[0] == 100 && (64..=127).contains(&v4.octets()[1])
    )
}

/// Best-effort detection of this machine's Tailscale `100.x` IPv4, for the
/// Settings hint/placeholder. Shells out to `tailscale ip -4` with a short
/// timeout; any failure (no binary, not signed in, timeout) yields `None` and
/// the UI falls back to a generic placeholder.
pub async fn detect_tailscale_ipv4() -> Option<String> {
    use tokio::process::Command;
    use tokio::time::{timeout, Duration};

    let output = timeout(
        Duration::from_millis(1500),
        Command::new("tailscale").arg("ip").arg("-4").output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| is_tailnet_ipv4(line))
        .map(|line| line.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
    const TAILNET: &str = "100.101.71.58";

    #[test]
    fn default_is_loopback() {
        // No env, no config, no pref → loopback. The security floor.
        let c = resolve_engine_bind(false, None, None, true, None, None);
        assert_eq!(c, BindChoice::Loopback);
        assert!(bind_socket_addr(&c, 3000).ip().is_loopback());
    }

    #[test]
    fn loopback_signal_beats_everything() {
        // Behind-gateway floor wins over BIND_ADDR, BIND_ALL, and config.
        let c = resolve_engine_bind(
            true,
            Some(TAILNET),
            Some("1"),
            true,
            Some("all"),
            Some(TAILNET),
        );
        assert_eq!(c, BindChoice::Loopback);
    }

    #[test]
    fn explicit_env_addr_wins_over_bool_and_config() {
        let c = resolve_engine_bind(false, Some(TAILNET), Some("1"), true, Some("loopback"), None);
        assert_eq!(c, BindChoice::Address(TAILNET.parse().unwrap()));
    }

    #[test]
    fn invalid_env_addr_falls_through_to_bool() {
        // Garbage BIND_ADDR is ignored (warns), then BIND_ALL applies — never
        // a panic, never silently loopback when all was requested.
        let c = resolve_engine_bind(false, Some("not-an-ip"), Some("1"), true, None, None);
        assert_eq!(c, BindChoice::All);
    }

    #[test]
    fn invalid_env_addr_with_no_bool_uses_config() {
        let c = resolve_engine_bind(false, Some("999.999.0.1"), None, true, Some(TAILNET), None);
        assert_eq!(c, BindChoice::Address(TAILNET.parse().unwrap()));
    }

    #[test]
    fn bind_all_env_yields_all() {
        let c = resolve_engine_bind(false, None, Some("true"), true, None, None);
        assert_eq!(c, BindChoice::All);
        assert_eq!(bind_socket_addr(&c, 5173).ip(), IpAddr::V6(Ipv6Addr::UNSPECIFIED));
    }

    #[test]
    fn inherit_true_uses_gateway_bind() {
        let c = resolve_engine_bind(false, None, None, true, Some(TAILNET), Some("loopback"));
        assert_eq!(c, BindChoice::Address(TAILNET.parse().unwrap()));
    }

    #[test]
    fn inherit_false_uses_per_workspace_bind() {
        // The whole point of the split: with inherit=false the engine ignores
        // the gateway bind and uses its own workspace pref.
        let c = resolve_engine_bind(false, None, None, false, Some("all"), Some("loopback"));
        assert_eq!(c, BindChoice::Loopback);
        let c2 = resolve_engine_bind(false, None, None, false, Some("loopback"), Some(TAILNET));
        assert_eq!(c2, BindChoice::Address(TAILNET.parse().unwrap()));
    }

    #[test]
    fn inherit_false_with_unset_pref_is_loopback() {
        let c = resolve_engine_bind(false, None, None, false, Some(TAILNET), None);
        assert_eq!(c, BindChoice::Loopback);
    }

    #[test]
    fn malformed_config_value_fails_safe_to_loopback() {
        // A typo'd IP must NEVER widen to all.
        let c = resolve_engine_bind(false, None, None, true, Some("100.999.x"), None);
        assert_eq!(c, BindChoice::Loopback);
        assert_eq!(bind_socket_addr(&c, 3000).ip(), LOOPBACK);
    }

    #[test]
    fn parse_bind_value_keywords_and_ip() {
        assert_eq!(parse_bind_value("loopback"), BindChoice::Loopback);
        assert_eq!(parse_bind_value(" ALL "), BindChoice::All);
        assert_eq!(
            parse_bind_value(TAILNET),
            BindChoice::Address(TAILNET.parse().unwrap())
        );
        assert_eq!(parse_bind_value("garbage"), BindChoice::Loopback);
    }

    #[test]
    fn parse_network_toml_defaults_and_values() {
        assert_eq!(parse_network_toml(""), NetworkToml::default());

        let full = "[gateway]\nbind = \"100.101.71.58\"\n[engine]\ninherit = false\n";
        let parsed = parse_network_toml(full);
        assert_eq!(parsed.gateway_bind.as_deref(), Some(TAILNET));
        assert!(!parsed.engine_inherit);

        // [engine] present but no inherit key → defaults true.
        let partial = "[engine]\n";
        assert!(parse_network_toml(partial).engine_inherit);

        // Malformed → safe defaults.
        assert_eq!(parse_network_toml("this is not toml = ="), NetworkToml::default());
    }

    #[test]
    fn validate_bind_input_accepts_keywords_and_ip_rejects_garbage() {
        assert!(validate_bind_input("loopback").is_ok());
        assert!(validate_bind_input(" ALL ").is_ok());
        assert!(validate_bind_input(TAILNET).is_ok());
        assert!(validate_bind_input("::1").is_ok());
        assert!(validate_bind_input("nope").is_err());
        assert!(validate_bind_input("100.999.0.1").is_err());
        assert!(validate_bind_input("").is_err());
    }

    #[test]
    fn tailnet_ipv4_detection() {
        assert!(is_tailnet_ipv4("100.101.71.58"));
        assert!(is_tailnet_ipv4("100.64.0.1"));
        assert!(is_tailnet_ipv4("100.127.255.255"));
        assert!(!is_tailnet_ipv4("100.63.0.1")); // below the /10
        assert!(!is_tailnet_ipv4("100.128.0.1")); // above the /10
        assert!(!is_tailnet_ipv4("192.168.1.2"));
        assert!(!is_tailnet_ipv4("not-an-ip"));
    }

    #[test]
    fn bind_socket_addrs_specific_address_also_binds_loopback() {
        // The whole fix: a specific tailnet IP must NOT drop loopback, or the
        // gateway proxy/health probe + the dev scripts + the restart callback
        // (all over 127.0.0.1) are refused.
        let addrs = bind_socket_addrs(&BindChoice::Address(TAILNET.parse().unwrap()), 5173);
        assert_eq!(
            addrs,
            vec![
                SocketAddr::from((TAILNET.parse::<IpAddr>().unwrap(), 5173)),
                SocketAddr::from((Ipv4Addr::LOCALHOST, 5173)),
            ]
        );
    }

    #[test]
    fn bind_socket_addrs_loopback_and_all_are_single() {
        // Loopback is already loopback; All ([::]) accepts loopback IPv4 — neither
        // needs a second socket (a duplicate would EADDRINUSE on a normal launch).
        assert_eq!(
            bind_socket_addrs(&BindChoice::Loopback, 3000),
            vec![SocketAddr::from((Ipv4Addr::LOCALHOST, 3000))]
        );
        assert_eq!(
            bind_socket_addrs(&BindChoice::All, 3000),
            vec![SocketAddr::from((Ipv6Addr::UNSPECIFIED, 3000))]
        );
    }

    #[test]
    fn bind_socket_addrs_dedupes_explicit_loopback() {
        // An explicit 127.0.0.1 Address must not bind the same socket twice.
        let loop4: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert_eq!(
            bind_socket_addrs(&BindChoice::Address(loop4), 3000),
            vec![SocketAddr::from((Ipv4Addr::LOCALHOST, 3000))]
        );
    }

    #[test]
    fn bind_scope_label_address_notes_loopback() {
        assert_eq!(
            bind_scope_label(&BindChoice::Address(TAILNET.parse().unwrap())),
            format!("address {TAILNET} + loopback")
        );
    }
}
