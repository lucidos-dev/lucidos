//! The update address in the packaged updater: how a check resolves it, and
//! proof that an address never changes which key verifies the bundle.

use super::*;
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};

const FIXTURE_BUNDLE: &[u8] = include_bytes!("updater_fixtures/bundle.tar.gz");
const FIXTURE_SIGNATURE: &str = include_str!("updater_fixtures/bundle.tar.gz.sig");
const FIXTURE_PUBKEY: &str = include_str!("updater_fixtures/throwaway.pub");

fn tauri_conf() -> Value {
    serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json")
}

fn status(release_check: Value) -> String {
    json!({ "release_check": release_check }).to_string()
}

fn refusal(status: Option<&str>) -> String {
    match resolve_update_source(status) {
        Err(e) => e,
        Ok(source) => panic!("expected the check to stop, got {source:?}"),
    }
}

// ── Resolving the address ────────────────────────────────────────────────────

#[test]
fn an_unset_address_checks_the_published_release() {
    let body = status(json!({ "update_address": null, "update_address_error": null }));
    assert_eq!(
        resolve_update_source(Some(&body)),
        Ok(UpdateSource::Published)
    );
}

#[test]
fn a_gateway_older_than_the_setting_means_the_published_release() {
    for body in [status(json!({ "enabled": true })), json!({}).to_string()] {
        assert_eq!(
            resolve_update_source(Some(&body)),
            Ok(UpdateSource::Published),
            "{body}"
        );
    }
}

#[test]
fn a_set_address_is_the_one_checked() {
    let url = "http://127.0.0.1:8080/api/v1/data/artifacts/releases/rc/latest.json";
    let body = status(json!({ "update_address": url, "update_address_error": null }));
    let Ok(UpdateSource::Address(address)) = resolve_update_source(Some(&body)) else {
        panic!("expected the address");
    };
    assert_eq!(address.as_str(), url);
}

#[test]
fn an_address_the_gateway_refused_stops_the_check() {
    let body = status(json!({
        "update_address": "http://example.com/latest.json",
        "update_address_error": "the update address must be …",
    }));
    assert!(refusal(Some(&body)).contains("the update address must be"));
}

#[test]
fn the_app_applies_the_rule_itself_and_never_trusts_the_gateway_blindly() {
    let body = status(json!({ "update_address": "http://example.com/latest.json" }));
    assert!(refusal(Some(&body)).contains(lucidos_update_address::RULE));
}

#[test]
fn a_silent_or_garbled_gateway_stops_the_check() {
    assert!(refusal(None).contains("did not answer"));
    assert!(refusal(Some("<html>")).contains("unreadable gateway status"));
    let body = status(json!({ "update_address": 7 }));
    assert!(refusal(Some(&body)).contains("update_address 7"));
}

// ── The configuration the address sits beside ────────────────────────────────

/// Loopback http needs the plugin's insecure-transport switch. That switch
/// must never let the published endpoints drop to plain http.
#[test]
fn every_configured_endpoint_is_https_and_passes_the_address_rule() {
    let conf = tauri_conf();
    let updater = &conf["plugins"]["updater"];
    assert_eq!(updater["dangerousInsecureTransportProtocol"], json!(true));
    let endpoints = updater["endpoints"].as_array().expect("endpoints");
    assert!(!endpoints.is_empty());
    for endpoint in endpoints {
        let endpoint = endpoint.as_str().expect("a string endpoint");
        assert!(endpoint.starts_with("https://"), "{endpoint}");
        let parsed = UpdateAddress::parse(endpoint).expect("the rule accepts it");
        assert!(parsed.is_some(), "{endpoint}");
    }
}

/// The key is baked into `tauri.conf.json`. Overriding it in code would let
/// an update address bring its own key.
#[test]
fn no_shipped_source_overrides_the_updater_key() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![src];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.ends_with(".rs") || name.ends_with("_tests.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read source");
            assert!(
                !text.contains(".pubkey("),
                "{} sets the updater key in code",
                path.display()
            );
        }
    }
}

