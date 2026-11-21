use super::*;
use crate::gateway_protocol::{GATEWAY_PROTOCOL, HEADER_GATEWAY_CREDENTIAL};
use crate::peers::GATEWAY_HEALTH_PATH;
use lucidos_local_token::test_support::serve_https;
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
    let port = serve_https(crate::server::gateway_router(state.clone())).port();
    GatewayB {
        state,
        address: format!("https://localhost:{port}"),
        client: PeerConnection::new().unwrap(),
        _dir: dir,
    }
}

/// A stand-in answering health with `body`, over https.
async fn stand_in(body: serde_json::Value) -> (String, PeerConnection) {
    let router = axum::Router::new().route(
        GATEWAY_HEALTH_PATH,
        get(move || {
            let body = body.clone();
            async move { Json(body) }
        }),
    );
    let port = serve_https(router).port();
    (
        format!("https://localhost:{port}"),
        PeerConnection::new().unwrap(),
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
        "http://192.168.1.10:5252",
        "http://100.64.0.1",
        "http://0.0.0.0:5252",
    ] {
        assert!(valid_address(bad).is_err(), "{bad}");
    }
}

/// `https` adds nothing on loopback, where nothing crosses a network.
#[test]
fn a_loopback_address_may_be_plain_http() {
    for loopback in [
        "http://127.0.0.1:5252",
        "http://127.8.9.10:5252",
        "http://localhost:5252",
        "http://[::1]:5252",
    ] {
        assert_eq!(valid_address(loopback).unwrap(), loopback);
    }
}

#[test]
fn only_a_loopback_host_counts_as_loopback() {
    for loopback in [
        "http://127.0.0.1:1",
        "https://localhost:1",
        "http://[::1]:1",
        "http://[::ffff:127.0.0.1]:1",
    ] {
        assert!(is_loopback_origin(loopback), "{loopback}");
    }
    for remote in [
        "http://lucidos.example.com",
        "https://localhost.example.com",
        "http://192.168.1.10:5252",
        "http://[fe80::1]:1",
        "not an origin",
    ] {
        assert!(!is_loopback_origin(remote), "{remote}");
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
        gateway_id: None,
        name: "cloud".to_string(),
        address: "https://cloud.example.com".to_string(),
        pairing_id: "p1".to_string(),
        credential: "secret".to_string(),
        headers: Default::default(),
        gateway_protocol: 1,
        added_at: "2026-01-01T00:00:00Z".to_string(),
        rotation: Default::default(),
        return_access: None,
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
            gateway_id: None,
            name: "cloud".to_string(),
            address: "https://cloud.example.com".to_string(),
            pairing_id: "p1".to_string(),
            credential: "secret".to_string(),
            headers: Default::default(),
            gateway_protocol: 1,
            added_at: "2026-01-01T00:00:00Z".to_string(),
            rotation: Default::default(),
            return_access: None,
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
    let code = b
        .state
        .paired_gateways()
        .mint_secret(None, false, crate::pairing::Minter::PairedDevice)
        .unwrap();

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

/// Plan invariant 9: B's local process minted the secret, and B has a paired
/// device. So A waits for that device's Allow, then adds B on a retry.
#[tokio::test]
async fn a_waits_for_b_s_owner_then_pairs_on_a_retry() {
    let b = gateway_b().await;
    b.state
        .seed_device_for_test(crate::auth::PairedDevice {
            id: "device-1".into(),
            label: "My iPhone".into(),
            credential_digest: crate::auth::digest("cred-phone"),
            paired_at: "2020-01-01T00:00:00Z".into(),
            last_seen_at: None,
        })
        .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let code = b
        .state
        .paired_gateways()
        .mint_secret(None, false, crate::pairing::Minter::LocalProcess)
        .unwrap();
    let attempt = || {
        add(
            &a,
            &b.client,
            A_INSTANCE,
            "Lucidos on a",
            request(&b.address, &code, "cloud"),
        )
    };

    assert!(matches!(attempt().await, Err(AddError::AwaitingOwner)));
    assert!(a.list().is_empty(), "nothing is recorded while B waits");
    let waiting = b.state.pairing_requests();
    assert_eq!(waiting.len(), 1);
    assert_eq!(waiting[0].1.label, "Lucidos on a");
    assert!(b
        .state
        .answer_pairing_request(&waiting[0].1.id, crate::pairing::PairingAnswer::Allow)
        .unwrap());

    let row = attempt().await.unwrap();
    assert_eq!(row.name, "cloud");
    assert_eq!(b.state.paired_gateways().list().len(), 1);
}

#[tokio::test]
async fn removing_b_on_a_asks_b_to_forget_the_grant() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let code = b
        .state
        .paired_gateways()
        .mint_secret(None, false, crate::pairing::Minter::PairedDevice)
        .unwrap();
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

/// Pair A with B as `cloud`, sending `headers`.
async fn paired_as_cloud(b: &GatewayB, a: &RemoteGatewayStore, headers: ExtraHeaders) {
    let code = b
        .state
        .paired_gateways()
        .mint_secret(None, false, crate::pairing::Minter::PairedDevice)
        .unwrap();
    let mut pairing = request(&b.address, &code, "cloud");
    pairing.headers = headers;
    add(a, &b.client, A_INSTANCE, "a", pairing).await.unwrap();
}

fn fresh_secret(b: &GatewayB) -> String {
    b.state
        .paired_gateways()
        .mint_secret(None, false, crate::pairing::Minter::PairedDevice)
        .unwrap()
}

#[tokio::test]
async fn a_revoked_pairing_re_pairs_in_place_keeping_name_and_headers() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    paired_as_cloud(&b, &a, proxy_headers()).await;
    let revoked = a.list()[0].clone();
    b.state
        .paired_gateways()
        .unpair(&revoked.pairing_id)
        .unwrap();

    let (renewed, replaced) = repair(&a, &b.client, A_INSTANCE, "a", "cloud", &fresh_secret(&b))
        .await
        .unwrap();

    assert_eq!(replaced, revoked);
    let held = a.list();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0], renewed);
    assert_eq!(renewed.name, "cloud");
    assert_eq!(renewed.address, revoked.address);
    assert_eq!(renewed.headers, revoked.headers);
    assert_eq!(renewed.added_at, revoked.added_at);
    assert_eq!(renewed.gateway_id, revoked.gateway_id);
    assert_ne!(renewed.pairing_id, revoked.pairing_id);
    assert!(renewed.rotation.last_rotated_at.is_some());
    assert_eq!(store(&dir).list(), held, "the swap is saved");
    assert!(b
        .state
        .paired_gateways()
        .authorize(&credential_header(&renewed.credential))
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn a_failed_re_pair_changes_nothing() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    paired_as_cloud(&b, &a, Default::default()).await;
    let before = a.list();

    let refused = repair(&a, &b.client, A_INSTANCE, "a", "cloud", "00000000").await;
    assert!(matches!(refused, Err(AddError::Remote(ref m)) if m.contains("not valid")));
    let absent = repair(&a, &b.client, A_INSTANCE, "a", "office", &fresh_secret(&b)).await;
    assert!(matches!(absent, Err(AddError::Missing)));
    assert_eq!(a.list(), before);
}

