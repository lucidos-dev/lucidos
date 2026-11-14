use std::net::{IpAddr, Ipv4Addr};
use std::sync::Mutex;

use lucidos_tailscale::TailnetNode;

use super::*;

/// A name that matches no known tailnet suffix: nothing may depend on its shape.
const NAME: &str = "node-7.some-base.example";
/// The name a node reports after it moves to another tailnet.
const OTHER_NAME: &str = "node-7.other-base.example";
const TAILNET_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(100, 70, 0, 1));
/// A documentation address (RFC 5737), standing in for a LAN one.
const LAN_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));

fn listening_on(listeners: Vec<IpAddr>) -> CertStore {
    let store = CertStore::default();
    store.set_listeners(listeners);
    store
}

/// A store whose gateway listens on the node's tailnet address.
fn listening() -> CertStore {
    listening_on(vec![TAILNET_IP])
}

struct Pem {
    cert: Vec<u8>,
    key: Vec<u8>,
}

fn mint(name: &str) -> Pem {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let issued = rcgen::generate_simple_self_signed(vec![name.to_string()]).unwrap();
    Pem {
        cert: issued.cert.pem().into_bytes(),
        key: issued.key_pair.serialize_pem().into_bytes(),
    }
}

/// A CA standing in for a public one. A test passes it as the trusted roots.
struct TestCa {
    cert: rcgen::Certificate,
    key: rcgen::KeyPair,
}

fn test_ca() -> TestCa {
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    TestCa { cert, key }
}

impl TestCa {
    /// A server certificate for `names`, as PEM.
    fn issue(&self, names: &[&str]) -> Vec<u8> {
        let names = names
            .iter()
            .map(|name| name.to_string())
            .collect::<Vec<_>>();
        let mut params = rcgen::CertificateParams::new(names).unwrap();
        params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.signed_by(&key, &self.cert, &self.key).unwrap();
        cert.pem().into_bytes()
    }

    fn roots(&self) -> Arc<rustls::RootCertStore> {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(self.cert.der().clone()).unwrap();
        Arc::new(roots)
    }
}

/// A store listening on the tailnet address, with an operator certificate.
fn with_operator(chain_pem: &[u8], roots: Arc<rustls::RootCertStore>) -> CertStore {
    let store = listening();
    store.set_operator(OperatorCert::new(chain_pem, roots));
    store
}

fn joined(name: Option<&str>, certifiable: bool) -> TailnetState {
    TailnetState::Joined(TailnetNode {
        ipv4: Ipv4Addr::new(100, 70, 0, 1),
        addresses: vec![TAILNET_IP],
        magic_dns_name: name.map(str::to_string),
        can_serve_https: certifiable,
    })
}

/// Answers with a fixed state and pair, and records every name asked for.
struct FakeTailnet {
    state: TailnetState,
    pair: Result<Pem, String>,
    asked: Mutex<Vec<String>>,
}

impl FakeTailnet {
    fn new(state: TailnetState, pair: Result<Pem, String>) -> Arc<FakeTailnet> {
        Arc::new(FakeTailnet {
            state,
            pair,
            asked: Mutex::default(),
        })
    }

    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

impl Tailnet for FakeTailnet {
    fn state(&self) -> TailnetState {
        self.state.clone()
    }

    fn cert_pair(&self, name: &str) -> Result<CertPair, String> {
        self.asked.lock().unwrap().push(name.to_string());
        self.pair
            .as_ref()
            .map_err(Clone::clone)
            .map(|pem| CertPair {
                cert_pem: pem.cert.clone(),
                key_pem: pem.key.clone(),
            })
    }
}

#[tokio::test]
async fn it_asks_for_the_reported_name_only_and_serves_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = listening();
    let tailnet = FakeTailnet::new(joined(Some(NAME), true), Ok(mint(NAME)));
    assert_eq!(
        Refreshed::Fresh,
        refresh(&store, tailnet.clone(), dir.path()).await
    );
    assert_eq!(tailnet.asked(), [NAME]);
    assert_eq!(store.current().unwrap().name, NAME);
    let state = store.state();
    assert_eq!(state.source, TlsSource::Tailnet);
    assert_eq!(state.name.as_deref(), Some(NAME));
    assert!(state.expires_at.is_some());
    assert_eq!(state.error, None);
}

