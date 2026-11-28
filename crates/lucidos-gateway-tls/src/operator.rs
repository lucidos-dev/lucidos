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
    pub fn new(chain_pem: &[u8], roots: Arc<RootCertStore>) -> OperatorCert {
        OperatorCert {
            chain: CertificateDer::pem_slice_iter(chain_pem)
                .filter_map(Result::ok)
                .collect(),
            roots,
            expires_at: expiry::not_after(chain_pem),
        }
    }

    pub fn public(chain_pem: &[u8]) -> OperatorCert {
        let roots = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        OperatorCert::new(chain_pem, Arc::new(roots))
    }

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