#[tokio::test]
async fn a_legacy_re_pair_refuses_an_address_that_reaches_another_entry() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    paired_as_cloud(&b, &a, Default::default()).await;
    let mut legacy = remote("office");
    legacy.address = b.address.clone();
    legacy.gateway_id = None;
    a.add(legacy).unwrap();
    let before = a.list();

    let refused = repair(&a, &b.client, A_INSTANCE, "a", "office", &fresh_secret(&b)).await;

    assert!(matches!(refused, Err(AddError::Duplicate(ref name)) if name == "cloud"));
    assert_eq!(a.list(), before);
    assert_eq!(
        b.state.paired_gateways().list().len(),
        1,
        "B minted no second grant"
    );
}

#[tokio::test]
async fn a_re_pair_never_moves_a_name_to_another_gateway() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let mut elsewhere = remote("cloud");
    elsewhere.address = b.address.clone();
    elsewhere.gateway_id = Some("another-gateway".into());
    a.add(elsewhere).unwrap();
    let before = a.list();

    let refused = repair(&a, &b.client, A_INSTANCE, "a", "cloud", &fresh_secret(&b)).await;

    assert!(matches!(refused, Err(AddError::Invalid(ref m)) if m.contains("different gateway")));
    assert_eq!(a.list(), before);
    assert!(
        b.state.paired_gateways().list().is_empty(),
        "the grant B just made is handed back"
    );
}

#[tokio::test]
async fn re_pair_renews_return_access_only_for_an_own_device() {
    let b = gateway_b().await;
    let a_dir = tempfile::tempdir().unwrap();
    let a = GatewayState::for_tests_with_static_dir(Some(a_dir.path().to_path_buf()));
    a.seed_device_for_test(auth::PairedDevice {
        id: "own-device".into(),
        label: "Test desktop".into(),
        credential_digest: auth::digest("device-credential"),
        paired_at: "2020-01-01T00:00:00Z".into(),
        last_seen_at: None,
    })
    .unwrap();
    let device = auth::AuthenticatedDevice {
        id: "own-device".into(),
        label: "Test desktop".into(),
    };
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::ORIGIN,
        "http://localhost:5400".parse().unwrap(),
    );
    paired_as_cloud(&b, a.remote_gateways(), Default::default()).await;
    let _ = update_return_access_handler(
        State(a.clone()),
        axum::extract::Path("cloud".into()),
        Some(Extension(device.clone())),
        headers.clone(),
        Json(ReturnAccessRequest { enabled: true }),
    )
    .await
    .unwrap();

    let renewed = revoke_and_repair(&a, &b, Some(&device), &headers).await;
    assert!(renewed.return_access);
    assert!(renewed.return_access_warning.is_none());
    let route = b.state.paired_gateways().list()[0]
        .return_route
        .clone()
        .unwrap();
    assert_eq!(
        Some(route.grant_id),
        a.remote_gateways().list()[0]
            .return_access
            .as_ref()
            .map(|g| g.id.clone())
    );

    let unowned = revoke_and_repair(&a, &b, None, &headers).await;
    assert!(!unowned.return_access);
    assert!(unowned.return_access_warning.is_some());
    assert!(a.remote_gateways().list()[0].return_access.is_none());
}

