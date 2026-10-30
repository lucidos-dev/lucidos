//! Where the managed certificate comes from, how it is kept, and its renewal.
//!
//! The source ladder, first that works wins:
//!
//! 1. An operator certificate (`LUCIDOS_TLS_CERT`/`KEY`). Nothing is fetched.
//! 2. tailscaled, for the name it reports in `CertDomains`
//!    ([`lucidos_tailscale::TailnetNode::https_name`]). Never a guessed name.
//! 3. None. The [`TlsState`] then says why, in one sentence for the user.
//!
//! A copy of the fetched pair lives in `<gateway data>/tls/`, owner-only, so a
//! restart serves at once. A failed renewal keeps the current certificate, but
//! only while tailscaled still reports its name.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use lucidos_tailscale::{CertPair, TailnetNode, TailnetState};
use serde::Serialize;

use super::{expiry, CertStore};

/// tailscaled renews well before expiry, so a daily fetch picks it up in time.
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// After a failed certificate request. Hourly stays inside the issuer's
/// failed-validation limit.
pub const RETRY_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// While tailscaled reports no certifiable name. Nothing reaches the issuer,
/// so the gateway can look again soon, as when tailscaled is still starting.
pub const NAME_RETRY_INTERVAL: Duration = Duration::from_secs(5 * 60);

const NAME_FILE: &str = "name";
const CERT_FILE: &str = "cert.pem";
const KEY_FILE: &str = "key.pem";

/// Where the certificate on non-loopback connections comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TlsSource {
    Operator,
    Tailnet,
    None,
}

/// What the gateway serves over TLS, and what went wrong last.
///
/// `error` beside a `Tailnet` source is a problem with the last update, a
/// failed renewal or a failed copy: the current certificate still serves.
/// Beside `None` it is why there is no HTTPS.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TlsState {
    pub source: TlsSource,
    pub name: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

impl TlsState {
    pub fn none(reason: impl Into<String>) -> TlsState {
        TlsState {
            source: TlsSource::None,
            name: None,
            expires_at: None,
            error: Some(reason.into()),
        }
    }

    pub fn operator(cert_pem: Option<&[u8]>) -> TlsState {
        TlsState {
            source: TlsSource::Operator,
            name: None,
            expires_at: cert_pem.and_then(expiry::not_after),
            error: None,
        }
    }

    fn tailnet(name: &str, cert_pem: &[u8]) -> TlsState {
        TlsState {
            source: TlsSource::Tailnet,
            name: Some(name.to_string()),
            expires_at: expiry::not_after(cert_pem),
            error: None,
        }
    }

    /// The origin the managed certificate serves on `port`. An operator
    /// certificate names no host the gateway knows, so it has none.
    pub fn https_origin(&self, port: u16) -> Option<String> {
        let name = self
            .name
            .as_deref()
            .filter(|_| self.source == TlsSource::Tailnet)?;
        Some(match port {
            443 => format!("https://{name}"),
            _ => format!("https://{name}:{port}"),
        })
    }

    pub fn report(self, port: u16) -> TlsReport {
        TlsReport {
            https_origin: self.https_origin(port),
            state: self,
        }
    }
}

/// What the control API publishes: the state, plus the origin it serves.
#[derive(Debug, Serialize)]
pub struct TlsReport {
    #[serde(flatten)]
    pub state: TlsState,
    pub https_origin: Option<String>,
}

impl Default for TlsState {
    fn default() -> Self {
        TlsState::none("The gateway has not asked for a certificate yet.")
    }
}

/// The authority for this node's name and certificate.
pub trait Tailnet: Send + Sync + 'static {
    /// Blocking.
    fn state(&self) -> TailnetState;
    /// Blocking. `name` is always one [`Tailnet::state`] reported.
    fn cert_pair(&self, name: &str) -> Result<CertPair, String>;
}

/// The real tailscaled, over its LocalAPI.
pub struct Tailscaled;

impl Tailnet for Tailscaled {
    fn state(&self) -> TailnetState {
        lucidos_tailscale::tailnet_state()
    }

    fn cert_pair(&self, name: &str) -> Result<CertPair, String> {
        lucidos_tailscale::cert_pair(name)
    }
}

pub fn tls_dir(gateway_data: &Path) -> PathBuf {
    gateway_data.join("tls")
}

/// How one [`refresh`] ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refreshed {
    /// A fresh certificate serves.
    Fresh,
    /// tailscaled reports no name it can certify. Nothing serves.
    NoName,
    /// The certificate request failed. A kept one may still serve.
    RequestFailed,
}

impl Refreshed {
    fn next_wait(self) -> Duration {
        match self {
            Refreshed::Fresh => REFRESH_INTERVAL,
            Refreshed::NoName => NAME_RETRY_INTERVAL,
            Refreshed::RequestFailed => RETRY_INTERVAL,
        }
    }
}

