use crate::workspace::BoxError;

const DEFAULT_PORTS: [u16; 2] = [
    lucidos_installs::DEFAULT_GATEWAY_PORT,
    lucidos_installs::DEFAULT_DEV_GATEWAY_PORT,
];

pub(crate) const HEALTH_PATH: &str = "/~/api/v1/health";

pub(crate) fn client() -> Result<reqwest::blocking::Client, BoxError> {
    Ok(lucidos_local_token::client::blocking_client_builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?)
}

pub(crate) fn candidate_ports() -> Vec<u16> {
    candidate_ports_from(dev_port_override())
}

pub(crate) fn dev_port_override() -> Option<u16> {
    std::env::var("LUCIDOS_DEV_GATEWAY_PORT")
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FoundGateway {
    pub port: u16,
    pub base: String,
}

pub(crate) fn find_gateways(
    client: &reqwest::blocking::Client,
    ports: &[u16],
) -> Vec<FoundGateway> {
    find_gateways_with(ports, |base| {
        client
            .get(format!("{base}{HEALTH_PATH}"))
            .send()
            .is_ok_and(|response| response.status().is_success())
    })
}

fn find_gateways_with(ports: &[u16], mut probe: impl FnMut(&str) -> bool) -> Vec<FoundGateway> {
    ports
        .iter()
        .map(|&port| FoundGateway {
            port,
            base: lucidos_local_token::client::origin(port),
        })
        .filter(|gateway| probe(&gateway.base))
        .collect()
}

pub(crate) fn preferred(found: &[FoundGateway], port: Option<u16>) -> Option<&FoundGateway> {
    port.and_then(|p| found.iter().find(|g| g.port == p))
}

pub(crate) fn no_gateway_on(port: u16) -> BoxError {
    format!("no gateway answered on port {port}").into()
}

pub(crate) const NO_GATEWAY: &str =
    "no gateway found. Start one, or pass --port if it is on an unusual port";

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

    #[test]
    fn every_port_is_probed_once_over_https() {
        let mut attempts = Vec::new();
        let gateways = find_gateways_with(&[5998, 5999], |base| {
            attempts.push(base.to_owned());
            base.ends_with(":5999")
        });
        assert_eq!(
            attempts,
            ["https://127.0.0.1:5998", "https://127.0.0.1:5999"]
        );
        assert_eq!(gateways.len(), 1);
        assert_eq!(gateways[0].port, 5999);
    }

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
        let both = found(&[5252, 5251]);
        let err = choose_gateway(&both, None, WHY).unwrap_err().to_string();
        assert!(err.contains("5252") && err.contains("5251"), "{err}");
        assert!(err.ends_with(WHY), "the refusal must say what to do: {err}");
    }

    #[test]
    fn a_preferred_port_wins_over_a_second_gateway() {
        let both = found(&[5252, 5251]);
        assert_eq!(
            choose_gateway(&both, Some(5251), WHY).unwrap(),
            "https://127.0.0.1:5251"
        );
    }

    #[test]
    fn a_preferred_port_that_did_not_answer_falls_back_to_the_rules() {
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
        assert_eq!(candidate_ports_from(Some(5999)), vec![5999, PACKAGED, DEV]);
    }

    #[test]
    fn an_override_equal_to_a_default_is_not_listed_twice() {
        assert_eq!(candidate_ports_from(Some(DEV)), vec![DEV, PACKAGED]);
        assert_eq!(candidate_ports_from(Some(PACKAGED)), vec![PACKAGED, DEV]);
    }

    #[test]
    fn the_defaults_are_probed_when_nothing_is_set() {
        assert_eq!(candidate_ports_from(None), vec![PACKAGED, DEV]);
    }
}