/// B unpairs A's `cloud`, then A pairs it again as `repair_handler` does.
/// Not through the handler: A and B share this process's `instance_id()`.
async fn revoke_and_repair(
    a: &GatewayState,
    b: &GatewayB,
    device: Option<&auth::AuthenticatedDevice>,
    headers: &axum::http::HeaderMap,
) -> RemoteGatewayRow {
    let pairing_id = a.remote_gateways().list()[0].pairing_id.clone();
    b.state.paired_gateways().unpair(&pairing_id).unwrap();
    let _operation = a.remote_gateways().operations.lock().await;
    let (renewed, replaced) = repair(
        a.remote_gateways(),
        a.peer_connection(),
        A_INSTANCE,
        "a",
        "cloud",
        &fresh_secret(b),
    )
    .await
    .unwrap();
    renew_return_access(a, renewed, &replaced, device, headers)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_plain_http_address_is_refused_before_the_code_is_spent() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let code = b
        .state
        .paired_gateways()
        .mint_secret(None, false, crate::pairing::Minter::PairedDevice)
        .unwrap();
    let http = "http://lucidos.example.com";

    let refused = add(
        &a,
        &b.client,
        A_INSTANCE,
        "a",
        request(http, &code, "cloud"),
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

fn health_router() -> axum::Router {
    axum::Router::new().route(
        GATEWAY_HEALTH_PATH,
        get(|| async { Json(json!({"role": "gateway"})) }),
    )
}

/// An address stored as `http://` before loopback served TLS still reaches
/// its gateway, now over https.
#[tokio::test]
async fn a_stored_http_loopback_address_is_called_over_https() {
    let port = serve_https(health_router()).port();
    let client = PeerConnection::new().unwrap();
    let body = client
        .health(&format!("http://127.0.0.1:{port}"))
        .await
        .expect("the loopback address is read as https");
    assert_eq!(body["role"], "gateway");

    // Refused before any connection: the scheme alone decides.
    let error = client
        .health("http://lucidos.example.com")
        .await
        .unwrap_err();
    assert!(error.contains("scheme"), "{error}");
}

/// Every gateway serves a leaf from the loopback CA on loopback, which no
/// public root signs.
#[tokio::test]
async fn a_loopback_gateway_answers_under_the_loopback_ca() {
    let port = serve_https(health_router()).port();
    let body = PeerConnection::new()
        .unwrap()
        .health(&format!("https://localhost:{port}"))
        .await
        .expect("the loopback client trusts the loopback CA");
    assert_eq!(body["role"], "gateway");
}

/// Plain http on a loopback port is not a gateway: it never gets a request,
/// so a squatter there never sees a credential.
#[tokio::test]
async fn a_plaintext_loopback_listener_is_not_reached() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, health_router()).await.ok() });
    let result = PeerConnection::new()
        .unwrap()
        .health(&format!("http://127.0.0.1:{port}"))
        .await;
    assert!(result.is_err(), "{result:?}");
}

#[tokio::test]
async fn a_loopback_gateway_pairs_at_its_stored_http_address() {
    let dir = tempfile::tempdir().unwrap();
    let b = GatewayState::for_tests_with_static_dir(Some(dir.path().to_path_buf()));
    let port = serve_https(crate::server::gateway_router(b.clone())).port();
    let address = format!("http://127.0.0.1:{port}");
    let code = b
        .paired_gateways()
        .mint_secret(None, false, crate::pairing::Minter::PairedDevice)
        .unwrap();
    let a_dir = tempfile::tempdir().unwrap();
    let a = store(&a_dir);

    let row = add(
        &a,
        &PeerConnection::new().unwrap(),
        A_INSTANCE,
        "a",
        request(&address, &code, "dmg"),
    )
    .await
    .expect("a loopback peer pairs over https");
    assert_eq!(row.address, address);
    assert_eq!(b.paired_gateways().list().len(), 1);
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
    let code = b
        .state
        .paired_gateways()
        .mint_secret(None, false, crate::pairing::Minter::PairedDevice)
        .unwrap();
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
        gateway_id: None,
        name: name.to_string(),
        address: "https://cloud.example.com".to_string(),
        pairing_id: "pairing".to_string(),
        credential: "credential".to_string(),
        headers: Default::default(),
        gateway_protocol: GATEWAY_PROTOCOL,
        added_at: "2020-01-01T00:00:00Z".to_string(),
        rotation: Default::default(),
        return_access: None,
    }
}

