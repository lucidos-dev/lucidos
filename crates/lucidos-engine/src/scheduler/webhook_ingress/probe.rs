//! INVARIANT: One request per public address, over the public path.
//!
//! A loopback self-probe would have passed all night through the outage this
//! feature exists to catch. So every request leaves the machine and comes back
//! in through the public route.
//!
//! Each address gets its own client, with the public hostname pinned to that
//! one address. TLS and SNI still see the hostname, so the far side answers as
//! it would for a real sender. Three A records mean three requests, because a
//! dual-stack client would have picked the one healthy family and reported the
//! ingress fine.
//!
//! The request carries a bearer this cycle minted. It is refused like any wrong
//! token, and `api::webhooks::stamp_refused` recognises it and leaves the
//! hook's refusal stamp alone.
//!
//! A family that failed everywhere gets a second reading before the probe
//! blames the ingress. A host that cannot open the public port to anything is
//! describing its own network, not the far side.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::core::webhook_ingress::{
    classify_status, nothing_answered, AddressProbe, Family, Stage,
};

const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

const EGRESS_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// INVARIANT: The port the egress check compares the public port against.
///
/// A route already on 443 has nothing to compare against, so the question is
/// not asked there. That costs nothing: 443 is the port a network almost never
/// filters, so the reading this check exists to correct does not arise on it.
const REFERENCE_PORT: u16 = 443;

const DETAIL_MAX_CHARS: usize = 160;

const ROUTE_PROBE_V4: &str = "203.0.113.1:9";
const ROUTE_PROBE_V6: &str = "[2001:db8::1]:9";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalRoutes {
    pub ipv4: bool,
    pub ipv6: bool,
}

impl LocalRoutes {
    pub fn detect() -> Self {
        Self {
            ipv4: has_local_route(Family::Ipv4),
            ipv6: has_local_route(Family::Ipv6),
        }
    }

