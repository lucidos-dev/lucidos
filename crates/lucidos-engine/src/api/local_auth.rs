//! The engine's own door: who may call this engine when it faces a network.
//!
//! Full design: `docs/plans/2026-08-29-the-engine-has-its-own-door.md`.
//!
//! # Two questions, two modules
//!
//! [`crate::api::browser_origin`] asks *which document sent this*, which is a
//! CSRF question. This module asks *who is calling*, which is authentication.
//! Browser metadata cannot answer the second: any client may omit every header,
//! so an absent `Sec-Fetch-Site` proves nothing about a caller.

//! # Why the bind decides whether the door is locked
//!
//! On a loopback bind, reaching the socket already proves the caller is a
//! process on this machine, and the engine's whole contract rests on that. The
//! frontend, every app iframe, the CLI and both e2e suites call in freely.
//! Locking that door would break all of them for nothing.
//!
//! A wide bind retires the premise. The caller may now be anyone on the LAN or
//! the tailnet, and nothing else stands behind this. So the door locks, and the
//! only callers left are processes that can read a mode 0600 file.
//!
//! A browser is not one of those, deliberately. A phone reaches a workspace
//! through the gateway at `/<slug>/`, which authenticates it by pairing (ADR
//! 0094, ADR 0096). The gateway then proves ITSELF on the proxy hop.

//! # A loopback peer address proves nothing
//!
//! [`presented_scope`] takes no peer address, exactly as the gateway's
//! `authorize` does not. `tailscale serve` proxies from this machine, so a
//! phone's request arrives with a loopback source. Trusting the source address
//! would trust the whole tailnet.

//! # Why the whole surface, not just the mutating half
//!
//! Splitting reads from writes would leave every thread, artifact and revealed
//! credential open. `/data` alone serves the workspace's files off disk. So the
//! gate wraps the OUTERMOST router, covering `/api/v1`, `/app`, `/data` and the
//! frontend fallback. `/api/v1/health` is the single exemption, because the
//! gateway health-probes it before it can prove anything.

//! # The same token also answers a second question, on every bind
//!
//! [`is_local_process`] asks whether a caller is a process on this machine, for
//! ATTRIBUTION rather than for the door. `api::actor` reads it to stamp the
//! engine's own build-watch and release scripts, which hold no thread-bound
//! origin token and are not a person (ADR 0169).
//!
//! It answers on a loopback bind too, where [`EngineAuth::is_required`] is
//! false and nothing is refused. The door and the name are separate questions,
//! which is the split this module's header opens with.
//!
//! **The answer names a machine, never a person.** So it can never be the
//! evidence ADR 0168's clause 4 asks for. Two scripts presenting the token are
//! one identity in the log, and that is accepted.

use crate::net_config::BindChoice;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use lucidos_local_token as local_token;
use std::sync::OnceLock;

/// What a presented credential authorizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineScope {
    /// The machine-local token: every route.
    Full,
    /// The webhook token: one route, the delivery hop.
    WebhookDelivery,
}

/// The credentials this engine accepts, and whether it checks them at all.
///
/// Read ONCE at startup rather than per request. The read is a file read, and
/// this sits in front of every request including the SSE stream. The gateway
/// mints both tokens before it spawns any engine, so there is no window where a
/// fresher value would have been found (`lucidos_gateway::server::run`).
#[derive(Debug, Clone)]
pub struct EngineAuth {
    /// False on a loopback bind, where reaching the socket is the proof.
    required: bool,
    local: Option<String>,
    webhook: Option<String>,
}

