use std::net::{IpAddr, Ipv4Addr};
use std::sync::Mutex;

use lucidos_tailscale::TailnetNode;

use super::*;

/// A name that matches no known tailnet suffix: nothing may depend on its shape.
const NAME: &str = "node-7.some-base.example";
/// The name a node reports after it moves to another tailnet.
const OTHER_NAME: &str = "node-7.other-base.example";
const TAILNET_IP: IpAddr = IpAddr::V4(Ipv4Addr::new(100, 70, 0, 1));

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
    let store = CertStore::default();
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
    let store = CertStore::default();
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
        let store = CertStore::default();
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
    assert_eq!(
        Refreshed::Fresh,
        refresh(&CertStore::default(), tailnet, &dir).await
    );
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
    let store = CertStore::default();
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
    let store = CertStore::default();
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
        refresh(&CertStore::default(), tailnet, dir.path()).await
    );

    let restarted = CertStore::default();
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
        refresh(&CertStore::default(), tailnet, dir.path()).await
    );

    let moved = CertStore::default();
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
    let store = CertStore::default();
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
    let store = CertStore::default();
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

#[test]
fn only_a_failed_request_waits_the_hour() {
    assert_eq!(Refreshed::Fresh.next_wait(), REFRESH_INTERVAL);
    assert_eq!(Refreshed::RequestFailed.next_wait(), RETRY_INTERVAL);
    assert!(Refreshed::NoName.next_wait() < RETRY_INTERVAL);
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

    let store = CertStore::default();
    let offline = FakeTailnet::new(joined(Some(NAME), true), Err("issuer unreachable".into()));
    refresh(&store, offline, dir.path()).await;
    assert!(store.current().is_none());

    store
        .install(NAME, pair.cert_pem.clone(), pair.key_pem.clone())
        .await
        .unwrap();
    store.set_addresses(vec![TAILNET_IP]);
    store.record(TlsState::tailnet(NAME, &pair.cert_pem));
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
fn only_a_tailnet_certificate_publishes_an_origin() {
    let tailnet = TlsState::tailnet(NAME, &mint(NAME).cert);
    assert_eq!(
        tailnet.https_origin(5252).as_deref(),
        Some("https://node-7.some-base.example:5252")
    );
    assert_eq!(
        tailnet.https_origin(443).as_deref(),
        Some("https://node-7.some-base.example")
    );
    assert_eq!(TlsState::operator(None).https_origin(5252), None);
    assert_eq!(TlsState::none("why").https_origin(5252), None);
}

#[test]
fn the_report_carries_the_state_and_the_origin_side_by_side() {
    let json =
        serde_json::to_value(TlsState::tailnet(NAME, &mint(NAME).cert).report(5252)).unwrap();
    assert_eq!(json["source"], "tailnet");
    assert_eq!(json["name"], NAME);
    assert_eq!(
        json["https_origin"],
        "https://node-7.some-base.example:5252"
    );
}
