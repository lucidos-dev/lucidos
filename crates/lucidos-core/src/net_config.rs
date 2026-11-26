pub const API_V1_PREFIX: &str = "/api/v1";

use serde::Deserialize;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindChoice {
    Loopback,
    All,
    Address(IpAddr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkToml {
    pub gateway_bind: Option<String>,
    pub engine_inherit: bool,
}

impl Default for NetworkToml {
    fn default() -> Self {
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

pub fn network_toml_path() -> Option<PathBuf> {
    lucidos_home::private_entry(lucidos_home::private::NETWORK_TOML)
}

pub fn read_network_toml() -> NetworkToml {
    let Some(path) = network_toml_path() else {
        return NetworkToml::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(contents) => parse_network_toml(&contents),
        Err(e) => {
            if let Some(warning) = read_failure_warning(&path, &e) {
                crate::log_legacy!("{warning}");
            }
            NetworkToml::default()
        }
    }
}

fn read_failure_warning(path: &Path, e: &std::io::Error) -> Option<String> {
    match e.kind() {
        std::io::ErrorKind::NotFound => None,
        _ => Some(format!(
            "[NetConfig] cannot read {} ({e}); using safe defaults (loopback)",
            path.display()
        )),
    }
}

pub fn parse_network_toml(contents: &str) -> NetworkToml {
    match toml::from_str::<RawNetworkToml>(contents) {
        Ok(raw) => NetworkToml {
            gateway_bind: raw.gateway.and_then(|g| g.bind),
            engine_inherit: raw.engine.map(|e| e.inherit).unwrap_or(true),
        },
        Err(e) => {
            crate::log_legacy!(
                "[NetConfig] malformed network.toml ({e}); using safe defaults (loopback)"
            );
            NetworkToml::default()
        }
    }
}

pub fn parse_bind_value(value: &str) -> BindChoice {
    match value.trim().to_ascii_lowercase().as_str() {
        "loopback" | "" => BindChoice::Loopback,
        "all" => BindChoice::All,
        _ => match value.trim().parse::<IpAddr>() {
            Ok(ip) => BindChoice::Address(ip),
            Err(_) => {
                crate::log_legacy!(
                    "[NetConfig] invalid bind address '{value}' — falling back to loopback only"
                );
                BindChoice::Loopback
            }
        },
    }
}

fn truthy(value: &str) -> bool {
    crate::core::prefs::env_switch_is_on(value)
}

pub fn validate_bind_input(value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "loopback" | "all" => Ok(()),
        _ => trimmed.parse::<IpAddr>().map(|_| ()).map_err(|_| {
            format!("'{value}' is not a valid bind — use 'loopback', 'all', or an IP address")
        }),
    }
}

// INVARIANT: ---------------------------------------------------------------------------
// Wire scheme (http vs https) — the SINGLE source of truth.
// ---------------------------------------------------------------------------
//
// A Lucidos process serves TLS on its own port iff BOTH `LUCIDOS_TLS_CERT` and
// `LUCIDOS_TLS_KEY` point at non-empty paths. Every caller that needs that
// scheme MUST resolve it through [`tls_scheme`] / [`tls_scheme_from`] (or the
// constants below), never by re-deriving from `LUCIDOS_TLS_*` inline. A call to
// the gateway has no scheme to choose: every gateway serves its loopback CA on
// loopback (`lucidos_local_token::client::origin`).

pub const SCHEME_HTTP: &str = "http";
pub const SCHEME_HTTPS: &str = "https";

pub fn tls_scheme_from(cert: Option<&str>, key: Option<&str>) -> &'static str {
    match (cert, key) {
        (Some(c), Some(k)) if !c.trim().is_empty() && !k.trim().is_empty() => SCHEME_HTTPS,
        _ => SCHEME_HTTP,
    }
}

pub fn tls_scheme() -> &'static str {
    tls_scheme_from(
        std::env::var("LUCIDOS_TLS_CERT").ok().as_deref(),
        std::env::var("LUCIDOS_TLS_KEY").ok().as_deref(),
    )
}

pub fn resolve_engine_bind(
    loopback_signal: bool,
    bind_addr_env: Option<&str>,
    bind_all_env: Option<&str>,
    inherit: bool,
    gateway_bind: Option<&str>,
    per_workspace_bind: Option<&str>,
) -> BindChoice {
    // INVARIANT: 1. Behind-gateway / packaged floor — never face the network.
    if loopback_signal {
        return BindChoice::Loopback;
    }
    if let Some(addr) = bind_addr_env {
        let trimmed = addr.trim();
        if !trimmed.is_empty() {
            match trimmed.parse::<IpAddr>() {
                Ok(ip) => return BindChoice::Address(ip),
                Err(_) => crate::log_legacy!(
                    "[NetConfig] invalid LUCIDOS_BIND_ADDR '{trimmed}' — ignoring, falling back"
                ),
            }
        }
    }
    if bind_all_env.map(truthy).unwrap_or(false) {
        return BindChoice::All;
    }
    let configured = if inherit {
        gateway_bind
    } else {
        per_workspace_bind
    };
    match configured {
        Some(s) if !s.trim().is_empty() => parse_bind_value(s),
        _ => BindChoice::Loopback,
    }
}

/// INVARIANT: The primary socket address for a resolved choice. `All` uses `[::]`
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

/// INVARIANT: Every socket address the server must listen on for a resolved choice.
///
/// A specific `Address(ip)` binds a single socket to that one IP, which
/// **excludes loopback** — but the gateway always reaches this engine over
/// `127.0.0.1:<port>` (proxy + health probe), the dev scripts POST the control
/// API over loopback, and the engine's own Apply-restart callback hits the
/// gateway over loopback. So an `Address` ALSO binds IPv4 loopback; otherwise
/// every co-located, intra-host caller is refused. A choice that already covers
/// loopback binds a single socket ([`covers_loopback`]). Never empty.
pub fn bind_socket_addrs(choice: &BindChoice, port: u16) -> Vec<SocketAddr> {
    let primary = bind_socket_addr(choice, port);
    if covers_loopback(choice) {
        vec![primary]
    } else {
        vec![primary, SocketAddr::from((Ipv4Addr::LOCALHOST, port))]
    }
}

fn covers_loopback(choice: &BindChoice) -> bool {
    match choice {
        BindChoice::Loopback | BindChoice::All => true,
        BindChoice::Address(ip) => ip.is_unspecified() || *ip == Ipv4Addr::LOCALHOST,
    }
}

pub fn bind_scope_label(choice: &BindChoice) -> String {
    match choice {
        BindChoice::Loopback => "loopback only".to_string(),
        BindChoice::All => "all interfaces, dual-stack".to_string(),
        BindChoice::Address(ip) => format!("address {ip} + loopback"),
    }
}

/// INVARIANT: This machine's tailnet IPv4 as Tailscale reports it, for the Settings
/// hint/placeholder. `None` when Tailscale is absent, not joined, or holds no
/// such address on this host (userspace networking).
///
/// **A display hint only.** It never influences what this process binds to; the
/// resolvers above own that, and they fail safe to loopback.
///
/// Asking Tailscale is bounded blocking I/O, so it runs off the async workers.
pub async fn detect_tailscale_ipv4() -> Option<String> {
    tokio::task::spawn_blocking(lucidos_tailscale::bindable_tailnet_ipv4)
        .await
        .ok()
        .flatten()
        .map(|ip| ip.to_string())
}

/// INVARIANT: Install the one rustls crypto provider this crate uses.
///
/// **Nothing that opens a TLS connection works before this runs.** Several
/// crates in the tree enable rustls' `aws-lc-rs` feature and several enable
/// `ring`, so rustls refuses to pick one and panics at the first handshake.
/// `reqwest` never hits it, because its own feature set builds a config per
/// client. A bare `connect_async` does.
///
/// Idempotent. An `Err` means somebody already installed one, which is the
/// outcome this wants: a second caller must not panic just for asking.
///
/// Call it from every entry point that reaches the network, including a test
/// that does. The engine binary calls it first thing in `run`.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
    const TAILNET: &str = "100.64.0.1";

    #[test]
    fn default_is_loopback() {
        let c = resolve_engine_bind(false, None, None, true, None, None);
        assert_eq!(c, BindChoice::Loopback);
        assert!(bind_socket_addr(&c, 3000).ip().is_loopback());
    }

    #[test]
    fn loopback_signal_beats_everything() {
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
        let c = resolve_engine_bind(
            false,
            Some(TAILNET),
            Some("1"),
            true,
            Some("loopback"),
            None,
        );
        assert_eq!(c, BindChoice::Address(TAILNET.parse().unwrap()));
    }

    #[test]
    fn invalid_env_addr_falls_through_to_bool() {
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
        assert_eq!(
            bind_socket_addr(&c, 5173).ip(),
            IpAddr::V6(Ipv6Addr::UNSPECIFIED)
        );
    }

    #[test]
    fn inherit_true_uses_gateway_bind() {
        let c = resolve_engine_bind(false, None, None, true, Some(TAILNET), Some("loopback"));
        assert_eq!(c, BindChoice::Address(TAILNET.parse().unwrap()));
    }

    #[test]
    fn inherit_false_uses_per_workspace_bind() {
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

        let full = "[gateway]\nbind = \"100.64.0.1\"\n[engine]\ninherit = false\n";
        let parsed = parse_network_toml(full);
        assert_eq!(parsed.gateway_bind.as_deref(), Some(TAILNET));
        assert!(!parsed.engine_inherit);

        let partial = "[engine]\n";
        assert!(parse_network_toml(partial).engine_inherit);

        assert_eq!(
            parse_network_toml("this is not toml = ="),
            NetworkToml::default()
        );
    }

    #[test]
    fn an_unreadable_config_warns_while_an_absent_one_stays_silent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().to_path_buf();

        let absent_path = dir.join("network.toml");
        let absent = std::fs::read_to_string(&absent_path).expect_err("nothing was written there");
        assert_eq!(absent.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(read_failure_warning(&absent_path, &absent), None);

        let unreadable = std::fs::read_to_string(&dir).expect_err("a directory is not a file");
        let warning = read_failure_warning(&dir, &unreadable)
            .expect("an unreadable config must not fall back to loopback in silence");
        assert!(warning.contains(&dir.display().to_string()), "{warning}");
        assert!(warning.contains(&unreadable.to_string()), "{warning}");
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
    fn tls_scheme_from_requires_both_nonempty() {
        assert_eq!(
            tls_scheme_from(Some("/c.pem"), Some("/k.pem")),
            SCHEME_HTTPS
        );
        assert_eq!(tls_scheme_from(None, Some("/k.pem")), SCHEME_HTTP);
        assert_eq!(tls_scheme_from(Some("/c.pem"), None), SCHEME_HTTP);
        assert_eq!(tls_scheme_from(None, None), SCHEME_HTTP);
        assert_eq!(tls_scheme_from(Some(""), Some("/k.pem")), SCHEME_HTTP);
        assert_eq!(tls_scheme_from(Some("/c.pem"), Some("  ")), SCHEME_HTTP);
        assert_eq!(tls_scheme_from(Some(""), Some("")), SCHEME_HTTP);
        assert_eq!(tls_scheme_from(Some("   "), Some("   ")), SCHEME_HTTP);
    }

    #[test]
    fn bind_socket_addrs_specific_address_also_binds_loopback() {
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
        let loop4: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert_eq!(
            bind_socket_addrs(&BindChoice::Address(loop4), 3000),
            vec![SocketAddr::from((Ipv4Addr::LOCALHOST, 3000))]
        );
    }

    #[test]
    fn a_wildcard_address_binds_one_socket() {
        for ip in [
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        ] {
            assert_eq!(
                bind_socket_addrs(&BindChoice::Address(ip), 3000),
                vec![SocketAddr::new(ip, 3000)]
            );
        }
    }

    #[test]
    fn bind_scope_label_address_notes_loopback() {
        assert_eq!(
            bind_scope_label(&BindChoice::Address(TAILNET.parse().unwrap())),
            format!("address {TAILNET} + loopback")
        );
    }
}
