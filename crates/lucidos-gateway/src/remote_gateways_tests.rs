use super::*;
use crate::gateway_protocol::{GATEWAY_PROTOCOL, HEADER_GATEWAY_CREDENTIAL};
use crate::peers::GATEWAY_HEALTH_PATH;
use serde_json::json;

/// What A calls itself in these tests. Not `instance_id()`: A and B share
/// this test process, so they would share that id too.
const A_INSTANCE: &str = "gateway-a";

fn store(dir: &tempfile::TempDir) -> RemoteGatewayStore {
    RemoteGatewayStore::load(remote_gateways_path(dir.path())).unwrap()
}

fn request(address: &str, code: &str, name: &str) -> AddRequest {
    AddRequest {
        address: address.to_string(),
        code: code.to_string(),
        name: name.to_string(),
        headers: Default::default(),
    }
}

/// A cert for `localhost`, minted per run so it never expires on a test.
pub(crate) fn localhost_cert() -> rcgen::CertifiedKey {
    // Installing twice in one binary is an error; the first caller wins.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap()
}

/// Serve `router` over https on a free loopback port, behind `cert`.
pub(crate) async fn serve_https(router: axum::Router, cert: &rcgen::CertifiedKey) -> u16 {
    let config = axum_server::tls_rustls::RustlsConfig::from_pem(
        cert.cert.pem().into_bytes(),
        cert.key_pair.serialize_pem().into_bytes(),
    )
    .await
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        axum_server::from_tcp_rustls(listener, config)
            .serve(router.into_make_service())
            .await
            .ok();
    });
    port
}

/// Stand-in access proxy: reject a missing, duplicated, or overridden token.
pub(crate) fn proxy_guard(router: axum::Router) -> axum::Router {
    router.layer(axum::middleware::from_fn(
        |req: axum::extract::Request, next: axum::middleware::Next| async move {
            let values: Vec<_> = req.headers().get_all("x-proxy-token").iter().collect();
            if values.len() != 1 || values[0] != "proxy-secret" {
                return Err(axum::http::StatusCode::FORBIDDEN);
            }
            Ok(next.run(req).await)
        },
    ))
}

pub(crate) fn proxy_headers() -> ExtraHeaders {
    ExtraHeaders::new(std::collections::BTreeMap::from([(
        "x-proxy-token".into(),
        "proxy-secret".into(),
    )]))
    .unwrap()
}

/// Gateway B, serving the router the process serves, over https.
struct GatewayB {
    state: GatewayState,
    address: String,
    client: PeerConnection,
    _dir: tempfile::TempDir,
}

async fn gateway_b() -> GatewayB {
    let dir = tempfile::tempdir().unwrap();
    let state = GatewayState::for_tests_with_static_dir(Some(dir.path().to_path_buf()));
    let cert = localhost_cert();
    let port = serve_https(crate::server::gateway_router(state.clone()), &cert).await;
    GatewayB {
        state,
        address: format!("https://localhost:{port}"),
        client: PeerConnection::trusting(cert.cert.pem().as_bytes()),
        _dir: dir,
    }
}

/// A stand-in answering health with `body`, over https.
async fn stand_in(body: serde_json::Value) -> (String, PeerConnection) {
    let cert = localhost_cert();
    let router = axum::Router::new().route(
        GATEWAY_HEALTH_PATH,
        get(move || {
            let body = body.clone();
            async move { Json(body) }
        }),
    );
    let port = serve_https(router, &cert).await;
    (
        format!("https://localhost:{port}"),
        PeerConnection::trusting(cert.cert.pem().as_bytes()),
    )
}

#[test]
fn a_short_name_is_route_safe() {
    assert_eq!(valid_name(" cloud ").unwrap(), "cloud");
    assert_eq!(valid_name("home-box-2").unwrap(), "home-box-2");
    for bad in [
        "",
        "Cloud",
        "a@b",
        "a/b",
        "-cloud",
        "cloud-",
        &"x".repeat(33),
    ] {
        assert!(valid_name(bad).is_err(), "{bad:?}");
    }
}

