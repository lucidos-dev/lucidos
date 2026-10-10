//! One request per public address, over the public path.
//!
//! A loopback self-probe would have passed all night through the outage this
//! feature exists to catch. So every request leaves the machine and comes back
//! in through the funnel.
//!
//! Each address gets its own client, with the funnel hostname pinned to that
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
//! blames the ingress. A host that cannot open the funnel port to anything is
//! describing its own network, not the far side.

use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::core::webhook_ingress::{
    classify_status, nothing_answered, AddressProbe, Family, Stage,
};

/// How long one request gets, connect and read together.
///
/// The scheduler runs this every 15 minutes, so a slow answer costs nothing.
/// A wedged relay must still give up long before the next cycle.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// How long one egress dial gets.
///
/// Well under `PROBE_TIMEOUT`, because this asks only whether a handshake
/// starts. A filtered port stays silent, and waiting on silence buys nothing.
const EGRESS_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The port the egress check compares the funnel port against.
///
/// A funnel already on 443 has nothing to compare against, so the question is
/// not asked there. That costs nothing: 443 is the port a network almost never
/// filters, so the reading this check exists to correct does not arise on it.
const REFERENCE_PORT: u16 = 443;

/// Longest `detail` line a payload carries. A TLS stack can hand back a
/// paragraph, and this is read on a settings row.
const DETAIL_MAX_CHARS: usize = 160;

/// Where the route lookup points, per family.
///
/// Both are documentation ranges (RFC 5737 and RFC 3849), so no real host is
/// named. Port 9 is discard. Nothing is ever sent to either.
const ROUTE_PROBE_V4: &str = "203.0.113.1:9";
const ROUTE_PROBE_V6: &str = "[2001:db8::1]:9";

/// Which families this host can reach at all.
///
/// Passed in rather than looked up inside the probe, so the skip is testable
/// without a network.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalRoutes {
    pub ipv4: bool,
    pub ipv6: bool,
}

impl LocalRoutes {
    /// Ask the routing table, sending nothing.
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

/// The public front door, and what one probe presents to it.
#[derive(Debug, Clone)]
pub struct ProbeTarget {
    /// The funnel hostname. Stays in the URL, so TLS and SNI see it.
    pub host: String,
    /// The public funnel port, not the loopback port behind it.
    pub port: u16,
    /// The gateway's delivery path, `/<slug>/<hook id>`.
    pub path: String,
    /// This cycle's bearer, so the engine knows its own probe.
    pub token: String,
}

impl ProbeTarget {
    fn url(&self) -> String {
        format!("https://{}:{}{}", self.host, self.port, self.path)
    }
}

/// Probe every address, one request each.
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

/// Ask whether this host could send at all, for a family that failed everywhere.
///
/// Only a failure is worth explaining, so the healthy path never dials. One
/// answer of any kind already proves the port leaves this machine, and
/// `nothing_answered` is the gate that says so.
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

/// Blame this host for a family whose every request stayed in the building.
fn blame_local_egress(results: &mut [AddressProbe], family: Family, port: u16) {
    for probe in results.iter_mut().filter(|probe| probe.family == family) {
        probe.stage = Stage::LocalEgressBlocked;
        probe.detail = Some(format!("this host cannot reach port {port} on any address"));
    }
}

/// Whether this host can open a port to anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortEgress {
    /// Something answered on the port, so it leaves this machine.
    Open,
    /// Nothing answered on the port, and something answered on the reference
    /// port. The network under this host filters the port.
    Blocked,
    /// Neither port answered, so the addresses are out of reach. That is not a
    /// fact about this host, so nothing here excuses the ingress.
    Unknown,
}

/// Can this host open the funnel port to any of these addresses?
///
/// A dial can only ever prove that a port WORKS. A completed handshake and a
/// refusal both mean a packet left and an answer came back. Silence says
/// nothing on its own, because a target that serves nothing on a port is silent
/// too. The reference port supplies the missing half: an address that answers
/// on 443 and stays silent on the funnel port is reachable, and the port is not.
///
/// `docs/adr/0172-a-blocked-port-is-not-a-dead-ingress.md` rejects dialling an
/// unrelated public host on the funnel port, and carries the measurements that
/// rule it out. It also names the one fault this reading cannot tell apart.
async fn port_egress(port: u16, addresses: &[IpAddr]) -> PortEgress {
    // The reference leg is asked only when the funnel port stayed silent, so a
    // working port costs one round rather than two.
    let on_port = any_answers(addresses, port).await;
    let on_reference =
        !on_port && port != REFERENCE_PORT && any_answers(addresses, REFERENCE_PORT).await;
    read_egress(on_port, on_reference)
}

