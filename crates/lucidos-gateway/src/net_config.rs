//! Network bind resolution for the gateway + read/write of the machine-global
//! `~/.lucidos/network.toml`.
//!
//! The gateway is the network-facing process, so its bind is **machine-global**
//! (it binds before any workspace is selected). Resolution precedence (highest
//! wins):
//!
//!   1. `LUCIDOS_GATEWAY_BIND_ADDR` parses to an `IpAddr` → bind exactly that.
//!      Set but unparseable → log a warning, ignore it, continue.
//!   2. `LUCIDOS_GATEWAY_BIND_ALL` truthy → all interfaces (`[::]`, dual-stack).
//!   3. `~/.lucidos/network.toml` `[gateway] bind` → `"loopback"|"all"|<IP>`
//!      (invalid IP → warn + loopback).
//!   4. loopback (the safe default — the gateway fronts every workspace API and
//!      its own destructive control plane, so a default launch is never exposed).
//!
//! This file ALSO owns reading + writing `network.toml` (the picker's machine-
//! global Network access control writes it through the gateway control plane).
//! The per-workspace engine half — `[engine] inherit` and each engine's own
//! `network_bind` preference — is resolved in
//! `crates/lucidos-engine/src/net_config.rs`. The **bind resolution** in the two
//! `net_config` modules is deliberately duplicated rather than shared: the
//! gateway has no dependency on `lucidos-engine` (ADR 0014 §1). Their
//! **Tailscale** half is not duplicated any more, because keeping two copies in
//! step is precisely what failed; it lives in the `lucidos-tailscale` crate,
//! which is small enough for the gateway to depend on.

use serde::Deserialize;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::PathBuf;

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

/// Does the machine-global config file exist? Used by the dev launch path /
/// `spawn_engine` to decide whether to apply the dev all-interfaces default or
/// step aside and let the resolvers + per-workspace prefs decide.
pub fn network_toml_exists() -> bool {
    network_toml_path().map(|p| p.exists()).unwrap_or(false)
}

/// Read + parse the machine-global config. Any failure yields safe defaults.
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

/// Render the config file with explanatory comments (the `toml` crate's
/// serializer drops comments, so we template it).
pub fn render_network_toml(gateway_bind: &str, engine_inherit: bool) -> String {
    format!(
        "# Lucidos machine-level network bind configuration.\n\
         # Read at process start by the gateway AND every directly-launched\n\
         # engine. Environment variables override this file. Changes take effect\n\
         # only after a gateway / engine restart (a live socket cannot re-bind).\n\
         #\n\
         # [gateway].bind   \"loopback\" (default, most secure) | \"all\" (LAN +\n\
         #                  tailnet) | a literal IP to bind exactly, e.g. your\n\
         #                  Tailscale 100.x address.\n\
         # [engine].inherit  true  = every workspace engine binds the gateway's\n\
         #                          bind; false = each engine reads its own\n\
         #                          per-workspace bind (Settings -> Network access).\n\
         \n\
         [gateway]\n\
         bind = \"{gateway_bind}\"\n\
         \n\
         [engine]\n\
         inherit = {engine_inherit}\n"
    )
}

/// Write `network.toml` atomically (temp + rename), creating `~/.lucidos` if
/// needed. Values are assumed already validated by [`validate_bind_input`].
pub fn write_network_toml(gateway_bind: &str, engine_inherit: bool) -> std::io::Result<()> {
    let path = network_toml_path()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME is not set"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, render_network_toml(gateway_bind, engine_inherit))?;
    std::fs::rename(&tmp, &path)
}

/// Map a config string to a [`BindChoice`]; unparseable → warn + loopback.
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

