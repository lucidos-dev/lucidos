//! Resolving the funnel hostname the way the outside world resolves it.
//!
//! Never the system resolver. MagicDNS answers this node's own name with its
//! tailnet address, so a system lookup sends the probe down the internal path.
//! It would then pass all night while every public relay was dead. That is the
//! trap this module exists to avoid, and `is_public` is the second lock on it.
//!
//! The public resolvers answer DNS over HTTPS on a bare IP literal, so there is
//! no bootstrap lookup and no new dependency. Both endpoints speak the same
//! JSON.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Public DoH endpoints, tried in order. Addressed by IP, never by name.
pub const DOH_ENDPOINTS: &[&str] = &["https://1.1.1.1/dns-query", "https://8.8.8.8/resolve"];

/// DNS record type numbers, as the JSON answers report them.
const TYPE_A: u16 = 1;
const TYPE_AAAA: u16 = 28;

/// What the public resolvers said about a hostname.
///
/// An empty answer and an unanswered question look identical as a bare list,
/// and they are opposite readings. One says the hostname carries no record. The
/// other says a resolver we could not talk to told us nothing at all.
///
/// Neither is a verdict about the ingress. Only `Found` gives the probe an
/// address to knock on, and only a knock decides whether a family is degraded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicAddresses {
    /// At least one address the outside world can reach.
    Found(Vec<IpAddr>),
    /// Both families answered, and neither carries a public record.
    NoRecord,
    /// A family went unanswered, so nothing is known.
    Unknown,
}

/// Every public address the outside world would reach this host on.
///
/// This asks for both families. A client that gets one of each picks IPv6, and
/// never notices a dead IPv4 relay.
pub async fn public_addresses(client: &reqwest::Client, host: &str) -> PublicAddresses {
    public_addresses_from(client, DOH_ENDPOINTS, host).await
}

/// The same lookup against a named endpoint list, so a test can serve the
/// bodies itself. Production always passes [`DOH_ENDPOINTS`].
async fn public_addresses_from(
    client: &reqwest::Client,
    endpoints: &[&str],
    host: &str,
) -> PublicAddresses {
    let mut found = Vec::new();
    let mut every_family_answered = true;
    for record_type in [TYPE_A, TYPE_AAAA] {
        match resolve_one_type(client, endpoints, host, record_type).await {
            Some(addresses) => found.extend(addresses),
            None => every_family_answered = false,
        }
    }
    found.sort();
    found.dedup();
    if !found.is_empty() {
        return PublicAddresses::Found(found);
    }
    if every_family_answered {
        PublicAddresses::NoRecord
    } else {
        PublicAddresses::Unknown
    }
}

/// Ask each endpoint in turn, and take the first one that names an address.
///
/// A settled "no such record" does NOT stop the walk. Public resolvers disagree
/// about funnel hostnames: measured over ten queries, Cloudflare intermittently
/// answered NXDOMAIN for a name Google resolved every time. Stopping at the
/// first refusal turned that flap into a phantom total outage.
///
/// So the walk ends on a non-empty list. An empty list needs UNANIMITY: every
/// endpoint answered, and none named a record. Anything less is `None`, meaning
/// nothing is known.
///
/// Unanimity is the same lesson stated once more. A lone "no such record" is
/// the answer the measurement showed to be unreliable. It cannot carry the
/// question on its own while its neighbour stays silent.
async fn resolve_one_type(
    client: &reqwest::Client,
    endpoints: &[&str],
    host: &str,
    record_type: u16,
) -> Option<Vec<IpAddr>> {
    let wanted = record_type.to_string();
    let mut answered = 0usize;
    for endpoint in endpoints {
        let response = client
            .get(*endpoint)
            .query(&[("name", host), ("type", wanted.as_str())])
            .header("accept", "application/dns-json")
            .send()
            .await;
        let body = match response {
            Ok(r) if r.status().is_success() => r.text().await,
            Ok(r) => {
                log!(
                    "[WebhookIngress] {endpoint} answered {} for {host}",
                    r.status()
                );
                continue;
            }
            Err(e) => {
                log!("[WebhookIngress] {endpoint} could not be reached: {e}");
                continue;
            }
        };
        match body {
            Ok(body) => match addresses_from_doh(&body, record_type) {
                Some(addresses) if !addresses.is_empty() => return Some(addresses),
                // Settled, and naming nothing. Count the answer and ask the
                // next endpoint anyway: one resolver refusing a name its
                // neighbour resolves is the flap this walk exists to survive.
                Some(_) => {
                    answered += 1;
                    log!("[WebhookIngress] {endpoint} named no public address for {host}");
                }
                None => log!("[WebhookIngress] {endpoint} did not answer for {host}"),
            },
            Err(e) => log!("[WebhookIngress] {endpoint} answer could not be read: {e}"),
        }
    }
    (answered > 0 && answered == endpoints.len()).then(Vec::new)
}

/// The two response codes that settle a question. Anything else is a resolver
/// failure, which tells us nothing about the hostname.
const DNS_NOERROR: u64 = 0;
const DNS_NXDOMAIN: u64 = 3;

/// Pull the usable addresses out of one DoH JSON answer.
///
/// Only records of the type we asked for, and only public ones. A CNAME in the
/// chain is answered alongside the address, so the type filter is required.
///
/// `None` means the body is not an answer we can read. An empty list means the
/// resolver answered and the hostname carries no such public record.
pub fn addresses_from_doh(body: &str, record_type: u16) -> Option<Vec<IpAddr>> {
    let root: serde_json::Value = serde_json::from_str(body).ok()?;
    // Both endpoints always carry `Status`, so its absence marks a body that is
    // not a DoH answer at all.
    match root.get("Status").and_then(|status| status.as_u64()) {
        Some(DNS_NOERROR | DNS_NXDOMAIN) => {}
        _ => return None,
    }
    // A name that exists with no record of this type answers with no `Answer`
    // array, which is a fact rather than a failure.
    let Some(answers) = root.get("Answer").and_then(|a| a.as_array()) else {
        return Some(Vec::new());
    };
    Some(
        answers
            .iter()
            .filter(|answer| {
                answer.get("type").and_then(|t| t.as_u64()) == Some(record_type as u64)
            })
            .filter_map(|answer| answer.get("data").and_then(|d| d.as_str()))
            .filter_map(|data| data.trim().parse::<IpAddr>().ok())
            .filter(is_public)
            .collect(),
    )
}

/// Could the outside world reach this address?
///
/// The tailnet's own range is the one that matters, because MagicDNS hands it
/// back for this very hostname. The rest are rejected on the same principle: an
/// address only this machine can reach proves nothing about the public path.
pub fn is_public(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_public_v4(&v4),
            None => is_public_v6(v6),
        },
    }
}

fn is_public_v4(ip: &Ipv4Addr) -> bool {
    !(lucidos_tailscale::is_tailnet_addr(*ip)
        || ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_unspecified()
        || ip.is_multicast())
}

fn is_public_v6(ip: &Ipv6Addr) -> bool {
    // Tailscale's IPv6 range sits inside fc00::/7, so the unique-local test
    // covers it.
    let first = ip.segments()[0];
    let unique_local = first & 0xfe00 == 0xfc00;
    let link_local = first & 0xffc0 == 0xfe80;
    !(unique_local || link_local || ip.is_loopback() || ip.is_unspecified() || ip.is_multicast())
}

#[cfg(test)]
#[path = "dns_tests.rs"]
mod tests;

// Separate from the module above because these drive the endpoint walk against
// a stub server, where those read one body with no network at all.
#[cfg(test)]
#[path = "dns_walk_tests.rs"]
mod walk_tests;