#[tokio::test]
async fn a_name_outside_cert_domains_is_never_requested() {
    let dir = tempfile::tempdir().unwrap();
    let store = listening();
    let tailnet = FakeTailnet::new(joined(Some(NAME), false), Ok(mint(NAME)));
    assert_ne!(
        Refreshed::Fresh,
        refresh(&store, tailnet.clone(), dir.path()).await
    );
    assert!(tailnet.asked().is_empty());
    let state = store.state();
    assert_eq!(state.source, TlsSource::None);
    assert!(state.error.unwrap().contains(NAME));
}

#[tokio::test]
async fn every_way_to_have_no_name_says_why() {
    for (tailnet_state, says) in [
        (joined(None, false), "MagicDNS is off"),
        (TailnetState::NotInstalled, "not installed"),
        (
            TailnetState::NotJoined {
                backend_state: "NeedsLogin".into(),
            },
            "not signed in",
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = listening();
        let tailnet = FakeTailnet::new(tailnet_state, Ok(mint(NAME)));
        refresh(&store, tailnet.clone(), dir.path()).await;
        let error = store.state().error.unwrap();
        assert!(error.contains(says), "{error}");
        assert!(tailnet.asked().is_empty());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn the_kept_copy_is_readable_by_its_owner_only() {
    use std::os::unix::fs::PermissionsExt;

    let data = tempfile::tempdir().unwrap();
    let dir = tls_dir(data.path());
    let tailnet = FakeTailnet::new(joined(Some(NAME), true), Ok(mint(NAME)));
    assert_eq!(Refreshed::Fresh, refresh(&listening(), tailnet, &dir).await);
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&dir), 0o700);
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 3, "{files:?}");
    for file in files {
        assert_eq!(mode(&file), 0o600, "{}", file.display());
    }
}

#[tokio::test]
async fn a_failed_renewal_keeps_the_current_cert_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let store = listening();
    let good = FakeTailnet::new(joined(Some(NAME), true), Ok(mint(NAME)));
    assert_eq!(Refreshed::Fresh, refresh(&store, good, dir.path()).await);
    let served = store.state();

    let failing = FakeTailnet::new(joined(Some(NAME), true), Err("rate limited".into()));
    assert_ne!(Refreshed::Fresh, refresh(&store, failing, dir.path()).await);
    assert_eq!(store.current().unwrap().name, NAME);
    let state = store.state();
    assert_eq!(state.source, TlsSource::Tailnet);
    assert_eq!(state.expires_at, served.expires_at);
    assert_eq!(state.error.as_deref(), Some("rate limited"));
}

#[tokio::test]
async fn a_pair_that_will_not_load_is_a_failure_not_an_install() {
    let dir = tempfile::tempdir().unwrap();
    let store = listening();
    let junk = Pem {
        cert: b"junk".to_vec(),
        key: b"junk".to_vec(),
    };
    let tailnet = FakeTailnet::new(joined(Some(NAME), true), Ok(junk));
    assert_ne!(Refreshed::Fresh, refresh(&store, tailnet, dir.path()).await);
    assert!(store.current().is_none());
    assert_eq!(store.state().source, TlsSource::None);
    assert!(!dir.path().join(CERT_FILE).exists(), "junk is never kept");
}

#[tokio::test]
async fn a_restart_serves_the_kept_copy_at_once() {
    let dir = tempfile::tempdir().unwrap();
    let tailnet = FakeTailnet::new(joined(Some(NAME), true), Ok(mint(NAME)));
    assert_eq!(
        Refreshed::Fresh,
        refresh(&listening(), tailnet, dir.path()).await
    );

    let restarted = listening();
    let offline = FakeTailnet::new(joined(Some(NAME), true), Err("issuer unreachable".into()));
    refresh(&restarted, offline, dir.path()).await;
    assert_eq!(restarted.current().unwrap().name, NAME);
    assert!(restarted.serving(TAILNET_IP).is_some());
    let state = restarted.state();
    assert_eq!(state.source, TlsSource::Tailnet);
    assert_eq!(state.error.as_deref(), Some("issuer unreachable"));
}