// ── The key verifies the bundle, whatever the address ────────────────────────

/// Serves the fixture `latest.json` and bundle on loopback until the test
/// process ends. The manifest names the bundle at `bundle_host`. Returns the
/// `latest.json` address.
fn serve_fixture(bundle_host: &str) -> UpdateAddress {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("local addr").port();
    let platform = tauri_plugin_updater::target().expect("a supported platform");
    let manifest = json!({
        "version": "99.0.0",
        "notes": "fixture",
        "platforms": { platform: {
            "url": format!("http://{bundle_host}:{port}/bundle.tar.gz"),
            "signature": FIXTURE_SIGNATURE.trim(),
        }},
    })
    .to_string();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request_line = String::new();
            let mut reader = BufReader::new(&stream);
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            while let Ok(n) = reader.read_line(&mut String::new()) {
                if n <= 2 {
                    break;
                }
            }
            let body: &[u8] = if request_line.contains("/bundle.tar.gz") {
                FIXTURE_BUNDLE
            } else {
                manifest.as_bytes()
            };
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(body);
        }
    });
    UpdateAddress::parse(&format!("http://127.0.0.1:{port}/latest.json"))
        .expect("valid")
        .expect("set")
}

/// A mock app whose updater config is the production one, key included.
/// `pubkey` swaps the key for the positive control only.
fn mock_app(pubkey: Option<&str>) -> tauri::App<MockRuntime> {
    let mut context = mock_context(noop_assets());
    let updater = tauri_conf()["plugins"]["updater"].clone();
    context
        .config_mut()
        .plugins
        .0
        .insert("updater".to_string(), updater);
    let mut plugin = tauri_plugin_updater::Builder::new();
    if let Some(key) = pubkey {
        plugin = plugin.pubkey(key);
    }
    mock_builder()
        .plugin(plugin.build())
        .build(context)
        .expect("mock app")
}

/// Check and download through the production `updater_for`.
fn fetch_through_the_address(
    app: &tauri::App<MockRuntime>,
    bundle_host: &str,
) -> Result<Vec<u8>, String> {
    let source = UpdateSource::Address(serve_fixture(bundle_host));
    let updater = updater_for(app.handle(), &source)?;
    tauri::async_runtime::block_on(async move {
        let update = check_offer(&updater)
            .await?
            .ok_or("the fixture offered no update")?;
        update
            .download(|_, _| {}, || {})
            .await
            .map_err(|e| e.to_string())
    })
}

#[test]
fn a_bundle_signed_by_another_key_is_refused_under_an_address() {
    let err = fetch_through_the_address(&mock_app(None), "127.0.0.1")
        .expect_err("a bundle signed by a throwaway key must not verify");
    assert!(err.to_lowercase().contains("signature"), "{err}");
}

/// The control: the same fixture verifies once its own key is configured,
/// so the refusal above is the key and not a broken fixture or server.
#[test]
fn the_same_bundle_verifies_under_its_own_key() {
    let bytes = fetch_through_the_address(&mock_app(Some(FIXTURE_PUBKEY.trim())), "127.0.0.1")
        .expect("the fixture verifies under its own key");
    assert_eq!(bytes, FIXTURE_BUNDLE);
}

/// The insecure-transport switch is for a loopback address. An offer whose
/// bundle would come over plain http from another host is refused before
/// any download.
#[test]
fn an_offer_naming_a_plain_http_bundle_elsewhere_is_refused() {
    let err = fetch_through_the_address(&mock_app(Some(FIXTURE_PUBKEY.trim())), "example.com")
        .expect_err("a plain-http bundle on another host must be refused");
    assert!(
        err.starts_with(lucidos_update_address::BUNDLE_RULE),
        "{err}"
    );
}
