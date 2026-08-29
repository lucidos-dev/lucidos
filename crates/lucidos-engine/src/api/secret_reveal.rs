//! The guard in front of any route that hands a stored secret to its caller.
//!
//! Two secrets use it: a credential's plaintext (ADR 0117) and the workspace
//! backup key. Both routes ask the same three questions. Did this come from an
//! app document? Does a live one-shot token come with it? And is the read on
//! the record?
//!
//! # No header authenticates the Settings page against a same-origin app
//!
//! Apps load at `/app/<id>/` on the engine's own origin, in an iframe carrying
//! `allow-same-origin`. App JavaScript therefore reaches `window.top.fetch` and
//! runs in the shell's own realm. Every browser-set signal then reads as the
//! shell's. ADR 0117 states that model, and ADR 0144 records why it is
//! permanent rather than a gap awaiting a fix.
//!
//! So the origin half is defense in depth. The guard buys three narrower
//! things. No plaintext is one bare GET away. A leaked capability is worth
//! thirty seconds against one secret. And a reveal leaves an attributed row.

//! # Every route that hands back a secret, and its verdict
//!
//! A new one joins this table or it joins this module. Nothing else on
//! `/api/v1` returns stored secret material today.
//!
//! | Route | Verdict |
//! |---|---|
//! | `GET /credential-value` | guarded here |
//! | `GET /backup/key` | guarded here |
//! | `POST /backup/key` | guarded here; it is idempotent, so it returns the same plaintext |
//! | `GET /oauth/:provider/access-token` | deliberately app-facing, as `lucidos.oauth.getAccessToken`. Short-lived, and the refresh token stays engine-side |
//! | `POST /webhooks`, `PUT /webhooks` | hand back a secret this call minted, never a stored one. A read cannot reach it |
//! | `GET /credentials`, `GET /oauth/accounts`, `GET /email-account` | carry no plaintext |

use super::*;
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

/// Which secret a token opens.
///
/// A token carries its subject. So one minted for a credential can never spend
/// on the backup key, and one credential's token can never open another's.
/// Without that binding a single click would open every secret in the workspace
/// for thirty seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RevealSubject {
    /// One credential row, by primary key.
    Credential(uuid::Uuid),
    /// The workspace backup key. A singleton, so it needs no id.
    BackupKey,
}

struct Reveal {
    subject: RevealSubject,
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

    /// Mint a token that reveals exactly `subject`, once.
    ///
    /// Expired entries are dropped on the way through, so unused tokens cannot
    /// accumulate. A poisoned mutex answers `None` rather than panicking the
    /// worker, matching the engine's other `std::sync::Mutex` sites.
    pub(super) fn mint(&self, subject: RevealSubject) -> Option<String> {
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
                subject,
                expires_at: now + TOKEN_TTL,
            },
        );
        Some(token)
    }

    /// Spend `token` on `subject`, or refuse.
    ///
    /// The entry goes whichever way this lands. A token presented against the
    /// wrong subject has been mishandled, and re-offering it is not a case to
    /// support.
    pub(super) fn redeem(&self, token: &str, subject: RevealSubject) -> bool {
        let Ok(mut guard) = self.inner.lock() else {
            return false;
        };
        let Some(entry) = guard.remove(token) else {
            return false;
        };
        entry.expires_at > Instant::now() && entry.subject == subject
    }
}

/// What a browser-shaped request has to present to pass the origin check.
///
/// The two steps of a reveal answer this differently, and the difference is
/// load-bearing rather than an inconsistency.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    /// rule, and it spends once, for one subject.
    WhenPresent,
}

/// May this request reach a secret's plaintext, as far as its origin goes?
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

/// The 403 for a request an app document sent. `what` names the secret, so the
/// refusal says which route turned the caller away.
pub(super) fn forbidden(what: &str) -> (StatusCode, String) {
    (
        StatusCode::FORBIDDEN,
        format!("{what} is not readable from an app"),
    )
}

/// The 403 for a request with no live token, naming the route that mints one.
///
/// The route is spelled out because this is the whole recovery path. A caller
/// outside this repository has no other way to find it.
pub(super) fn token_required(mint_route: &str) -> (StatusCode, String) {
    (
        StatusCode::FORBIDDEN,
        format!("a one-shot reveal token is required; mint one at POST {mint_route}"),
    )
}

/// What a mint hands back: the token, and how long the caller has to spend it.
#[derive(Serialize)]
pub(super) struct RevealTokenResponse {
    pub token: String,
    pub expires_in_secs: u64,
}

impl RevealTokenResponse {
    pub(super) fn new(token: String) -> Self {
        Self {
            token,
            expires_in_secs: TOKEN_TTL.as_secs(),
        }
    }
}

#[cfg(test)]
#[path = "secret_reveal_tests.rs"]
mod tests;
