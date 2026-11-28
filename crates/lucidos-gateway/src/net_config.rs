use lucidos_home::private::{NETWORK_TOML, NETWORK_TOML_LOCK};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindChoice {
    Loopback,
    All,
    Address(IpAddr),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NetworkToml {
    pub gateway_bind: Option<String>,
    pub external_addresses: BTreeMap<u16, String>,
}

#[derive(Deserialize, Default)]
struct RawNetworkToml {
    gateway: Option<RawGateway>,
    external_address: Option<BTreeMap<String, toml::Value>>,
}

#[derive(Deserialize, Default)]
struct RawGateway {
    bind: Option<String>,
}

pub fn network_toml_path() -> Option<PathBuf> {
    lucidos_home::private_entry(NETWORK_TOML)
}

pub fn read_network_toml() -> NetworkToml {
    match network_toml_path() {
        Some(path) => read_network_toml_at(&path),
        None => NetworkToml::default(),
    }
}

fn read_network_toml_at(path: &Path) -> NetworkToml {
    match std::fs::read_to_string(path) {
        Ok(contents) => parse_network_toml(&contents),
        Err(e) => {
            if let Some(warning) = read_failure_warning(path, &e) {
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
            external_addresses: raw
                .external_address
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(port, origin)| parse_external_entry(&port, &origin))
                .collect(),
        },
        Err(e) => {
            crate::log_legacy!(
                "[NetConfig] malformed network.toml ({e}); using safe defaults (loopback)"
            );
            NetworkToml::default()
        }
    }
}

pub fn render_network_toml(net: &NetworkToml) -> String {
    let gateway_bind = net.gateway_bind.as_deref().unwrap_or("loopback");
    let external_table = if net.external_addresses.is_empty() {
        String::new()
    } else {
        let entries: String = net
            .external_addresses
            .iter()
            .map(|(port, origin)| format!("{port} = \"{origin}\"\n"))
            .collect();
        format!("\n[external_address]\n{entries}")
    };
    format!(
        "# Lucidos machine-level network bind configuration.\n\
         # Read at process start by the gateway. Every engine binds loopback.\n\
         # Environment variables override this file. A bind change takes effect\n\
         # only after a gateway restart (a live socket cannot re-bind).\n\
         #\n\
         # [gateway].bind   \"loopback\" (default, most secure) | \"all\" (LAN +\n\
         #                  tailnet) | a literal IP to bind exactly, e.g. your\n\
         #                  Tailscale 100.x address.\n\
         # [external_address]  optional, per gateway port: the origin other\n\
         #                  devices reach that gateway at, such as\n\
         #                  5252 = \"https://lucidos.example.com\". It goes in\n\
         #                  the pairing QR, and is read live (no restart).\n\
         \n\
         [gateway]\n\
         bind = \"{gateway_bind}\"\n\
         {external_table}"
    )
}

pub fn update_network_toml(change: impl FnOnce(&mut NetworkToml)) -> std::io::Result<()> {
    let path = lucidos_home::ensure_private_entry(NETWORK_TOML)?;
    let dir = path.parent().expect("a private entry has a parent");
    update_network_toml_in(dir, change)
}

fn update_network_toml_in(
    dir: &Path,
    change: impl FnOnce(&mut NetworkToml),
) -> std::io::Result<()> {
    use fs2::FileExt;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(NETWORK_TOML_LOCK))?;
    lock.lock_exclusive()?;
    let path = dir.join(NETWORK_TOML);
    let mut net = read_network_toml_at(&path);
    change(&mut net);
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    std::io::Write::write_all(&mut tmp, render_network_toml(&net).as_bytes())?;
    tmp.persist(&path).map_err(|e| e.error)?;
    Ok(())
}

pub const EXTERNAL_ADDRESS_ENV: &str = "LUCIDOS_EXTERNAL_ADDRESS";

