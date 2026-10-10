//! Tests for public resolution.
//!
//! The first one is the property the whole feature rests on: a tailnet address
//! is never probed as if it were the public path.

use super::*;

fn ip(s: &str) -> IpAddr {
    s.parse().expect("test address parses")
}

/// The addresses a readable answer carries.
fn answered(body: &str, record_type: u16) -> Vec<IpAddr> {
    addresses_from_doh(body, record_type).expect("the resolver answered")
}

#[test]
fn a_tailnet_address_is_never_probed_as_the_public_path() {
    // MagicDNS answers this node's own hostname with its 100.x address, so a
    // system lookup would send the probe down the internal path. It would pass
    // while every public relay was dead.
    for tailnet in [
        "100.64.0.1",
        "100.101.102.103",
        "100.127.255.254",
        "fd7a:115c:a1e0::1",
    ] {
        assert!(
            !is_public(&ip(tailnet)),
            "{tailnet} is a tailnet address and must never be probed"
        );
    }

    // The same rule applied to a real DoH answer: only the public record
    // survives.
    let body = r#"{"Status":0,"Answer":[
        {"name":"node.tailnet.ts.net.","type":1,"TTL":60,"data":"100.101.102.103"},
        {"name":"node.tailnet.ts.net.","type":1,"TTL":60,"data":"203.0.113.7"}
    ]}"#;
    assert_eq!(answered(body, 1), vec![ip("203.0.113.7")]);
}

#[test]
fn the_resolvers_are_addressed_by_ip_so_nothing_bootstraps_through_the_system() {
    // A hostname here would need the system resolver to find the resolver,
    // which is the dependency this design removes.
    for endpoint in DOH_ENDPOINTS {
        let authority = endpoint
            .trim_start_matches("https://")
            .split('/')
            .next()
            .unwrap();
        assert!(
            authority.parse::<IpAddr>().is_ok(),
            "{endpoint} must address the resolver by IP"
        );
    }
}

#[test]
fn nothing_the_outside_world_cannot_reach_is_probed() {
    for local in [
        "127.0.0.1",
        "10.0.0.4",
        "172.16.3.9",
        "192.168.1.20",
        "169.254.10.1",
        "0.0.0.0",
        "224.0.0.1",
        "255.255.255.255",
        "::1",
        "::",
        "fe80::1",
        "fc00::1",
        "ff02::1",
        "::ffff:100.64.0.1",
    ] {
        assert!(!is_public(&ip(local)), "{local} is not a public address");
    }
}

#[test]
fn a_public_address_of_either_family_survives() {
    for public in ["203.0.113.7", "198.51.100.4", "2001:db8::1", "2606:4700::1"] {
        assert!(is_public(&ip(public)), "{public} is publicly reachable");
    }
}

#[test]
fn only_the_record_type_that_was_asked_for_comes_back() {
    // A CNAME rides along in the same answer, and an A record answers an AAAA
    // query when the chain crosses families.
    let body = r#"{"Status":0,"Answer":[
        {"name":"hooks.example.com.","type":5,"TTL":60,"data":"node.tailnet.ts.net."},
        {"name":"node.tailnet.ts.net.","type":1,"TTL":60,"data":"203.0.113.7"},
        {"name":"node.tailnet.ts.net.","type":28,"TTL":60,"data":"2001:db8::1"}
    ]}"#;
    assert_eq!(answered(body, 1), vec![ip("203.0.113.7")]);
    assert_eq!(answered(body, 28), vec![ip("2001:db8::1")]);
}

#[test]
fn every_public_address_is_kept_because_one_probe_is_not_enough() {
    // Three A records, all of which have to be probed. One request to the
    // hostname would have reached exactly one of them.
    let body = r#"{"Status":0,"Answer":[
        {"name":"node.tailnet.ts.net.","type":1,"TTL":60,"data":"203.0.113.7"},
        {"name":"node.tailnet.ts.net.","type":1,"TTL":60,"data":"203.0.113.8"},
        {"name":"node.tailnet.ts.net.","type":1,"TTL":60,"data":"203.0.113.9"}
    ]}"#;
    assert_eq!(answered(body, 1).len(), 3);
}

#[test]
fn an_answer_we_cannot_read_is_not_an_answer() {
    // Silence and "there is no such record" are opposite readings, so a body we
    // cannot read must never pass as the second one.
    for junk in [
        "",
        "not json",
        "{}",
        r#"{"Answer":[]}"#,
        // SERVFAIL. The resolver failed rather than settling the question.
        r#"{"Status":2,"Answer":[]}"#,
    ] {
        assert!(addresses_from_doh(junk, 1).is_none(), "junk: {junk}");
    }
}

#[test]
fn a_resolver_that_answers_with_nothing_has_still_answered() {
    // This is the total-outage case: the hostname carries no public record, so
    // no delivery can reach the funnel at all.
    for empty in [
        // NXDOMAIN.
        r#"{"Status":3}"#,
        // The name exists with no record of this type.
        r#"{"Status":0}"#,
        r#"{"Status":0,"Answer":[]}"#,
        r#"{"Status":0,"Answer":"nope"}"#,
        r#"{"Status":0,"Answer":[{"type":1,"data":"not-an-address"}]}"#,
        r#"{"Status":0,"Answer":[{"type":1}]}"#,
        // Only a tailnet address, which the outside world cannot reach.
        r#"{"Status":0,"Answer":[{"type":1,"data":"100.101.102.103"}]}"#,
    ] {
        assert_eq!(addresses_from_doh(empty, 1), Some(Vec::new()), "{empty}");
    }
}
