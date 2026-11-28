use super::*;

fn served(json: &str, hook_port: u16) -> PublicIngress {
    match parse_serve_status(json, hook_port) {
        FunnelState::Serving(ingress) => ingress,
        other => panic!("expected a funnel, got {other:?}"),
    }
}

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
    assert_eq!(ingress.port, 8443);
}

#[test]
fn a_tailnet_only_port_is_never_probed_as_public() {
    let tailnet_only = SERVE_STATUS.replace("\"node.tailnet.ts.net:8443\": true", "");
    assert_eq!(
        parse_serve_status(&tailnet_only, 5261),
        FunnelState::NotServed
    );
}

#[test]
fn a_funnel_serving_some_other_port_is_not_ours() {
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
    let json = r#"{ "AllowFunnel": { "node.tailnet.ts.net:8443": true } }"#;
    assert_eq!(parse_serve_status(json, 5261), FunnelState::NotServed);
}

#[test]
fn a_handler_that_is_not_a_proxy_is_skipped() {
    let json = r#"{
      "Web": { "node.tailnet.ts.net:8443": { "Handlers": { "/": { "Text": "hello" } } } },
      "AllowFunnel": { "node.tailnet.ts.net:8443": true }
    }"#;
    assert_eq!(parse_serve_status(json, 5261), FunnelState::NotServed);
}

#[test]
fn a_proxy_that_leaves_this_machine_is_not_the_hook_socket() {
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
