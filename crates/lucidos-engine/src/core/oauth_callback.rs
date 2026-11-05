//! How an authorization's callback reaches the flow that started it.
//!
//! Two delivery paths end in one step, [`super::oauth::callback_received`]:
//!
//! - **Under a gateway**, the gateway owns the loopback callback port. It routes
//!   each callback by the prefix in `state` and forwards it to
//!   `GET /api/v1/internal/oauth/callback`, which calls [`deliver_callback`].
//! - **With no gateway** (`LUCIDOS_NO_GATEWAY`), the engine binds the port itself
//!   for each flow (`oauth::CallbackListener`, ADR 0068).
//!
//! Plan: `docs/plans/2026-11-05-gateway-owned-oauth-callback.md`.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use axum::http::HeaderMap;

use super::oauth::{callback_received, query_param};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub use lucidos_gateway_constants::OAUTH_STATE_PREFIX_SEPARATOR as STATE_PREFIX_SEPARATOR;
use lucidos_gateway_constants::{HEADER_LOCAL_BROWSER, OAUTH_WEB_CALLBACK_PATH};

/// Where the browser that will finish an authorization reached Lucidos.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserContext {
    /// The outside prefix its requests arrive under: `/dev/` through its own
    /// gateway, `/dev@cloud/` through a remote one. The catching gateway routes
    /// the callback by it.
    pub prefix: String,
    /// Whether it runs on the machine of the gateway it reached, which the
    /// gateway stamps. Only such a browser can follow a loopback redirect.
    pub local: bool,
    /// Its own origin (`https://host.example`), from `Origin`. A browser sends
    /// one on a POST, not on every request.
    pub origin: Option<String>,
}

impl BrowserContext {
    /// The context a request carries, or `None` when no gateway proxied it.
    pub fn from_headers(headers: &HeaderMap) -> Option<Self> {
        crate::api::base_path::arrived_through_gateway_proxy(headers).then(|| Self {
            prefix: crate::api::base_path::forwarded_prefix(headers),
            local: headers
                .get(HEADER_LOCAL_BROWSER)
                .is_some_and(|v| v.as_bytes() == b"1"),
            origin: headers
                .get(axum::http::header::ORIGIN)
                .and_then(|v| v.to_str().ok())
                .and_then(web_origin),
        })
    }

    /// The *web redirect URI* on this browser's origin, if it has one.
    pub fn web_redirect_uri(&self) -> Option<String> {
        self.origin
            .as_ref()
            .map(|origin| format!("{origin}{OAUTH_WEB_CALLBACK_PATH}"))
    }
}

/// An `Origin` value as `scheme://host[:port]`. `None` for `null` and anything
/// that is not http or https.
fn web_origin(value: &str) -> Option<String> {
    let url = reqwest::Url::parse(value.trim()).ok()?;
    matches!(url.scheme(), "http" | "https")
        .then(|| url.origin().ascii_serialization())
        .filter(|origin| origin != "null")
}

/// The last [`BrowserContext`] each device presented, by device id.
///
/// A cache: an agent-started flow has no request of its own, so it reads the
/// entry for the device the authorization page goes to. An engine restart
/// empties it, and the device's next request fills it again.
static BROWSERS: LazyLock<Mutex<HashMap<String, BrowserContext>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Remember how `device_id` reached this engine.
///
/// A request without `Origin` keeps the origin the device last sent, since a
/// browser sends one only on some requests.
pub fn note_browser(device_id: &str, mut browser: BrowserContext) {
    let mut browsers = BROWSERS.lock().unwrap_or_else(|e| e.into_inner());
    let known = browsers.get(device_id);
    if browser.origin.is_none() {
        browser.origin = known.and_then(|b| b.origin.clone());
    }
    if known != Some(&browser) {
        browsers.insert(device_id.to_string(), browser);
    }
}

/// How `device_id` last reached this engine, if it has since the engine began.
pub fn browser_for_device(device_id: &str) -> Option<BrowserContext> {
    BROWSERS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(device_id)
        .cloned()
}

/// Record the browser context of every request a gateway proxied for a device.
pub async fn note_browser_context(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let headers = request.headers();
    let device = headers
        .get(crate::api::actor::HEADER_DEVICE_ID)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|d| !d.is_empty());
    if let (Some(device), Some(browser)) = (device, BrowserContext::from_headers(headers)) {
        note_browser(device, browser);
    }
    next.run(request).await
}