    fn covers(&self, family: Family) -> bool {
        match family {
            Family::Ipv4 => self.ipv4,
            Family::Ipv6 => self.ipv6,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProbeTarget {
    pub host: String,
    pub port: u16,
    pub path: String,
    pub token: String,
}

impl ProbeTarget {
    fn url(&self) -> String {
        format!("https://{}:{}{}", self.host, self.port, self.path)
    }
}

/// INVARIANT: Probe every address, one request each.
///
/// An address whose family this host cannot reach gets no request at all, and
/// reads `local-stack-unavailable`. That reading is not degraded: a machine with
/// no IPv6 egress would otherwise report a permanent outage of an ingress that
/// is fine.
pub async fn probe_all(
    target: &ProbeTarget,
    addresses: &[IpAddr],
    routes: LocalRoutes,
) -> Vec<AddressProbe> {
    let mut results = Vec::with_capacity(addresses.len());
    let mut dialled = Vec::with_capacity(addresses.len());
    for address in addresses {
        let address = normalize(*address);
        let family = family_of(&address);
        dialled.push(address);
        if routes.covers(family) {
            results.push(probe_one(target, address, family).await);
        } else {
            results.push(AddressProbe {
                address: address.to_string(),
                family,
                stage: Stage::LocalStackUnavailable,
                status: None,
                detail: Some(format!("this host has no {} route", family_word(family))),
            });
        }
    }
    explain_local_egress(target.port, &dialled, &mut results).await;
    results
}

async fn explain_local_egress(port: u16, dialled: &[IpAddr], results: &mut [AddressProbe]) {
    for family in Family::BOTH {
        if !nothing_answered(results, family) {
            continue;
        }
        let of_family: Vec<IpAddr> = dialled
            .iter()
            .copied()
            .filter(|address| family_of(address) == family)
            .collect();
        if port_egress(port, &of_family).await == PortEgress::Blocked {
            blame_local_egress(results, family, port);
        }
    }
}

fn blame_local_egress(results: &mut [AddressProbe], family: Family, port: u16) {
    for probe in results.iter_mut().filter(|probe| probe.family == family) {
        probe.stage = Stage::LocalEgressBlocked;
        probe.detail = Some(format!("this host cannot reach port {port} on any address"));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortEgress {
    /// INVARIANT: Something answered on the port, so it leaves this machine.
    Open,
    Blocked,
    Unknown,
}

async fn port_egress(port: u16, addresses: &[IpAddr]) -> PortEgress {
    let on_port = any_answers(addresses, port).await;
    let on_reference =
        !on_port && port != REFERENCE_PORT && any_answers(addresses, REFERENCE_PORT).await;
    read_egress(on_port, on_reference)
}

fn read_egress(on_port: bool, on_reference: bool) -> PortEgress {
    match (on_port, on_reference) {
        (true, _) => PortEgress::Open,
        (false, true) => PortEgress::Blocked,
        (false, false) => PortEgress::Unknown,
    }
}

async fn any_answers(addresses: &[IpAddr], port: u16) -> bool {
    for address in addresses {
        if answers_on(*address, port).await {
            return true;
        }
    }
    false
}

async fn answers_on(address: IpAddr, port: u16) -> bool {
    let target = SocketAddr::new(address, port);
    match timeout(EGRESS_PROBE_TIMEOUT, TcpStream::connect(target)).await {
        Ok(Ok(_)) => true,
        Ok(Err(e)) => e.kind() == std::io::ErrorKind::ConnectionRefused,
        Err(_) => false,
    }
}

async fn probe_one(target: &ProbeTarget, address: IpAddr, family: Family) -> AddressProbe {
    let client = match build_client(&target.host, target.port, address) {
        Ok(client) => client,
        Err(e) => {
            return AddressProbe {
                address: address.to_string(),
                family,
                stage: Stage::LocalStackUnavailable,
                status: None,
                detail: Some(detail_line("no client", &e.to_string())),
            };
        }
    };

    let sent = client
        .post(target.url())
        .header("authorization", format!("Bearer {}", target.token))
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await;

    match sent {
        Ok(response) => {
            let status = response.status().as_u16();
            let stage = classify_status(status);
            AddressProbe {
                address: address.to_string(),
                family,
                stage,
                status: Some(status),
                detail: (stage != Stage::Healthy).then(|| format!("answered HTTP {status}")),
            }
        }
        Err(e) => AddressProbe {
            address: address.to_string(),
            family,
            stage: Stage::IngressUnreachable,
            status: None,
            detail: Some(transport_detail(&e)),
        },
    }
}

/// INVARIANT: A client that dials exactly one address and nothing else.
///
/// Certificate validation stays on. A bad cert is a real diagnosis, and turning
/// it off would report a hijacked ingress as healthy.
fn build_client(host: &str, port: u16, address: IpAddr) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        // INVARIANT: The pin. The URL keeps the hostname, so SNI and the certificate are
        // checked against it while the connection goes to this address.
        .resolve(host, SocketAddr::new(address, port))
        // INVARIANT: A followed redirect would resolve its new host through the system
        // resolver, which is the one lookup this feature must never make.
        .redirect(reqwest::redirect::Policy::none())
        // INVARIANT: A proxy would dial on our behalf and the pin would mean nothing.
        .no_proxy()
        .timeout(PROBE_TIMEOUT)
        .build()
}

fn has_local_route(family: Family) -> bool {
    let (bind, probe) = match family {
        Family::Ipv4 => ("0.0.0.0:0", ROUTE_PROBE_V4),
        Family::Ipv6 => ("[::]:0", ROUTE_PROBE_V6),
    };
    UdpSocket::bind(bind)
        .and_then(|socket| socket.connect(probe))
        .is_ok()
}

fn normalize(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 => v4,
    }
}

pub fn family_of(address: &IpAddr) -> Family {
    match normalize(*address) {
        IpAddr::V4(_) => Family::Ipv4,
        IpAddr::V6(_) => Family::Ipv6,
    }
}

fn family_word(family: Family) -> &'static str {
    match family {
        Family::Ipv4 => "IPv4",
        Family::Ipv6 => "IPv6",
    }
}

fn transport_detail(error: &reqwest::Error) -> String {
    let class = if error.is_timeout() {
        "timed out"
    } else if error.is_connect() {
        "could not connect"
    } else {
        "request failed"
    };
    detail_line(class, &innermost_cause(error))
}

fn innermost_cause(error: &reqwest::Error) -> String {
    let mut cause: &dyn std::error::Error = error;
    while let Some(next) = cause.source() {
        cause = next;
    }
    cause.to_string()
}

fn detail_line(class: &str, cause: &str) -> String {
    let cause = cause.lines().next().unwrap_or_default().trim();
    if cause.is_empty() {
        return class.to_string();
    }
    let line = format!("{class}: {cause}");
    // INVARIANT: Counted in characters, not bytes. A TLS cause can carry a hostname in any
    // script, and a byte budget would cut it far shorter than the row allows.
    match line.char_indices().nth(DETAIL_MAX_CHARS) {
        Some((cut, _)) => format!("{}...", &line[..cut]),
        None => line,
    }
}

#[cfg(test)]
#[path = "probe_tests.rs"]
mod tests;