impl EngineAuth {
    /// Resolve the door for an engine bound as `choice` says.
    ///
    /// A wide bind that can read neither token authenticates nobody. That is
    /// logged at boot rather than discovered one 401 at a time. It is not
    /// fatal: refusing every caller is the safe end of the trade, and
    /// `/api/v1/health` still answers so a supervisor can see the process.
    ///
    /// The local token is minted here when the file is absent. See
    /// [`local_token_for_this_engine`].
    pub fn resolve(choice: &BindChoice) -> Self {
        let required = auth_required(choice);
        let auth = EngineAuth {
            required,
            local: local_token_for_this_engine(),
            webhook: local_token::read_named(local_token::WEBHOOK_TOKEN_FILE),
        };
        // One file read serves both questions. Publishing here rather than
        // reading again keeps the door and the attribution answer from ever
        // disagreeing about what the token is.
        publish_local_token(auth.local.clone());
        if required {
            let scope = crate::net_config::bind_scope_label(choice);
            if auth.local.is_none() && auth.webhook.is_none() {
                crate::log!(
                    "[API] bound to {scope} with no credential on disk, so every call is \
                     refused. The mint above says why it could not write one; bind \
                     loopback until it can."
                );
            } else {
                crate::log!("[API] bound to {scope}, so callers must present a credential");
            }
        }
        auth
    }

    /// A door that checks nothing, for a test driving the loopback posture.
    #[cfg(test)]
    pub fn open_for_tests() -> Self {
        EngineAuth {
            required: false,
            local: None,
            webhook: None,
        }
    }

    /// A locked door holding exactly these two secrets.
    #[cfg(test)]
    pub fn locked_for_tests(local: Option<&str>, webhook: Option<&str>) -> Self {
        EngineAuth {
            required: true,
            local: local.map(str::to_string),
            webhook: webhook.map(str::to_string),
        }
    }

    /// Does this engine check credentials at all?
    pub fn is_required(&self) -> bool {
        self.required
    }
}

/// The machine-local token, minting it when no gateway has.
///
/// The gateway mints before it spawns any engine, so a fronted engine finds
/// that value and this writes nothing. A bare single-engine run has no gateway
/// to mint for it, and until this existed it had no token file at all.
///
/// That gap was not academic. The engine's own build-watch reaches
/// `POST /api/v1/events/emit` through the CLI, holding no thread-bound origin
/// token, so the machine-local token is the only identity it can present. With
/// no file, `api::mutating_gate` would refuse the engine's own rebuild.
///
/// A mint that fails is logged and treated as absent, which is exactly the
/// posture the caller had before. It is never fatal: on the loopback bind that
/// ships everywhere, nothing is refused for want of it.
fn local_token_for_this_engine() -> Option<String> {
    match local_token::ensure_named(local_token::LOCAL_TOKEN_FILE) {
        Ok(token) => Some(token),
        Err(e) => {
            crate::log!(
                "[API] could not read or mint the machine-local token ({e}), so no caller \
                 can prove it is a local process"
            );
            None
        }
    }
}

/// Does a listener bound as `choice` says need to authenticate its callers?
///
/// `Address(ip)` is the case worth naming. It binds that address AND loopback
/// (`net_config::bind_socket_addrs`), so a non-loopback `Address` locks the
/// loopback socket too. One router serves both sockets, and per the module note
/// a loopback source address would prove nothing even if it could be read.
pub fn auth_required(choice: &BindChoice) -> bool {
    match choice {
        BindChoice::Loopback => false,
        BindChoice::All => true,
        BindChoice::Address(ip) => !ip.is_loopback(),
    }
}

/// May this path be served with no credential?
///
/// Exact-matched, never prefix-matched, so a future `/api/v1/health/secrets`
/// cannot inherit its parent's exemption. Mirrors the gateway's
/// `auth_api::is_public_path`, which carries the same reasoning.
pub fn is_public_path(path: &str) -> bool {
    // Paths arrive un-normalized. Without this a request like
    // `/data/../api/v1/health` would read as the exempt route.
    if path.split('/').any(|seg| seg == ".." || seg == ".") {
        return false;
    }
    path == "/api/v1/health"
}