// ---------------------------------------------------------------------------
// Wire scheme (http vs https): the gateway's copy of the ONE rule.
// ---------------------------------------------------------------------------
//
// A Lucidos process serves TLS iff BOTH `LUCIDOS_TLS_CERT` and `LUCIDOS_TLS_KEY`
// point at non-empty paths. This mirrors `lucidos_engine::net_config`'s
// `tls_scheme_from` exactly, and it has to: the gateway decides what scheme to
// PROXY and HEALTH-PROBE a spawned engine on, while that engine decides what it
// SERVES with the engine-side copy. A rule that differs here by so much as a
// missing key is a scheme mismatch on a loopback hop, which is the recurring
// `error sending request for url` bug (ADR 0014, `.claude/rules/rust.md`
// § "Intra-host scheme"). Duplicated rather than shared because the gateway
// links no engine code (ADR 0014 §1), so keep the two in step.

/// Plain-HTTP scheme literal.
pub const SCHEME_HTTP: &str = "http";
/// TLS scheme literal.
pub const SCHEME_HTTPS: &str = "https";

/// Does a process configured with this cert/key pair serve TLS? True only when
/// BOTH are present and non-empty: a set-but-empty value is how a launch script
/// spells "unset", and treating it as TLS is what makes the two sides disagree.
pub fn serves_tls(cert: Option<&str>, key: Option<&str>) -> bool {
    matches!((cert, key), (Some(c), Some(k)) if !c.trim().is_empty() && !k.trim().is_empty())
}

/// [`serves_tls`] reading `LUCIDOS_TLS_CERT` / `LUCIDOS_TLS_KEY` from the
/// process environment: does THIS gateway serve TLS on its own port?
///
/// Every site that asks goes through here, so the listener and the cookie's
/// `Secure` flag cannot answer it differently. Mirrors the engine's
/// `net_config::tls_scheme`, which is the same wrapper over the same rule.
pub fn serves_tls_from_env() -> bool {
    serves_tls(
        std::env::var("LUCIDOS_TLS_CERT").ok().as_deref(),
        std::env::var("LUCIDOS_TLS_KEY").ok().as_deref(),
    )
}

/// Validate a user-supplied bind value for the control endpoint (server-side
/// mirror of the picker's client-side validation).
pub fn validate_bind_input(value: &str) -> Result<(), String> {
    let trimmed = value.trim();
    match trimmed.to_ascii_lowercase().as_str() {
        "loopback" | "all" => Ok(()),
        _ => trimmed.parse::<IpAddr>().map(|_| ()).map_err(|_| {
            format!("'{value}' is not a valid bind — use 'loopback', 'all', or an IP address")
        }),
    }
}

/// Resolve the gateway bind. See the module doc for the precedence. Pure — the
/// caller supplies env values + the parsed `[gateway] bind`.
pub fn resolve_gateway_bind(
    bind_addr_env: Option<&str>,
    bind_all_env: Option<&str>,
    gateway_bind: Option<&str>,
) -> BindChoice {
    if let Some(addr) = bind_addr_env {
        let trimmed = addr.trim();
        if !trimmed.is_empty() {
            match trimmed.parse::<IpAddr>() {
                Ok(ip) => return BindChoice::Address(ip),
                Err(_) => crate::log!(
                    "[NetConfig] invalid LUCIDOS_GATEWAY_BIND_ADDR '{trimmed}' — ignoring, falling back"
                ),
            }
        }
    }
    if bind_all_env.map(truthy).unwrap_or(false) {
        return BindChoice::All;
    }
    match gateway_bind {
        Some(s) if !s.trim().is_empty() => parse_bind_value(s),
        _ => BindChoice::Loopback,
    }
}

/// The primary socket address for a resolved choice. `All` uses `[::]`
/// (dual-stack). For an explicit `Address` this is the configured IP only — use
/// [`bind_socket_addrs`] to also retain loopback.
pub fn bind_socket_addr(choice: &BindChoice, port: u16) -> SocketAddr {
    match choice {
        BindChoice::Loopback => SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        BindChoice::All => SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)),
        BindChoice::Address(ip) => SocketAddr::from((*ip, port)),
    }
}