#[test]
fn an_address_must_be_an_https_origin() {
    assert_eq!(
        valid_address(" https://lucidos.example.com/ ").unwrap(),
        "https://lucidos.example.com"
    );
    assert_eq!(
        valid_address("https://localhost:5252").unwrap(),
        "https://localhost:5252"
    );
    let http = valid_address("http://lucidos.example.com").unwrap_err();
    assert!(http.contains("https only"), "{http}");
    assert!(
        http.contains("tailscale serve"),
        "the refusal names the fix: {http}"
    );
    for bad in [
        "lucidos.example.com",
        "https://lucidos.example.com/path",
        "ftp://lucidos.example.com",
        "https://169.254.169.254",
        "https://0.0.0.0",
    ] {
        assert!(valid_address(bad).is_err(), "{bad}");
    }
}

#[test]
fn health_must_name_a_gateway_new_enough_and_not_us() {
    let ok = json!({"role": "gateway", "instance_id": "b", "gateway_protocol": 1});
    assert_eq!(
        read_health(&ok, A_INSTANCE).unwrap(),
        RemoteHealth {
            instance_id: "b".to_string(),
            gateway_protocol: 1
        }
    );
    let engine = json!({"role": "engine", "instance_id": "b", "gateway_protocol": 1});
    assert!(read_health(&engine, A_INSTANCE).is_err());
    let old = json!({"role": "gateway", "instance_id": "b"});
    assert!(read_health(&old, A_INSTANCE)
        .unwrap_err()
        .contains("too old"));
    let us = json!({"role": "gateway", "instance_id": A_INSTANCE, "gateway_protocol": 1});
    assert!(read_health(&us, A_INSTANCE).unwrap_err().contains("itself"));
}

#[test]
fn the_store_refuses_a_second_gateway_under_one_name() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let gateway = RemoteGateway {
        name: "cloud".to_string(),
        address: "https://cloud.example.com".to_string(),
        pairing_id: "p1".to_string(),
        credential: "secret".to_string(),
        headers: Default::default(),
        gateway_protocol: 1,
        added_at: "2026-01-01T00:00:00Z".to_string(),
        rotation: Default::default(),
    };
    store.add(gateway.clone()).unwrap();
    assert_eq!(store.add(gateway.clone()), Err(AddError::NameTaken));
    assert_eq!(store.remove("cloud").unwrap(), Some(gateway));
    assert!(store.list().is_empty());
}

#[cfg(unix)]
#[test]
fn the_store_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    store
        .add(RemoteGateway {
            name: "cloud".to_string(),
            address: "https://cloud.example.com".to_string(),
            pairing_id: "p1".to_string(),
            credential: "secret".to_string(),
            headers: Default::default(),
            gateway_protocol: 1,
            added_at: "2026-01-01T00:00:00Z".to_string(),
            rotation: Default::default(),
        })
        .unwrap();
    let mode = std::fs::metadata(remote_gateways_path(dir.path()))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o077, 0, "mode {mode:o}");
}

#[tokio::test]
async fn a_pairs_with_b_over_https_and_b_records_the_grant() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let code = b.state.paired_gateways().mint_secret(None).unwrap();

    let row = add(
        &a,
        &b.client,
        A_INSTANCE,
        "Lucidos on a",
        request(&b.address, &code, "cloud"),
    )
    .await
    .unwrap();

    assert_eq!(row.name, "cloud");
    assert_eq!(row.address, b.address);
    assert_eq!(row.gateway_protocol, GATEWAY_PROTOCOL);
    let granted = b.state.paired_gateways().list();
    assert_eq!(granted.len(), 1);
    assert_eq!(granted[0].id, row.pairing_id);
    assert_eq!(granted[0].label, "Lucidos on a");
    let held = &a.list()[0];
    assert!(b
        .state
        .paired_gateways()
        .authorize(&credential_header(&held.credential))
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn removing_b_on_a_asks_b_to_forget_the_grant() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let code = b.state.paired_gateways().mint_secret(None).unwrap();
    add(
        &a,
        &b.client,
        A_INSTANCE,
        "a",
        request(&b.address, &code, "cloud"),
    )
    .await
    .unwrap();

    let removed = a.remove("cloud").unwrap().unwrap();
    b.client.leave(&removed).await.unwrap();
    assert!(b.state.paired_gateways().list().is_empty());
}