/// What a request proved, or `None`.
///
/// Takes no peer address, on purpose: see the module note. The local token is
/// checked first, so a caller holding both gets the wider scope.
pub fn presented_scope(headers: &HeaderMap, auth: &EngineAuth) -> Option<EngineScope> {
    if let Some(expected) = auth.local.as_deref() {
        if presented(headers, local_token::HEADER_LOCAL_TOKEN)
            .is_some_and(|t| local_token::ct_eq(t, expected))
        {
            return Some(EngineScope::Full);
        }
    }
    if let Some(expected) = auth.webhook.as_deref() {
        if presented(headers, local_token::HEADER_WEBHOOK_TOKEN)
            .is_some_and(|t| local_token::ct_eq(t, expected))
        {
            return Some(EngineScope::WebhookDelivery);
        }
    }
    None
}

/// The machine-local token, published by [`EngineAuth::resolve`] at startup.
///
/// `None` before the router is built, and `Some(None)` on a machine with no
/// gateway to mint one. Both mean the same thing to [`is_local_process`]: no
/// caller can prove it is a local process, so none is attributed as one.
static LOCAL_TOKEN: OnceLock<Option<String>> = OnceLock::new();

/// Publish the token for [`is_local_process`]. First writer wins, matching the
/// production invariant that one engine startup builds one router.
fn publish_local_token(token: Option<String>) {
    let _ = LOCAL_TOKEN.set(token);
}

/// Is this caller a process on this machine, running as this user?
///
/// Attribution only. See the module note: it is asked on every bind, including
/// the loopback one where nothing is refused, and it names a machine rather
/// than a person.
pub fn is_local_process(headers: &HeaderMap) -> bool {
    carries_local_token(headers, LOCAL_TOKEN.get().and_then(Option::as_deref))
}

/// The pure half of [`is_local_process`], so the comparison is testable without
/// the process-wide token.
///
/// No expected token means nobody qualifies. That is the fail-closed direction:
/// an absent secret must not make every caller local.
fn carries_local_token(headers: &HeaderMap, expected: Option<&str>) -> bool {
    expected.is_some_and(|want| {
        presented(headers, local_token::HEADER_LOCAL_TOKEN)
            .is_some_and(|got| local_token::ct_eq(got, want))
    })
}

/// Publish a known token so a test can drive [`is_local_process`]. Idempotent,
/// and every caller installs the same value, so parallel tests agree.
///
/// No unit test builds a router, so nothing else writes [`LOCAL_TOKEN`] in the
/// test binary.
#[cfg(test)]
pub fn publish_test_local_token() -> &'static str {
    const TEST_TOKEN: &str = "test-machine-local-token";
    publish_local_token(Some(TEST_TOKEN.to_string()));
    TEST_TOKEN
}

/// A header read as a trimmed, non-empty value.
fn presented<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// May a caller holding `scope` reach `path`?
///
/// [`EngineScope::WebhookDelivery`] reaches exactly
/// `/api/v1/webhooks/<id>/deliver`. That is the hop the gateway's hook socket
/// makes, and a hook socket is what `tailscale funnel` may expose to the open
/// internet (ADR 0097). Every other route is off limits to it. So a delivery
/// cannot become a workspace restart, a data write, or a credentialed upstream
/// call on the user's own API accounts.
pub fn scope_allows(scope: EngineScope, path: &str) -> bool {
    match scope {
        EngineScope::Full => true,
        EngineScope::WebhookDelivery => is_webhook_delivery_path(path),
    }
}

/// Is `path` exactly the webhook delivery route?
///
/// Segment-matched rather than compared with `starts_with`, so neither a dot
/// segment nor a longer sibling route can wear the shape.
fn is_webhook_delivery_path(path: &str) -> bool {
    let mut segments = path.split('/').filter(|s| !s.is_empty());
    let head = [segments.next(), segments.next(), segments.next()];
    if head != [Some("api"), Some("v1"), Some("webhooks")] {
        return false;
    }
    let Some(id) = segments.next() else {
        return false;
    };
    if id == ".." || id == "." {
        return false;
    }
    segments.next() == Some("deliver") && segments.next().is_none()
}

