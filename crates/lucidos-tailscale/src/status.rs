//! Reading Tailscale's own status answer into a [`TailnetState`].
//!
//! The same JSON comes from the LocalAPI (`/localapi/v0/status`) and from
//! `tailscale status --json`. Only four fields matter, so this reads a
//! `serde_json::Value` rather than deriving a schema Tailscale never promised.

use std::net::{IpAddr, Ipv4Addr};

use serde_json::Value;

/// What this machine is on its tailnet, as Tailscale itself reported it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailnetNode {
    /// The first IPv4 in `Self.TailscaleIPs`. Never inferred from a range.
    pub ipv4: Ipv4Addr,
    /// Every address in `Self.TailscaleIPs`, IPv4 and IPv6.
    pub addresses: Vec<IpAddr>,
    /// `Self.DNSName` without its trailing dot, while MagicDNS is on.
    pub magic_dns_name: Option<String>,
    /// Tailscale can issue a certificate for that name (it is in
    /// `CertDomains`), so `tailscale serve` can give it https.
    pub can_serve_https: bool,
}

impl TailnetNode {
    /// The MagicDNS name worth an `https://` probe, or `None`.
    pub fn https_name(&self) -> Option<&str> {
        self.magic_dns_name
            .as_deref()
            .filter(|_| self.can_serve_https)
    }
}

/// Where this machine stands with Tailscale. One answer for every surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TailnetState {
    /// Running, with a tailnet IPv4.
    Joined(TailnetNode),
    /// Tailscale answered with a backend state other than `Running`.
    NotJoined { backend_state: String },
    /// Tailscale is installed, but its daemon did not answer.
    NotRunning,
    /// Tailscale answered, and the answer could not be read.
    Unreadable { detail: String },
    /// No sign of Tailscale on this machine.
    NotInstalled,
}

impl TailnetState {
    /// The node, when joined.
    pub fn node(&self) -> Option<&TailnetNode> {
        match self {
            Self::Joined(node) => Some(node),
            _ => None,
        }
    }

    /// One sentence for the user on why there is no tailnet address.
    /// `None` when joined.
    pub fn reason(&self) -> Option<String> {
        let sentence = match self {
            Self::Joined(_) => return None,
            Self::NotJoined { backend_state } => match backend_state.as_str() {
                "NeedsLogin" => "Tailscale is installed, but not signed in.".to_string(),
                "Stopped" => "Tailscale is signed in, but turned off.".to_string(),
                "NeedsMachineAuth" => {
                    "This machine is waiting for the tailnet admin to approve it.".to_string()
                }
                "Starting" | "NoState" => "Tailscale is still connecting.".to_string(),
                other => format!("Tailscale is not connected (state: {other})."),
            },
            Self::NotRunning => "Tailscale is installed, but it did not answer.".to_string(),
            Self::Unreadable { detail } => {
                format!("Tailscale answered, but Lucidos could not read its status: {detail}")
            }
            Self::NotInstalled => "Tailscale is not installed on this machine.".to_string(),
        };
        Some(sentence)
    }
}