#[test]
fn return_access_is_durable_off_by_default_and_renewed_after_revocation() {
    let dir = tempfile::tempdir().unwrap();
    let saved = store(&dir);
    saved.add(remote("cloud")).unwrap();
    assert!(saved.list()[0].return_access.is_none());
    let first = saved.set_return_access("cloud", true).unwrap().unwrap();
    assert_eq!(store(&dir).list()[0].return_access, first.return_access);
    saved.set_return_access("cloud", false).unwrap().unwrap();
    assert!(store(&dir).list()[0].return_access.is_none());
    let second = saved.set_return_access("cloud", true).unwrap().unwrap();
    assert_ne!(first.return_access, second.return_access);
    assert!(saved.set_return_access("absent", true).unwrap().is_none());
}

#[tokio::test]
async fn an_own_device_registers_return_access_and_refreshes_an_older_pairing() {
    use crate::gateway_protocol::GatewayRedemption;
    use crate::pairing::{Minter, OwnerGate};
    let b = gateway_b().await;
    let a_dir = tempfile::tempdir().unwrap();
    let a = GatewayState::for_tests_with_static_dir(Some(a_dir.path().to_path_buf()));
    let secret = b
        .state
        .paired_gateways()
        .mint_secret(Some("Routing gateway".into()), false, Minter::PairedDevice)
        .unwrap();
    let GatewayRedemption::Paired {
        pairing_id,
        credential,
    } = b
        .state
        .paired_gateways()
        .redeem(&secret, None, OwnerGate::NoDevicePaired)
        .unwrap()
    else {
        panic!()
    };
    let mut peer = remote("cloud");
    peer.address = b.address;
    peer.pairing_id = pairing_id.clone();
    peer.credential = credential;
    peer.gateway_id = None;
    peer.gateway_protocol = 4;
    a.remote_gateways().add(peer).unwrap();
    a.seed_device_for_test(auth::PairedDevice {
        id: "own-device".into(),
        label: "Test desktop".into(),
        credential_digest: auth::digest("device-credential"),
        paired_at: "2020-01-01T00:00:00Z".into(),
        last_seen_at: None,
    })
    .unwrap();
    let device = auth::AuthenticatedDevice {
        id: "own-device".into(),
        label: "Test desktop".into(),
    };
    let mut headers = axum::http::HeaderMap::new();
    headers.insert(
        axum::http::header::ORIGIN,
        "http://localhost:5400".parse().unwrap(),
    );
    let Json(enabled) = update_return_access_handler(
        State(a.clone()),
        axum::extract::Path("cloud".into()),
        Some(Extension(device.clone())),
        headers.clone(),
        Json(ReturnAccessRequest { enabled: true }),
    )
    .await
    .unwrap();
    assert!(enabled.return_access);
    assert_eq!(
        a.remote_gateways().list()[0].gateway_id.as_deref(),
        Some(b.state.gateway_id())
    );
    assert_eq!(enabled.gateway_protocol, GATEWAY_PROTOCOL);
    let registered = b.state.paired_gateways().list()[0]
        .return_route
        .clone()
        .unwrap();
    assert_eq!(registered.gateway_id, a.gateway_id());
    assert_eq!(
        registered.grant_id,
        a.remote_gateways().list()[0]
            .return_access
            .as_ref()
            .unwrap()
            .id
    );

    let mut agent_headers = headers.clone();
    agent_headers.insert(
        auth::HEADER_AGENT_ORIGIN_TOKEN,
        "claimed-agent".parse().unwrap(),
    );
    let denied = update_return_access_handler(
        State(a.clone()),
        axum::extract::Path("cloud".into()),
        Some(Extension(device.clone())),
        agent_headers,
        Json(ReturnAccessRequest { enabled: false }),
    )
    .await;
    assert_eq!(
        denied.unwrap_err().into_response().status(),
        axum::http::StatusCode::FORBIDDEN
    );
    assert!(a.remote_gateways().list()[0].return_access.is_some());

    let Json(disabled) = update_return_access_handler(
        State(a.clone()),
        axum::extract::Path("cloud".into()),
        Some(Extension(device.clone())),
        headers.clone(),
        Json(ReturnAccessRequest { enabled: false }),
    )
    .await
    .unwrap();
    assert!(!disabled.return_access);
    assert!(b.state.paired_gateways().list()[0].return_route.is_none());

    let updated = update_return_access_handler(
        State(a.clone()),
        axum::extract::Path("cloud".into()),
        Some(Extension(device.clone())),
        headers.clone(),
        Json(ReturnAccessRequest { enabled: true }),
    )
    .await
    .unwrap();
    assert!(updated.0.return_access);
    b.state.paired_gateways().unpair(&pairing_id).unwrap();
    let Json(disabled) = update_return_access_handler(
        State(a.clone()),
        axum::extract::Path("cloud".into()),
        Some(Extension(device)),
        headers,
        Json(ReturnAccessRequest { enabled: false }),
    )
    .await
    .unwrap();
    assert!(!disabled.return_access);
    assert!(disabled.return_access_warning.is_some());
    assert!(a.remote_gateways().list()[0].return_access.is_none());
}

