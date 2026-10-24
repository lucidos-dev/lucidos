//! Finding this machine's gateway, for the commands that talk to it rather
//! than to a workspace engine (`lucidos pair`, `lucidos update-address`).
//!
//! They resolve their own target instead of going through `crate::workspace`.
//! A gateway serves every workspace, so no workspace names it.

use crate::workspace::BoxError;

/// Gateway ports to try, in order: the packaged gateway's, then the dev one's.
/// They differ so both can run at once. Probing beats asking the user, since
/// most people have exactly one.
const DEFAULT_PORTS: [u16; 2] = [
    lucidos_installs::DEFAULT_GATEWAY_PORT,
    lucidos_installs::DEFAULT_DEV_GATEWAY_PORT,
];

/// Where the gateway's own surface lives, behind the reserved sigil namespace.
pub(crate) const HEALTH_PATH: &str = "/~/api/v1/health";

/// The loopback client every gateway call uses. A gateway may serve a
/// self-signed certificate, and a proxy must never see a loopback call.
pub(crate) fn client() -> Result<reqwest::blocking::Client, BoxError> {
    Ok(reqwest::blocking::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(5))
        .build()?)
}

/// Ports to probe: the dev override from the environment, then the defaults.
pub(crate) fn candidate_ports() -> Vec<u16> {
    candidate_ports_from(dev_port_override())
}

/// The port `LUCIDOS_DEV_GATEWAY_PORT` names, when it names a valid one.
///
/// Setting it is a deliberate pick, so a caller takes that gateway even with
/// another one answering beside it.
pub(crate) fn dev_port_override() -> Option<u16> {
    std::env::var("LUCIDOS_DEV_GATEWAY_PORT")
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
}

/// The pure half, so the ordering is table-tested without touching the
/// environment. `set_var` is process-global and Rust runs tests in parallel
/// threads, so two tests sharing one variable race each other.
fn candidate_ports_from(override_port: Option<u16>) -> Vec<u16> {
    let mut ports = Vec::new();
    if let Some(p) = override_port {
        ports.push(p);
    }
    for p in DEFAULT_PORTS {
        if !ports.contains(&p) {
            ports.push(p);
        }
    }
    ports
}

/// One gateway that answered its health endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FoundGateway {
    pub port: u16,
    pub base: String,
}

/// Every gateway that answers, in probe order.
///
/// Both schemes are tried per port. The dev gateway serves TLS and the packaged
/// one serves plain HTTP, and the caller cannot know which it faces. All of
/// them rather than the first, so a caller that must refuse ambiguity sees the
/// second one.
pub(crate) fn find_gateways(
    client: &reqwest::blocking::Client,
    ports: &[u16],
) -> Vec<FoundGateway> {
    let mut found = Vec::new();
    for &port in ports {
        for scheme in ["https", "http"] {
            let base = format!("{scheme}://127.0.0.1:{port}");
            if client
                .get(format!("{base}{HEALTH_PATH}"))
                .send()
                .is_ok_and(|r| r.status().is_success())
            {
                found.push(FoundGateway { port, base });
                // One gateway per port. The other scheme is the same process
                // refusing a handshake, never a second gateway.
                break;
            }
        }
    }
    found
}

/// The gateway the caller named, through `--port` or the dev override, when it
/// answered.
pub(crate) fn preferred(found: &[FoundGateway], port: Option<u16>) -> Option<&FoundGateway> {
    port.and_then(|p| found.iter().find(|g| g.port == p))
}

/// The error for a gateway that did not answer on the port the caller named.
pub(crate) fn no_gateway_on(port: u16) -> BoxError {
    format!("no gateway answered on port {port}").into()
}

/// The error for no gateway answering on any probed port.
pub(crate) const NO_GATEWAY: &str =
    "no gateway found. Start one, or pass --port if it is on an unusual port";

#[cfg(test)]
mod tests {
    use super::*;
    use lucidos_installs::{DEFAULT_DEV_GATEWAY_PORT as DEV, DEFAULT_GATEWAY_PORT as PACKAGED};

    #[test]
    fn an_explicit_dev_override_is_probed_before_the_defaults() {
        // Set on a dev machine that also runs a packaged install, where probing
        // the packaged port first would reach the wrong gateway.
        assert_eq!(candidate_ports_from(Some(5999)), vec![5999, PACKAGED, DEV]);
    }

    #[test]
    fn an_override_equal_to_a_default_is_not_listed_twice() {
        assert_eq!(candidate_ports_from(Some(DEV)), vec![DEV, PACKAGED]);
        assert_eq!(candidate_ports_from(Some(PACKAGED)), vec![PACKAGED, DEV]);
    }

    #[test]
    fn the_defaults_are_probed_when_nothing_is_set() {
        // Packaged first: it is the one an ordinary install has.
        assert_eq!(candidate_ports_from(None), vec![PACKAGED, DEV]);
    }
}
