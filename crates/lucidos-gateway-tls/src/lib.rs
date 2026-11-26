mod acceptor;
mod expiry;
mod handoff;
pub mod managed;
mod operator;

pub use acceptor::GatewayAcceptor;
pub use managed::TlsState;
pub use operator::OperatorCert;

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, RwLock};

use axum::extract::{FromRequestParts, Request};
use axum::http::request::Parts;
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum_server::tls_rustls::RustlsConfig;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Peer {
    pub remote: SocketAddr,
    pub local: SocketAddr,
    pub public_listener: bool,
}

impl Peer {
    pub fn is_same_machine(&self) -> bool {
        let remote = self.remote.ip().to_canonical();
        remote.is_loopback() || remote == self.local.ip().to_canonical()
    }
}

pub type LoopbackTls = Arc<rustls::ServerConfig>;

#[derive(Clone)]
pub enum ListenerTls {
    Plain,
    Operator(RustlsConfig, Arc<CertStore>, LoopbackTls),
    Managed(Arc<CertStore>, LoopbackTls),
}

impl ListenerTls {
    pub fn scheme_label(&self, addr: SocketAddr) -> &'static str {
        match self {
            ListenerTls::Plain => "http",
            ListenerTls::Operator(..) | ListenerTls::Managed(..) if is_loopback(addr) => {
                "https (loopback CA)"
            }
            ListenerTls::Operator(..) => "https (TLS)",
            ListenerTls::Managed(..) => "http, https once a certificate is held",
        }
    }
}

pub fn is_loopback(addr: SocketAddr) -> bool {
    addr.ip().to_canonical().is_loopback()
}

pub struct ManagedCert {
    pub name: String,
    expires_at: Option<DateTime<Utc>>,
    config: Arc<rustls::ServerConfig>,
}

impl ManagedCert {
    fn expired(&self) -> bool {
        self.expires_at.is_some_and(|at| at <= Utc::now())
    }
}

#[derive(Default)]
pub struct CertStore {
    current: RwLock<Option<Arc<ManagedCert>>>,
    addresses: RwLock<Vec<IpAddr>>,
    tailnet_addresses: RwLock<Vec<IpAddr>>,
    listeners: RwLock<Vec<IpAddr>>,
    listeners_changed: tokio::sync::Notify,
    operator: RwLock<Option<Arc<OperatorCert>>>,
    state: RwLock<TlsState>,
}

impl CertStore {
    pub fn current(&self) -> Option<Arc<ManagedCert>> {
        let current = self.current.read().ok().and_then(|c| c.clone())?;
        (!current.expired()).then_some(current)
    }

    pub fn serving(&self, local: IpAddr) -> Option<Arc<ManagedCert>> {
        let local = local.to_canonical();
        let tailnet = self
            .addresses
            .read()
            .is_ok_and(|addresses| addresses.contains(&local));
        self.current().filter(|_| tailnet)
    }

    pub fn set_addresses(&self, addresses: Vec<IpAddr>) {
        if let Ok(mut current) = self.addresses.write() {
            *current = addresses;
        }
    }

    pub fn set_tailnet_addresses(&self, addresses: Vec<IpAddr>) {
        if let Ok(mut current) = self.tailnet_addresses.write() {
            *current = addresses;
        }
    }

    pub fn is_tailnet_address(&self, local: IpAddr) -> bool {
        let local = local.to_canonical();
        self.tailnet_addresses
            .read()
            .is_ok_and(|addresses| addresses.iter().any(|a| a.to_canonical() == local))
    }

    pub fn set_listeners(&self, listeners: Vec<IpAddr>) {
        if let Ok(mut current) = self.listeners.write() {
            *current = listeners;
        }
    }

    pub fn add_listener(&self, listener: IpAddr) {
        if let Ok(mut current) = self.listeners.write() {
            if !current.contains(&listener) {
                current.push(listener);
            }
        }
        self.listeners_changed.notify_one();
    }

    pub fn remove_listener(&self, listener: IpAddr) {
        if let Ok(mut current) = self.listeners.write() {
            current.retain(|l| *l != listener);
        }
        self.listeners_changed.notify_one();
    }

