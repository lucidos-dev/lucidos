//! Whether the operator's certificate (`LUCIDOS_TLS_CERT`) serves a name.
//!
//! It serves a name only where a phone would accept it: it covers the name,
//! it is in date, and it chains to a trusted root. rustls' own verifier
//! decides, against Mozilla's root set (`webpki-roots`). A certificate from a
//! private CA such as mkcert fails, so the tailnet name gets tailscaled's
//! certificate instead.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use rustls::client::danger::ServerCertVerifier;
use rustls::client::WebPkiServerVerifier;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::RootCertStore;

use super::expiry;

pub struct OperatorCert {
    chain: Vec<CertificateDer<'static>>,
    roots: Arc<RootCertStore>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl OperatorCert {
    /// The chain in `chain_pem`, judged against `roots`. A chain that will not
    /// parse serves no name.
    pub fn new(chain_pem: &[u8], roots: Arc<RootCertStore>) -> OperatorCert {
        OperatorCert {
            chain: CertificateDer::pem_slice_iter(chain_pem)
                .filter_map(Result::ok)
                .collect(),
            roots,
            expires_at: expiry::not_after(chain_pem),
        }
    }

    /// The chain in `chain_pem`, judged against the public roots a phone trusts.
    pub fn public(chain_pem: &[u8]) -> OperatorCert {
        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        OperatorCert::new(chain_pem, Arc::new(roots))
    }

    /// Whether a client trusting the roots would accept this chain for `name`.
    pub fn serves(&self, name: &str) -> bool {
        let Some((leaf, intermediates)) = self.chain.split_first() else {
            return false;
        };
        let Ok(name) = ServerName::try_from(name.to_string()) else {
            return false;
        };
        let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
        let Ok(verifier) =
            WebPkiServerVerifier::builder_with_provider(self.roots.clone(), provider).build()
        else {
            return false;
        };
        verifier
            .verify_server_cert(leaf, intermediates, &name, &[], UnixTime::now())
            .is_ok()
    }
}
