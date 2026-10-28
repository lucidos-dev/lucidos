use super::*;
use crate::gateway_protocol::GATEWAY_PROTOCOL;
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

/// Gateway B, serving the router the process serves, over https.
struct GatewayB {
    state: GatewayState,
    address: String,
    client: PeerClient,
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
        client: PeerClient::trusting(cert.cert.pem().as_bytes()),
        _dir: dir,
    }
}

/// A stand-in answering health with `body`, over https.
async fn stand_in(body: serde_json::Value) -> (String, PeerClient) {
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
        PeerClient::trusting(cert.cert.pem().as_bytes()),
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
        gateway_protocol: 1,
        added_at: "2026-01-01T00:00:00Z".to_string(),
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
            gateway_protocol: 1,
            added_at: "2026-01-01T00:00:00Z".to_string(),
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
    let code = b.state.paired_gateways().mint_code(None).unwrap();

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
        .is_some());
}

#[tokio::test]
async fn removing_b_on_a_asks_b_to_forget_the_grant() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let code = b.state.paired_gateways().mint_code(None).unwrap();
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
    let code = b.state.paired_gateways().mint_code(None).unwrap();
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

    let client = PeerClient::new().unwrap();
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
    let code = b.state.paired_gateways().mint_code(None).unwrap();
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
        gateway_protocol: GATEWAY_PROTOCOL,
        added_at: "2020-01-01T00:00:00Z".to_string(),
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
