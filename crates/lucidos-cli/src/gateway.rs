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
    Ok(client_builder().build()?)
}

/// [`client`] for an identity probe, which never follows a redirect. A
/// redirect to loopback would otherwise prove this gateway at an origin that
/// is not its own.
pub(crate) fn probe_client() -> Result<reqwest::blocking::Client, BoxError> {
    Ok(client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}

fn client_builder() -> reqwest::blocking::ClientBuilder {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .timeout(std::time::Duration::from_secs(5))
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

/// The one gateway a per-gateway command acts on: the one on `port`, else the
/// only one running. `why` is the sentence a refusal gives for asking.
pub(crate) fn the_gateway(
    client: &reqwest::blocking::Client,
    port: Option<u16>,
    why: &str,
) -> Result<String, BoxError> {
    match port {
        Some(p) => find_gateways(client, &[p])
            .into_iter()
            .next()
            .map(|g| g.base)
            .ok_or_else(|| no_gateway_on(p)),
        None => choose_gateway(
            &find_gateways(client, &candidate_ports()),
            dev_port_override(),
            why,
        ),
    }
}

/// Pure: which gateway to act on, or why the caller has to say.
///
/// Ambiguity is refused rather than guessed. A pairing code, the device it
/// enrols and the external address all belong to one gateway. Acting on the
/// wrong one is invisible: both gateways answer, and nothing names a port.
///
/// `preferred` is a port the caller chose, through `--port` or the dev
/// override. It wins whenever it answered.
fn choose_gateway(
    found: &[FoundGateway],
    preferred: Option<u16>,
    why: &str,
) -> Result<String, BoxError> {
    if let Some(hit) = self::preferred(found, preferred) {
        return Ok(hit.base.clone());
    }
    match found {
        [] => Err(NO_GATEWAY.into()),
        [only] => Ok(only.base.clone()),
        many => {
            let ports: Vec<String> = many.iter().map(|g| g.port.to_string()).collect();
            Err(format!(
                "{} gateways are running (ports {}). {why}",
                many.len(),
                ports.join(", ")
            )
            .into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lucidos_installs::{DEFAULT_DEV_GATEWAY_PORT as DEV, DEFAULT_GATEWAY_PORT as PACKAGED};

    const WHY: &str = "Pass --port to name one.";

    fn found(ports: &[u16]) -> Vec<FoundGateway> {
        ports
            .iter()
            .map(|&port| FoundGateway {
                port,
                base: format!("https://127.0.0.1:{port}"),
            })
            .collect()
    }

    #[test]
    fn one_running_gateway_is_used_without_asking() {
        let only = found(&[5252]);
        assert_eq!(
            choose_gateway(&only, None, WHY).unwrap(),
            "https://127.0.0.1:5252"
        );
    }

    #[test]
    fn two_running_gateways_are_refused_rather_than_guessed() {
        // The reported bug. Both answer, the first wins, and the code is minted
        // on a gateway the new device may never reach. Its own store holds the
        // device, so the phone is told the code expired.
        let both = found(&[5252, 5251]);
        let err = choose_gateway(&both, None, WHY).unwrap_err().to_string();
        assert!(err.contains("5252") && err.contains("5251"), "{err}");
        assert!(err.ends_with(WHY), "the refusal must say what to do: {err}");
    }

    #[test]
    fn a_preferred_port_wins_over_a_second_gateway() {
        // `--port` and the dev override are both the caller saying which one.
        let both = found(&[5252, 5251]);
        assert_eq!(
            choose_gateway(&both, Some(5251), WHY).unwrap(),
            "https://127.0.0.1:5251"
        );
    }

    #[test]
    fn a_preferred_port_that_did_not_answer_falls_back_to_the_rules() {
        // A stale override must not mint on a gateway nobody chose either.
        let both = found(&[5252, 5251]);
        assert!(choose_gateway(&both, Some(5999), WHY).is_err());
        let one = found(&[5252]);
        assert_eq!(
            choose_gateway(&one, Some(5999), WHY).unwrap(),
            "https://127.0.0.1:5252"
        );
    }

    #[test]
    fn no_gateway_at_all_says_how_to_start_one() {
        let err = choose_gateway(&[], None, WHY).unwrap_err().to_string();
        assert!(err.contains("no gateway found"), "{err}");
    }

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