#[tokio::test]
async fn a_kept_copy_for_another_name_is_never_served() {
    let dir = tempfile::tempdir().unwrap();
    let tailnet = FakeTailnet::new(joined(Some(NAME), true), Ok(mint(NAME)));
    assert_eq!(
        Refreshed::Fresh,
        refresh(&listening(), tailnet, dir.path()).await
    );

    let moved = listening();
    let other = FakeTailnet::new(joined(Some(OTHER_NAME), true), Err("rate limited".into()));
    assert_eq!(
        Refreshed::RequestFailed,
        refresh(&moved, other, dir.path()).await
    );
    assert!(moved.current().is_none());
    assert_eq!(moved.state().source, TlsSource::None);
}

#[tokio::test]
async fn a_name_tailscaled_no_longer_reports_withdraws_the_certificate() {
    let dir = tempfile::tempdir().unwrap();
    let store = listening();
    let good = FakeTailnet::new(joined(Some(NAME), true), Ok(mint(NAME)));
    assert_eq!(Refreshed::Fresh, refresh(&store, good, dir.path()).await);

    let certificates_off = FakeTailnet::new(joined(Some(NAME), false), Ok(mint(NAME)));
    assert_eq!(
        Refreshed::NoName,
        refresh(&store, certificates_off, dir.path()).await
    );
    assert!(store.current().is_none());
    assert!(store.serving(TAILNET_IP).is_none());
    let state = store.state();
    assert_eq!(state.source, TlsSource::None);
    assert_eq!(state.https_origin(5252), None);
}

#[tokio::test]
async fn a_renamed_node_never_keeps_the_old_certificate() {
    let dir = tempfile::tempdir().unwrap();
    let store = listening();
    let good = FakeTailnet::new(joined(Some(NAME), true), Ok(mint(NAME)));
    assert_eq!(Refreshed::Fresh, refresh(&store, good, dir.path()).await);

    let renamed = FakeTailnet::new(joined(Some(OTHER_NAME), true), Err("rate limited".into()));
    assert_eq!(
        Refreshed::RequestFailed,
        refresh(&store, renamed, dir.path()).await
    );
    assert!(store.current().is_none());
    assert_eq!(store.state().source, TlsSource::None);
}

#[tokio::test]
async fn a_lan_only_listener_gets_no_certificate_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let store = listening_on(vec![LAN_IP]);
    let tailnet = FakeTailnet::new(joined(Some(NAME), true), Ok(mint(NAME)));
    assert_eq!(
        Refreshed::NoName,
        refresh(&store, tailnet.clone(), dir.path()).await
    );
    assert!(tailnet.asked().is_empty(), "nothing reaches the issuer");
    assert!(store.current().is_none());
    let state = store.state();
    assert_eq!(state.source, TlsSource::None);
    assert_eq!(state.https_origin(5252), None);
    let error = state.error.unwrap();
    assert!(error.contains("192.0.2.10"), "{error}");
    assert!(error.contains("100.70.0.1"), "{error}");
}

#[tokio::test]
async fn a_listener_that_stops_covering_the_tailnet_withdraws_the_certificate() {
    let dir = tempfile::tempdir().unwrap();
    let store = listening();
    let good = FakeTailnet::new(joined(Some(NAME), true), Ok(mint(NAME)));
    assert_eq!(Refreshed::Fresh, refresh(&store, good, dir.path()).await);

    store.set_listeners(vec![LAN_IP]);
    let again = FakeTailnet::new(joined(Some(NAME), true), Ok(mint(NAME)));
    assert_eq!(Refreshed::NoName, refresh(&store, again, dir.path()).await);
    assert!(store.serving(TAILNET_IP).is_none());
    assert_eq!(store.state().source, TlsSource::None);
}

/// A tailnet address the OS will not bind, as under userspace networking,
/// never counts as a listener. Only a bound one does.
#[tokio::test]
async fn an_optional_tailnet_address_counts_only_while_bound() {
    let dir = tempfile::tempdir().unwrap();
    let store = listening_on(Vec::new());
    let tailnet = FakeTailnet::new(joined(Some(NAME), true), Ok(mint(NAME)));
    assert_eq!(
        Refreshed::NoName,
        refresh(&store, tailnet.clone(), dir.path()).await
    );
    let error = store.state().error.unwrap();
    assert!(error.contains("has not bound a tailnet address"), "{error}");
    assert_eq!(store.state().https_origin(5252), None);

    store.add_listener(TAILNET_IP);
    tokio::time::timeout(Duration::from_secs(1), store.listeners_changed())
        .await
        .expect("a bind wakes the refresh");
    assert_eq!(
        Refreshed::Fresh,
        refresh(&store, tailnet.clone(), dir.path()).await
    );
    assert!(store.serving(TAILNET_IP).is_some());

    store.remove_listener(TAILNET_IP);
    tokio::time::timeout(Duration::from_secs(1), store.listeners_changed())
        .await
        .expect("a lost listener wakes the refresh");
    assert_eq!(
        Refreshed::NoName,
        refresh(&store, tailnet, dir.path()).await
    );
    assert!(store.serving(TAILNET_IP).is_none());
}

