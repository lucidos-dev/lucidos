//! Tests for reading the funnel's own report of itself.
//!
//! The fixture below is the real shape of `tailscale serve status --json`,
//! captured on a machine running daemon 1.102.3, with the node and tailnet
//! names replaced.

use super::*;

/// The front door this JSON says carries the hook port.
fn served(json: &str, hook_port: u16) -> PublicIngress {
    match parse_serve_status(json, hook_port) {
        FunnelState::Serving(ingress) => ingress,
        other => panic!("expected a funnel, got {other:?}"),
    }
}

/// Hook port 5261 is public on 8443 and tailnet-only on 9443. Both are real
/// entries in the captured output, and they are why the two maps are
/// intersected.
const SERVE_STATUS: &str = r#"{
  "TCP": {
    "443": { "HTTPS": true },
    "8443": { "HTTPS": true },
    "9443": { "HTTPS": true }
  },
  "Web": {
    "node.tailnet.ts.net:443": {
      "Handlers": { "/": { "Proxy": "http://127.0.0.1:5173" } }
    },
    "node.tailnet.ts.net:8443": {
      "Handlers": { "/": { "Proxy": "http://127.0.0.1:5261" } }
    },
    "node.tailnet.ts.net:9443": {
      "Handlers": { "/": { "Proxy": "http://127.0.0.1:5261" } }
    }
  },
  "AllowFunnel": { "node.tailnet.ts.net:8443": true }
}"#;

#[test]
fn the_only_tailscale_command_is_a_read() {
    // The engine reports an outage. It never re-arms a funnel, so no mutating
    // subcommand may appear here.
    assert_eq!(SERVE_STATUS_ARGS, &["serve", "status", "--json"]);
    for mutating in ["up", "down", "funnel", "set", "login", "logout", "reset"] {
        assert!(
            !SERVE_STATUS_ARGS.contains(&mutating),
            "{mutating} mutates the tailnet and must never be run"
        );
    }
}

#[test]
fn the_funnel_port_is_found_by_intersecting_the_two_maps() {
    let ingress = served(SERVE_STATUS, 5261);
    assert_eq!(ingress.host, "node.tailnet.ts.net");
    // 8443, never 9443: only 8443 is in AllowFunnel, and 9443 reaches the same
    // loopback port from inside the tailnet alone.
    assert_eq!(ingress.port, 8443);
}

#[test]
fn a_tailnet_only_port_is_never_probed_as_public() {
    // The same JSON with the funnel switched off. `Web` still lists both ports,
    // so a parser reading `Web` alone would happily probe one.
    let tailnet_only = SERVE_STATUS.replace("\"node.tailnet.ts.net:8443\": true", "");
    assert_eq!(
        parse_serve_status(&tailnet_only, 5261),
        FunnelState::NotServed
    );
}

#[test]
fn a_funnel_serving_some_other_port_is_not_ours() {
    // The web app is funnelled and the hook socket is not. Probing 443 would
    // report a healthy ingress for a port no delivery uses.
    assert_eq!(
        parse_serve_status(SERVE_STATUS, 9999),
        FunnelState::NotServed
    );
}

#[test]
fn allow_funnel_false_is_not_a_funnel() {
    let json = r#"{
      "Web": { "node.tailnet.ts.net:8443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:5261" } } } },
      "AllowFunnel": { "node.tailnet.ts.net:8443": false }
    }"#;
    assert_eq!(parse_serve_status(json, 5261), FunnelState::NotServed);
}

#[test]
fn an_entry_with_no_web_handler_is_not_probed() {
    // AllowFunnel on its own says the port is public, not what answers there.
    let json = r#"{ "AllowFunnel": { "node.tailnet.ts.net:8443": true } }"#;
    assert_eq!(parse_serve_status(json, 5261), FunnelState::NotServed);
}

#[test]
fn a_handler_that_is_not_a_proxy_is_skipped() {
    // A static text handler serves the funnel port without reaching the engine.
    let json = r#"{
      "Web": { "node.tailnet.ts.net:8443": { "Handlers": { "/": { "Text": "hello" } } } },
      "AllowFunnel": { "node.tailnet.ts.net:8443": true }
    }"#;
    assert_eq!(parse_serve_status(json, 5261), FunnelState::NotServed);
}

#[test]
fn a_proxy_that_leaves_this_machine_is_not_the_hook_socket() {
    // The hook socket binds loopback only, so anything else on port 5261 is a
    // different service.
    let json = r#"{
      "Web": { "node.tailnet.ts.net:8443": { "Handlers": { "/": { "Proxy": "http://203.0.113.9:5261" } } } },
      "AllowFunnel": { "node.tailnet.ts.net:8443": true }
    }"#;
    assert_eq!(parse_serve_status(json, 5261), FunnelState::NotServed);
}

#[test]
fn a_proxy_with_a_path_or_no_scheme_still_resolves() {
    for proxy in [
        "http://127.0.0.1:5261",
        "https://localhost:5261",
        "127.0.0.1:5261",
        "http://127.0.0.1:5261/",
    ] {
        assert!(
            proxy_targets_port(proxy, 5261),
            "{proxy} points at the hook"
        );
    }
    assert!(!proxy_targets_port("http://127.0.0.1:5262", 5261));
    assert!(!proxy_targets_port("http://127.0.0.1", 5261));
    assert!(!proxy_targets_port("", 5261));
}

#[test]
fn output_that_is_not_the_json_we_know_leaves_us_knowing_nothing() {
    // A CLI that changed shape, or a wedged daemon. Reading this as "no funnel"
    // would retract a live outage warning on the strength of a broken command.
    for junk in ["", "not json at all", r#"{"AllowFunnel": "yes"}"#] {
        assert_eq!(
            parse_serve_status(junk, 5261),
            FunnelState::Unknown,
            "{junk}"
        );
    }
}

#[test]
fn an_answer_with_no_funnel_in_it_is_still_an_answer() {
    // The CLI omits an empty map, so this is what turning the funnel off looks
    // like. It has to be told apart from the daemon not answering.
    for empty in [
        "null",
        "{}",
        r#"{"AllowFunnel": null}"#,
        r#"{"TCP": {"443": {"HTTPS": true}}}"#,
        r#"{"AllowFunnel": {"": true}}"#,
        r#"{"AllowFunnel": {"node.tailnet.ts.net:not-a-port": true}}"#,
    ] {
        assert_eq!(
            parse_serve_status(empty, 5261),
            FunnelState::NotServed,
            "{empty}"
        );
    }
}

#[test]
fn two_funnels_on_one_hook_port_pick_the_same_one_every_cycle() {
    // A map has no order, so an arbitrary pick would flip the probed port
    // between cycles and read as an outage each time it moved.
    let json = r#"{
      "Web": {
        "node.tailnet.ts.net:8443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:5261" } } },
        "node.tailnet.ts.net:443": { "Handlers": { "/": { "Proxy": "http://127.0.0.1:5261" } } }
      },
      "AllowFunnel": {
        "node.tailnet.ts.net:8443": true,
        "node.tailnet.ts.net:443": true
      }
    }"#;
    for _ in 0..8 {
        assert_eq!(served(json, 5261).port, 443);
    }
}
