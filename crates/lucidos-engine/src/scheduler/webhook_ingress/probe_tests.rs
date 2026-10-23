//! Tests for the probe itself.
//!
//! Every one is hermetic. The cases that would send a request pass
//! `LocalRoutes` with the family switched off, so the probe stops before it
//! builds a client.

use super::*;

const NO_ROUTES: LocalRoutes = LocalRoutes {
    ipv4: false,
    ipv6: false,
};

fn target() -> ProbeTarget {
    ProbeTarget {
        host: "node.tailnet.ts.net".to_string(),
        port: 8443,
        path: "/dev/6f1c0f3e-0000-4000-8000-000000000001".to_string(),
        token: "probe-token".to_string(),
    }
}

fn ip(s: &str) -> IpAddr {
    s.parse().expect("test address parses")
}

/// One reading, as `probe_one` would have written it.
fn reading(address: &str, family: Family, stage: Stage) -> AddressProbe {
    AddressProbe {
        address: address.to_string(),
        family,
        stage,
        status: (stage == Stage::Healthy).then_some(401),
        detail: (stage == Stage::IngressUnreachable).then(|| "timed out".to_string()),
    }
}

#[tokio::test]
async fn a_family_this_host_cannot_reach_is_never_sent_to() {
    // No IPv6 egress is an ordinary machine, not an outage. Sending anyway
    // would report a permanent failure of an ingress that is fine.
    let addresses = [ip("203.0.113.7"), ip("2001:db8::1")];
    let results = probe_all(&target(), &addresses, NO_ROUTES).await;

    assert_eq!(results.len(), 2);
    for result in &results {
        assert_eq!(result.stage, Stage::LocalStackUnavailable);
        assert_eq!(result.status, None);
        assert!(result.detail.as_deref().unwrap().contains("no "));
    }
    // And the judgement that follows says "not asked", never "degraded".
    let families = crate::core::webhook_ingress::judge(&results);
    assert!(crate::core::webhook_ingress::degraded_families(&families).is_empty());
}

#[tokio::test]
async fn every_address_gets_its_own_entry() {
    // Three A records, three results. One request to the hostname would have
    // reached exactly one of them.
    let addresses = [ip("203.0.113.7"), ip("203.0.113.8"), ip("203.0.113.9")];
    let results = probe_all(&target(), &addresses, NO_ROUTES).await;

    let reported: Vec<&str> = results.iter().map(|r| r.address.as_str()).collect();
    assert_eq!(reported, ["203.0.113.7", "203.0.113.8", "203.0.113.9"]);
}

#[tokio::test]
async fn a_mapped_address_is_reported_as_the_family_it_dials() {
    // `::ffff:203.0.113.7` leaves this machine over IPv4. Calling it IPv6 would
    // credit a healthy IPv4 relay to a family that was never touched.
    let results = probe_all(&target(), &[ip("::ffff:203.0.113.7")], NO_ROUTES).await;

    assert_eq!(results[0].family, Family::Ipv4);
    assert_eq!(results[0].address, "203.0.113.7");
}

#[test]
fn the_family_of_an_address_is_how_it_will_be_dialled() {
    assert_eq!(family_of(&ip("203.0.113.7")), Family::Ipv4);
    assert_eq!(family_of(&ip("2001:db8::1")), Family::Ipv6);
    assert_eq!(family_of(&ip("::ffff:203.0.113.7")), Family::Ipv4);
}

#[test]
fn the_probe_goes_to_the_public_port_over_https() {
    // The funnel port, not the loopback port behind it, and the gateway's own
    // delivery path.
    assert_eq!(
        target().url(),
        "https://node.tailnet.ts.net:8443/dev/6f1c0f3e-0000-4000-8000-000000000001"
    );
}

#[test]
fn the_route_lookup_names_a_documentation_address() {
    // Nothing is ever sent, but the address still has to belong to nobody.
    let v4: SocketAddr = ROUTE_PROBE_V4.parse().expect("a socket address");
    let v6: SocketAddr = ROUTE_PROBE_V6.parse().expect("a socket address");
    assert_eq!(v4.ip(), ip("203.0.113.1"));
    assert_eq!(v6.ip(), ip("2001:db8::1"));
    // Port 9 is discard, so even a stray packet lands nowhere.
    assert_eq!(v4.port(), 9);
    assert_eq!(v6.port(), 9);
}

#[test]
fn blocked_needs_an_answer_on_the_reference_port() {
    // Silence on the funnel port is not evidence on its own. A relay that is
    // merely down is silent on both legs, and blaming this host for that would
    // trade one false reading for another.
    assert_eq!(read_egress(false, true), PortEgress::Blocked);
    assert_eq!(read_egress(false, false), PortEgress::Unknown);
    // An answer on the funnel port ends the question, whatever 443 said.
    assert_eq!(read_egress(true, false), PortEgress::Open);
    assert_eq!(read_egress(true, true), PortEgress::Open);
}

#[test]
fn the_egress_dial_gives_up_long_before_the_request_does() {
    // It asks only whether a handshake starts, and a filtered port is silent.
    // A dial as patient as the request would cost a cycle three more timeouts.
    assert!(EGRESS_PROBE_TIMEOUT < PROBE_TIMEOUT);
}

#[test]
fn a_blocked_family_stops_blaming_the_ingress() {
    // The false alarm this exists to stop: three relays timing out on a network
    // that filters the port, for a hook the sender was reaching.
    let mut results = vec![
        reading("203.0.113.7", Family::Ipv4, Stage::IngressUnreachable),
        reading("203.0.113.8", Family::Ipv4, Stage::IngressUnreachable),
        reading("2001:db8::1", Family::Ipv6, Stage::Healthy),
    ];
    blame_local_egress(&mut results, Family::Ipv4, 8443);

    for result in &results[..2] {
        assert_eq!(result.stage, Stage::LocalEgressBlocked);
        assert_eq!(
            result.detail.as_deref(),
            Some("this host cannot reach port 8443 on any address")
        );
    }
    // The other family is untouched, and the verdict follows the readings.
    assert_eq!(results[2].stage, Stage::Healthy);
    let families = crate::core::webhook_ingress::judge(&results);
    assert!(crate::core::webhook_ingress::degraded_families(&families).is_empty());
}

#[test]
fn a_detail_is_one_short_line() {
    // A TLS stack answers with a paragraph, and this is read on a settings row.
    let sprawling = format!("first line\nsecond line\n{}", "x".repeat(400));
    let line = detail_line("could not connect", &sprawling);
    assert!(!line.contains('\n'));
    assert!(line.len() <= DETAIL_MAX_CHARS + 3, "{line}");
    assert!(line.starts_with("could not connect: first line"));
}

#[test]
fn a_cause_that_says_nothing_leaves_the_class_alone() {
    assert_eq!(detail_line("timed out", ""), "timed out");
    assert_eq!(detail_line("timed out", "  \n "), "timed out");
    assert_eq!(
        detail_line("could not connect", "tls handshake eof"),
        "could not connect: tls handshake eof"
    );
}