/// What the two legs add up to.
///
/// `Blocked` needs both of them: silence on the funnel port, and an answer on
/// the reference port from the same addresses. A relay that is merely down is
/// silent on both, which is why it reads `Unknown` rather than blaming anybody.
fn read_egress(on_port: bool, on_reference: bool) -> PortEgress {
    match (on_port, on_reference) {
        (true, _) => PortEgress::Open,
        (false, true) => PortEgress::Blocked,
        (false, false) => PortEgress::Unknown,
    }
}

/// Did any of these addresses answer on this port?
///
/// Stops at the first one that does, so a working port costs one dial.
async fn any_answers(addresses: &[IpAddr], port: u16) -> bool {
    for address in addresses {
        if answers_on(*address, port).await {
            return true;
        }
    }
    false
}

/// Did this address answer on this port?
///
/// A completed handshake and a refusal both count. Each proves a packet left
/// and an answer came back, which is the whole question. The socket is dropped
/// at once, because nothing is ever sent on it.
async fn answers_on(address: IpAddr, port: u16) -> bool {
    let target = SocketAddr::new(address, port);
    match timeout(EGRESS_PROBE_TIMEOUT, TcpStream::connect(target)).await {
        Ok(Ok(_)) => true,
        Ok(Err(e)) => e.kind() == std::io::ErrorKind::ConnectionRefused,
        Err(_) => false,
    }
}

/// One request to one address.
async fn probe_one(target: &ProbeTarget, address: IpAddr, family: Family) -> AddressProbe {
    let client = match build_client(&target.host, target.port, address) {
        Ok(client) => client,
        Err(e) => {
            // Nothing left this machine, so this says nothing about the
            // ingress.
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

/// A client that dials exactly one address and nothing else.
///
/// Certificate validation stays on. A bad cert is a real diagnosis, and turning
/// it off would report a hijacked ingress as healthy.
fn build_client(host: &str, port: u16, address: IpAddr) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        // The pin. The URL keeps the hostname, so SNI and the certificate are
        // checked against it while the connection goes to this address.
        .resolve(host, SocketAddr::new(address, port))
        // A followed redirect would resolve its new host through the system
        // resolver, which is the one lookup this feature must never make.
        .redirect(reqwest::redirect::Policy::none())
        // A proxy would dial on our behalf and the pin would mean nothing.
        .no_proxy()
        .timeout(PROBE_TIMEOUT)
        .build()
}

/// Does this host have any route for the family?
///
/// A UDP `connect` picks a source address and sends no packet, so this reads
/// the routing table without touching the address it names.
fn has_local_route(family: Family) -> bool {
    let (bind, probe) = match family {
        Family::Ipv4 => ("0.0.0.0:0", ROUTE_PROBE_V4),
        Family::Ipv6 => ("[::]:0", ROUTE_PROBE_V6),
    };
    UdpSocket::bind(bind)
        .and_then(|socket| socket.connect(probe))
        .is_ok()
}

/// The address as it is dialled and reported.
///
/// An IPv4-mapped IPv6 literal goes out over IPv4, so it is unwrapped here and
/// judged as IPv4.
fn normalize(address: IpAddr) -> IpAddr {
    match address {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 => v4,
    }
}

/// Which family a request to this address uses.
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

/// What a transport failure says, short enough to read on a row.
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

/// The bottom of the error chain, which is where the diagnosis lives.
///
/// The outer message repeats the URL, and the payload already carries the
/// address and the port.
fn innermost_cause(error: &reqwest::Error) -> String {
    let mut cause: &dyn std::error::Error = error;
    while let Some(next) = cause.source() {
        cause = next;
    }
    cause.to_string()
}

/// One line, capped. Multi-line causes are common from a TLS stack.
fn detail_line(class: &str, cause: &str) -> String {
    let cause = cause.lines().next().unwrap_or_default().trim();
    if cause.is_empty() {
        return class.to_string();
    }
    let line = format!("{class}: {cause}");
    // Counted in characters, not bytes. A TLS cause can carry a hostname in any
    // script, and a byte budget would cut it far shorter than the row allows.
    match line.char_indices().nth(DETAIL_MAX_CHARS) {
        Some((cut, _)) => format!("{}...", &line[..cut]),
        None => line,
    }
}

#[cfg(test)]
#[path = "probe_tests.rs"]
mod tests;