/// Ask tailscaled for this node's name, then for that name's certificate.
///
/// The name decides what may serve. A certificate for any other name is
/// withdrawn first, and the kept copy serves only when its name matches.
pub async fn refresh(store: &CertStore, tailnet: Arc<dyn Tailnet>, dir: &Path) -> Refreshed {
    let asked = tailnet.clone();
    let node = match blocking(move || certifiable(&asked.state())).await {
        Ok(node) => node,
        Err(reason) => {
            store.withdraw();
            fail(store, reason);
            return Refreshed::NoName;
        }
    };
    if store.current().is_some_and(|cert| cert.name != node.name) {
        store.withdraw();
    }
    store.set_addresses(node.addresses);
    if store.current().is_none() {
        serve_copy(store, dir, &node.name).await;
    }
    let name = node.name.clone();
    match blocking(move || tailnet.cert_pair(&name)).await {
        Ok(pair) => {
            if install(store, dir, &node.name, pair).await {
                Refreshed::Fresh
            } else {
                Refreshed::RequestFailed
            }
        }
        Err(reason) => {
            fail(store, reason);
            Refreshed::RequestFailed
        }
    }
}

/// Refresh now, then daily, or sooner while failing. Runs until the process ends.
pub fn spawn_refresh(store: Arc<CertStore>, tailnet: Arc<dyn Tailnet>, dir: PathBuf) {
    tokio::spawn(async move {
        loop {
            let refreshed = refresh(&store, tailnet.clone(), &dir).await;
            tokio::time::sleep(refreshed.next_wait()).await;
        }
    });
}

/// The name tailscaled can certify and the addresses it serves on.
struct Certifiable {
    name: String,
    addresses: Vec<IpAddr>,
}

async fn blocking<T: Send + 'static>(
    ask: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tokio::task::spawn_blocking(ask)
        .await
        .map_err(|e| format!("the certificate request stopped unexpectedly ({e})"))?
}

/// This node's certifiable name, or why there is none.
fn certifiable(state: &TailnetState) -> Result<Certifiable, String> {
    let Some(node) = state.node() else {
        return Err(state.reason().unwrap_or_default());
    };
    let name = certifiable_name(node)?;
    Ok(Certifiable {
        name,
        addresses: node.addresses.clone(),
    })
}

fn certifiable_name(node: &TailnetNode) -> Result<String, String> {
    if let Some(name) = node.https_name() {
        return Ok(name.to_string());
    }
    Err(match &node.magic_dns_name {
        None => "MagicDNS is off in this tailnet, so this machine has no name to certify.".into(),
        Some(name) => format!(
            "The tailnet issues no HTTPS certificate for {name}. On Tailscale, turn on HTTPS \
             certificates in the admin console. Headscale node certificates are on main but \
             unreleased as of 2026-10-30. Deploy a release with node certificates, then enable \
             them. Until then, Headscale stays on plain HTTP. Source and headless installs can set \
             LUCIDOS_TLS_CERT and LUCIDOS_TLS_KEY instead. The packaged app cannot take an \
             operator certificate."
        ),
    })
}

/// Serve the copy a previous run kept, if it is for `name` and unexpired.
async fn serve_copy(store: &CertStore, dir: &Path, name: &str) {
    let read = |file| std::fs::read(dir.join(file));
    let (Ok(kept), Ok(cert), Ok(key)) = (read(NAME_FILE), read(CERT_FILE), read(KEY_FILE)) else {
        return;
    };
    if String::from_utf8_lossy(&kept).trim() != name {
        return;
    }
    if expiry::not_after(&cert).is_some_and(|at| at <= Utc::now()) {
        return;
    }
    let state = TlsState::tailnet(name, &cert);
    match store.install(name, cert, key).await {
        Ok(()) => store.record(state),
        Err(e) => crate::log!(
            "[Gateway] the kept certificate in {} did not load: {e}",
            dir.display()
        ),
    }
}

async fn install(store: &CertStore, dir: &Path, name: &str, pair: CertPair) -> bool {
    let mut state = TlsState::tailnet(name, &pair.cert_pem);
    if let Err(e) = store
        .install(name, pair.cert_pem.clone(), pair.key_pem.clone())
        .await
    {
        fail(
            store,
            format!("tailscaled's certificate for {name} did not load: {e}"),
        );
        return false;
    }
    if let Err(e) = keep_copy(dir, name, &pair) {
        let error = format!(
            "The certificate serves, but its copy in {} failed: {e}",
            dir.display()
        );
        crate::log!("[Gateway] HTTPS certificate: {error}");
        state.error = Some(error);
    }
    store.record(state);
    true
}

/// Record `reason`. A certificate still serving keeps its state beside it.
fn fail(store: &CertStore, reason: String) {
    crate::log!("[Gateway] HTTPS certificate: {reason}");
    let state = if store.current().is_some() {
        TlsState {
            error: Some(reason),
            ..store.state()
        }
    } else {
        TlsState::none(reason)
    };
    store.record(state);
}

/// Each file replaced atomically and created owner-only, in an owner-only dir.
fn keep_copy(dir: &Path, name: &str, pair: &CertPair) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;
    for (file, bytes) in [
        (KEY_FILE, pair.key_pem.as_slice()),
        (CERT_FILE, pair.cert_pem.as_slice()),
        (NAME_FILE, name.as_bytes()),
    ] {
        let mut temp = tempfile::NamedTempFile::new_in(dir)?;
        std::io::Write::write_all(&mut temp, bytes)?;
        temp.as_file().sync_all()?;
        temp.persist(dir.join(file)).map_err(|e| e.error)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "managed_tests.rs"]
mod tests;