pub fn valid_external_address(raw: &str) -> Result<String, String> {
    let origin = crate::pairing_qr::valid_origin(raw).ok_or_else(|| {
        format!(
            "'{}' is not an origin. Use a scheme, a host and an optional port, \
             such as https://lucidos.example.com or http://10.8.0.2:5252, with no path",
            raw.trim()
        )
    })?;
    let parsed = reqwest::Url::parse(origin)
        .map_err(|e| format!("'{origin}' is not an origin the gateway can open ({e})"))?;
    let host = parsed.host_str().unwrap_or_default();
    let ip = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .ok()
        .map(|ip| ip.to_canonical());
    let loopback = host == "localhost"
        || host.ends_with(".localhost")
        || ip.is_some_and(|ip| ip.is_loopback());
    if loopback {
        return Err(format!(
            "'{origin}' only reaches this machine. Use an address other devices can open"
        ));
    }
    let unroutable = ip.is_some_and(|ip| match ip {
        IpAddr::V4(v4) => v4.is_link_local() || v4.is_unspecified(),
        IpAddr::V6(v6) => v6.is_unicast_link_local() || v6.is_unspecified(),
    });
    if unroutable {
        return Err(format!(
            "'{origin}' is a link-local or unspecified address, which no other device can open"
        ));
    }
    Ok(origin.to_string())
}

fn parse_external_entry(port: &str, origin: &toml::Value) -> Option<(u16, String)> {
    let Ok(port) = port.parse::<u16>() else {
        crate::log_legacy!(
            "[NetConfig] ignoring external address key '{port}': not a gateway port"
        );
        return None;
    };
    let Some(origin) = origin.as_str() else {
        crate::log_legacy!(
            "[NetConfig] ignoring the external address for port {port}: not a string"
        );
        return None;
    };
    let origin = external_address_or_warn(origin, "network.toml")?;
    Some((port, origin))
}

