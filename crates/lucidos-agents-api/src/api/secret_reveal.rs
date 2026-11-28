use super::*;
use rand::distributions::Alphanumeric;
use rand::Rng;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const TOKEN_TTL: Duration = Duration::from_secs(30);

const TOKEN_LEN: usize = 32;

pub(super) fn fresh_token() -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(TOKEN_LEN)
        .map(char::from)
        .collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RevealSubject {
    Credential(uuid::Uuid),
    BackupKey,
}

struct Reveal {
    subject: RevealSubject,
    expires_at: Instant,
}

#[derive(Clone, Default)]
pub struct RevealTokens {
    inner: Arc<Mutex<HashMap<String, Reveal>>>,
}

impl RevealTokens {
    pub fn new() -> Self {
        Self::default()
    }

    pub(super) fn mint(&self, subject: RevealSubject) -> Option<String> {
        let token = fresh_token();
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RefererRule {
    Required,
    WhenPresent,
}

/// INVARIANT: May this request reach a secret's plaintext, as far as its origin goes?
///
/// A request carrying no `Sec-Fetch-Site` and no `Origin` is not a browser.
/// Allowed under either rule, and bounded by the loopback bind: this is the CLI
/// and the API e2e suite. A browser-shaped one whose `Referer` names an app
/// document is refused under either.
pub(crate) fn reveal_request_allowed(headers: &HeaderMap, rule: RefererRule) -> bool {
    if headers.contains_key(lucidos_gateway_stamp::HEADER_DEVICE_SESSION) {
        return false;
    }
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

/// INVARIANT: Whether a `Referer` URL points at an app UI document.
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

pub(super) fn forbidden(what: &str) -> (StatusCode, String) {
    (
        StatusCode::FORBIDDEN,
        format!("{what} is not readable from an app"),
    )
}

pub(super) fn token_required(mint_route: &str) -> (StatusCode, String) {
    (
        StatusCode::FORBIDDEN,
        format!("a one-shot reveal token is required; mint one at POST {mint_route}"),
    )
}

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