#[tokio::test]
async fn a_plain_http_address_is_refused_before_the_code_is_spent() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let code = b.state.paired_gateways().mint_secret(None).unwrap();
    let http = b.address.replace("https://", "http://");

    let refused = add(
        &a,
        &b.client,
        A_INSTANCE,
        "a",
        request(&http, &code, "cloud"),
    )
    .await;
    assert!(matches!(refused, Err(AddError::Invalid(ref m)) if m.contains("https only")));

    add(
        &a,
        &b.client,
        A_INSTANCE,
        "a",
        request(&b.address, &code, "cloud"),
    )
    .await
    .expect("the refused attempt never spent the code");
}

#[tokio::test]
async fn the_client_never_speaks_plain_http() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let router = axum::Router::new().route(
        GATEWAY_HEALTH_PATH,
        get(|| async { Json(json!({"role": "gateway"})) }),
    );
    tokio::spawn(async move { axum::serve(listener, router).await.ok() });

    let client = PeerConnection::new().unwrap();
    assert!(client
        .health(&format!("http://127.0.0.1:{port}"))
        .await
        .is_err());
}

#[tokio::test]
async fn an_older_gateway_is_refused_with_the_reason() {
    let (address, client) = stand_in(json!({"role": "gateway", "instance_id": "old"})).await;
    let dir = tempfile::tempdir().unwrap();
    let refused = add(
        &store(&dir),
        &client,
        A_INSTANCE,
        "a",
        request(&address, "12345678", "old"),
    )
    .await;
    assert!(matches!(refused, Err(AddError::Invalid(ref m)) if m.contains("too old")));
}

#[tokio::test]
async fn a_wrong_code_is_b_refusing_and_records_nothing() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let refused = add(
        &a,
        &b.client,
        A_INSTANCE,
        "a",
        request(&b.address, "00000000", "cloud"),
    )
    .await;
    assert!(matches!(refused, Err(AddError::Remote(ref m)) if m.contains("not valid")));
    assert!(a.list().is_empty());
}

#[tokio::test]
async fn the_external_address_is_kept_only_when_it_reaches_the_same_b() {
    let b = gateway_b().await;
    let instance = crate::server::instance_id();
    let (other, _) = stand_in(json!({"role": "gateway", "instance_id": "other"})).await;
    let dead = {
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("https://localhost:{}", free.local_addr().unwrap().port())
    };

    let typed = "https://typed.invalid";
    assert_eq!(
        preferred_address(&b.client, typed, &b.address, instance).await,
        b.address
    );
    assert_eq!(
        preferred_address(&b.client, typed, &other, instance).await,
        typed
    );
    assert_eq!(
        preferred_address(&b.client, typed, &dead, instance).await,
        typed
    );
    assert_eq!(
        preferred_address(&b.client, typed, "http://plain.example.com", instance).await,
        typed
    );
}

#[tokio::test]
async fn the_list_never_carries_a_credential() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let code = b.state.paired_gateways().mint_secret(None).unwrap();
    add(
        &a,
        &b.client,
        A_INSTANCE,
        "a",
        request(&b.address, &code, "cloud"),
    )
    .await
    .unwrap();
    let rows: Vec<RemoteGatewayRow> = a.list().into_iter().map(Into::into).collect();
    let shown = serde_json::to_string(&rows).unwrap();
    assert!(!shown.contains(&a.list()[0].credential));
}

fn credential_header(credential: &str) -> axum::http::HeaderMap {
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(HEADER_GATEWAY_CREDENTIAL, credential.parse().unwrap());
    headers
}

/// Make `dir` refuse new files, so the next save fails. Undone on drop.
pub(crate) struct ReadOnlyDir<'a>(pub(crate) &'a std::path::Path);

impl<'a> ReadOnlyDir<'a> {
    pub(crate) fn new(dir: &'a std::path::Path) -> Self {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        Self(dir)
    }
}

impl Drop for ReadOnlyDir<'_> {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o700));
    }
}

fn remote(name: &str) -> RemoteGateway {
    RemoteGateway {
        name: name.to_string(),
        address: "https://cloud.example.com".to_string(),
        pairing_id: "pairing".to_string(),
        credential: "credential".to_string(),
        headers: Default::default(),
        gateway_protocol: GATEWAY_PROTOCOL,
        added_at: "2020-01-01T00:00:00Z".to_string(),
        rotation: Default::default(),
    }
}