/// Every socket address the gateway must listen on for a resolved choice.
///
/// A specific `Address(ip)` binds a single socket to that one IP, which
/// **excludes loopback** — but the dev launch scripts POST the gateway's control
/// API over `localhost:<port>` (start/restart a workspace), and each spawned
/// engine's Apply-restart callback hits `http://127.0.0.1:<port>/~/...`. So an
/// `Address` ALSO binds IPv4 loopback; otherwise those co-located callers are
/// refused (and a workspace can never be told to start). `Loopback` and `All`
/// already cover loopback (`[::]` accepts IPv4 loopback as v4-mapped), so they
/// bind a single socket. Never empty.
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

/// [`bind_socket_addrs`] split by what a bind failure MEANS, which is not the
/// same question as which addresses to listen on.
///
/// `required` is the set the gateway cannot usefully run without: failing to
/// bind one is fatal, exactly as every address was before this split existed.
/// `optional` is the set that may simply not exist yet, so a failure there is
/// retried in the background rather than taken as the end of the process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindPlan {
    /// Fatal on failure. Never empty.
    pub required: Vec<SocketAddr>,
    /// Retried on failure, forever, with backoff. Usually empty.
    pub optional: Vec<SocketAddr>,
}

/// Split the addresses for a resolved choice into fatal and retryable.
///
/// **Only a configured `Address(ip)` is optional**, and only the `ip` half of
/// it: loopback stays required. That asymmetry is the whole point. A tailnet or
/// LAN address is a *reachability addition* whose interface may not be up when
/// launchd starts the service at boot (`tailscaled` assigns the `100.x` address
/// well after login), and binding it then fails with `EADDRNOTAVAIL`. Before
/// this split, that failure took the loopback listener down with it and the
/// packaged window sat on its "Starting Lucidos…" splash for two minutes waiting
/// for a gateway that had already exited. Loopback is what the desktop client,
/// each engine's Apply-restart callback and the dev control POSTs actually
/// speak, so a machine that can serve loopback can serve its user.
///
/// `Loopback` and `All` keep fail-fast: their single address is required. An
/// operator who asked to be reachable on all interfaces is owed an error when
/// that cannot happen, not a quiet demotion, and a failure there is a held port
/// rather than an absent interface.
pub fn bind_plan(choice: &BindChoice, port: u16) -> BindPlan {
    let addrs = bind_socket_addrs(choice, port);
    match choice {
        BindChoice::Loopback | BindChoice::All => BindPlan {
            required: addrs,
            optional: Vec::new(),
        },
        // `bind_socket_addrs` puts the configured address first and appends
        // loopback, and collapses the two when the configured address IS
        // loopback (leaving one entry, which is required).
        BindChoice::Address(_) => {
            let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
            let (required, optional) = addrs.into_iter().partition(|a| *a == loopback);
            BindPlan { required, optional }
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

/// Best-effort detection of this machine's Tailscale `100.x` IPv4, for the
/// picker's Network access hint. `None` when Tailscale is absent or logged out.
///
/// **A display hint only.** It never influences what this process binds to; the
/// resolvers above own that, and they fail safe to loopback.
///
/// Reads the interface list rather than running `tailscale ip -4`. The spawn was
/// the whole problem: the packaged gateway is started by Finder/launchd with
/// **no `PATH` at all**, so the bare name never resolved, the picker offered no
/// address to click, and Tailnet / IP became unselectable because an empty
/// address is not a valid bind. Absolute-path resolution fixed that on
/// 2026-07-31, but the deeper answer is that a machine already on a tailnet can
/// simply be asked, at the cost of two syscalls and no CLI whatsoever. See
/// `lucidos-tailscale`.
pub fn detect_tailscale_ipv4() -> Option<String> {
    lucidos_tailscale::tailnet_ipv4().map(|ip| ip.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAILNET: &str = "100.101.71.58";

    #[test]
    fn default_is_loopback() {
        assert_eq!(resolve_gateway_bind(None, None, None), BindChoice::Loopback);
        assert!(bind_socket_addr(&BindChoice::Loopback, 5251)
            .ip()
            .is_loopback());
    }

    #[test]
    fn explicit_env_addr_wins_over_bool_and_config() {
        let c = resolve_gateway_bind(Some(TAILNET), Some("1"), Some("loopback"));
        assert_eq!(c, BindChoice::Address(TAILNET.parse().unwrap()));
    }

    #[test]
    fn invalid_env_addr_falls_through_to_bool() {
        let c = resolve_gateway_bind(Some("nope"), Some("true"), None);
        assert_eq!(c, BindChoice::All);
    }

    #[test]
    fn bind_all_env_yields_all_dual_stack() {
        let c = resolve_gateway_bind(None, Some("yes"), None);
        assert_eq!(c, BindChoice::All);
        assert_eq!(
            bind_socket_addr(&c, 5251).ip(),
            IpAddr::V6(Ipv6Addr::UNSPECIFIED)
        );
    }

    #[test]
    fn config_bind_used_when_no_env() {
        assert_eq!(
            resolve_gateway_bind(None, None, Some(TAILNET)),
            BindChoice::Address(TAILNET.parse().unwrap())
        );
        assert_eq!(
            resolve_gateway_bind(None, None, Some("all")),
            BindChoice::All
        );
    }

    #[test]
    fn malformed_config_fails_safe_to_loopback() {
        assert_eq!(
            resolve_gateway_bind(None, None, Some("100.999.x")),
            BindChoice::Loopback
        );
    }

    #[test]
    fn render_then_parse_roundtrips() {
        let rendered = render_network_toml(TAILNET, false);
        let parsed = parse_network_toml(&rendered);
        assert_eq!(parsed.gateway_bind.as_deref(), Some(TAILNET));
        assert!(!parsed.engine_inherit);

        let rendered2 = render_network_toml("loopback", true);
        let parsed2 = parse_network_toml(&rendered2);
        assert_eq!(parsed2.gateway_bind.as_deref(), Some("loopback"));
        assert!(parsed2.engine_inherit);
    }

    #[test]
    fn parse_network_toml_defaults() {
        assert_eq!(parse_network_toml(""), NetworkToml::default());
        assert_eq!(parse_network_toml("garbage = = ="), NetworkToml::default());
    }

    /// Every cert/key pair the two mirrored copies must agree on, and the
    /// verdict. ADR 0014 §1 forbids the gateway linking engine code, so this
    /// table is the whole sync mechanism: it is the same table the engine's
    /// `tls_scheme_from_requires_both_nonempty` asserts, and a row added
    /// there is added here in the same change.
    const TLS_RULE_TABLE: &[(Option<&str>, Option<&str>, bool)] = &[
        (Some("/c.pem"), Some("/k.pem"), true),
        // A cert alone is NOT TLS: the engine needs both, so it serves http.
        (Some("/c.pem"), None, false),
        (None, Some("/k.pem"), false),
        (None, None, false),
        // Set-but-empty, or whitespace, is how a launch script spells "unset".
        (Some(""), Some("/k.pem"), false),
        (Some("/c.pem"), Some("  "), false),
        // BOTH empty is the reported one. `export LUCIDOS_TLS_CERT="$UNSET"`
        // produces it, and so does `scripts/lib/service_test.sh`.
        (Some(""), Some(""), false),
        (Some("   "), Some("   "), false),
    ];

    /// The gateway's copy of the scheme rule must agree with the engine's
    /// `tls_scheme_from` in every case, including the two that used to differ
    /// here: a cert with no key, and a set-but-empty value. Disagreeing means
    /// the gateway proxies https to an engine serving http.
    #[test]
    fn serves_tls_requires_both_cert_and_key_non_empty() {
        for (cert, key, expected) in TLS_RULE_TABLE {
            assert_eq!(
                serves_tls(*cert, *key),
                *expected,
                "cert {cert:?} + key {key:?} must resolve to serves_tls={expected}"
            );
        }
        // The two literals the caller picks between are the ones a peer speaks.
        assert_eq!((SCHEME_HTTP, SCHEME_HTTPS), ("http", "https"));
    }

    /// The env wrapper must read the two names the rest of Lucidos writes, and
    /// must answer them through [`serves_tls`]. A typo in either name would
    /// otherwise put the gateway on http forever, silently.
    #[test]
    fn serves_tls_from_env_reads_the_two_tls_names() {
        // These two names are process-global, and a dev shell really does
        // export them, so put back whatever was there.
        let restore = |name: &str, was: Option<std::ffi::OsString>| match was {
            Some(v) => std::env::set_var(name, v),
            None => std::env::remove_var(name),
        };
        let had_cert = std::env::var_os("LUCIDOS_TLS_CERT");
        let had_key = std::env::var_os("LUCIDOS_TLS_KEY");

        for (cert, key, expected) in TLS_RULE_TABLE {
            restore("LUCIDOS_TLS_CERT", cert.map(Into::into));
            restore("LUCIDOS_TLS_KEY", key.map(Into::into));
            assert_eq!(
                serves_tls_from_env(),
                *expected,
                "env cert {cert:?} + key {key:?} must resolve to {expected}"
            );
        }

        restore("LUCIDOS_TLS_CERT", had_cert);
        restore("LUCIDOS_TLS_KEY", had_key);
    }

    /// `var_os` answers "is this name set", which is a different question from
    /// "does this process serve TLS". Reaching for it is how both gateway
    /// sites drifted from the rule, so no site outside this module may.
    #[test]
    fn no_gateway_site_asks_whether_a_tls_name_is_merely_set() {
        // Reads the directory rather than naming files, so a module added
        // later is covered without anyone remembering to list it. A hand-kept
        // list that silently stops covering a file is the same as no list.
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut scanned: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&src).expect("the gateway's own src directory") {
            let path = entry.expect("a readable directory entry").path();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            // This module owns the rule, so it is the one place that may name
            // the variables. Everything else asks it.
            if !name.ends_with(".rs") || name == "net_config.rs" {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("a readable source file");
            for var in ["LUCIDOS_TLS_CERT", "LUCIDOS_TLS_KEY"] {
                assert!(
                    !source.contains(&format!("var_os(\"{var}\")")),
                    "{name} asks whether {var} is set; ask serves_tls_from_env instead"
                );
            }
            scanned.push(name);
        }
        // A scan that read the wrong tree, or nothing, must fail rather than
        // pass vacuously.
        for expected in ["server.rs", "stack.rs", "auth_api.rs"] {
            assert!(
                scanned.iter().any(|n| n == expected),
                "the scan never reached {expected}; it saw {scanned:?}"
            );
        }
    }

    #[test]
    fn validate_bind_input_accepts_keywords_and_ip() {
        assert!(validate_bind_input("loopback").is_ok());
        assert!(validate_bind_input("ALL").is_ok());
        assert!(validate_bind_input(TAILNET).is_ok());
        assert!(validate_bind_input("garbage").is_err());
        assert!(validate_bind_input("").is_err());
    }

    // Tailscale detection and CLI resolution moved to the `lucidos-tailscale`
    // crate, which owns their tests: one copy now covers the gateway, the
    // engine and the desktop app, instead of three that could drift.

    #[test]
    fn bind_socket_addrs_specific_address_also_binds_loopback() {
        // The gateway must stay reachable on 127.0.0.1 for the dev scripts' control
        // POSTs + each engine's restart callback, even when bound to a tailnet IP.
        let addrs = bind_socket_addrs(&BindChoice::Address(TAILNET.parse().unwrap()), 5251);
        assert_eq!(
            addrs,
            vec![
                SocketAddr::from((TAILNET.parse::<IpAddr>().unwrap(), 5251)),
                SocketAddr::from((Ipv4Addr::LOCALHOST, 5251)),
            ]
        );
    }

    #[test]
    fn bind_socket_addrs_loopback_and_all_are_single() {
        assert_eq!(
            bind_socket_addrs(&BindChoice::Loopback, 5251),
            vec![SocketAddr::from((Ipv4Addr::LOCALHOST, 5251))]
        );
        assert_eq!(
            bind_socket_addrs(&BindChoice::All, 5251),
            vec![SocketAddr::from((Ipv6Addr::UNSPECIFIED, 5251))]
        );
    }

    #[test]
    fn bind_socket_addrs_dedupes_explicit_loopback() {
        let loop4: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert_eq!(
            bind_socket_addrs(&BindChoice::Address(loop4), 5251),
            vec![SocketAddr::from((Ipv4Addr::LOCALHOST, 5251))]
        );
    }

    #[test]
    fn bind_plan_makes_only_the_configured_address_optional() {
        // The failure this split exists for: the tailnet address is not up at
        // boot, and loopback must serve anyway.
        let plan = bind_plan(&BindChoice::Address(TAILNET.parse().unwrap()), 5251);
        assert_eq!(
            plan.required,
            vec![SocketAddr::from((Ipv4Addr::LOCALHOST, 5251))]
        );
        assert_eq!(
            plan.optional,
            vec![SocketAddr::from((TAILNET.parse::<IpAddr>().unwrap(), 5251))]
        );
    }

    #[test]
    fn bind_plan_keeps_loopback_and_all_fail_fast() {
        // An operator who asked for all interfaces is owed an error, not a quiet
        // demotion to loopback, so neither of these has anything retryable.
        for choice in [BindChoice::Loopback, BindChoice::All] {
            let plan = bind_plan(&choice, 5251);
            assert_eq!(plan.required, bind_socket_addrs(&choice, 5251));
            assert!(
                plan.optional.is_empty(),
                "{choice:?} must have no retryable address"
            );
        }
    }

    #[test]
    fn bind_plan_of_explicit_loopback_is_required_and_not_duplicated() {
        let plan = bind_plan(&BindChoice::Address(IpAddr::V4(Ipv4Addr::LOCALHOST)), 5251);
        assert_eq!(
            plan.required,
            vec![SocketAddr::from((Ipv4Addr::LOCALHOST, 5251))]
        );
        assert!(plan.optional.is_empty());
    }

    #[test]
    fn every_bind_plan_has_something_required() {
        // A plan with an empty `required` would serve nothing at all while
        // reporting success, which is worse than the fail-fast it replaced.
        for choice in [
            BindChoice::Loopback,
            BindChoice::All,
            BindChoice::Address(TAILNET.parse().unwrap()),
            BindChoice::Address(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        ] {
            assert!(
                !bind_plan(&choice, 5251).required.is_empty(),
                "{choice:?} must bind something fatally"
            );
        }
    }

    #[test]
    fn an_unassigned_address_fails_to_bind_while_loopback_succeeds() {
        // Pins the OS behaviour the whole fix rests on, rather than assuming it:
        // binding an address this machine does not hold fails with
        // `AddrNotAvailable` (the `Os { code: 49 }` from the reported stall),
        // and it fails INDEPENDENTLY of the loopback bind in the same plan.
        // 192.0.2.1 is TEST-NET-1 (RFC 5737), assigned on no machine.
        let plan = bind_plan(&BindChoice::Address("192.0.2.1".parse().unwrap()), 0);
        let required = std::net::TcpListener::bind(plan.required[0]);
        assert!(
            required.is_ok(),
            "loopback must bind even though the configured address cannot"
        );
        let optional = std::net::TcpListener::bind(plan.optional[0]);
        assert_eq!(
            optional.err().map(|e| e.kind()),
            Some(std::io::ErrorKind::AddrNotAvailable),
            "an unassigned address must be reported as unavailable, not held"
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
