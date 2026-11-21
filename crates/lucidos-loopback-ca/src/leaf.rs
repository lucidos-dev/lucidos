//! Leaves the loopback CA signs: the gateway's, minted in memory, and the
//! frontend preview's, written to disk for Vite.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
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

/// How long a leaf is valid: well under Apple's 825-day cap. The resolver
/// re-mints at half of it.
pub const LEAF_LIFETIME: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// How far `notBefore` lies in the past, so a client whose clock runs a little
/// slow still accepts a fresh certificate.
pub(crate) const BACKDATE: Duration = Duration::from_secs(60 * 60);

/// The preview leaf's file names inside the directory it is written to.
const PREVIEW_CERT_FILE: &str = "preview-cert.pem";
const PREVIEW_KEY_FILE: &str = "preview-key.pem";

/// A leaf for `localhost`, 127.0.0.1 and ::1.
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

/// Where a written preview leaf lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PreviewLeaf {
    pub cert: PathBuf,
    pub key: PathBuf,
}

impl LoopbackCa {
    /// A fresh leaf for every loopback name, valid from `now`.
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

    /// Write a fresh leaf into `dir` for the frontend preview, and say where.
    ///
    /// The one leaf key on disk: Vite reads its certificate from files. The
    /// key is 0600 from creation, inside a storage root.
    pub fn write_preview_leaf(&self, dir: &Path, now: SystemTime) -> io::Result<PreviewLeaf> {
        lucidos_home::ensure_storage_root(dir)?;
        let leaf = self.mint_leaf(now)?;
        let written = PreviewLeaf {
            cert: dir.join(PREVIEW_CERT_FILE),
            key: dir.join(PREVIEW_KEY_FILE),
        };
        for (path, contents) in [
            (&written.key, leaf.key_pem()),
            (&written.cert, leaf.cert_pem),
        ] {
            let staged = path.with_extension(format!("{}.tmp", std::process::id()));
            lucidos_local_token::write_owner_only(&staged, &contents)?;
            std::fs::rename(&staged, path)?;
        }
        Ok(written)
    }
}

type Clock = Box<dyn Fn() -> SystemTime + Send + Sync>;
type Report = Box<dyn Fn(&io::Error) + Send + Sync>;

/// The gateway's loopback certificate, re-minted on the first handshake past
/// half its life. No timer: a gateway that serves nobody mints nothing.
///
/// A failed re-mint keeps the current leaf until it expires, and reports the
/// error. Past `notAfter` it serves nothing, so the handshake fails rather
/// than presenting an expired certificate.
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
    /// Mint the first leaf now. `report` hears every later re-mint failure.
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

    /// The certificate a handshake gets now, renewing it when due.
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
    /// A server config presenting this resolver's leaf, offering h2 and
    /// http/1.1 as every TLS listener of the gateway does.
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