/// A failed save adds nothing, so a retry finds the name still free.
#[test]
fn a_failed_save_leaves_the_store_as_it_was() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    {
        let _locked = ReadOnlyDir::new(dir.path());
        assert!(matches!(
            store.add(remote("cloud")),
            Err(AddError::Internal(_))
        ));
        assert!(store.list().is_empty());
    }
    store.add(remote("cloud")).unwrap();
    assert!(store.contains("cloud"));
    {
        let _locked = ReadOnlyDir::new(dir.path());
        assert!(store.remove("cloud").is_err());
        assert!(store.contains("cloud"), "a failed remove keeps the gateway");
    }
}

#[test]
fn outbound_peer_calls_have_one_builder() {
    for source in [
        include_str!("remote_gateways.rs"),
        include_str!("relay.rs"),
        include_str!("remote_workspaces.rs"),
        include_str!("nudge.rs"),
    ] {
        assert!(!source.contains("reqwest::Client::builder()"));
        assert!(!source.contains("reqwest::Client::new()"));
    }
}

#[test]
fn upgrading_a_plaintext_store_seals_credentials_before_returning() {
    let dir = tempfile::tempdir().unwrap();
    let path = remote_gateways_path(dir.path());
    let original = RemoteGateways {
        gateways: vec![remote("remote")],
    };
    auth::save_store(&original, &path).unwrap();
    let loaded = RemoteGatewayStore::load(path.clone()).unwrap();
    assert_eq!(loaded.list(), original.gateways);
    let disk = std::fs::read_to_string(&path).unwrap();
    let json: serde_json::Value = serde_json::from_str(&disk).unwrap();
    assert!(json["gateways"][0]["credential"]
        .as_str()
        .unwrap()
        .starts_with("lsec1:"));
    assert_ne!(json["gateways"][0]["credential"], "credential");
    assert_eq!(
        RemoteGatewayStore::load(path).unwrap().list(),
        original.gateways
    );
}

#[test]
fn a_missing_key_never_replaces_the_key_of_a_sealed_store() {
    use lucidos_secret_store::key_store::{FileKeyStore, KeyStore};
    let dir = tempfile::tempdir().unwrap();
    let path = remote_gateways_path(dir.path());
    let key_dir = dir.path().join("keys");
    let load = || {
        RemoteGatewayStore::load_with_key_store(
            path.clone(),
            Box::new(FileKeyStore::new(key_dir.clone())),
        )
    };
    let store = load().unwrap();
    store.add(remote("remote")).unwrap();
    let name = store.key_name.clone();
    let disk = std::fs::read(&path).unwrap();
    let keys = FileKeyStore::new(key_dir.clone());
    let original_key = keys.get(&name).unwrap().unwrap();
    keys.delete(&name).unwrap();
    assert!(store.add(remote("second")).is_err());
    assert!(load().is_err());
    assert_eq!(keys.get(&name).unwrap(), None);
    assert_eq!(std::fs::read(&path).unwrap(), disk);
    keys.put(&name, &original_key).unwrap();
    assert_eq!(load().unwrap().list(), vec![remote("remote")]);
}

#[test]
fn a_failed_migration_preserves_the_complete_original_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = remote_gateways_path(dir.path());
    auth::save_store(
        &RemoteGateways {
            gateways: vec![remote("remote")],
        },
        &path,
    )
    .unwrap();
    let original = std::fs::read(&path).unwrap();
    // Block the atomic replacement, after key generation and sealing succeed.
    std::fs::create_dir(path.with_extension("json.tmp")).unwrap();
    assert!(RemoteGatewayStore::load(path.clone()).is_err());
    assert_eq!(std::fs::read(path).unwrap(), original);
}

#[test]
fn debug_output_redacts_remote_credentials() {
    let mut gateway = remote("remote");
    gateway.credential = "private-debug-credential".into();
    assert!(!format!("{gateway:?}").contains(&gateway.credential));
    assert!(!format!(
        "{:?}",
        RemoteGateways {
            gateways: vec![gateway]
        }
    )
    .contains("private-debug-credential"));
}