#[test]
fn older_remote_records_never_acquire_return_access() {
    let mut record = serde_json::to_value(remote("cloud")).unwrap();
    record.as_object_mut().unwrap().remove("return_access");
    let loaded: RemoteGateway = serde_json::from_value(record).unwrap();
    assert!(loaded.return_access.is_none());
}

#[test]
fn a_failed_return_access_save_preserves_the_previous_grant() {
    let dir = tempfile::tempdir().unwrap();
    let saved = store(&dir);
    saved.add(remote("cloud")).unwrap();
    let first = saved.set_return_access("cloud", true).unwrap().unwrap();
    {
        let _locked = ReadOnlyDir::new(dir.path());
        assert!(saved.set_return_access("cloud", false).is_err());
        assert_eq!(saved.list()[0].return_access, first.return_access);
    }
    assert_eq!(store(&dir).list()[0].return_access, first.return_access);
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
    let code = bstate
        .paired_gateways()
        .mint_secret(None, false, crate::pairing::Minter::PairedDevice)
        .unwrap();
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
    let port = serve_https(router).port();
    let client = PeerConnection::new().unwrap();
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
        let port = serve_https(router).port();
        let client = PeerConnection::new().unwrap();
        let error = client
            .health(&format!("https://localhost:{port}"))
            .await
            .unwrap_err();
        assert!(error.contains("sign-in page at localhost"), "{error}");
    }
}

#[tokio::test]
async fn proxy_headers_stay_on_the_typed_origin_despite_an_advertised_address() {
    let foreign_hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits = foreign_hits.clone();
    let foreign = axum::Router::new().fallback(move || {
        let hits = hits.clone();
        async move {
            hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            "unexpected"
        }
    });
    let foreign_port = serve_https(foreign).port();
    let foreign_address = format!("https://localhost:{foreign_port}");
    let advertised = foreign_address.clone();
    let router = axum::Router::new()
        .route(GATEWAY_HEALTH_PATH, get(|| async { Json(json!({"role":"gateway", "instance_id":"remote-b", "gateway_protocol": GATEWAY_PROTOCOL})) }))
        .route(&format!("{}{}", crate::gateway_protocol::GATEWAY_API_PREFIX, crate::gateway_protocol::PAIR_PATH), axum::routing::post(move || {
            let advertised = advertised.clone();
            async move { Json(json!({"pairing_id":"pairing", "credential":"credential", "gateway_protocol":GATEWAY_PROTOCOL, "external_address":advertised})) }
        }));
    let port = serve_https(router).port();
    let address = format!("https://localhost:{port}");
    let client = PeerConnection::new().unwrap();
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
    let code = b
        .state
        .paired_gateways()
        .mint_secret(None, false, crate::pairing::Minter::PairedDevice)
        .unwrap();
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
    let port = serve_https(crate::server::gateway_router(restarted_b.clone())).port();
    let client = PeerConnection::new().unwrap();
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
    let router = axum::Router::new().route(
        &format!(
            "{}{}",
            crate::gateway_protocol::GATEWAY_API_PREFIX,
            crate::gateway_protocol::PAIRING_PATH
        ),
        axum::routing::get(|| async { StatusCode::UNAUTHORIZED }),
    );
    let port = serve_https(router).port();
    let client = PeerConnection::new().unwrap();
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

#[test]
fn durable_destination_identity_survives_store_reload_and_legacy_rows_stay_readable() {
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let mut known = remote("known");
    known.gateway_id = Some("a".repeat(64));
    a.add(known.clone()).unwrap();
    let mut legacy_json = serde_json::to_value(remote("legacy")).unwrap();
    legacy_json.as_object_mut().unwrap().remove("gateway_id");
    let legacy: RemoteGateway = serde_json::from_value(legacy_json).unwrap();
    assert_eq!(legacy.gateway_id, None);
    a.add(legacy.clone()).unwrap();
    drop(a);
    assert_eq!(store(&dir).list(), vec![known, legacy]);
}

fn mint(b: &GatewayB) -> String {
    b.state
        .paired_gateways()
        .mint_secret(None, false, crate::pairing::Minter::PairedDevice)
        .unwrap()
}

#[tokio::test]
async fn duplicate_destination_uses_conflict_and_preserves_the_unused_code() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    add(
        &a,
        &b.client,
        A_INSTANCE,
        "a",
        request(&b.address, &mint(&b), "cloud"),
    )
    .await
    .unwrap();
    let code = mint(&b);
    let error = add(
        &a,
        &b.client,
        A_INSTANCE,
        "a",
        request(&format!("{}/", b.address), &code, "alias"),
    )
    .await
    .unwrap_err();
    let response = error.into_response();
    assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("cloud"));
    assert_eq!(b.state.paired_gateways().list().len(), 1);
    // Another initiating gateway can still redeem the unconsumed code.
    let other_dir = tempfile::tempdir().unwrap();
    add(
        &store(&other_dir),
        &b.client,
        "other",
        "other",
        request(&b.address, &code, "cloud"),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn concurrent_adds_only_redeem_one_grant() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let (one, two) = tokio::join!(
        add(
            &a,
            &b.client,
            A_INSTANCE,
            "a",
            request(&b.address, &mint(&b), "one")
        ),
        add(
            &a,
            &b.client,
            A_INSTANCE,
            "a",
            request(&b.address, &mint(&b), "two")
        ),
    );
    assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
    assert_eq!(a.list().len(), 1);
    assert_eq!(b.state.paired_gateways().list().len(), 1);
}

