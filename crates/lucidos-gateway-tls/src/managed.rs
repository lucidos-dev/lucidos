use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use lucidos_tailscale::{CertPair, TailnetNode, TailnetState};
use serde::Serialize;

use super::{expiry, CertStore, OperatorCert};

pub const REFRESH_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
pub const RETRY_INTERVAL: Duration = Duration::from_secs(60 * 60);
pub const NAME_RETRY_INTERVAL: Duration = Duration::from_secs(5 * 60);

const NAME_FILE: &str = "name";
const CERT_FILE: &str = "cert.pem";
const KEY_FILE: &str = "key.pem";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TlsSource {
    Operator,
    Tailnet,
    None,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TlsState {
    pub source: TlsSource,
    pub name: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
    pub operator_serves_other_names: bool,
}

impl TlsState {
    pub fn none(reason: impl Into<String>) -> TlsState {
        TlsState {
            source: TlsSource::None,
            name: None,
            expires_at: None,
            error: Some(reason.into()),
            operator_serves_other_names: false,
        }
    }

    pub fn operator(cert: &OperatorCert) -> TlsState {
        TlsState {
            source: TlsSource::Operator,
            name: None,
            expires_at: cert.expires_at,
            error: None,
            operator_serves_other_names: false,
        }
    }

    fn tailnet(name: &str, cert_pem: &[u8], operator_serves_other_names: bool) -> TlsState {
        TlsState {
            source: TlsSource::Tailnet,
            name: Some(name.to_string()),
            expires_at: expiry::not_after(cert_pem),
            error: None,
            operator_serves_other_names,
        }
    }

    pub fn https_origin(&self, port: u16) -> Option<String> {
        let name = self
            .name
            .as_deref()
            .filter(|_| matches!(self.source, TlsSource::Operator | TlsSource::Tailnet))?;
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

pub trait Tailnet: Send + Sync + 'static {
    fn state(&self) -> TailnetState;
    fn cert_pair(&self, name: &str) -> Result<CertPair, String>;
}

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refreshed {
    Fresh,
    OperatorServes { expires_at: Option<DateTime<Utc>> },
    NoName,
    RequestFailed,
}

impl Refreshed {
    fn next_wait(self, now: DateTime<Utc>) -> Duration {
        match self {
            Refreshed::Fresh => REFRESH_INTERVAL,
            Refreshed::OperatorServes { expires_at } => expires_at.map_or(REFRESH_INTERVAL, |at| {
                let left = (at - now).to_std().unwrap_or(Duration::ZERO);
                REFRESH_INTERVAL.min(left + Duration::from_secs(1))
            }),
            Refreshed::NoName => NAME_RETRY_INTERVAL,
            Refreshed::RequestFailed => RETRY_INTERVAL,
        }
    }
}

pub type Log = fn(String);

pub async fn refresh(
    store: &CertStore,
    tailnet: Arc<dyn Tailnet>,
    dir: &Path,
    log: Log,
) -> Refreshed {
    let asked = tailnet.clone();
    let node = match blocking(move || {
        let state = asked.state();
        let addresses = state
            .node()
            .map(|n| n.addresses.clone())
            .unwrap_or_default();
        Ok((addresses, certifiable(&state)))
    })
    .await
    .and_then(|(addresses, node)| {
        store.set_tailnet_addresses(addresses);
        node
    }) {
        Ok(node) => node,
        Err(reason) => {
            store.withdraw();
            fail(store, reason, log);
            return Refreshed::NoName;
        }
    };
    if !store.listens_on_any(&node.addresses) {
        store.withdraw();
        fail(store, unreachable_reason(&store.listeners(), &node), log);
        return Refreshed::NoName;
    }
    if let Some(operator) = store.operator().filter(|o| o.serves(&node.name)) {
        store.withdraw();
        store.record(TlsState {
            name: Some(node.name),
            ..TlsState::operator(&operator)
        });
        return Refreshed::OperatorServes {
            expires_at: operator.expires_at,
        };
    }
    if store.current().is_some_and(|cert| cert.name != node.name) {
        store.withdraw();
    }
    store.set_addresses(node.addresses);
    if store.current().is_none() {
        serve_copy(store, dir, &node.name, log).await;
    }
    let name = node.name.clone();
    match blocking(move || tailnet.cert_pair(&name)).await {
        Ok(pair) => {
            if install(store, dir, &node.name, pair, log).await {
                Refreshed::Fresh
            } else {
                Refreshed::RequestFailed
            }
        }
        Err(reason) => {
            fail(store, reason, log);
            Refreshed::RequestFailed
        }
    }
}

pub fn spawn_refresh(store: Arc<CertStore>, tailnet: Arc<dyn Tailnet>, dir: PathBuf, log: Log) {
    tokio::spawn(async move {
        loop {
            let refreshed = refresh(&store, tailnet.clone(), &dir, log).await;
            tokio::select! {
                () = tokio::time::sleep(refreshed.next_wait(Utc::now())) => {}
                () = store.listeners_changed() => {}
            }
        }
    });
}

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

fn unreachable_reason(listeners: &[IpAddr], node: &Certifiable) -> String {
    let list = |addresses: &[IpAddr]| {
        addresses
            .iter()
            .map(IpAddr::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let listening = match listeners {
        [] => "The gateway has not bound a tailnet address of this machine yet".to_string(),
        _ => format!(
            "The gateway listens on {}, and none of them is a tailnet address of this machine",
            list(listeners)
        ),
    };
    format!(
        "{listening} ({}). A certificate for {} would serve nothing, so the gateway asks for none.",
        list(&node.addresses),
        node.name
    )
}

async fn serve_copy(store: &CertStore, dir: &Path, name: &str, log: Log) {
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
    let state = TlsState::tailnet(name, &cert, store.operator().is_some());
    match store.install(name, cert, key).await {
        Ok(()) => store.record(state),
        Err(e) => log(format!(
            "[Gateway] the kept certificate in {} did not load: {e}",
            dir.display()
        )),
    }
}

async fn install(store: &CertStore, dir: &Path, name: &str, pair: CertPair, log: Log) -> bool {
    let mut state = TlsState::tailnet(name, &pair.cert_pem, store.operator().is_some());
    if let Err(e) = store
        .install(name, pair.cert_pem.clone(), pair.key_pem.clone())
        .await
    {
        fail(
            store,
            format!("tailscaled's certificate for {name} did not load: {e}"),
            log,
        );
        return false;
    }
    if let Err(e) = keep_copy(dir, name, &pair) {
        let error = format!(
            "The certificate serves, but its copy in {} failed: {e}",
            dir.display()
        );
        log(format!("[Gateway] HTTPS certificate: {error}"));
        state.error = Some(error);
    }
    store.record(state);
    true
}

fn fail(store: &CertStore, reason: String, log: Log) {
    log(format!("[Gateway] HTTPS certificate: {reason}"));
    let state = if store.current().is_some() {
        TlsState {
            error: Some(reason),
            ..store.state()
        }
    } else {
        store.without_tailnet(reason)
    };
    store.record(state);
}

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