#[tokio::test]
async fn a_wildcard_or_tailnet_address_listener_gets_a_certificate() {
    let mapped: IpAddr = "::ffff:100.70.0.1".parse().unwrap();
    for listeners in [
        vec![IpAddr::V4(Ipv4Addr::UNSPECIFIED)],
        vec![IpAddr::V6(std::net::Ipv6Addr::UNSPECIFIED)],
        vec![LAN_IP, TAILNET_IP],
        vec![mapped],
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = listening_on(listeners.clone());
        let tailnet = FakeTailnet::new(joined(Some(NAME), true), Ok(mint(NAME)));
        assert_eq!(
            Refreshed::Fresh,
            refresh(&store, tailnet, dir.path()).await,
            "{listeners:?}"
        );
        assert!(store.serving(TAILNET_IP).is_some(), "{listeners:?}");
    }
}

#[test]
fn only_a_failed_request_waits_the_hour() {
    let now = Utc::now();
    assert_eq!(Refreshed::Fresh.next_wait(now), REFRESH_INTERVAL);
    assert_eq!(Refreshed::RequestFailed.next_wait(now), RETRY_INTERVAL);
    assert!(Refreshed::NoName.next_wait(now) < RETRY_INTERVAL);
}

#[test]
fn an_operator_certificate_is_judged_again_once_it_expires() {
    let now = Utc::now();
    let serves_until = |at| Refreshed::OperatorServes { expires_at: at };
    let soon = Some(now + chrono::Duration::hours(3));
    assert_eq!(
        serves_until(soon).next_wait(now),
        Duration::from_secs(3 * 60 * 60 + 1)
    );
    let later = Some(now + chrono::Duration::days(30));
    assert_eq!(serves_until(later).next_wait(now), REFRESH_INTERVAL);
    assert_eq!(serves_until(None).next_wait(now), REFRESH_INTERVAL);
    let past = Some(now - chrono::Duration::seconds(5));
    assert_eq!(serves_until(past).next_wait(now), Duration::from_secs(1));
}

#[tokio::test]
async fn an_expired_copy_is_not_served() {
    let dir = tempfile::tempdir().unwrap();
    let mut params = rcgen::CertificateParams::new(vec![NAME.into()]).unwrap();
    params.not_before = rcgen::date_time_ymd(2020, 1, 1);
    params.not_after = rcgen::date_time_ymd(2021, 1, 1);
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    let pair = CertPair {
        cert_pem: cert.pem().into_bytes(),
        key_pem: key.serialize_pem().into_bytes(),
    };
    keep_copy(dir.path(), NAME, &pair).unwrap();

    let store = listening();
    let offline = FakeTailnet::new(joined(Some(NAME), true), Err("issuer unreachable".into()));
    refresh(&store, offline, dir.path()).await;
    assert!(store.current().is_none());

    store
        .install(NAME, pair.cert_pem.clone(), pair.key_pem.clone())
        .await
        .unwrap();
    store.set_addresses(vec![TAILNET_IP]);
    store.record(TlsState::tailnet(NAME, &pair.cert_pem, false));
    assert!(
        store.serving(TAILNET_IP).is_none(),
        "an installed certificate stops at expiry"
    );
    let state = store.state();
    assert_eq!(
        state.source,
        TlsSource::None,
        "and no surface advertises it"
    );
    assert_eq!(state.https_origin(5252), None);
    assert!(state.error.unwrap().contains("expired"));
}

#[test]
fn the_state_serializes_with_a_kebab_case_source() {
    let json = serde_json::to_value(TlsState::none("why")).unwrap();
    assert_eq!(json["source"], "none");
    assert_eq!(json["error"], "why");
}