#[test]
fn insertion_refuses_the_same_durable_destination_after_reload() {
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let mut one = remote("one");
    one.gateway_id = Some("b".repeat(64));
    a.add(one.clone()).unwrap();
    drop(a);
    let a = store(&dir);
    one.name = "two".into();
    one.address = "https://alias.example.com".into();
    let response = a.add(one).unwrap_err().into_response();
    assert_eq!(response.status(), axum::http::StatusCode::CONFLICT);
    assert_eq!(a.list().len(), 1);
}

/// Real pairing routes with a simulated independent process's public boot id.
fn router_with_boot(state: GatewayState, boot: &str) -> axum::Router {
    let boot = boot.to_string();
    crate::server::gateway_router(state).layer(axum::middleware::from_fn(
        move |req: axum::extract::Request, next: axum::middleware::Next| {
            let boot = boot.clone();
            async move {
                if req.uri().path() == GATEWAY_HEALTH_PATH {
                    assert!(!req.headers().contains_key(HEADER_GATEWAY_CREDENTIAL));
                    Json(json!({"role":"gateway", "instance_id":boot, "gateway_protocol":GATEWAY_PROTOCOL})).into_response()
                } else {
                    next.run(req).await
                }
            }
        },
    ))
}

fn closed_address() -> String {
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    format!("https://localhost:{}", socket.local_addr().unwrap().port())
}

#[tokio::test]
async fn aliases_of_the_same_gateway_are_rejected_before_redemption() {
    let b = gateway_b().await;
    let alias_port = serve_https(crate::server::gateway_router(b.state.clone())).port();
    let alias = format!("https://localhost:{alias_port}");
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    add(
        &a,
        &b.client,
        A_INSTANCE,
        "a",
        request(&b.address, &mint(&b), "cloud"),
    )
    .await
    .unwrap();
    let code = mint(&b);
    assert!(
        matches!(add(&a, &b.client, A_INSTANCE, "a", request(&alias, &code, "alias")).await, Err(AddError::Duplicate(name)) if name == "cloud")
    );
    assert_eq!(b.state.paired_gateways().list().len(), 1);
    let previous = a.remove("cloud").unwrap().unwrap();
    b.client.leave(&previous).await.unwrap();
    add(
        &a,
        &b.client,
        A_INSTANCE,
        "a",
        request(&alias, &code, "repaired"),
    )
    .await
    .unwrap();
    assert_eq!(b.state.paired_gateways().list().len(), 1);
}

#[tokio::test]
async fn legacy_identity_hydrates_through_saved_proxy_headers_and_distinct_peers_still_pair() {
    let b = gateway_b().await;
    let saved_port = serve_https(proxy_guard(router_with_boot(b.state.clone(), "old-boot"))).port();
    let saved = format!("https://localhost:{saved_port}");
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let mut req = request(&saved, &mint(&b), "cloud");
    req.headers = proxy_headers();
    add(&a, &b.client, A_INSTANCE, "a", req).await.unwrap();
    a.write(|data| data.gateways[0].gateway_id = None).unwrap();
    let c = gateway_b().await;
    // Different real gateways in one test process need distinct simulated boot ids.
    let c_port = serve_https(router_with_boot(c.state.clone(), "new-boot")).port();
    add(
        &a,
        &b.client,
        A_INSTANCE,
        "a",
        request(&format!("https://localhost:{c_port}"), &mint(&c), "other"),
    )
    .await
    .unwrap();
    assert_eq!(a.list().len(), 2);
    assert_eq!(
        a.list()[0].gateway_id.as_deref(),
        Some(b.state.gateway_id())
    );
    assert_eq!(
        store(&dir).list()[0].gateway_id.as_deref(),
        Some(b.state.gateway_id())
    );
}