/// Read a status body. `Err` carries what was wrong with it, for
/// [`TailnetState::Unreadable`].
pub(crate) fn parse_status(body: &str) -> Result<TailnetState, String> {
    let status: Value = serde_json::from_str(body).map_err(|e| format!("not JSON ({e})"))?;
    let backend_state = status
        .get("BackendState")
        .and_then(Value::as_str)
        .ok_or("no BackendState")?;
    if backend_state != "Running" {
        return Ok(TailnetState::NotJoined {
            backend_state: backend_state.to_string(),
        });
    }
    let own = status.get("Self");
    let ips = own
        .and_then(|s| s.get("TailscaleIPs"))
        .or_else(|| status.get("TailscaleIPs"));
    let addresses: Vec<IpAddr> = ips
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|ip| ip.parse().ok())
        .collect();
    let ipv4 = addresses
        .iter()
        .find_map(|ip| match ip {
            IpAddr::V4(v4) => Some(*v4),
            IpAddr::V6(_) => None,
        })
        .ok_or("running, but no tailnet IPv4 address")?;
    let magic_dns_on = status
        .pointer("/CurrentTailnet/MagicDNSEnabled")
        .and_then(Value::as_bool)
        == Some(true);
    let magic_dns_name = own
        .and_then(|s| s.get("DNSName"))
        .and_then(Value::as_str)
        .map(|name| name.trim_end_matches('.'))
        .filter(|name| magic_dns_on && !name.is_empty())
        .map(str::to_string);
    let can_serve_https = magic_dns_name.as_deref().is_some_and(|name| {
        status
            .get("CertDomains")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .any(|domain| domain.eq_ignore_ascii_case(name))
    });
    Ok(TailnetState::Joined(TailnetNode {
        ipv4,
        addresses,
        magic_dns_name,
        can_serve_https,
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A status body in the shape Tailscale sends, with invented values.
    pub(crate) fn status_json(
        backend: &str,
        ips: &[&str],
        dns_name: &str,
        magic_dns: bool,
        cert_domains: Option<&[&str]>,
    ) -> String {
        serde_json::json!({
            "BackendState": backend,
            "TUN": true,
            "Self": { "DNSName": dns_name, "TailscaleIPs": ips },
            "CurrentTailnet": { "Name": "example", "MagicDNSEnabled": magic_dns },
            "CertDomains": cert_domains,
        })
        .to_string()
    }

    fn joined(body: &str) -> TailnetNode {
        match parse_status(body) {
            Ok(TailnetState::Joined(node)) => node,
            other => panic!("expected Joined, got {other:?}"),
        }
    }

    const NAME: &str = "mymac.tailnet-name.ts.net.";

    #[test]
    fn the_address_is_the_one_tailscale_reported_beside_another_cgnat_vpn() {
        // Appgate's 100.64.10.175 sits on another utun. Tailscale never lists
        // it, so it can never be the answer, whatever the interface order.
        let body = status_json(
            "Running",
            &["100.70.0.1", "fd7a:115c:a1e0::1"],
            NAME,
            true,
            None,
        );
        let node = joined(&body);
        assert_eq!(node.ipv4, Ipv4Addr::new(100, 70, 0, 1));
        let v6: IpAddr = "fd7a:115c:a1e0::1".parse().unwrap();
        assert_eq!(node.addresses, [IpAddr::V4(node.ipv4), v6]);
    }

    #[test]
    fn a_headscale_ipv6_prefix_or_no_ipv6_changes_nothing() {
        let headscale = status_json(
            "Running",
            &["fd00:abcd::7", "100.64.0.7"],
            "mymac.example-tailnet.test.",
            true,
            None,
        );
        let node = joined(&headscale);
        assert_eq!(node.ipv4, Ipv4Addr::new(100, 64, 0, 7));
        // No suffix check: a Headscale base domain is a MagicDNS name too.
        assert_eq!(
            node.magic_dns_name.as_deref(),
            Some("mymac.example-tailnet.test")
        );
        let ipv4_only = status_json("Running", &["100.64.0.8"], NAME, true, None);
        assert_eq!(joined(&ipv4_only).ipv4, Ipv4Addr::new(100, 64, 0, 8));
    }

    #[test]
    fn a_running_node_without_an_ipv4_is_unreadable_not_joined() {
        let body = status_json("Running", &["fd7a:115c:a1e0::1"], NAME, true, None);
        assert!(parse_status(&body).is_err());
    }

    #[test]
    fn the_magic_dns_name_exists_only_while_magic_dns_is_on() {
        let on = status_json("Running", &["100.70.0.1"], NAME, true, None);
        assert_eq!(
            joined(&on).magic_dns_name.as_deref(),
            Some("mymac.tailnet-name.ts.net")
        );
        let off = status_json("Running", &["100.70.0.1"], NAME, false, None);
        assert_eq!(joined(&off).magic_dns_name, None);
        let missing = r#"{"BackendState":"Running","Self":{"DNSName":"mymac.tailnet-name.ts.net.","TailscaleIPs":["100.70.0.1"]}}"#;
        assert_eq!(joined(missing).magic_dns_name, None);
    }

    #[test]
    fn https_needs_the_name_in_cert_domains() {
        let with_cert = status_json(
            "Running",
            &["100.70.0.1"],
            NAME,
            true,
            Some(&["MyMac.tailnet-name.ts.net"]),
        );
        let node = joined(&with_cert);
        assert!(node.can_serve_https);
        assert_eq!(node.https_name(), Some("mymac.tailnet-name.ts.net"));

        // Headscale sends null: no certificates, so no https candidate.
        let no_certs = status_json("Running", &["100.70.0.1"], NAME, true, None);
        assert_eq!(joined(&no_certs).https_name(), None);
        let other_name = status_json(
            "Running",
            &["100.70.0.1"],
            NAME,
            true,
            Some(&["other.tailnet-name.ts.net"]),
        );
        assert_eq!(joined(&other_name).https_name(), None);
        // A cert domain is useless without the name it would certify.
        let dns_off = status_json(
            "Running",
            &["100.70.0.1"],
            NAME,
            false,
            Some(&["mymac.tailnet-name.ts.net"]),
        );
        assert_eq!(joined(&dns_off).https_name(), None);
    }

    #[test]
    fn any_state_but_running_is_not_joined_with_a_reason() {
        for backend in [
            "NeedsLogin",
            "Stopped",
            "NeedsMachineAuth",
            "Starting",
            "Odd",
        ] {
            let state = parse_status(&status_json(backend, &["100.70.0.1"], NAME, true, None))
                .expect("a readable status");
            assert_eq!(
                state,
                TailnetState::NotJoined {
                    backend_state: backend.to_string()
                }
            );
            assert!(state.node().is_none());
            assert!(state.reason().is_some_and(|r| !r.is_empty()));
        }
    }

    #[test]
    fn text_that_is_not_status_json_is_an_error() {
        // What the macOS GUI shim prints, with exit 0, outside a GUI session.
        assert!(parse_status("The Tailscale GUI failed to start").is_err());
        assert!(parse_status("{}").is_err());
    }

    #[test]
    fn every_state_but_joined_has_a_reason() {
        let node = TailnetNode {
            ipv4: Ipv4Addr::new(100, 70, 0, 1),
            addresses: vec![Ipv4Addr::new(100, 70, 0, 1).into()],
            magic_dns_name: None,
            can_serve_https: false,
        };
        assert_eq!(TailnetState::Joined(node).reason(), None);
        for state in [
            TailnetState::NotRunning,
            TailnetState::NotInstalled,
            TailnetState::Unreadable {
                detail: "not JSON".into(),
            },
        ] {
            assert!(state.reason().is_some_and(|r| !r.is_empty()));
        }
    }
}
