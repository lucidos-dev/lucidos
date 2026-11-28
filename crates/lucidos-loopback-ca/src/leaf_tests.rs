use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::tests::{fresh_ca, verify};

const APPLE_MAX_LEAF: Duration = Duration::from_secs(825 * 24 * 60 * 60);

fn manual_clock(start: SystemTime) -> (Arc<AtomicU64>, Clock) {
    let offset = Arc::new(AtomicU64::new(0));
    let read = offset.clone();
    let clock = Box::new(move || start + Duration::from_secs(read.load(Ordering::SeqCst)));
    (offset, clock)
}

fn served_cert(resolver: &LeafResolver) -> Option<CertificateDer<'static>> {
    resolver.current().map(|key| key.cert[0].clone())
}

#[test]
fn a_leaf_stays_under_apples_cap() {
    assert!(LEAF_LIFETIME + BACKDATE < APPLE_MAX_LEAF);
}

#[test]
fn the_resolver_re_mints_on_a_handshake_past_half_life_and_not_before() {
    let (_scratch, _dir, ca) = fresh_ca();
    let start = SystemTime::now();
    let (offset, clock) = manual_clock(start);
    let resolver = LeafResolver::with_clock(ca, clock, Box::new(|_| {})).unwrap();

    let first = served_cert(&resolver).unwrap();
    offset.store(LEAF_LIFETIME.as_secs() / 2 - 1, Ordering::SeqCst);
    assert_eq!(served_cert(&resolver).unwrap(), first);

    offset.store(LEAF_LIFETIME.as_secs() / 2, Ordering::SeqCst);
    let renewed = served_cert(&resolver).unwrap();
    assert_ne!(renewed, first);
    assert_eq!(served_cert(&resolver).unwrap(), renewed);
}

#[test]
fn a_renewed_leaf_validates_for_its_whole_new_life() {
    let (_scratch, dir, ca) = fresh_ca();
    let trusted = crate::LoopbackCa::load(&dir).unwrap();
    let start = SystemTime::now();
    let (offset, clock) = manual_clock(start);
    let resolver = LeafResolver::with_clock(ca, clock, Box::new(|_| {})).unwrap();

    offset.store(LEAF_LIFETIME.as_secs(), Ordering::SeqCst);
    let renewed = served_cert(&resolver).unwrap();
    let late = start + LEAF_LIFETIME + LEAF_LIFETIME / 2;
    verify(&trusted, &renewed, "localhost", late).unwrap();
}

#[test]
fn nothing_is_served_past_not_after() {
    let (_scratch, _dir, ca) = fresh_ca();
    let start = SystemTime::now();
    let (offset, clock) = manual_clock(start);
    let resolver = LeafResolver::with_clock(ca, clock, Box::new(|_| {})).unwrap();
    resolver.current.lock().unwrap().renew_at = start + LEAF_LIFETIME * 10;

    offset.store(LEAF_LIFETIME.as_secs() - 1, Ordering::SeqCst);
    assert!(served_cert(&resolver).is_some());
    offset.store(LEAF_LIFETIME.as_secs(), Ordering::SeqCst);
    assert!(served_cert(&resolver).is_none());
}

async fn answer_once(listener: tokio::net::TcpListener, config: Arc<rustls::ServerConfig>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (stream, _) = listener.accept().await.unwrap();
    let mut tls = tokio_rustls::TlsAcceptor::from(config)
        .accept(stream)
        .await
        .unwrap();
    let mut request = [0u8; 1024];
    let _ = tls.read(&mut request).await.unwrap();
    tls.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok")
        .await
        .unwrap();
    tls.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_client_trusting_only_the_ca_completes_a_handshake_with_the_resolver() {
    let (_scratch, dir, ca) = fresh_ca();
    let config = LeafResolver::new(ca, |_| {})
        .unwrap()
        .into_server_config()
        .unwrap();
    assert_eq!(
        config.alpn_protocols,
        [b"h2".to_vec(), b"http/1.1".to_vec()]
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(answer_once(listener, config));

    let root = reqwest::Certificate::from_pem(dir.read_certificate().unwrap().as_bytes()).unwrap();
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .tls_built_in_root_certs(false)
        .add_root_certificate(root)
        .http1_only()
        .build()
        .unwrap();
    let body = client
        .get(format!("https://localhost:{port}/"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_eq!(body, "ok");
    server.await.unwrap();
}
