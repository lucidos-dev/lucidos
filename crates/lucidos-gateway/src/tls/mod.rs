//! What each gateway connection is served over: plain HTTP or TLS.
//!
//! Three listener kinds ([`ListenerTls`]):
//!
//! - **Operator**: `LUCIDOS_TLS_CERT`/`KEY` are set. Every connection is TLS,
//!   on every address. When the operator certificate would not satisfy a
//!   phone for the tailnet name, the [`CertStore`] also holds the managed
//!   one. A connection to a tailnet address then picks by SNI: the tailnet
//!   name gets the managed certificate, every other name the operator one.
//! - **Managed**: the gateway's own certificate from the [`CertStore`]. A
//!   loopback connection stays plain HTTP, because the packaged window, the
//!   raw-socket health checks and `tailscale serve` all speak `http://127.0.0.1`.
//!   A connection to one of the node's tailnet addresses is sniffed: a TLS
//!   hello gets TLS, and plain HTTP gets a `308` to the certificate's own
//!   origin. Any other address (the LAN, another VPN) stays plain HTTP.
//! - **Plain**: the hook socket. Never TLS.
//!
//! The acceptor tags every request with its [`Connection`]. A scheme decision
//! (the cookie's `Secure` flag, the redirect) reads that tag, never a
//! gateway-wide flag.

mod acceptor;
mod expiry;
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

/// How one listener decides each connection's scheme.
#[derive(Clone)]
pub enum ListenerTls {
    Plain,
    Operator(RustlsConfig, Arc<CertStore>),
    Managed(Arc<CertStore>),
}

impl ListenerTls {
    /// The scheme a startup log line names for this listener on `addr`.
    pub fn scheme_label(&self, addr: SocketAddr) -> &'static str {
        match self {
            ListenerTls::Operator(..) => "https (TLS)",
            ListenerTls::Managed(_) if !is_loopback(addr) => {
                "http, https once a certificate is held"
            }
            _ => "http",
        }
    }
}

/// A loopback address, IPv4-mapped IPv6 included. A dual-stack `[::]` listener
/// sees an IPv4 loopback caller as `::ffff:127.0.0.1`.
pub fn is_loopback(addr: SocketAddr) -> bool {
    addr.ip().to_canonical().is_loopback()
}

/// The certificate the gateway obtained for itself, ready to serve.
pub struct ManagedCert {
    /// The DNS name the certificate covers, and the host a plain-HTTP request
    /// is redirected to.
    pub name: String,
    expires_at: Option<DateTime<Utc>>,
    config: Arc<rustls::ServerConfig>,
}

impl ManagedCert {
    fn expired(&self) -> bool {
        self.expires_at.is_some_and(|at| at <= Utc::now())
    }
}

/// The managed certificate, swapped in place on renewal.
///
/// A connection reads the current one once, at accept. Installing a new one
/// therefore never touches a connection already open.
///
/// It serves only on the addresses tailscaled reports for this node, and
/// never once it has expired.
#[derive(Default)]
pub struct CertStore {
    current: RwLock<Option<Arc<ManagedCert>>>,
    addresses: RwLock<Vec<IpAddr>>,
    /// The non-loopback addresses the gateway listens on. An optional address
    /// counts only while it is bound.
    listeners: RwLock<Vec<IpAddr>>,
    listeners_changed: tokio::sync::Notify,
    operator: RwLock<Option<Arc<OperatorCert>>>,
    state: RwLock<TlsState>,
}

impl CertStore {
    /// The certificate, unless it has expired.
    pub fn current(&self) -> Option<Arc<ManagedCert>> {
        let current = self.current.read().ok().and_then(|c| c.clone())?;
        (!current.expired()).then_some(current)
    }

    /// The certificate a connection to `local` gets, if that is a tailnet
    /// address of this node.
    pub fn serving(&self, local: IpAddr) -> Option<Arc<ManagedCert>> {
        let local = local.to_canonical();
        let tailnet = self
            .addresses
            .read()
            .is_ok_and(|addresses| addresses.contains(&local));
        self.current().filter(|_| tailnet)
    }

    /// The node's tailnet addresses, as tailscaled reported them.
    pub fn set_addresses(&self, addresses: Vec<IpAddr>) {
        if let Ok(mut current) = self.addresses.write() {
            *current = addresses;
        }
    }

    pub fn set_listeners(&self, listeners: Vec<IpAddr>) {
        if let Ok(mut current) = self.listeners.write() {
            *current = listeners;
        }
    }

    /// An optional address bound. The refresh looks again at once.
    pub fn add_listener(&self, listener: IpAddr) {
        if let Ok(mut current) = self.listeners.write() {
            if !current.contains(&listener) {
                current.push(listener);
            }
        }
        self.listeners_changed.notify_one();
    }

    /// An optional address went away. The refresh looks again at once.
    pub fn remove_listener(&self, listener: IpAddr) {
        if let Ok(mut current) = self.listeners.write() {
            current.retain(|l| *l != listener);
        }
        self.listeners_changed.notify_one();
    }

    /// Resolves at the next listener change, or at once for one since the last call.
    pub async fn listeners_changed(&self) {
        self.listeners_changed.notified().await;
    }

    pub fn listeners(&self) -> Vec<IpAddr> {
        self.listeners.read().map(|l| l.clone()).unwrap_or_default()
    }

    /// Whether a listener accepts connections to one of `addresses`. A
    /// wildcard listener accepts them all.
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

    /// The state with no tailnet certificate serving, for `reason`. An
    /// operator certificate still serves every name it covers.
    pub fn without_tailnet(&self, reason: String) -> TlsState {
        match self.operator() {
            Some(operator) => TlsState {
                error: Some(reason),
                ..TlsState::operator(&operator)
            },
            None => TlsState::none(reason),
        }
    }

    /// Stop serving any certificate, on every address.
    pub fn withdraw(&self) {
        if let Ok(mut current) = self.current.write() {
            *current = None;
        }
        self.set_addresses(Vec::new());
    }

    /// What the gateway serves now. A tailnet certificate that expired since
    /// it was recorded reads as none, so no surface advertises its origin.
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

    /// Parse `cert_pem` and `key_pem` and serve them for `name` from the next
    /// connection on. A parse failure leaves the current certificate serving.
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

/// The scheme one request arrived over, as its listener decided at accept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Connection {
    /// Plain HTTP, with no HTTPS to offer instead. Also what a request built
    /// without an acceptor (a test's `oneshot`) reads.
    #[default]
    Plain,
    Tls,
    /// Plain HTTP on an address that serves HTTPS at `authority`
    /// (`<cert name>:<port>`).
    PlainWithHttps {
        authority: String,
    },
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

/// Send a plain-HTTP request to the HTTPS origin its address serves.
///
/// A `308`, so a POST stays a POST. This is what makes an old `http://`
/// bookmark or pairing QR land on the secure origin.
pub async fn redirect_to_https(req: Request, next: Next) -> Response {
    let Connection::PlainWithHttps { authority } = Connection::of(req.extensions()) else {
        return next.run(req).await;
    };
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

#[cfg(test)]
mod tests;
