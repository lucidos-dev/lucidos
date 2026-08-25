//! Revealing a credential's plaintext to the Settings page, and to nothing
//! else.
//!
//! `GET /api/v1/credential-value` used to return the stored secret to any
//! caller. App UIs load at `/app/<id>/` on the engine's own origin, so an
//! installed app's JS could list the credentials and read every one. That
//! undercuts the premise of `api::proxy`, which exists so an iframe never sees
//! a credential.
//!
//! A reveal is now two steps inside a 30-second one-shot window, refused from
//! an app document, and audited. **No header can authenticate the Settings
//! page against a same-origin app**, so the origin half is defense in depth
//! rather than a boundary. ADR 0117 states the full model, what it does not
//! close, and why the two steps read `Referer` by different rules. The complete
//! fix is a distinct origin for apps, which is ADR 0014's open residual.

use super::*;
use crate::core::CredentialStore;
use rand::distributions::Alphanumeric;
use rand::Rng;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long a minted token stays usable. Long enough for the click that minted
/// it to finish, short enough that a leaked one is worthless.
const TOKEN_TTL: Duration = Duration::from_secs(30);

/// Characters in a minted token. 32 alphanumerics from a CSPRNG.
const TOKEN_LEN: usize = 32;

struct Reveal {
    credential_id: uuid::Uuid,
    expires_at: Instant,
}

/// The live one-shot reveal tokens, keyed by token.
///
/// In memory on purpose. A token is worth 30 seconds and is void the moment it
/// is used, so persisting it would outlive its own meaning. Losing the set on
/// restart costs the user one extra click.
#[derive(Clone, Default)]
pub struct RevealTokens {
    inner: Arc<Mutex<HashMap<String, Reveal>>>,
}

impl RevealTokens {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint a token that reveals exactly `credential_id`, once.
    ///
    /// Expired entries are dropped on the way through, so unused tokens cannot
    /// accumulate. A poisoned mutex answers `None` rather than panicking the
    /// worker, matching the engine's other `std::sync::Mutex` sites.
    fn mint(&self, credential_id: uuid::Uuid) -> Option<String> {
        let token: String = rand::thread_rng()
            .sample_iter(&Alphanumeric)
            .take(TOKEN_LEN)
            .map(char::from)
            .collect();
        let mut guard = self.inner.lock().ok()?;
        let now = Instant::now();
        guard.retain(|_, r| r.expires_at > now);
        guard.insert(
            token.clone(),
            Reveal {
                credential_id,
                expires_at: now + TOKEN_TTL,
            },
        );
        Some(token)
    }

    /// Spend `token` on `credential_id`, or refuse.
    ///
    /// The entry goes whichever way this lands. A token presented against the
    /// wrong id has been mishandled, and re-offering it is not a case to
    /// support.
    fn redeem(&self, token: &str, credential_id: uuid::Uuid) -> bool {
        let Ok(mut guard) = self.inner.lock() else {
            return false;
        };
        let Some(entry) = guard.remove(token) else {
            return false;
        };
        entry.expires_at > Instant::now() && entry.credential_id == credential_id
    }
}

/// What a browser-shaped request has to present to pass the origin check.
///
/// The two steps of a reveal answer this differently, and the difference is
/// load-bearing rather than an inconsistency.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RefererRule {
    /// Refuse a browser that presents no `Referer`. Stricter than the gateway's
    /// control plane, which lets one through.
    ///
    /// The mint can afford this. It is a `POST`, and the service worker hands
    /// every non-GET straight to the browser. So nothing sits between the page
    /// and the engine that could lose the header.
    Required,
    /// Refuse an app `Referer`, but let a missing one through.
    ///
    /// The redeem is a `GET`, which the service worker re-issues on iOS. A
    /// re-issue is meant to carry the original referrer, and a browser that
    /// dropped it would take the Copy button down in the installed PWA.
    ///
    /// It costs nothing. A token exists only because a mint passed the strict
    /// rule, and it spends once, for one row.
    WhenPresent,
}

/// May this request reach a credential's plaintext, as far as its origin goes?
///
/// A request carrying no `Sec-Fetch-Site` and no `Origin` is not a browser.
/// Allowed under either rule, and bounded by the loopback bind: this is the CLI
/// and the API e2e suite. A browser-shaped one whose `Referer` names an app
/// document is refused under either.
pub(super) fn reveal_request_allowed(headers: &HeaderMap, rule: RefererRule) -> bool {
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    let is_browser = header("sec-fetch-site").is_some() || header("origin").is_some();
    match header("referer") {
        Some(referer) => !referer_is_app_document(referer),
        None => !is_browser || rule == RefererRule::WhenPresent,
    }
}

