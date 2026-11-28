use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use rcgen::{
    CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, KeyPair,
    KeyUsagePurpose, SanType,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;

use crate::LoopbackCa;

pub const LEAF_LIFETIME: Duration = Duration::from_secs(90 * 24 * 60 * 60);

pub(crate) const BACKDATE: Duration = Duration::from_secs(60 * 60);

pub struct Leaf {
    pub cert_der: CertificateDer<'static>,
    pub cert_pem: String,
    key: KeyPair,
    pub not_after: SystemTime,
}

impl Leaf {
    pub fn key_pem(&self) -> String {
        self.key.serialize_pem()
    }

    fn certified_key(&self) -> io::Result<CertifiedKey> {
        let der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(self.key.serialize_der()));
        let signer = rustls::crypto::aws_lc_rs::sign::any_supported_type(&der)
            .map_err(|e| io::Error::other(e.to_string()))?;
        Ok(CertifiedKey::new(vec![self.cert_der.clone()], signer))
    }
}

impl LoopbackCa {
    pub fn mint_leaf(&self, now: SystemTime) -> io::Result<Leaf> {
        let names = vec![
            SanType::DnsName("localhost".try_into().map_err(io::Error::other)?),
            SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)),
            SanType::IpAddress(IpAddr::V6(Ipv6Addr::LOCALHOST)),
        ];
        self.sign_leaf(names, now)
    }

    pub(crate) fn sign_leaf(&self, names: Vec<SanType>, now: SystemTime) -> io::Result<Leaf> {
        let key = KeyPair::generate().map_err(io::Error::other)?;
        let mut params = CertificateParams::default();
        let mut name = DistinguishedName::new();
        name.push(DnType::CommonName, "localhost");
        params.distinguished_name = name;
        params.subject_alt_names = names;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.use_authority_key_identifier_extension = true;
        let not_after = now + LEAF_LIFETIME;
        params.not_before = (now - BACKDATE).into();
        params.not_after = not_after.into();
        let cert = params
            .signed_by(&key, &self.issuer, &self.key)
            .map_err(io::Error::other)?;
        Ok(Leaf {
            cert_der: cert.der().clone(),
            cert_pem: cert.pem(),
            key,
            not_after,
        })
    }
}

type Clock = Box<dyn Fn() -> SystemTime + Send + Sync>;
type Report = Box<dyn Fn(&io::Error) + Send + Sync>;

pub struct LeafResolver {
    ca: LoopbackCa,
    clock: Clock,
    report: Report,
    current: Mutex<Served>,
}

struct Served {
    key: Arc<CertifiedKey>,
    renew_at: SystemTime,
    not_after: SystemTime,
}

impl LeafResolver {
    pub fn new(
        ca: LoopbackCa,
        report: impl Fn(&io::Error) + Send + Sync + 'static,
    ) -> io::Result<LeafResolver> {
        LeafResolver::with_clock(ca, Box::new(SystemTime::now), Box::new(report))
    }

    fn with_clock(ca: LoopbackCa, clock: Clock, report: Report) -> io::Result<LeafResolver> {
        let served = serve(&ca, clock())?;
        Ok(LeafResolver {
            ca,
            clock,
            report,
            current: Mutex::new(served),
        })
    }

    pub fn current(&self) -> Option<Arc<CertifiedKey>> {
        let now = (self.clock)();
        let mut current = self.current.lock().unwrap_or_else(|p| p.into_inner());
        if now >= current.renew_at {
            match serve(&self.ca, now) {
                Ok(served) => *current = served,
                Err(e) => (self.report)(&e),
            }
        }
        (now < current.not_after).then(|| current.key.clone())
    }
}

fn serve(ca: &LoopbackCa, now: SystemTime) -> io::Result<Served> {
    let leaf = ca.mint_leaf(now)?;
    Ok(Served {
        key: Arc::new(leaf.certified_key()?),
        renew_at: now + LEAF_LIFETIME / 2,
        not_after: leaf.not_after,
    })
}

impl std::fmt::Debug for LeafResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LeafResolver").finish_non_exhaustive()
    }
}

impl LeafResolver {
    pub fn into_server_config(self) -> io::Result<Arc<rustls::ServerConfig>> {
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let mut config = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(io::Error::other)?
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(self));
        config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
        Ok(Arc::new(config))
    }
}

impl ResolvesServerCert for LeafResolver {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        self.current()
    }
}

#[cfg(test)]
#[path = "leaf_tests.rs"]
mod tests;