#[test]
fn a_versioned_store_refuses_a_plaintext_credential_instead_of_migrating_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = remote_gateways_path(dir.path());
    let store = RemoteGatewayStore::load(path.clone()).unwrap();
    store.add(remote("remote")).unwrap();
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(json["version"], 1);
    json["gateways"][0]["credential"] = "unsealed-replacement".into();
    let changed = serde_json::to_vec(&json).unwrap();
    std::fs::write(&path, &changed).unwrap();
    assert!(RemoteGatewayStore::load(path.clone()).is_err());
    assert_eq!(std::fs::read(path).unwrap(), changed);
}

#[test]
fn proxy_headers_are_validated_and_debug_redacted() {
    for name in [
        "Host",
        "Connection",
        "Content-Length",
        "Transfer-Encoding",
        "x-lucidos-device",
        "bad name",
    ] {
        assert!(
            ExtraHeaders::new(std::collections::BTreeMap::from([(
                name.into(),
                "secret".into()
            )]))
            .is_err(),
            "{name}"
        );
    }
    assert!(ExtraHeaders::new(std::collections::BTreeMap::from([(
        "x-token".into(),
        "bad\r\nvalue".into()
    )]))
    .is_err());
    let headers = ExtraHeaders::new(std::collections::BTreeMap::from([(
        "X-Token".into(),
        "proxy-secret".into(),
    )]))
    .unwrap();
    assert!(!format!("{headers:?}").contains("proxy-secret"));
}

#[test]
fn proxy_header_values_are_sealed_and_never_echoed() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(&dir);
    let mut gateway = remote("cloud");
    gateway.headers = ExtraHeaders::new(std::collections::BTreeMap::from([(
        "x-token".into(),
        "proxy-secret".into(),
    )]))
    .unwrap();
    store.add(gateway.clone()).unwrap();
    let disk = std::fs::read_to_string(remote_gateways_path(dir.path())).unwrap();
    assert!(!disk.contains("proxy-secret"));
    assert!(
        !serde_json::to_string(&RemoteGatewayRow::from(gateway.clone()))
            .unwrap()
            .contains("proxy-secret")
    );
    assert_eq!(
        RemoteGatewayStore::load(remote_gateways_path(dir.path()))
            .unwrap()
            .list(),
        vec![gateway]
    );
}

#[tokio::test]
async fn proxy_headers_reach_health_pair_requests_and_leave_without_changing_origin() {
    use axum::http::StatusCode;
    let dir = tempfile::tempdir().unwrap();
    let bdir = tempfile::tempdir().unwrap();
    let bstate = GatewayState::for_tests_with_static_dir(Some(bdir.path().to_path_buf()));
    let code = bstate.paired_gateways().mint_secret(None).unwrap();
    let seen = std::sync::Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    let router = crate::server::gateway_router(bstate.clone()).layer(axum::middleware::from_fn(
        move |req: axum::extract::Request, next: axum::middleware::Next| {
            let recorded = recorded.clone();
            async move {
                if req
                    .headers()
                    .get("x-proxy-token")
                    .and_then(|v| v.to_str().ok())
                    != Some("proxy-secret")
                {
                    return Err(StatusCode::FORBIDDEN);
                }
                recorded.lock().unwrap().push(req.uri().path().to_string());
                Ok(next.run(req).await)
            }
        },
    ));
    let cert = localhost_cert();
    let port = serve_https(router, &cert).await;
    let client = PeerConnection::trusting(cert.cert.pem().as_bytes());
    let address = format!("https://localhost:{port}");
    let store = store(&dir);
    let mut req = request(&address, &code, "cloud");
    req.headers = ExtraHeaders::new(std::collections::BTreeMap::from([(
        "x-proxy-token".into(),
        "proxy-secret".into(),
    )]))
    .unwrap();
    add(&store, &client, A_INSTANCE, "a", req).await.unwrap();
    let gateway = store.list().remove(0);
    let built = client
        .request(&gateway, reqwest::Method::GET, "/probe")
        .unwrap()
        .build()
        .unwrap();
    assert!(built.headers()["x-proxy-token"].is_sensitive());
    assert!(!format!("{built:?}").contains("proxy-secret"));
    store.rotate(&client, "cloud").await.unwrap();
    let gateway = store.list().remove(0);
    client.leave(&gateway).await.unwrap();
    assert!(bstate.paired_gateways().list().is_empty());
    let seen = seen.lock().unwrap();
    assert!(seen.iter().any(|p| p == GATEWAY_HEALTH_PATH));
    assert!(seen.iter().any(|p| p.ends_with("/pair")));
    assert!(seen.iter().any(|p| p.ends_with("/rotate")));
    assert!(seen.iter().any(|p| p.ends_with("/pairing")));
}