    pub async fn listeners_changed(&self) {
        self.listeners_changed.notified().await;
    }

    pub fn listeners(&self) -> Vec<IpAddr> {
        self.listeners.read().map(|l| l.clone()).unwrap_or_default()
    }

    pub fn listens_on_any(&self, addresses: &[IpAddr]) -> bool {
        self.listeners().iter().any(|listener| {
            let listener = listener.to_canonical();
            listener.is_unspecified() || addresses.iter().any(|a| a.to_canonical() == listener)
        })
    }

    pub fn set_operator(&self, operator: OperatorCert) {
        if let Ok(mut current) = self.operator.write() {
            *current = Some(Arc::new(operator));
        }
    }

    pub fn operator(&self) -> Option<Arc<OperatorCert>> {
        self.operator.read().ok().and_then(|o| o.clone())
    }

    pub fn without_tailnet(&self, reason: String) -> TlsState {
        match self.operator() {
            Some(operator) => TlsState {
                error: Some(reason),
                ..TlsState::operator(&operator)
            },
            None => TlsState::none(reason),
        }
    }

    pub fn withdraw(&self) {
        if let Ok(mut current) = self.current.write() {
            *current = None;
        }
        self.set_addresses(Vec::new());
    }

    pub fn state(&self) -> TlsState {
        let state = self
            .state
            .read()
            .map(|s| s.clone())
            .unwrap_or_else(|p| p.into_inner().clone());
        if state.source != managed::TlsSource::Tailnet || self.current().is_some() {
            return state;
        }
        let name = state.name.unwrap_or_default();
        let why = state.error.map(|e| format!(" {e}")).unwrap_or_default();
        self.without_tailnet(format!(
            "The certificate for {name} expired, and no renewal has succeeded yet.{why}"
        ))
    }

    pub fn record(&self, state: TlsState) {
        match self.state.write() {
            Ok(mut current) => *current = state,
            Err(poisoned) => *poisoned.into_inner() = state,
        }
    }

    pub async fn install(
        &self,
        name: &str,
        cert_pem: Vec<u8>,
        key_pem: Vec<u8>,
    ) -> std::io::Result<()> {
        let expires_at = expiry::not_after(&cert_pem);
        let config = RustlsConfig::from_pem(cert_pem, key_pem).await?.get_inner();
        let cert = Arc::new(ManagedCert {
            name: name.to_string(),
            expires_at,
            config,
        });
        if let Ok(mut current) = self.current.write() {
            *current = Some(cert);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Connection {
    #[default]
    Plain,
    Tls,
    PlainWithHttps {
        authority: String,
    },
    PlainLoopback,
}

impl Connection {
    pub fn of(extensions: &axum::http::Extensions) -> Connection {
        extensions.get::<Connection>().cloned().unwrap_or_default()
    }

    pub fn is_tls(&self) -> bool {
        matches!(self, Connection::Tls)
    }
}

#[axum::async_trait]
impl<S: Send + Sync> FromRequestParts<S> for Connection {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Connection::of(&parts.extensions))
    }
}

pub async fn plaintext_gate(mut req: Request, next: Next) -> Response {
    match Connection::of(req.extensions()) {
        Connection::PlainWithHttps { authority } => {
            let path = req
                .uri()
                .path_and_query()
                .map(|p| p.as_str())
                .unwrap_or("/");
            match HeaderValue::try_from(format!("https://{authority}{path}")) {
                Ok(location) => (
                    StatusCode::PERMANENT_REDIRECT,
                    [(header::LOCATION, location)],
                )
                    .into_response(),
                Err(_) => next.run(req).await,
            }
        }
        Connection::PlainLoopback if req.headers().contains_key("x-forwarded-for") => {
            req.extensions_mut().insert(Connection::Plain);
            next.run(req).await
        }
        Connection::PlainLoopback => handoff::respond(&req),
        Connection::Plain | Connection::Tls => next.run(req).await,
    }
}

#[cfg(test)]
mod tests;