/// Refuse a request that proved nothing, or that reached past its scope.
///
/// Layered on the OUTERMOST router, so it runs before routing and before any
/// handler resolves a credential or touches the database.
pub async fn enforce(
    axum::extract::State(auth): axum::extract::State<std::sync::Arc<EngineAuth>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if !auth.is_required() {
        return next.run(req).await;
    }
    let path = req.uri().path();
    if is_public_path(path) {
        return next.run(req).await;
    }
    match presented_scope(req.headers(), &auth) {
        Some(scope) if scope_allows(scope, path) => next.run(req).await,
        Some(scope) => {
            // Naming the scope is safe and is the point: the caller knows which
            // credential it sent, and a silent 403 here is undebuggable.
            crate::log!("[API] refused {path}: a {scope:?} credential does not reach it");
            (
                StatusCode::FORBIDDEN,
                "that credential does not reach this route",
            )
                .into_response()
        }
        None => {
            crate::log!("[API] refused {path}: this engine faces a network and proved no caller");
            (
                StatusCode::UNAUTHORIZED,
                "this engine is not on loopback, so a caller must present a local credential",
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    /// A synthetic address from the CGNAT range Tailscale hands out. Invented,
    /// never captured: only "valid, and not loopback" is load-bearing.
    const TAILNET: &str = "100.64.0.1";

    fn hm(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                axum::http::HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn a_loopback_bind_needs_no_credential() {
        assert!(!auth_required(&BindChoice::Loopback));
        assert!(!auth_required(&BindChoice::Address(IpAddr::V4(
            Ipv4Addr::LOCALHOST
        ))));
        assert!(!auth_required(&BindChoice::Address(IpAddr::V6(
            Ipv6Addr::LOCALHOST
        ))));
    }

    #[test]
    fn every_bind_that_faces_a_network_needs_one() {
        assert!(auth_required(&BindChoice::All));
        assert!(auth_required(&BindChoice::Address(
            TAILNET.parse().unwrap()
        )));
        assert!(auth_required(&BindChoice::Address(
            "192.168.1.10".parse().unwrap()
        )));
    }

    #[test]
    fn a_specific_address_locks_its_retained_loopback_socket_too() {
        // `bind_socket_addrs` keeps loopback beside a configured address, and
        // one router serves both. A caller arriving on the loopback socket of a
        // tailnet-bound engine is not thereby local: `tailscale serve` proxies
        // from this machine, which is the whole reason ADR 0094 exists.
        let choice = BindChoice::Address(TAILNET.parse().unwrap());
        let addrs = crate::net_config::bind_socket_addrs(&choice, 5173);
        assert!(
            addrs.iter().any(|a| a.ip().is_loopback()),
            "the premise of this test: {addrs:?}"
        );
        assert!(auth_required(&choice));
    }

    #[test]
    fn only_health_is_public_and_the_exemption_is_not_inherited() {
        assert!(is_public_path("/api/v1/health"));
        assert!(!is_public_path("/api/v1/health/detail"));
        assert!(!is_public_path("/api/v1/healthz"));
        assert!(!is_public_path("/api/v1/threads/list"));
        assert!(!is_public_path("/data/artifacts/notes.md"));
        assert!(!is_public_path("/app/habit-tracker/"));
        assert!(!is_public_path("/"));
    }

    #[test]
    fn a_dot_segment_never_reaches_the_exemption() {
        assert!(!is_public_path("/data/../api/v1/health"));
        assert!(!is_public_path("/api/v1/./health"));
    }

    /// The attribution half, which the bind does not decide. Reading a mode
    /// 0600 file is the thing a remote caller cannot do.
    #[test]
    fn the_local_token_names_a_local_process() {
        let h = hm(&[(local_token::HEADER_LOCAL_TOKEN, "local-secret")]);
        assert!(carries_local_token(&h, Some("local-secret")));
        assert!(!carries_local_token(&h, Some("other-secret")));
    }

    /// Fail closed on both halves of the pair. A machine with no gateway has
    /// no token, and a caller that sends nothing proves nothing. Neither may
    /// read as local: that would attribute every stranger to this machine.
    #[test]
    fn nobody_is_local_without_both_a_secret_and_a_header() {
        let with = hm(&[(local_token::HEADER_LOCAL_TOKEN, "local-secret")]);
        let without = hm(&[]);
        assert!(!carries_local_token(&with, None));
        assert!(!carries_local_token(&without, Some("local-secret")));
        assert!(!carries_local_token(&without, None));
        // An empty header is an absent one, per `presented`.
        let blank = hm(&[(local_token::HEADER_LOCAL_TOKEN, "   ")]);
        assert!(!carries_local_token(&blank, Some("local-secret")));
    }

    /// The webhook token reaches one route and names no local process. Its
    /// holder is whatever `tailscale funnel` exposed, so reading it as local
    /// would hand the open internet the engine's own identity.
    #[test]
    fn the_webhook_token_never_names_a_local_process() {
        let h = hm(&[(local_token::HEADER_WEBHOOK_TOKEN, "hook-secret")]);
        assert!(!carries_local_token(&h, Some("hook-secret")));
    }

    #[test]
    fn the_local_token_proves_full_authority() {
        let auth = EngineAuth::locked_for_tests(Some("local-secret"), Some("hook-secret"));
        let h = hm(&[(local_token::HEADER_LOCAL_TOKEN, "local-secret")]);
        assert_eq!(presented_scope(&h, &auth), Some(EngineScope::Full));
    }

    #[test]
    fn the_webhook_token_proves_only_its_own_scope() {
        let auth = EngineAuth::locked_for_tests(Some("local-secret"), Some("hook-secret"));
        let h = hm(&[(local_token::HEADER_WEBHOOK_TOKEN, "hook-secret")]);
        assert_eq!(
            presented_scope(&h, &auth),
            Some(EngineScope::WebhookDelivery)
        );
    }

    #[test]
    fn a_wrong_or_blank_credential_proves_nothing() {
        let auth = EngineAuth::locked_for_tests(Some("local-secret"), Some("hook-secret"));
        for (name, value) in [
            (local_token::HEADER_LOCAL_TOKEN, "nope"),
            (local_token::HEADER_LOCAL_TOKEN, "   "),
            // Presenting one secret under the OTHER name must not work, or the
            // scope would be the caller's to choose.
            (local_token::HEADER_LOCAL_TOKEN, "hook-secret"),
            (local_token::HEADER_WEBHOOK_TOKEN, "local-secret"),
            (local_token::HEADER_WEBHOOK_TOKEN, ""),
        ] {
            assert_eq!(
                presented_scope(&hm(&[(name, value)]), &auth),
                None,
                "{name}: {value:?} must not authenticate"
            );
        }
        assert_eq!(presented_scope(&hm(&[]), &auth), None);
    }

    #[test]
    fn a_credential_the_engine_does_not_hold_authenticates_nobody() {
        // The no-gateway machine. An absent secret must never compare equal to
        // an absent header, which is the classic fail-open here.
        let auth = EngineAuth::locked_for_tests(None, None);
        assert_eq!(presented_scope(&hm(&[]), &auth), None);
        for name in [
            local_token::HEADER_LOCAL_TOKEN,
            local_token::HEADER_WEBHOOK_TOKEN,
        ] {
            assert_eq!(presented_scope(&hm(&[(name, "")]), &auth), None);
            assert_eq!(presented_scope(&hm(&[(name, "anything")]), &auth), None);
        }
    }

    #[test]
    fn a_device_id_header_authorizes_nothing_here_either() {
        // ADR 0050: the device id names who to credit, never what they may do.
        let auth = EngineAuth::locked_for_tests(Some("local-secret"), None);
        let h = hm(&[("x-lucidos-device-id", "some-real-device-id")]);
        assert_eq!(presented_scope(&h, &auth), None);
    }

    #[test]
    fn full_authority_reaches_everything() {
        for path in [
            "/api/v1/threads/list",
            "/api/v1/webhooks/abc/deliver",
            "/data/artifacts/notes.md",
            "/app/habit-tracker/",
            "/",
        ] {
            assert!(scope_allows(EngineScope::Full, path), "{path}");
        }
    }

    #[test]
    fn the_webhook_scope_reaches_one_route_and_no_other() {
        assert!(scope_allows(
            EngineScope::WebhookDelivery,
            "/api/v1/webhooks/01990ce2-fbed-7fd1-85de-dbc8000418c8/deliver"
        ));

        // The whole point of the scope: none of these is a delivery.
        for path in [
            "/api/v1/threads/list",
            "/api/v1/webhooks",
            "/api/v1/webhooks/abc",
            "/api/v1/webhooks/abc/deliver/extra",
            "/api/v1/webhooks/abc/../../threads/list",
            "/api/v1/webhooks/../threads/list",
            "/api/v1/webhooks//deliver",
            "/api/v1/data/config/apis.json",
            "/api/v1/proxy/openai/v1/messages",
            "/api/v1/changes/apply",
            "/data/artifacts/notes.md",
            "/app/habit-tracker/",
            "/",
        ] {
            assert!(
                !scope_allows(EngineScope::WebhookDelivery, path),
                "a webhook credential must not reach {path}"
            );
        }
    }

    #[test]
    fn an_open_door_is_open_and_a_locked_one_says_so() {
        assert!(!EngineAuth::open_for_tests().is_required());
        assert!(EngineAuth::locked_for_tests(Some("x"), None).is_required());
    }

    // ── The middleware, driven through a router ─────────────────────────────
    //
    // The pure helpers above answer the policy. These answer whether axum
    // composes it the way the policy assumes: before routing, over every mount,
    // and with the response codes the two refusals promise.
    //
    // What they deliberately do NOT prove is WHERE `create_router` applies the
    // layer. That needs a database, so it is covered by reading the one call
    // site and by the e2e suites, which drive a real engine.

    /// The engine's four mount points, as `create_router` nests them.
    ///
    /// `/app` and `/data` are the reason the layer sits outside the `/api/v1`
    /// nest: they are its siblings, so a gate inside it would miss them.
    const MOUNTS: &[&str] = &[
        "/api/v1/threads/list",
        "/app/habit-tracker/",
        "/data/artifacts/notes.md",
        "/",
    ];

    /// A router shaped like the engine's, behind the real middleware. The inner
    /// handler answers a teapot, so seeing one means the gate let it through.
    fn gated(auth: EngineAuth) -> axum::Router {
        axum::Router::new()
            .fallback(|| async { StatusCode::IM_A_TEAPOT })
            .layer(axum::middleware::from_fn_with_state(
                std::sync::Arc::new(auth),
                enforce,
            ))
    }

    async fn call(auth: EngineAuth, path: &str, hdrs: &[(&str, &str)]) -> StatusCode {
        use tower::ServiceExt as _;
        let mut builder = axum::extract::Request::builder().uri(path);
        for (k, v) in hdrs {
            builder = builder.header(*k, *v);
        }
        gated(auth)
            .oneshot(builder.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    fn locked() -> EngineAuth {
        EngineAuth::locked_for_tests(Some("local-secret"), Some("hook-secret"))
    }

    #[tokio::test]
    async fn a_loopback_engine_asks_nobody_for_anything() {
        // The migration invariant. Every shipped topology binds loopback, so
        // this is the path that must be indistinguishable from before.
        for path in MOUNTS {
            assert_eq!(
                call(EngineAuth::open_for_tests(), path, &[]).await,
                StatusCode::IM_A_TEAPOT,
                "{path} must stay open on a loopback bind"
            );
        }
    }

    #[tokio::test]
    async fn a_wide_engine_refuses_every_mount_to_a_caller_with_no_credential() {
        // The hole this closes. `/data` is the sharpest of the four: it serves
        // the workspace's files off disk and sits outside the origin gate.
        for path in MOUNTS {
            assert_eq!(
                call(locked(), path, &[]).await,
                StatusCode::UNAUTHORIZED,
                "{path} must be refused on a wide bind"
            );
        }
    }

    #[tokio::test]
    async fn a_wide_engine_still_answers_its_health_probe() {
        // The gateway probes this before the engine can prove anything to it.
        assert_eq!(
            call(locked(), "/api/v1/health", &[]).await,
            StatusCode::IM_A_TEAPOT
        );
    }

    #[tokio::test]
    async fn the_local_token_reaches_every_mount_on_a_wide_engine() {
        for path in MOUNTS {
            assert_eq!(
                call(
                    locked(),
                    path,
                    &[(local_token::HEADER_LOCAL_TOKEN, "local-secret")]
                )
                .await,
                StatusCode::IM_A_TEAPOT,
                "the gateway hop and the CLI must still reach {path}"
            );
        }
    }

    #[tokio::test]
    async fn an_open_door_streams_a_body_instead_of_collecting_it() {
        // This layer sits in front of `/api/v1/events`, an SSE stream that
        // never ends. A middleware that collected the body would hold every
        // transient event forever. The first casualty would be a test waiting
        // on `AppUiRefreshRequested`.
        //
        // Driven with a body that yields one chunk and then blocks, so a
        // collecting layer cannot reach the assertion at all: it would still be
        // waiting for a second chunk that only arrives after the assertion.
        use futures::StreamExt as _;

        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let stream = futures::stream::once(async { Ok::<_, std::io::Error>("first") }).chain(
            futures::stream::once(async move {
                let _ = released.await;
                Ok::<_, std::io::Error>("second")
            }),
        );
        // An axum handler must be callable more than once, so the one-shot body
        // is handed over rather than captured.
        let body = std::sync::Arc::new(std::sync::Mutex::new(Some(axum::body::Body::from_stream(
            stream,
        ))));
        let router = axum::Router::new()
            .fallback(|| async move { StatusCode::IM_A_TEAPOT })
            .route(
                "/api/v1/events",
                axum::routing::get(move || {
                    let body = body.clone();
                    async move {
                        let body = body.lock().unwrap().take().expect("one request only");
                        axum::response::Response::new(body)
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                std::sync::Arc::new(EngineAuth::open_for_tests()),
                enforce,
            ));

        use tower::ServiceExt as _;
        let response = router
            .oneshot(
                axum::extract::Request::builder()
                    .uri("/api/v1/events")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        // The head arrived while the body was still open, which is the
        // property the SSE contract rests on.
        let mut chunks = response.into_body().into_data_stream();
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), chunks.next())
            .await
            .expect("the first chunk must arrive before the stream ends")
            .expect("a chunk")
            .expect("no error");
        assert_eq!(first.as_ref(), b"first");

        let _ = release.send(());
        let second = chunks.next().await.expect("a chunk").expect("no error");
        assert_eq!(second.as_ref(), b"second");
    }

    #[tokio::test]
    async fn a_webhook_credential_delivers_and_can_do_nothing_else() {
        // `t3-inbound-auth-scopes` in one test: the delivery lands, and the
        // same credential is refused everywhere else. 403 rather than 401,
        // because the caller DID authenticate. It just cannot reach this.
        let hdr = &[(local_token::HEADER_WEBHOOK_TOKEN, "hook-secret")];
        assert_eq!(
            call(locked(), "/api/v1/webhooks/abc/deliver", hdr).await,
            StatusCode::IM_A_TEAPOT
        );
        for path in MOUNTS {
            assert_eq!(
                call(locked(), path, hdr).await,
                StatusCode::FORBIDDEN,
                "a webhook credential must not reach {path}"
            );
        }
    }
}