/// How a flow's callback will arrive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallbackRoute {
    /// No gateway: this engine binds the loopback port for the flow.
    OwnListener,
    /// A gateway owns the port and forwards the callback, routed by `state`.
    Gateway(BrowserContext),
}

impl CallbackRoute {
    /// The route for a flow whose page opens in `browser`.
    ///
    /// A gateway-spawned engine never binds the port. With no known browser it
    /// falls back to its own workspace prefix and a local browser, which is
    /// right for a browser on this machine.
    pub fn for_browser(browser: Option<BrowserContext>) -> Self {
        let under_gateway = crate::api::base_path::gateway_port().is_some();
        Self::select(
            under_gateway,
            browser,
            crate::api::base_path::workspace_id(),
        )
    }

    fn select(
        under_gateway: bool,
        browser: Option<BrowserContext>,
        own_slug: Option<String>,
    ) -> Self {
        if !under_gateway {
            return Self::OwnListener;
        }
        Self::Gateway(browser.unwrap_or_else(|| BrowserContext {
            prefix: own_slug.map_or_else(|| "/".to_string(), |slug| format!("/{slug}/")),
            local: true,
            origin: None,
        }))
    }

    /// The `state` an authorization request carries for `nonce`.
    pub fn state_for(&self, nonce: &str) -> String {
        match self {
            Self::OwnListener => nonce.to_string(),
            Self::Gateway(browser) => format!("{}{STATE_PREFIX_SEPARATOR}{nonce}", browser.prefix),
        }
    }
}

/// A gateway-mode flow's open slot, keyed by its full `state`.
struct PendingEntry {
    provider: String,
    deliver: tokio::sync::oneshot::Sender<Result<String, BoxError>>,
}

static PENDING: LazyLock<Mutex<HashMap<String, PendingEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn pending() -> std::sync::MutexGuard<'static, HashMap<String, PendingEntry>> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

/// A flow waiting for the gateway to deliver its callback.
///
/// Dropping it closes the slot, so a timed-out or aborted flow can never be
/// delivered to afterwards.
pub(crate) struct PendingCallback {
    state: String,
    received: tokio::sync::oneshot::Receiver<Result<String, BoxError>>,
}

impl PendingCallback {
    /// Open the slot for `state` before the authorization URL leaves the engine.
    pub(crate) fn open(state: &str, provider: &str) -> Self {
        let (deliver, received) = tokio::sync::oneshot::channel();
        pending().insert(
            state.to_string(),
            PendingEntry {
                provider: provider.to_string(),
                deliver,
            },
        );
        Self {
            state: state.to_string(),
            received,
        }
    }

    /// The authorization code, or the provider's reason for refusing.
    pub(crate) async fn received(mut self) -> Result<String, BoxError> {
        (&mut self.received)
            .await
            .unwrap_or_else(|_| Err("the OAuth callback slot closed without a result".into()))
    }
}

impl Drop for PendingCallback {
    fn drop(&mut self) {
        pending().remove(&self.state);
    }
}

/// What the gateway's forwarded callback did here.
#[derive(Debug)]
pub enum CallbackDelivery {
    /// A flow of this engine's owned the `state`. `ok` is whether the callback
    /// carried a code.
    Accepted { ok: bool, page: String },
    /// No open flow here owns this `state`. The gateway may try a peer.
    Unclaimed { page: String },
}

/// Hand a forwarded callback query to the flow that owns its `state`.
///
/// The full `state` must match an open slot: a forged, replayed or re-routed
/// value finds none and is refused. Delivery closes the slot.
pub fn deliver_callback(query: &str) -> CallbackDelivery {
    let entry = query_param(query, "state", false).and_then(|state| pending().remove(&state));
    let Some(entry) = entry else {
        return CallbackDelivery::Unclaimed {
            page: super::oauth::callback_page("", false),
        };
    };
    let received = callback_received(&entry.provider, query);
    let ok = received.code.is_ok();
    if entry.deliver.send(received.code).is_err() {
        crate::log!(
            "[OAuth] A {} callback arrived as its flow ended",
            entry.provider
        );
    }
    CallbackDelivery::Accepted {
        ok,
        page: received.page,
    }
}

#[cfg(test)]
#[path = "oauth_callback_tests.rs"]
mod tests;