#[test]
fn a_certificate_serving_the_tailnet_name_publishes_its_origin() {
    let tailnet = TlsState::tailnet(NAME, &mint(NAME).cert, false);
    assert_eq!(
        tailnet.https_origin(5252).as_deref(),
        Some("https://node-7.some-base.example:5252")
    );
    assert_eq!(
        tailnet.https_origin(443).as_deref(),
        Some("https://node-7.some-base.example")
    );
    let operator = TlsState::operator(&OperatorCert::public(&mint(NAME).cert));
    assert_eq!(
        operator.https_origin(5252),
        None,
        "an operator certificate for no known name"
    );
    let named = TlsState {
        name: Some(NAME.into()),
        ..operator
    };
    assert_eq!(
        named.https_origin(443).as_deref(),
        Some("https://node-7.some-base.example")
    );
    assert_eq!(TlsState::none("why").https_origin(5252), None);
}

#[test]
fn the_report_carries_the_state_and_the_origin_side_by_side() {
    let json = serde_json::to_value(TlsState::tailnet(NAME, &mint(NAME).cert, false).report(5252))
        .unwrap();
    assert_eq!(json["source"], "tailnet");
    assert_eq!(json["name"], NAME);
    assert_eq!(
        json["https_origin"],
        "https://node-7.some-base.example:5252"
    );
}

#[tokio::test]
async fn a_trusted_operator_certificate_for_the_name_serves_it_with_no_fetch() {
    let dir = tempfile::tempdir().unwrap();
    let ca = test_ca();
    let store = with_operator(&ca.issue(&[NAME]), ca.roots());
    let tailnet = FakeTailnet::new(joined(Some(NAME), true), Ok(mint(NAME)));
    refresh(&store, tailnet.clone(), dir.path()).await;
    assert!(tailnet.asked().is_empty(), "nothing is fetched");
    assert!(store.current().is_none());
    let state = store.state();
    assert_eq!(state.source, TlsSource::Operator);
    assert_eq!(state.error, None);
    assert_eq!(
        state.https_origin(5252).as_deref(),
        Some("https://node-7.some-base.example:5252")
    );
}

/// An mkcert certificate made for an old name and address, as on a machine
/// that moved tailnets. A phone would refuse it for the current name.
#[tokio::test]
async fn an_operator_certificate_with_stale_names_gets_a_tailnet_one_beside_it() {
    let dir = tempfile::tempdir().unwrap();
    let ca = test_ca();
    let store = with_operator(&ca.issue(&[OTHER_NAME, "100.64.0.9"]), ca.roots());
    let tailnet = FakeTailnet::new(joined(Some(NAME), true), Ok(mint(NAME)));
    assert_eq!(
        Refreshed::Fresh,
        refresh(&store, tailnet.clone(), dir.path()).await
    );
    assert_eq!(tailnet.asked(), [NAME]);
    assert!(store.serving(TAILNET_IP).is_some());
    let state = store.state();
    assert_eq!(state.source, TlsSource::Tailnet);
    assert!(state.operator_serves_other_names);
    assert_eq!(
        state.https_origin(5252).as_deref(),
        Some("https://node-7.some-base.example:5252")
    );
}

#[tokio::test]
async fn an_operator_certificate_from_an_untrusted_root_loses_the_name() {
    let dir = tempfile::tempdir().unwrap();
    let (trusted, stranger) = (test_ca(), test_ca());
    let store = with_operator(&stranger.issue(&[NAME]), trusted.roots());
    let tailnet = FakeTailnet::new(joined(Some(NAME), true), Ok(mint(NAME)));
    refresh(&store, tailnet.clone(), dir.path()).await;
    assert_eq!(tailnet.asked(), [NAME]);
    assert_eq!(store.state().source, TlsSource::Tailnet);
}

#[test]
fn a_self_signed_operator_certificate_is_not_publicly_trusted() {
    assert!(!OperatorCert::public(&mint(NAME).cert).serves(NAME));
}

#[tokio::test]
async fn with_no_tailnet_name_the_operator_certificate_still_serves_and_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let ca = test_ca();
    let store = with_operator(&ca.issue(&[OTHER_NAME]), ca.roots());
    let tailnet = FakeTailnet::new(joined(None, false), Ok(mint(NAME)));
    refresh(&store, tailnet, dir.path()).await;
    let state = store.state();
    assert_eq!(state.source, TlsSource::Operator);
    assert_eq!(state.name, None);
    assert!(state.error.unwrap().contains("MagicDNS is off"));
}