#[tokio::test]
async fn proxy_sign_in_walls_name_the_host_without_following() {
    for status in [
        axum::http::StatusCode::FOUND,
        axum::http::StatusCode::UNAUTHORIZED,
        axum::http::StatusCode::FORBIDDEN,
    ] {
        let cert = localhost_cert();
        let router = axum::Router::new().route(
            GATEWAY_HEALTH_PATH,
            get(move || async move {
                (
                    status,
                    [
                        (axum::http::header::CONTENT_TYPE, "text/html"),
                        (
                            axum::http::header::LOCATION,
                            "https://elsewhere.example.com",
                        ),
                    ],
                    "<html>Sign in</html>",
                )
            }),
        );
        let port = serve_https(router, &cert).await;
        let client = PeerConnection::trusting(cert.cert.pem().as_bytes());
        let error = client
            .health(&format!("https://localhost:{port}"))
            .await
            .unwrap_err();
        assert!(error.contains("sign-in page at localhost"), "{error}");
    }
}

#[tokio::test]
async fn proxy_headers_stay_on_the_typed_origin_despite_an_advertised_address() {
    let cert = localhost_cert();
    let foreign_hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits = foreign_hits.clone();
    let foreign = axum::Router::new().fallback(move || {
        let hits = hits.clone();
        async move {
            hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            "unexpected"
        }
    });
    let foreign_port = serve_https(foreign, &cert).await;
    let foreign_address = format!("https://localhost:{foreign_port}");
    let advertised = foreign_address.clone();
    let router = axum::Router::new()
        .route(GATEWAY_HEALTH_PATH, get(|| async { Json(json!({"role":"gateway", "instance_id":"remote-b", "gateway_protocol": GATEWAY_PROTOCOL})) }))
        .route(&format!("{}{}", crate::gateway_protocol::GATEWAY_API_PREFIX, crate::gateway_protocol::PAIR_PATH), axum::routing::post(move || {
            let advertised = advertised.clone();
            async move { Json(json!({"pairing_id":"pairing", "credential":"credential", "gateway_protocol":GATEWAY_PROTOCOL, "external_address":advertised})) }
        }));
    let port = serve_https(router, &cert).await;
    let address = format!("https://localhost:{port}");
    let client = PeerConnection::trusting(cert.cert.pem().as_bytes());
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(&address, "12345678", "cloud");
    req.headers = ExtraHeaders::new(std::collections::BTreeMap::from([(
        "x-token".into(),
        "proxy-secret".into(),
    )]))
    .unwrap();
    let row = add(&store(&dir), &client, A_INSTANCE, "a", req)
        .await
        .unwrap();
    assert_eq!(row.address, address);
    assert_eq!(foreign_hits.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[test]
fn sealed_headers_cannot_be_moved_between_names_or_pairings() {
    for other_pairing in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let mut gateway = remote("cloud");
        gateway.headers = proxy_headers();
        store.add(gateway).unwrap();
        let mut other = remote("other");
        other.pairing_id = "another-pairing".into();
        other.headers = proxy_headers();
        store.add(other).unwrap();
        let path = remote_gateways_path(dir.path());
        let mut disk: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        if other_pairing {
            disk["gateways"][0]["headers"]["x-proxy-token"] =
                disk["gateways"][1]["headers"]["x-proxy-token"].clone();
        } else {
            let headers = disk["gateways"][0]["headers"].as_object_mut().unwrap();
            let ciphertext = headers.remove("x-proxy-token").unwrap();
            headers.insert("x-another-token".into(), ciphertext);
        }
        std::fs::write(&path, serde_json::to_vec(&disk).unwrap()).unwrap();
        assert!(RemoteGatewayStore::load(path).is_err());
    }
}

#[test]
fn pending_rotation_is_sealed_and_recovers_after_reload() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("remote-gateways.json");
    let store = RemoteGatewayStore::load(path.clone()).unwrap();
    store.add(remote("office")).unwrap();
    let pending = store.begin_rotation("office").unwrap();
    let candidate = pending.rotation.pending.as_ref().unwrap();
    assert_eq!(candidate.len(), 64);
    assert!(!std::fs::read_to_string(&path).unwrap().contains(candidate));
    let recovered = RemoteGatewayStore::load(path).unwrap();
    assert_eq!(recovered.list()[0], pending);
    assert_eq!(recovered.begin_rotation("office").unwrap(), pending);
}