#[tokio::test]
async fn offline_legacy_identity_blocks_an_add_without_consuming_a_code() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let mut old = remote("offline");
    old.address = closed_address();
    a.add(old).unwrap();
    let code = mint(&b);
    assert!(
        matches!(add(&a, &b.client, A_INSTANCE, "a", request(&b.address, &code, "new")).await, Err(AddError::Invalid(message)) if message.contains("reconnect or update") && message.contains("offline"))
    );
    assert!(b.state.paired_gateways().list().is_empty());
    a.remove("offline").unwrap();
    add(
        &a,
        &b.client,
        A_INSTANCE,
        "a",
        request(&b.address, &code, "new"),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn saved_durable_identity_catches_a_new_alias_after_restart_and_only_returns_the_new_grant() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    add(
        &a,
        &b.client,
        A_INSTANCE,
        "a",
        request(&b.address, &mint(&b), "cloud"),
    )
    .await
    .unwrap();
    let original_grant = b.state.paired_gateways().list()[0].id.clone();
    // The old origin no longer answers after the destination restarts elsewhere.
    a.write(|data| data.gateways[0].address = closed_address())
        .unwrap();
    drop(a);
    let a = store(&dir);
    let port = serve_https(router_with_boot(b.state.clone(), "after-restart")).port();
    assert!(
        matches!(add(&a, &b.client, A_INSTANCE, "a", request(&format!("https://localhost:{port}"), &mint(&b), "alias")).await, Err(AddError::Duplicate(name)) if name == "cloud")
    );
    assert_eq!(a.list().len(), 1);
    let grants = b.state.paired_gateways().list();
    assert_eq!(grants.len(), 1);
    assert_eq!(grants[0].id, original_grant);
}

#[tokio::test]
async fn an_offline_known_destination_does_not_block_a_distinct_modern_peer() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let mut old = remote("offline");
    old.address = closed_address();
    old.gateway_id = Some("different-destination".into());
    a.add(old).unwrap();
    add(
        &a,
        &b.client,
        A_INSTANCE,
        "a",
        request(&b.address, &mint(&b), "new"),
    )
    .await
    .unwrap();
    assert_eq!(a.list().len(), 2);
}

#[tokio::test]
async fn failed_persistence_returns_the_new_grant() {
    let b = gateway_b().await;
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let _locked = ReadOnlyDir::new(dir.path());
    assert!(matches!(
        add(
            &a,
            &b.client,
            A_INSTANCE,
            "a",
            request(&b.address, &mint(&b), "new")
        )
        .await,
        Err(AddError::Internal(_))
    ));
    assert!(a.list().is_empty());
    assert!(b.state.paired_gateways().list().is_empty());
}

#[tokio::test]
async fn failed_compensation_reports_the_unrevoked_grant() {
    let router = axum::Router::new()
        .route(GATEWAY_HEALTH_PATH, get(|| async { Json(json!({"role":"gateway", "instance_id":"new-boot", "gateway_protocol":GATEWAY_PROTOCOL})) }))
        .route(&format!("{}{}", crate::gateway_protocol::GATEWAY_API_PREFIX, crate::gateway_protocol::PAIR_PATH), axum::routing::post(|| async { Json(json!({"gateway_id":"same-destination", "pairing_id":"new-grant", "credential":"new-credential", "gateway_protocol":GATEWAY_PROTOCOL})) }))
        .route(&format!("{}{}", crate::gateway_protocol::GATEWAY_API_PREFIX, crate::gateway_protocol::PAIRING_PATH), axum::routing::delete(|headers: axum::http::HeaderMap| async move {
            assert_eq!(headers[HEADER_GATEWAY_CREDENTIAL], "new-credential");
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        }));
    let port = serve_https(router).port();
    let client = PeerConnection::new().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let mut old = remote("cloud");
    old.address = closed_address();
    old.gateway_id = Some("same-destination".into());
    a.add(old).unwrap();
    let error = add(
        &a,
        &client,
        A_INSTANCE,
        "a",
        request(&format!("https://localhost:{port}"), "12345678", "alias"),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, AddError::Internal(message) if message.contains("could not be revoked") && message.contains("cloud") && message.contains("Access page"))
    );
    assert_eq!(a.list().len(), 1);
}

/// An older peer with real grants but no durable identity fields on the wire.
fn legacy_router(state: GatewayState, boot: &str) -> axum::Router {
    router_with_boot(state, boot).layer(axum::middleware::from_fn(
        |req: axum::extract::Request, next: axum::middleware::Next| async move {
            let response = next.run(req).await;
            let (parts, body) = response.into_parts();
            let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
            if let Ok(mut body) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                if let Some(object) = body.as_object_mut() {
                    object.remove("gateway_id");
                }
                let mut response = Json(body).into_response();
                *response.status_mut() = parts.status;
                response
            } else {
                axum::response::Response::from_parts(parts, axum::body::Body::from(bytes))
            }
        },
    ))
}