/// Whether a `Referer` URL points at an app UI document.
///
/// App UIs are served at `/app/<id>/…` direct, and at `/<slug>/app/<id>/…`
/// behind the gateway, so `app` is either the first or the second segment. It
/// must be followed by an app id, which keeps a workspace whose slug is
/// literally `app` from reading as one.
///
/// The gateway keeps its own copy (`control::referer_is_app_iframe`) because it
/// deliberately does not depend on this crate. That one need only know the
/// gateway's own shape.
fn referer_is_app_document(referer: &str) -> bool {
    let after_scheme = referer.split_once("://").map(|(_, r)| r).unwrap_or(referer);
    let path = match after_scheme.find('/') {
        Some(idx) if referer.contains("://") => &after_scheme[idx..],
        _ if referer.starts_with('/') => referer,
        _ => return false,
    };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    matches!(segments.first(), Some(&"app") if segments.len() >= 2)
        || matches!(segments.get(1), Some(&"app") if segments.len() >= 3)
}

fn forbidden() -> (StatusCode, String) {
    (
        StatusCode::FORBIDDEN,
        "credential values are not readable from an app".to_string(),
    )
}

#[derive(Serialize)]
pub(super) struct RevealTokenResponse {
    token: String,
    expires_in_secs: u64,
}

/// `POST /api/v1/credential-reveal-token`: mint a one-shot reveal token.
///
/// Step one of two. The Settings page mints, then immediately spends. Splitting
/// the read in half is what stops a scrape of reachable GET endpoints finding
/// the plaintext.
pub(super) async fn mint_reveal_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CredentialIdQuery>,
) -> Result<Json<RevealTokenResponse>, (StatusCode, String)> {
    if !reveal_request_allowed(&headers, RefererRule::Required) {
        log!("[Credentials] refused a reveal-token mint from an app document");
        return Err(forbidden());
    }
    // Refuse to mint against a row that does not exist, so a caller learns that
    // here rather than after spending the token.
    match CredentialStore::get_by_id(&state.pool, query.id).await {
        Ok(Some(_)) => {}
        Ok(None) => return Err((StatusCode::NOT_FOUND, "Credential not found".to_string())),
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to get credential: {}", e),
            ))
        }
    }
    let token = state.reveal_tokens.mint(query.id).ok_or((
        StatusCode::INTERNAL_SERVER_ERROR,
        "reveal-token store is poisoned".to_string(),
    ))?;
    Ok(Json(RevealTokenResponse {
        token,
        expires_in_secs: TOKEN_TTL.as_secs(),
    }))
}

/// Query for the reveal itself: the row, plus the token minted for it.
///
/// `token` is optional in the SHAPE so a caller that omits it meets the
/// handler's own refusal, which names the route that mints one. Left required,
/// axum answers a 400 that says only "failed to deserialize query string".
#[derive(Deserialize)]
pub(super) struct CredentialValueQuery {
    id: uuid::Uuid,
    #[serde(default)]
    token: Option<String>,
}

/// `GET /api/v1/credential-value`: the plaintext, for the Settings copy buttons
/// and the edit form's prefill.
///
/// By id, not name. `CredentialStore::get` is blind to `oauth_client` rows on
/// purpose. A name-keyed lookup could therefore never reach the client ID and
/// secret that row's own Copy buttons ask for.
///
/// Every success writes a `CredentialRevealed` row naming the service and the
/// device, never the value. An app that does defeat the origin check therefore
/// cannot do it quietly.
pub(super) async fn get_credential_value(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<CredentialValueQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if !reveal_request_allowed(&headers, RefererRule::WhenPresent) {
        log!("[Credentials] refused a credential-value read from an app document");
        return Err(forbidden());
    }
    let presented = query.token.as_deref().unwrap_or_default();
    if !state.reveal_tokens.redeem(presented, query.id) {
        log!("[Credentials] refused a credential-value read: no live reveal token");
        return Err((
            StatusCode::FORBIDDEN,
            "a one-shot reveal token is required; mint one at POST /api/v1/credential-reveal-token"
                .to_string(),
        ));
    }
    match CredentialStore::get_by_id(&state.pool, query.id).await {
        Ok(Some(cred)) => {
            state
                .engine
                .event_bus
                .emit_user_system(
                    &headers,
                    &state.pool,
                    "[Credentials] CredentialRevealed",
                    |actor| crate::engine::event_bus::SystemEvent::CredentialRevealed {
                        service_name: cred.service_name.clone(),
                        auth_type: cred.auth_type.to_string(),
                        actor,
                    },
                )
                .await;
            Ok(Json(serde_json::json!({
                "auth_type": cred.auth_type.to_string(),
                "auth_value": cred.auth_value,
            })))
        }
        Ok(None) => Err((StatusCode::NOT_FOUND, "Credential not found".to_string())),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to get credential: {}", e),
        )),
    }
}

/// Routes for the two-step credential reveal.
pub(super) fn router() -> Router<AppState> {
    Router::new()
        .route("/credential-reveal-token", post(mint_reveal_token))
        .route("/credential-value", get(get_credential_value))
}

#[cfg(test)]
#[path = "credential_reveal_tests.rs"]
mod tests;