#[tokio::test]
async fn sender_recovers_before_and_after_remote_acceptance() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("remotes.json");
    let a = RemoteGatewayStore::load(path.clone()).unwrap();
    let code = b.state.paired_gateways().mint_secret(None).unwrap();
    add(
        &a,
        &b.client,
        A_INSTANCE,
        "a",
        request(&b.address, &code, "cloud"),
    )
    .await
    .unwrap();
    let original = a.list()[0].credential.clone();
    let intent = a.begin_rotation("cloud").unwrap();
    // Restart before B receives anything.
    drop(a);
    let a = RemoteGatewayStore::load(path.clone()).unwrap();
    a.rotate(&b.client, "cloud").await.unwrap();
    assert_eq!(
        a.list()[0].credential,
        *intent.rotation.pending.as_ref().unwrap()
    );
    assert!(a.list()[0].rotation.pending.is_none());
    assert!(b
        .state
        .paired_gateways()
        .authorize(&credential_header(&original))
        .unwrap()
        .is_none());

    let intent = a.begin_rotation("cloud").unwrap();
    let candidate = intent.rotation.pending.as_ref().unwrap();
    // B commits, but A never sees the response. A restarts and must recover
    // with the same candidate instead of minting another credential.
    assert!(b
        .state
        .paired_gateways()
        .rotate(&credential_header(&intent.credential), candidate)
        .unwrap());
    drop(a);
    let bpath = crate::gateway_protocol::paired_gateways_path(&b._dir.path().join("state"));
    let mut disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&bpath).unwrap()).unwrap();
    disk["gateways"][0]["previous_credential"]["expires_at"] = json!(0);
    std::fs::write(&bpath, serde_json::to_vec(&disk).unwrap()).unwrap();
    let restarted_b = GatewayState::for_tests_with_static_dir(Some(b._dir.path().to_path_buf()));
    let cert = localhost_cert();
    let port = serve_https(crate::server::gateway_router(restarted_b.clone()), &cert).await;
    let client = PeerConnection::trusting(cert.cert.pem().as_bytes());
    let a = RemoteGatewayStore::load(path).unwrap();
    a.write(|data| data.gateways[0].address = format!("https://localhost:{port}"))
        .unwrap();
    a.rotate(&client, "cloud").await.unwrap();
    assert_eq!(a.list()[0].credential, *candidate);
    assert!(a.list()[0].rotation.error.is_none());
}

#[tokio::test]
async fn unsupported_rotation_preserves_the_pairing_and_retries_the_same_intent() {
    use axum::http::StatusCode;
    let cert = localhost_cert();
    let router = axum::Router::new().route(
        &format!(
            "{}{}",
            crate::gateway_protocol::GATEWAY_API_PREFIX,
            crate::gateway_protocol::PAIRING_PATH
        ),
        axum::routing::get(|| async { StatusCode::UNAUTHORIZED }),
    );
    let port = serve_https(router, &cert).await;
    let client = PeerConnection::trusting(cert.cert.pem().as_bytes());
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let mut gateway = remote("cloud");
    gateway.address = format!("https://localhost:{port}");
    gateway.gateway_protocol = 1;
    a.add(gateway.clone()).unwrap();
    assert!(a
        .rotate(&client, "cloud")
        .await
        .unwrap_err()
        .contains("does not support"));
    let failed = a.list()[0].clone();
    assert_eq!(failed.credential, gateway.credential);
    assert!(failed.rotation.pending.is_some());
    assert!(failed.rotation.error.is_some());
    a.rotate_due(&client).await;
    assert_eq!(a.list()[0], failed);
}