#[tokio::test]
async fn fresh_boot_evidence_supports_distinct_older_peers_and_refuses_their_aliases() {
    let b = gateway_b().await;
    let c = gateway_b().await;
    let bport = serve_https(legacy_router(b.state.clone(), "older-b")).port();
    let cport = serve_https(legacy_router(c.state.clone(), "older-c")).port();
    let c_alias = serve_https(legacy_router(c.state.clone(), "older-c")).port();
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    for (peer, port, name) in [(&b, bport, "old-b"), (&c, cport, "old-c")] {
        add(
            &a,
            &b.client,
            A_INSTANCE,
            "a",
            request(&format!("https://localhost:{port}"), &mint(peer), name),
        )
        .await
        .unwrap();
    }
    assert_eq!(a.list().len(), 2);
    assert!(a.list().iter().all(|r| r.gateway_id.is_none()));
    assert!(
        matches!(add(&a, &b.client, A_INSTANCE, "a", request(&format!("https://localhost:{c_alias}"), &mint(&c), "alias")).await, Err(AddError::Duplicate(name)) if name == "old-c")
    );
    assert_eq!(c.state.paired_gateways().list().len(), 1);
}

#[tokio::test]
async fn older_candidate_cannot_bypass_an_offline_known_destination_and_returns_its_new_grant() {
    let b = gateway_b().await;
    let port = serve_https(legacy_router(b.state.clone(), "older-b")).port();
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let mut old = remote("offline");
    old.address = closed_address();
    old.gateway_id = Some("known-destination".into());
    a.add(old).unwrap();
    assert!(
        matches!(add(&a, &b.client, A_INSTANCE, "a", request(&format!("https://localhost:{port}"), &mint(&b), "new")).await, Err(AddError::Invalid(message)) if message.contains("offline"))
    );
    assert_eq!(a.list().len(), 1);
    assert!(b.state.paired_gateways().list().is_empty());
}

#[test]
fn preexisting_duplicate_destinations_are_preserved_on_load() {
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let mut first = remote("first");
    first.gateway_id = Some("same-destination".into());
    let mut second = first.clone();
    second.name = "second".into();
    a.write(|data| data.gateways = vec![first.clone(), second.clone()])
        .unwrap();
    drop(a);
    assert_eq!(store(&dir).list(), vec![first, second]);
}

#[tokio::test]
async fn hydration_checks_pairing_identity_and_keeps_saved_credentials_off_new_origins() {
    let own_path = format!(
        "{}{}",
        crate::gateway_protocol::GATEWAY_API_PREFIX,
        crate::gateway_protocol::PAIRING_PATH
    );
    let saved_router = proxy_guard(axum::Router::new()
        .route(GATEWAY_HEALTH_PATH, get(|headers: axum::http::HeaderMap| async move {
            assert!(!headers.contains_key(HEADER_GATEWAY_CREDENTIAL));
            Json(json!({"role":"gateway", "instance_id":"old", "gateway_protocol":GATEWAY_PROTOCOL}))
        }))
        .route(&own_path, get(|headers: axum::http::HeaderMap| async move {
            assert_eq!(headers[HEADER_GATEWAY_CREDENTIAL], "credential");
            // This is not the saved grant: its claimed destination must not hydrate it.
            Json(json!({"pairing_id":"wrong-pairing", "gateway_id":"untrusted-identity"}))
        })));
    let port = serve_https(saved_router).port();
    let b = gateway_b().await;
    let new_router = router_with_boot(b.state.clone(), "new").layer(axum::middleware::from_fn(
        |req: axum::extract::Request, next: axum::middleware::Next| async move {
            assert!(!req.headers().contains_key(HEADER_GATEWAY_CREDENTIAL));
            assert!(!req.headers().contains_key("x-proxy-token"));
            next.run(req).await
        },
    ));
    let newport = serve_https(new_router).port();
    let dir = tempfile::tempdir().unwrap();
    let a = store(&dir);
    let mut old = remote("old");
    old.address = format!("https://localhost:{port}");
    old.headers = proxy_headers();
    a.add(old).unwrap();
    add(
        &a,
        &b.client,
        A_INSTANCE,
        "a",
        request(&format!("https://localhost:{newport}"), &mint(&b), "new"),
    )
    .await
    .unwrap();
    assert_eq!(a.list()[0].gateway_id, None);
    assert_eq!(a.list().len(), 2);
}

#[tokio::test]
async fn add_http_handler_names_the_existing_remote_in_a_conflict() {
    use tower::ServiceExt;
    let b = gateway_b().await;
    let port = serve_https(router_with_boot(b.state.clone(), "other-process")).port();
    let dir = tempfile::tempdir().unwrap();
    let a = GatewayState::for_tests_with_static_dir(Some(dir.path().to_path_buf()));
    for (name, expected) in [
        ("cloud", axum::http::StatusCode::OK),
        ("alias", axum::http::StatusCode::CONFLICT),
    ] {
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/~/api/v1/remote-gateways")
            .header(crate::auth::HEADER_LOCAL_TOKEN, "test-local-token")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(json!({"name":name, "address":format!("https://localhost:{port}"), "code":mint(&b)}).to_string())).unwrap();
        let response = crate::server::gateway_router(a.clone())
            .oneshot(req)
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(status, expected, "{}", String::from_utf8_lossy(&body));
        if status == axum::http::StatusCode::CONFLICT {
            assert!(String::from_utf8_lossy(&body).contains("cloud"));
        }
    }
    assert_eq!(b.state.paired_gateways().list().len(), 1);
}
