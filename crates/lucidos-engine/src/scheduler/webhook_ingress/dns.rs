//! INVARIANT: Resolving the webhook hostname the way the outside world resolves it.
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

pub const DOH_ENDPOINTS: &[&str] = &["https://1.1.1.1/dns-query", "https://8.8.8.8/resolve"];

const TYPE_A: u16 = 1;
const TYPE_AAAA: u16 = 28;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicAddresses {
    Found(Vec<IpAddr>),
    NoRecord,
    Unknown,
}

/// INVARIANT: Every public address the outside world would reach this host on.
///
/// This asks for both families. A client that gets one of each picks IPv6, and
/// never notices a dead IPv4 relay.
pub async fn public_addresses(client: &reqwest::Client, host: &str) -> PublicAddresses {
    public_addresses_from(client, DOH_ENDPOINTS, host).await
}

async fn public_addresses_from(
    client: &reqwest::Client,
    endpoints: &[&str],
    host: &str,
) -> PublicAddresses {
    // URL hosts wrap IPv6 literals in brackets. Preserve them for the request,
    // but remove them here to classify the address before any lookup or dial.
    let literal = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = literal.parse::<IpAddr>() {
        return if is_public(&ip) {
            PublicAddresses::Found(vec![ip])
        } else {
            PublicAddresses::NoRecord
        };
    }
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

/// INVARIANT: Ask each endpoint in turn, and take the first one that names an address.
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
                log_legacy!(
                    "[WebhookIngress] {endpoint} answered {} for {host}",
                    r.status()
                );
                continue;
            }
            Err(e) => {
                log_legacy!("[WebhookIngress] {endpoint} could not be reached: {e}");
                continue;
            }
        };
        match body {
            Ok(body) => match addresses_from_doh(&body, record_type) {
                Some(addresses) if !addresses.is_empty() => return Some(addresses),
                // INVARIANT: Settled, and naming nothing. Count the answer and ask the
                // next endpoint anyway: one resolver refusing a name its
                // neighbour resolves is the flap this walk exists to survive.
                Some(_) => {
                    answered += 1;
                    log_legacy!("[WebhookIngress] {endpoint} named no public address for {host}");
                }
                None => log_legacy!("[WebhookIngress] {endpoint} did not answer for {host}"),
            },
            Err(e) => log_legacy!("[WebhookIngress] {endpoint} answer could not be read: {e}"),
        }
    }
    (answered > 0 && answered == endpoints.len()).then(Vec::new)
}

const DNS_NOERROR: u64 = 0;
const DNS_NXDOMAIN: u64 = 3;

pub fn addresses_from_doh(body: &str, record_type: u16) -> Option<Vec<IpAddr>> {
    let root: serde_json::Value = serde_json::from_str(body).ok()?;
    match root.get("Status").and_then(|status| status.as_u64()) {
        Some(DNS_NOERROR | DNS_NXDOMAIN) => {}
        _ => return None,
    }
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

/// INVARIANT: Could the outside world reach this address?
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
        || ip.octets()[0] == 0
        || ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_unspecified()
        || ip.is_multicast())
}

fn is_public_v6(ip: &Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    let unique_local = first & 0xfe00 == 0xfc00;
    let link_local = first & 0xffc0 == 0xfe80;
    !(unique_local || link_local || ip.is_loopback() || ip.is_unspecified() || ip.is_multicast())
}

#[cfg(test)]
#[path = "dns_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "dns_walk_tests.rs"]
mod walk_tests;
