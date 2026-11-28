use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, OnceLock};

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair, SanType,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

struct TestCa {
    ca_pem: String,
    chain: Vec<CertificateDer<'static>>,
    leaf_key_der: Vec<u8>,
}

fn test_ca() -> &'static TestCa {
    static CA: OnceLock<TestCa> = OnceLock::new();
    CA.get_or_init(|| {
        let ca_key = KeyPair::generate().expect("generate the test CA key");
        let mut ca_params = CertificateParams::default();
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "Lucidos test loopback CA");
        ca_params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
        let ca = ca_params.self_signed(&ca_key).expect("sign the test CA");

        let leaf_key = KeyPair::generate().expect("generate the test leaf key");
        let mut leaf_params = CertificateParams::default();
        leaf_params.subject_alt_names = vec![
            SanType::DnsName("localhost".try_into().expect("a DNS name")),
            SanType::IpAddress(Ipv4Addr::LOCALHOST.into()),
            SanType::IpAddress(Ipv6Addr::LOCALHOST.into()),
        ];
        leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let leaf = leaf_params
            .signed_by(&leaf_key, &ca, &ca_key)
            .expect("sign the test leaf");
        TestCa {
            ca_pem: ca.pem(),
            chain: vec![leaf.der().clone()],
            leaf_key_der: leaf_key.serialize_der(),
        }
    })
}

pub fn ca_pem() -> &'static str {
    &test_ca().ca_pem
}

pub fn server_config() -> Arc<rustls::ServerConfig> {
    static CONFIG: OnceLock<Arc<rustls::ServerConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let ca = test_ca();
            let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(ca.leaf_key_der.clone()));
            let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
            let mut config = rustls::ServerConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .expect("the default TLS versions")
                .with_no_client_auth()
                .with_single_cert(ca.chain.clone(), key)
                .expect("the test leaf and its key");
            config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
            Arc::new(config)
        })
        .clone()
}

pub struct StandIn {
    pub addr: SocketAddr,
    handle: axum_server::Handle,
}

impl StandIn {
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    pub fn abort(&self) {
        self.handle.shutdown();
    }
}

pub fn serve_https_on(listener: std::net::TcpListener, router: axum::Router) -> StandIn {
    listener
        .set_nonblocking(true)
        .expect("a non-blocking listener");
    let addr = listener.local_addr().expect("a bound listener");
    let config = axum_server::tls_rustls::RustlsConfig::from_config(server_config());
    let handle = axum_server::Handle::new();
    let serving = axum_server::from_tcp_rustls(listener, config).handle(handle.clone());
    tokio::spawn(async move {
        let _ = serving.serve(router.into_make_service()).await;
    });
    StandIn { addr, handle }
}

pub fn serve_https(router: axum::Router) -> StandIn {
    let listener =
        std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind a loopback port");
    serve_https_on(listener, router)
}
