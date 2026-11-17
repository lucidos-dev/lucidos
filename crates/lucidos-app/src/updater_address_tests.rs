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
    assert!(refusal(Some(&body)).contains(lucidos_update_channel::RULE));
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
    serve_fixture_version(bundle_host, "99.0.0")
}

fn serve_fixture_version(bundle_host: &str, version: &str) -> UpdateAddress {
    serve_channel_fixture(bundle_host, version, None, None).0
}

/// Each observed request includes its headers, so Local proves authentication
/// on both HTTP hops. Redirects can be injected on either hop.
fn serve_channel_fixture(
    bundle_host: &str,
    version: &str,
    token: Option<&str>,
    redirect_path: Option<&str>,
) -> (UpdateAddress, std::sync::mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("local addr").port();
    let platform = tauri_plugin_updater::target().expect("a supported platform");
    let manifest = json!({
        "version": version,
        "notes": "fixture",
        "platforms": { platform: {
            "url": format!("http://{bundle_host}:{port}/~/updates/local/bundle.tar.gz"),
            "signature": FIXTURE_SIGNATURE.trim(),
        }},
    })
    .to_string();
    let token = token.map(str::to_string);
    let redirect_path = redirect_path.map(str::to_string);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut request_line = String::new();
            let mut reader = BufReader::new(&stream);
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            let mut headers = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) <= 2 {
                    break;
                }
                headers.push_str(&line);
            }
            let _ = tx.send(format!("{request_line}{headers}"));
            let authorized = token.as_ref().is_none_or(|token| {
                headers.contains(&format!(
                    "{}: {token}\r\n",
                    lucidos_local_token::HEADER_LOCAL_TOKEN
                ))
            });
            if !authorized {
                let _ = stream.write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
                continue;
            }
            if redirect_path
                .as_ref()
                .is_some_and(|path| request_line.contains(path))
            {
                let _ = stream.write_all(b"HTTP/1.1 302 Found\r\nLocation: /redirect-target\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                continue;
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
    let address = UpdateAddress::parse(&format!(
        "http://127.0.0.1:{port}/~/updates/local/latest.json"
    ))
    .expect("valid")
    .expect("set");
    (address, rx)
}

#[test]
fn a_failed_gateway_poll_does_not_block_the_apps_stable_check() {
    let (endpoint, requests) = serve_channel_fixture("127.0.0.1", "99.0.0", None, None);
    let body = status(json!({
        "channel": "stable", "update_address": null,
        "channel_error": null, "last_error": "Check origin unavailable"
    }));
    let source = resolve_update_source(Some(&body)).unwrap();
    assert_eq!(source, UpdateSource::Published);
    let app = mock_app_at(Some(FIXTURE_PUBKEY.trim()), Some(&endpoint));
    let updater = updater_for(app.handle(), &source).unwrap();
    assert!(tauri::async_runtime::block_on(check_offer(&updater))
        .unwrap()
        .is_some());
    assert!(requests
        .recv_timeout(std::time::Duration::from_secs(2))
        .is_ok());
}

#[test]
fn a_channel_error_stops_the_check_without_falling_back_to_stable() {
    let body = status(json!({ "channel": "local", "channel_error": "Local feed is broken" }));
    assert!(refusal(Some(&body)).contains("Local feed is broken"));
}

#[test]
fn a_correctly_signed_downgrade_is_refused_against_the_baked_release() {
    let app = mock_app(Some(FIXTURE_PUBKEY.trim()));
    // Above the crate's 0.1.0, below every shipped Lucidos release.
    let source = UpdateSource::Address(serve_fixture_version("127.0.0.1", "0.2.0"));
    let updater = updater_for(app.handle(), &source).unwrap();
    let result = tauri::async_runtime::block_on(check_offer(&updater));
    match result {
        Err(message) => assert!(message.contains("ahead"), "{message}"),
        Ok(_) => panic!(
            "a lower channel must report ahead, never offer a downgrade or report up to date"
        ),
    }
}

/// A mock app whose updater config is the production one, key included.
/// `pubkey` swaps the key for the positive control only.
fn mock_app(pubkey: Option<&str>) -> tauri::App<MockRuntime> {
    mock_app_at(pubkey, None)
}

fn mock_app_at(pubkey: Option<&str>, endpoint: Option<&UpdateAddress>) -> tauri::App<MockRuntime> {
    let mut context = mock_context(noop_assets());
    let mut updater = tauri_conf()["plugins"]["updater"].clone();
    if let Some(endpoint) = endpoint {
        updater["endpoints"] = json!([endpoint.as_str()]);
    }
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

fn fixture_source(channel: &str, address: &UpdateAddress) -> UpdateSource {
    resolve_update_source(Some(&status(json!({
        "channel": channel,
        "update_address": if channel == "stable" { Value::Null } else { json!(address.as_str()) },
    }))))
    .unwrap()
}

#[test]
fn every_channel_keeps_the_baked_key_and_authenticates_local_downloads() {
    for channel in ["stable", "beta", "local", "custom:Test"] {
        for trusted in [false, true] {
            let token = (channel == "local").then_some("fixture-local-token");
            let (address, requests) = serve_channel_fixture("127.0.0.1", "99.0.0", token, None);
            let app = mock_app_at(trusted.then_some(FIXTURE_PUBKEY.trim()), Some(&address));
            let updater =
                updater_with_token(app.handle(), &fixture_source(channel, &address), || {
                    token.map(str::to_string)
                })
                .unwrap();
            let result = tauri::async_runtime::block_on(async {
                let update = check_offer(&updater).await?.expect("newer fixture");
                assert_eq!(update.no_proxy, token.is_some(), "{channel}");
                update
                    .download(|_, _| {}, || {})
                    .await
                    .map_err(|e| e.to_string())
            });
            if trusted {
                assert_eq!(result.unwrap(), FIXTURE_BUNDLE, "{channel}");
            } else {
                let error = result.expect_err("foreign key must fail");
                assert!(
                    error.to_lowercase().contains("signature"),
                    "{channel}: {error}"
                );
            }
            for path in ["latest.json", "bundle.tar.gz"] {
                let request = requests
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap();
                assert!(request.contains(path), "{request}");
                assert_eq!(
                    request.contains(lucidos_local_token::HEADER_LOCAL_TOKEN),
                    token.is_some(),
                    "{channel}: {request}"
                );
            }
        }
    }
}

#[test]
fn every_channel_refuses_downgrades_and_equal_versions_but_accepts_newer_releases() {
    for channel in ["stable", "beta", "local", "custom:Test"] {
        for version in ["0.2.0", crate::LUCIDOS_RELEASE, "99.0.0-rc.1"] {
            let (address, _) = serve_channel_fixture("127.0.0.1", version, None, None);
            let app = mock_app_at(Some(FIXTURE_PUBKEY.trim()), Some(&address));
            let updater =
                updater_with_token(app.handle(), &fixture_source(channel, &address), || {
                    Some("fixture-local-token".into())
                })
                .unwrap();
            let result = tauri::async_runtime::block_on(check_offer(&updater));
            match version {
                "0.2.0" => assert!(result.err().unwrap().contains("ahead"), "{channel}"),
                "99.0.0-rc.1" => assert!(result.unwrap().is_some(), "{channel}"),
                _ => assert!(result.unwrap().is_none(), "{channel}"),
            }
        }
    }
}

#[test]
fn local_requires_a_token_and_refuses_redirects_on_both_hops() {
    for path in ["latest.json", "bundle.tar.gz"] {
        let (address, requests) =
            serve_channel_fixture("127.0.0.1", "99.0.0", Some("fixture-token"), Some(path));
        let app = mock_app(Some(FIXTURE_PUBKEY.trim()));
        let source = fixture_source("local", &address);
        assert!(updater_with_token(app.handle(), &source, || None).is_err());
        let updater =
            updater_with_token(app.handle(), &source, || Some("fixture-token".into())).unwrap();
        let result = tauri::async_runtime::block_on(async {
            let update = check_offer(&updater).await?.expect("newer fixture");
            update
                .download(|_, _| {}, || {})
                .await
                .map_err(|e| e.to_string())
        });
        assert!(result.is_err(), "{path} redirect must fail");
        let expected = if path == "latest.json" { 1 } else { 2 };
        assert_eq!(
            requests.try_iter().count(),
            expected,
            "no redirected request"
        );
    }
}

#[test]
fn local_refuses_a_bundle_outside_its_gateway_origin() {
    let (address, _) = serve_channel_fixture("localhost", "99.0.0", None, None);
    let app = mock_app(Some(FIXTURE_PUBKEY.trim()));
    let updater = updater_with_token(app.handle(), &fixture_source("local", &address), || {
        Some("fixture-token".into())
    })
    .unwrap();
    assert!(tauri::async_runtime::block_on(check_offer(&updater))
        .err()
        .unwrap()
        .contains("Local bundle"));
}

#[test]
fn malformed_channels_never_fall_back_or_receive_local_credentials() {
    for channel in ["beta", "local", "custom:Test", "nightly", "bogus"] {
        for address in [Value::Null, json!("")] {
            let body = status(json!({"channel": channel, "update_address": address}));
            assert!(resolve_update_source(Some(&body)).is_err(), "{body}");
        }
    }
    for address in [
        "https://example.com/~/updates/local/latest.json",
        "http://127.0.0.1:1234/latest.json",
    ] {
        let body = status(json!({"channel": "local", "update_address": address}));
        assert!(resolve_update_source(Some(&body)).is_err());
    }
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
        err.starts_with(lucidos_update_channel::BUNDLE_RULE),
        "{err}"
    );
}