fn external_address_or_warn(raw: &str, source: &str) -> Option<String> {
    if raw.trim().is_empty() {
        return None;
    }
    valid_external_address(raw)
        .map_err(|why| {
            crate::log_legacy!("[NetConfig] ignoring the external address in {source}: {why}")
        })
        .ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ExternalAddressSource {
    Unset,
    File,
    Env,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalAddress {
    pub origin: Option<String>,
    pub source: ExternalAddressSource,
}

impl ExternalAddress {
    pub fn locked(&self) -> bool {
        self.source == ExternalAddressSource::Env
    }
}

pub fn resolve_external_address(env: Option<&str>, file: Option<&str>) -> ExternalAddress {
    if let Some(origin) = env.and_then(|raw| external_address_or_warn(raw, EXTERNAL_ADDRESS_ENV)) {
        return ExternalAddress {
            origin: Some(origin),
            source: ExternalAddressSource::Env,
        };
    }
    match file {
        Some(origin) => ExternalAddress {
            origin: Some(origin.to_string()),
            source: ExternalAddressSource::File,
        },
        None => ExternalAddress {
            origin: None,
            source: ExternalAddressSource::Unset,
        },
    }
}

pub fn external_address(port: u16) -> ExternalAddress {
    let net = read_network_toml();
    resolve_external_address(
        std::env::var(EXTERNAL_ADDRESS_ENV).ok().as_deref(),
        net.external_addresses.get(&port).map(String::as_str),
    )
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
    matches!(value.trim(), "1" | "true" | "yes" | "on")
}

pub const SCHEME_HTTP: &str = "http";
pub const SCHEME_HTTPS: &str = "https";

pub fn serves_tls(cert: Option<&str>, key: Option<&str>) -> bool {
    matches!((cert, key), (Some(c), Some(k)) if !c.trim().is_empty() && !k.trim().is_empty())
}

pub fn serves_tls_from_env() -> bool {
    serves_tls(
        std::env::var("LUCIDOS_TLS_CERT").ok().as_deref(),
        std::env::var("LUCIDOS_TLS_KEY").ok().as_deref(),
    )
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
                Err(_) => crate::log_legacy!(
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

pub fn bind_socket_addr(choice: &BindChoice, port: u16) -> SocketAddr {
    match choice {
        BindChoice::Loopback => SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        BindChoice::All => SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)),
        BindChoice::Address(ip) => SocketAddr::from((*ip, port)),
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindPlan {
    pub required: Vec<SocketAddr>,
    pub optional: Vec<SocketAddr>,
}

pub fn bind_plan(choice: &BindChoice, port: u16) -> BindPlan {
    let addrs = bind_socket_addrs(choice, port);
    if covers_loopback(choice) {
        return BindPlan {
            required: addrs,
            optional: Vec::new(),
        };
    }
    let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let (required, optional) = addrs.into_iter().partition(|a| *a == loopback);
    BindPlan { required, optional }
}

pub fn side_port(
    var: &str,
    configured: Option<&str>,
    gateway_port: u16,
    offset: u16,
) -> Option<u16> {
    match configured.map(str::trim) {
        Some(raw) if !raw.is_empty() => match raw.parse::<u16>() {
            Ok(0) => None,
            Ok(port) => Some(port),
            Err(_) => {
                crate::log_legacy!(
                    "[Gateway] {var} '{raw}' is not a port, so that listener stays off"
                );
                None
            }
        },
        _ => gateway_port.checked_add(offset),
    }
}

pub fn side_bind_addr(var: &str, configured: Option<&str>, port: u16) -> SocketAddr {
    let ip = match configured.map(str::trim) {
        Some(raw) if !raw.is_empty() => raw.parse::<IpAddr>().unwrap_or_else(|_| {
            crate::log_legacy!(
                "[Gateway] {var} '{raw}' is not an IP address, so that listener stays on loopback"
            );
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        }),
        _ => IpAddr::V4(Ipv4Addr::LOCALHOST),
    };
    SocketAddr::new(ip, port)
}

pub fn bind_scope_label(choice: &BindChoice) -> String {
    match choice {
        BindChoice::Loopback => "loopback only".to_string(),
        BindChoice::All => "all interfaces, dual-stack".to_string(),
        BindChoice::Address(ip) => format!("address {ip} + loopback"),
    }
}

pub async fn detect_tailscale_ipv4() -> Option<String> {
    tokio::task::spawn_blocking(lucidos_tailscale::bindable_tailnet_ipv4)
        .await
        .ok()
        .flatten()
        .map(|ip| ip.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAILNET: &str = "100.64.0.1";

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
        let configs = [
            NetworkToml {
                gateway_bind: Some(TAILNET.to_string()),
                external_addresses: BTreeMap::new(),
            },
            NetworkToml {
                gateway_bind: Some("loopback".to_string()),
                external_addresses: BTreeMap::from([(
                    5252,
                    "https://lucidos.example.com".to_string(),
                )]),
            },
            NetworkToml {
                gateway_bind: Some("all".to_string()),
                external_addresses: BTreeMap::from([
                    (5251, "http://[2001:db8::2]:5251".to_string()),
                    (5252, "http://10.8.0.2:5252".to_string()),
                ]),
            },
        ];
        for net in configs {
            assert_eq!(parse_network_toml(&render_network_toml(&net)), net);
        }
    }

    #[test]
    fn legacy_engine_inherit_table_parses_and_rerender_drops_it() {
        let legacy = format!("[gateway]\nbind = \"{TAILNET}\"\n\n[engine]\ninherit = true\n");
        let net = parse_network_toml(&legacy);
        assert_eq!(net.gateway_bind.as_deref(), Some(TAILNET));
        assert!(!render_network_toml(&net).contains("[engine]"));
    }

    #[test]
    fn an_unset_bind_renders_as_the_loopback_it_resolves_to() {
        let rendered = render_network_toml(&NetworkToml::default());
        assert_eq!(
            parse_network_toml(&rendered).gateway_bind.as_deref(),
            Some("loopback")
        );
        assert!(!rendered.contains("\n[external_address]"), "{rendered}");
    }

    #[test]
    fn each_gateway_reads_only_its_own_external_address() {
        let parsed = parse_network_toml(
            "[gateway]\nbind = \"all\"\n\n[external_address]\n\
             5252 = \"https://lucidos.example.com\"\n",
        );
        let ours = parsed.external_addresses.get(&5252).map(String::as_str);
        assert_eq!(
            resolve_external_address(None, ours).origin.as_deref(),
            Some("https://lucidos.example.com")
        );
        let other = parsed.external_addresses.get(&5251).map(String::as_str);
        assert_eq!(resolve_external_address(None, other).origin, None);
    }

    #[test]
    fn a_non_string_external_address_drops_only_itself() {
        let parsed = parse_network_toml(
            "[gateway]\nbind = \"all\"\n\n[external_address]\n\
             5251 = 5251\n5252 = \"https://lucidos.example.com\"\n",
        );
        assert_eq!(parsed.gateway_bind.as_deref(), Some("all"));
        assert_eq!(
            parsed.external_addresses,
            BTreeMap::from([(5252, "https://lucidos.example.com".to_string())])
        );
    }

    #[test]
    fn concurrent_writers_keep_every_gateways_entry() {
        let home = tempfile::tempdir().unwrap();
        let writers: Vec<_> = (0..8u16)
            .map(|i| {
                let dir = home.path().to_path_buf();
                std::thread::spawn(move || {
                    for round in 0..20 {
                        update_network_toml_in(&dir, |net| {
                            net.external_addresses
                                .insert(5000 + i, format!("https://gw{i}-{round}.example.com"));
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for writer in writers {
            writer.join().unwrap();
        }
        let net = read_network_toml_at(&home.path().join("network.toml"));
        let ports: Vec<u16> = net.external_addresses.keys().copied().collect();
        assert_eq!(ports, (5000..5008).collect::<Vec<_>>());
        assert_eq!(net.external_addresses[&5003], "https://gw3-19.example.com");
    }

    #[test]
    fn a_stored_external_address_that_is_not_an_origin_reads_as_unset() {
        for bad in [
            "https://lucidos.example.com/path",
            "lucidos.example.com",
            "ftp://lucidos.example.com",
            "http://127.0.0.1:5252",
        ] {
            let parsed = parse_network_toml(&format!(
                "[gateway]\nbind = \"all\"\n\n[external_address]\n\
                 5252 = \"{bad}\"\n5251 = \"https://lucidos.example.com\"\n"
            ));
            assert_eq!(parsed.external_addresses.get(&5252), None, "{bad}");
            assert_eq!(parsed.gateway_bind.as_deref(), Some("all"), "{bad}");
            assert!(parsed.external_addresses.contains_key(&5251), "{bad}");
        }
    }

    #[test]
    fn an_external_address_under_a_key_that_is_no_port_is_ignored() {
        let parsed = parse_network_toml(
            "[external_address]\nmain = \"https://lucidos.example.com\"\n\
             99999 = \"https://lucidos.example.com\"\n",
        );
        assert!(parsed.external_addresses.is_empty(), "{parsed:?}");
    }

    #[test]
    fn an_external_address_is_an_origin_only() {
        for good in [
            "https://lucidos.example.com",
            " http://10.8.0.2:5252 ",
            "https://mymac.example-tailnet.test",
            "http://[2001:db8::2]:5252",
        ] {
            assert_eq!(valid_external_address(good), Ok(good.trim().to_string()));
        }
        for bad in [
            "",
            "https://lucidos.example.com/",
            "https://lucidos.example.com/lucidos",
            "https://user@lucidos.example.com",
            "https://lucidos.example.com?x=1",
            "lucidos.example.com:5252",
            "http://10.8.0.2:99999",
        ] {
            assert!(valid_external_address(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_loopback_external_address_is_refused() {
        for bad in [
            "http://127.0.0.1:5252",
            "http://127.4.5.6",
            "http://localhost:5252",
            "https://app.localhost",
            "http://[::1]:5252",
            "http://127.1:5252",
            "http://2130706433:5252",
            "http://0x7f.1",
            "http://[::ffff:127.0.0.1]:5252",
        ] {
            let why = valid_external_address(bad).expect_err(bad);
            assert!(why.contains("only reaches this machine"), "{bad}: {why}");
        }
    }

    #[test]
    fn a_link_local_or_unspecified_external_address_is_refused() {
        for bad in [
            "http://169.254.169.254",
            "http://169.254.3.4:5252",
            "http://0.0.0.0:5252",
            "http://[fe80::1]:5252",
            "http://[::]:5252",
            "http://0:5252",
            "http://169.254.43518",
            "http://[::ffff:169.254.169.254]",
        ] {
            let why = valid_external_address(bad).expect_err(bad);
            assert!(why.contains("link-local or unspecified"), "{bad}: {why}");
        }
    }

    #[test]
    fn the_environment_variable_wins_and_locks() {
        let file = Some("https://file.example.com");
        let env = resolve_external_address(Some("https://env.example.com"), file);
        assert_eq!(env.origin.as_deref(), Some("https://env.example.com"));
        assert_eq!(env.source, ExternalAddressSource::Env);
        assert!(env.locked());

        let from_file = resolve_external_address(None, file);
        assert_eq!(from_file.origin.as_deref(), file);
        assert_eq!(from_file.source, ExternalAddressSource::File);
        assert!(!from_file.locked());

        let unset = resolve_external_address(None, None);
        assert_eq!(unset.origin, None);
        assert_eq!(unset.source, ExternalAddressSource::Unset);
        assert!(!unset.locked());
    }

    #[test]
    fn an_empty_or_invalid_environment_value_neither_wins_nor_locks() {
        let file = Some("https://file.example.com");
        for env in ["", "   ", "not an origin", "https://env.example.com/path"] {
            let resolved = resolve_external_address(Some(env), file);
            assert_eq!(resolved.origin.as_deref(), file, "{env:?}");
            assert!(!resolved.locked(), "{env:?}");
        }
    }

    #[test]
    fn parse_network_toml_defaults() {
        assert_eq!(parse_network_toml(""), NetworkToml::default());
        assert_eq!(parse_network_toml("garbage = = ="), NetworkToml::default());
    }

    #[test]
    fn an_unreadable_config_warns_while_an_absent_one_stays_silent() {
        let tmp = tempfile::tempdir().expect("a writable temp directory");
        let dir = tmp.path();

        let absent_path = dir.join("network.toml");
        let absent = std::fs::read_to_string(&absent_path).expect_err("nothing was written there");
        assert_eq!(absent.kind(), std::io::ErrorKind::NotFound);
        assert_eq!(read_failure_warning(&absent_path, &absent), None);

        let unreadable = std::fs::read_to_string(dir).expect_err("a directory is not a file");
        let warning = read_failure_warning(dir, &unreadable)
            .expect("an unreadable config must not fall back to loopback in silence");
        assert!(warning.contains(&dir.display().to_string()), "{warning}");
        assert!(warning.contains(&unreadable.to_string()), "{warning}");
    }

    const TLS_RULE_TABLE: &[(Option<&str>, Option<&str>, bool)] = &[
        (Some("/c.pem"), Some("/k.pem"), true),
        (Some("/c.pem"), None, false),
        (None, Some("/k.pem"), false),
        (None, None, false),
        (Some(""), Some("/k.pem"), false),
        (Some("/c.pem"), Some("  "), false),
        (Some(""), Some(""), false),
        (Some("   "), Some("   "), false),
    ];

    #[test]
    fn serves_tls_requires_both_cert_and_key_non_empty() {
        for (cert, key, expected) in TLS_RULE_TABLE {
            assert_eq!(
                serves_tls(*cert, *key),
                *expected,
                "cert {cert:?} + key {key:?} must resolve to serves_tls={expected}"
            );
        }
        assert_eq!((SCHEME_HTTP, SCHEME_HTTPS), ("http", "https"));
    }

    #[test]
    fn serves_tls_from_env_reads_the_two_tls_names() {
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

    #[test]
    fn no_gateway_site_asks_whether_a_tls_name_is_merely_set() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut scanned: Vec<String> = Vec::new();
        for entry in std::fs::read_dir(&src).expect("the gateway's own src directory") {
            let path = entry.expect("a readable directory entry").path();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
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

    #[test]
    fn bind_socket_addrs_specific_address_also_binds_loopback() {
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
    fn a_wildcard_address_binds_one_required_socket() {
        for ip in [
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            IpAddr::V6(Ipv6Addr::UNSPECIFIED),
        ] {
            let choice = BindChoice::Address(ip);
            let only = vec![SocketAddr::new(ip, 5251)];
            assert_eq!(bind_socket_addrs(&choice, 5251), only);
            let plan = bind_plan(&choice, 5251);
            assert_eq!(plan.required, only);
            assert!(plan.optional.is_empty(), "{ip} must fail fast");
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
    fn a_side_listener_binds_loopback_unless_an_address_is_set() {
        let loopback = SocketAddr::from((Ipv4Addr::LOCALHOST, 5262));
        for unset in [None, Some(""), Some("  ")] {
            assert_eq!(side_bind_addr("X", unset, 5262), loopback, "{unset:?}");
        }
        assert_eq!(
            side_bind_addr("X", Some(" 0.0.0.0 "), 5262),
            SocketAddr::from((Ipv4Addr::UNSPECIFIED, 5262))
        );
        assert_eq!(
            side_bind_addr("X", Some("::"), 5262),
            SocketAddr::from((Ipv6Addr::UNSPECIFIED, 5262))
        );
        assert_eq!(
            side_bind_addr("X", Some(TAILNET), 5262).ip(),
            TAILNET.parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn a_side_listener_setting_that_will_not_parse_stays_on_loopback() {
        for bad in ["all", "bogus", "0.0.0.0:5262", "10.0.0"] {
            assert!(
                side_bind_addr("X", Some(bad), 5262).ip().is_loopback(),
                "{bad} must never widen the bind"
            );
        }
    }

    #[test]
    fn every_bind_plan_has_something_required() {
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
