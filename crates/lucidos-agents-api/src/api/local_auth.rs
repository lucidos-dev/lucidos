//! INVARIANT: # A loopback peer address proves nothing
//!
//! [`presented_scope`] takes no peer address, exactly as the gateway's
//! `authorize` does not. `tailscale serve` proxies from this machine, so a
//! phone's request arrives with a loopback source. Trusting the source address
//! would trust the whole tailnet.

//! INVARIANT: # Why the whole surface, not just the mutating half
//!
//! Splitting reads from writes would leave every thread, artifact and revealed
//! credential open. `/data` alone serves the workspace's files off disk. So the
//! gate wraps the OUTERMOST router, covering `/api/v1`, `/app`, `/data` and the
//! frontend fallback. `/api/v1/health` is the single exemption, because the
//! gateway health-probes it before it can prove anything.

use crate::engine::thread_events::{ActorMode, MessageOrigin};
use crate::net_config::BindChoice;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use lucidos_local_token as local_token;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineScope {
    Full,
    WebhookDelivery,
}

/// INVARIANT: The credentials this engine accepts, and whether it checks them at all.
///
/// Read ONCE at startup rather than per request. The read is a file read, and
/// this sits in front of every request including the SSE stream. The engine
/// reads or mints each token under the mint lock the gateway also takes, and
/// nothing replaces a published token. So no later read could find a fresher
/// value, whichever of the two booted first.
#[derive(Debug, Clone)]
pub struct EngineAuth {
    required: bool,
    local: Option<String>,
    hook_socket: Option<String>,
}

impl EngineAuth {
    /// INVARIANT: Resolve the door for an engine bound as `choice` says.
    ///
    /// A wide bind that can read neither token authenticates nobody. That is
    /// logged at boot rather than discovered one 401 at a time. It is not
    /// fatal: refusing every caller is the safe end of the trade, and
    /// `/api/v1/health` still answers so a supervisor can see the process.
    ///
    /// Both tokens are minted here when their file is absent. See
    /// [`token_for_this_engine`].
    pub fn resolve(choice: &BindChoice) -> Self {
        let required = auth_required(choice);
        let auth = EngineAuth {
            required,
            local: token_for_this_engine(
                local_token::LOCAL_TOKEN_FILE,
                "no caller can prove it is a local process",
            ),
            hook_socket: token_for_this_engine(
                local_token::HOOK_SOCKET_TOKEN_FILE,
                "no webhook delivery can reach this engine on a wide bind",
            ),
        };
        publish_local_token(auth.local.clone());
        crate::diagnostic_log::register_secret_values(
            auth.local.iter().chain(&auth.hook_socket).cloned(),
        );
        if required {
            let scope = crate::net_config::bind_scope_label(choice);
            if auth.local.is_none() && auth.hook_socket.is_none() {
                crate::log_legacy!(
                    "[API] bound to {scope} with no credential on disk, so every call is \
                     refused. The mint above says why it could not write one; bind \
                     loopback until it can."
                );
            } else {
                crate::log_legacy!("[API] bound to {scope}, so callers must present a credential");
            }
        }
        auth
    }

    #[cfg(test)]
    pub fn open_for_tests() -> Self {
        EngineAuth {
            required: false,
            local: None,
            hook_socket: None,
        }
    }

    #[cfg(test)]
    pub fn locked_for_tests(local: Option<&str>, hook_socket: Option<&str>) -> Self {
        EngineAuth {
            required: true,
            local: local.map(str::to_string),
            hook_socket: hook_socket.map(str::to_string),
        }
    }

    pub fn is_required(&self) -> bool {
        self.required
    }
}

/// INVARIANT: The named token, minting it when no gateway has yet.
///
/// The gateway mints before it spawns any engine, so a fronted engine finds
/// that value and this writes nothing. Two engines mint anyway:
///
/// - A bare single-engine run has no gateway. Its build-watch reaches
///   `POST /api/v1/events/emit` through the CLI with no thread-bound origin
///   token, so the local token is the only identity it can present. With no
///   file, `api::mutating_gate` would refuse the engine's own rebuild.
/// - An engine that boots before its gateway does. A gateway re-exec re-adopts
///   running engines, so an engine that cached "absent" would refuse every
///   delivery until it restarted. The gateway reads what this one minted.
///
/// A mint that fails is logged with `consequence` and treated as absent. It is
/// never fatal: on the loopback bind that ships everywhere, nothing is refused
/// for want of it.
fn token_for_this_engine(name: &str, consequence: &str) -> Option<String> {
    match local_token::ensure_named(name) {
        Ok(token) => Some(token),
        Err(e) => {
            crate::log_legacy!(
                "[API] could not read or mint the private entry {name} ({e}), so {consequence}"
            );
            None
        }
    }
}

pub fn auth_required(choice: &BindChoice) -> bool {
    match choice {
        BindChoice::Loopback => false,
        BindChoice::All => true,
        BindChoice::Address(ip) => !ip.is_loopback(),
    }
}

pub fn is_public_path(path: &str) -> bool {
    if path.split('/').any(|seg| seg == ".." || seg == ".") {
        return false;
    }
    path == "/api/v1/health"
}

pub fn presented_scope(headers: &HeaderMap, auth: &EngineAuth) -> Option<EngineScope> {
    if let Some(expected) = auth.local.as_deref() {
        if presented(headers, local_token::HEADER_LOCAL_TOKEN)
            .is_some_and(|t| local_token::ct_eq(t, expected))
        {
            return Some(EngineScope::Full);
        }
    }
    if let Some(expected) = auth.hook_socket.as_deref() {
        if presented(headers, local_token::HEADER_HOOK_SOCKET_TOKEN)
            .is_some_and(|t| local_token::ct_eq(t, expected))
        {
            return Some(EngineScope::WebhookDelivery);
        }
    }
    None
}

static LOCAL_TOKEN: OnceLock<Option<String>> = OnceLock::new();

fn publish_local_token(token: Option<String>) {
    let _ = LOCAL_TOKEN.set(token);
}

pub(super) fn machine_local_token() -> Option<&'static str> {
    LOCAL_TOKEN.get().and_then(Option::as_deref)
}

/// INVARIANT: Is this caller a process on this machine, running as this user?
///
/// Attribution only. See the module note: it is asked on every bind, including
/// the loopback one where nothing is refused, and it names a machine rather
/// than a person.
pub fn is_local_process(headers: &HeaderMap) -> bool {
    carries_local_token(headers, LOCAL_TOKEN.get().and_then(Option::as_deref))
}

/// Does the machine-local token prove the workspace owner sent a request?
///
/// No, while every subprocess Lucidos starts runs as the owner with no OS
/// sandbox: any of them can read the token. The sandbox flips this back.
/// Temporary measure: `docs/temporary-measures.md` § Local token proves nothing.
///
/// The one site that gives the token an owner meaning. Read it only through
/// [`local_process_origin`].
const LOCAL_TOKEN_PROVES_OWNER: bool = false;

pub(crate) fn local_process_origin(user_agent: Option<String>) -> MessageOrigin {
    local_process_origin_under(LOCAL_TOKEN_PROVES_OWNER, user_agent)
}

fn local_process_origin_under(proves_owner: bool, user_agent: Option<String>) -> MessageOrigin {
    if proves_owner {
        MessageOrigin::Api {
            user_agent,
            mode: ActorMode::Engine,
            source_thread_id: None,
            emitting_trigger_id: None,
        }
    } else {
        MessageOrigin::UnattestedCaller {
            mode: ActorMode::Engine,
            user_agent,
            claimed_device_id: None,
            claimed_local_token: true,
            claimed_workspace: None,
            claimed_thread_id: None,
        }
    }
}

fn carries_local_token(headers: &HeaderMap, expected: Option<&str>) -> bool {
    expected.is_some_and(|want| {
        presented(headers, local_token::HEADER_LOCAL_TOKEN)
            .is_some_and(|got| local_token::ct_eq(got, want))
    })
}

#[cfg(test)]
pub fn publish_test_local_token() -> &'static str {
    const TEST_TOKEN: &str = "test-machine-local-token";
    publish_local_token(Some(TEST_TOKEN.to_string()));
    TEST_TOKEN
}

fn presented<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

pub fn scope_allows(scope: EngineScope, path: &str) -> bool {
    match scope {
        EngineScope::Full => true,
        EngineScope::WebhookDelivery => is_webhook_delivery_path(path),
    }
}

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

fn is_loopback_host(host: &str) -> bool {
    let literal: Option<std::net::IpAddr> =
        match host.strip_prefix('[').and_then(|h| h.strip_suffix(']')) {
            Some(v6) => v6.parse::<std::net::Ipv6Addr>().ok().map(Into::into),
            None => host.parse::<std::net::Ipv4Addr>().ok().map(Into::into),
        };
    if let Some(ip) = literal {
        return ip.to_canonical().is_loopback();
    }
    host.eq_ignore_ascii_case("localhost")
}

fn is_loopback_authority(authority: &str) -> bool {
    if authority.contains('@') {
        return false;
    }
    authority
        .parse::<axum::http::uri::Authority>()
        .is_ok_and(|a| is_loopback_host(a.host()))
}

fn foreign_authority(req: &axum::extract::Request) -> Option<String> {
    let hosts = req
        .headers()
        .get_all(axum::http::header::HOST)
        .iter()
        .map(|v| v.to_str().map_or_else(|_| format!("{v:?}"), str::to_string));
    let uri = req.uri().authority().map(|a| a.as_str().to_string());
    hosts
        .chain(uri)
        .find(|authority| !is_loopback_authority(authority))
}

/// INVARIANT: Refuse a request that proved nothing, or that reached past its scope.
///
/// Layered on the OUTERMOST router, so it runs before routing and before any
/// handler resolves a credential or touches the database.
pub async fn enforce(
    axum::extract::State(auth): axum::extract::State<std::sync::Arc<EngineAuth>>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    if !auth.is_required() {
        if let Some(authority) = foreign_authority(&req) {
            crate::log_legacy!(
                "[API] refused {}: a loopback engine does not answer for host {authority:?}",
                req.uri().path()
            );
            return (
                StatusCode::MISDIRECTED_REQUEST,
                "this engine answers only for a loopback host such as 127.0.0.1 or localhost. \
                 Reach it through the Lucidos gateway, or bind it wide and present the local token",
            )
                .into_response();
        }
        return next.run(req).await;
    }
    let path = req.uri().path();
    if is_public_path(path) {
        return next.run(req).await;
    }
    match presented_scope(req.headers(), &auth) {
        Some(scope) if scope_allows(scope, path) => next.run(req).await,
        Some(scope) => {
            crate::log_legacy!("[API] refused {path}: a {scope:?} credential does not reach it");
            (
                StatusCode::FORBIDDEN,
                "that credential does not reach this route",
            )
                .into_response()
        }
        None => {
            crate::log_legacy!(
                "[API] refused {path}: this engine faces a network and proved no caller"
            );
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
    fn the_local_token_switch_changes_only_the_origin() {
        let off = local_process_origin_under(false, Some("lucidos-cli".into()));
        let on = local_process_origin_under(true, Some("lucidos-cli".into()));
        assert_eq!(
            off,
            MessageOrigin::UnattestedCaller {
                mode: ActorMode::Engine,
                user_agent: Some("lucidos-cli".into()),
                claimed_device_id: None,
                claimed_local_token: true,
                claimed_workspace: None,
                claimed_thread_id: None,
            }
        );
        assert_eq!(
            on,
            MessageOrigin::Api {
                user_agent: Some("lucidos-cli".into()),
                mode: ActorMode::Engine,
                source_thread_id: None,
                emitting_trigger_id: None,
            }
        );
        assert_eq!(off.mode(), on.mode());
    }

    #[test]
    fn the_local_token_does_not_prove_the_owner_today() {
        assert!(matches!(
            local_process_origin(None),
            MessageOrigin::UnattestedCaller { .. }
        ));
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

    #[test]
    fn the_local_token_names_a_local_process() {
        let h = hm(&[(local_token::HEADER_LOCAL_TOKEN, "local-secret")]);
        assert!(carries_local_token(&h, Some("local-secret")));
        assert!(!carries_local_token(&h, Some("other-secret")));
    }

    #[test]
    fn nobody_is_local_without_both_a_secret_and_a_header() {
        let with = hm(&[(local_token::HEADER_LOCAL_TOKEN, "local-secret")]);
        let without = hm(&[]);
        assert!(!carries_local_token(&with, None));
        assert!(!carries_local_token(&without, Some("local-secret")));
        assert!(!carries_local_token(&without, None));
        let blank = hm(&[(local_token::HEADER_LOCAL_TOKEN, "   ")]);
        assert!(!carries_local_token(&blank, Some("local-secret")));
    }

    #[test]
    fn the_hook_socket_token_never_names_a_local_process() {
        let h = hm(&[(local_token::HEADER_HOOK_SOCKET_TOKEN, "hook-secret")]);
        assert!(!carries_local_token(&h, Some("hook-secret")));
    }

    #[test]
    fn the_local_token_proves_full_authority() {
        let auth = EngineAuth::locked_for_tests(Some("local-secret"), Some("hook-secret"));
        let h = hm(&[(local_token::HEADER_LOCAL_TOKEN, "local-secret")]);
        assert_eq!(presented_scope(&h, &auth), Some(EngineScope::Full));
    }

    #[test]
    fn the_hook_socket_token_proves_only_its_own_scope() {
        let auth = EngineAuth::locked_for_tests(Some("local-secret"), Some("hook-secret"));
        let h = hm(&[(local_token::HEADER_HOOK_SOCKET_TOKEN, "hook-secret")]);
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
            (local_token::HEADER_LOCAL_TOKEN, "hook-secret"),
            (local_token::HEADER_HOOK_SOCKET_TOKEN, "local-secret"),
            (local_token::HEADER_HOOK_SOCKET_TOKEN, ""),
            ("x-lucidos-webhook-token", "hook-secret"),
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
        let auth = EngineAuth::locked_for_tests(None, None);
        assert_eq!(presented_scope(&hm(&[]), &auth), None);
        for name in [
            local_token::HEADER_LOCAL_TOKEN,
            local_token::HEADER_HOOK_SOCKET_TOKEN,
        ] {
            assert_eq!(presented_scope(&hm(&[(name, "")]), &auth), None);
            assert_eq!(presented_scope(&hm(&[(name, "anything")]), &auth), None);
        }
    }

    #[test]
    fn a_device_id_header_authorizes_nothing_here_either() {
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
                "the hook socket token must not reach {path}"
            );
        }
    }

    #[test]
    fn an_open_door_is_open_and_a_locked_one_says_so() {
        assert!(!EngineAuth::open_for_tests().is_required());
        assert!(EngineAuth::locked_for_tests(Some("x"), None).is_required());
    }

    #[test]
    fn every_loopback_name_is_admitted_on_any_port() {
        for authority in [
            "127.0.0.1",
            "127.0.0.1:5251",
            "127.8.9.10:80",
            "localhost",
            "localhost:5173",
            "LocalHost:5173",
            "[::1]",
            "[::FFFF:127.0.0.1]",
            "[::1]:5251",
            "[::ffff:127.0.0.1]:5251",
        ] {
            assert!(is_loopback_authority(authority), "{authority} must pass");
        }
    }

    #[test]
    fn a_look_alike_name_is_refused() {
        for authority in [
            "rebind.example",
            "rebind.example:5251",
            "localhost.attacker.example",
            "127.0.0.1.attacker.example",
            "attacker-localhost",
            "app.localhost:5251",
            "a.b.localhost",
            "localhost.",
            ".localhost",
            "a..localhost",
            "0.0.0.0:5251",
            "[::]:5251",
            "192.168.1.10:5251",
            TAILNET,
            "user@localhost",
            "",
            "local host",
        ] {
            assert!(!is_loopback_authority(authority), "{authority} must fail");
        }
    }

    const MOUNTS: &[&str] = &[
        "/api/v1/threads/list",
        "/app/habit-tracker/",
        "/data/artifacts/notes.md",
        "/",
    ];

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
        use futures::StreamExt as _;

        let (release, released) = tokio::sync::oneshot::channel::<()>();
        let stream = futures::stream::once(async { Ok::<_, std::io::Error>("first") }).chain(
            futures::stream::once(async move {
                let _ = released.await;
                Ok::<_, std::io::Error>("second")
            }),
        );
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
    async fn a_hook_socket_token_delivers_and_can_do_nothing_else() {
        let hdr = &[(local_token::HEADER_HOOK_SOCKET_TOKEN, "hook-secret")];
        assert_eq!(
            call(locked(), "/api/v1/webhooks/abc/deliver", hdr).await,
            StatusCode::IM_A_TEAPOT
        );
        for path in MOUNTS {
            assert_eq!(
                call(locked(), path, hdr).await,
                StatusCode::FORBIDDEN,
                "the hook socket token must not reach {path}"
            );
        }
    }

    const REBOUND: &[(&str, &str)] = &[
        ("host", "rebind.example:5251"),
        ("sec-fetch-site", "same-origin"),
    ];

    #[tokio::test]
    async fn a_loopback_engine_refuses_a_rebound_page_on_every_mount() {
        for path in MOUNTS.iter().chain(&["/api/v1/devices/register"]) {
            assert_eq!(
                call(EngineAuth::open_for_tests(), path, REBOUND).await,
                StatusCode::MISDIRECTED_REQUEST,
                "{path} must refuse a foreign Host on a loopback bind"
            );
        }
    }

    #[tokio::test]
    async fn a_loopback_engine_admits_every_loopback_caller() {
        for host in [
            "127.0.0.1:5251",
            "localhost:5251",
            "[::1]:5251",
            "localhost",
        ] {
            for path in MOUNTS {
                assert_eq!(
                    call(EngineAuth::open_for_tests(), path, &[("host", host)]).await,
                    StatusCode::IM_A_TEAPOT,
                    "{host} must reach {path}"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_second_host_header_cannot_smuggle_a_foreign_name() {
        let hdrs = &[("host", "localhost:5251"), ("host", "rebind.example:5251")];
        assert_eq!(
            call(EngineAuth::open_for_tests(), "/", hdrs).await,
            StatusCode::MISDIRECTED_REQUEST
        );
    }

    #[tokio::test]
    async fn a_foreign_uri_authority_is_refused_beside_a_loopback_host() {
        let uri = "http://rebind.example:5251/api/v1/threads/list";
        assert_eq!(
            call(
                EngineAuth::open_for_tests(),
                uri,
                &[("host", "127.0.0.1:5251")]
            )
            .await,
            StatusCode::MISDIRECTED_REQUEST
        );
        let loopback_uri = "http://127.0.0.1:5251/api/v1/threads/list";
        assert_eq!(
            call(EngineAuth::open_for_tests(), loopback_uri, &[]).await,
            StatusCode::IM_A_TEAPOT
        );
    }

    #[tokio::test]
    async fn a_wide_engine_lets_the_credential_decide_whatever_the_host() {
        let token = (local_token::HEADER_LOCAL_TOKEN, "local-secret");
        for path in MOUNTS {
            assert_eq!(
                call(locked(), path, &[REBOUND[0], token]).await,
                StatusCode::IM_A_TEAPOT,
                "the token must reach {path} under any Host"
            );
            assert_eq!(
                call(locked(), path, REBOUND).await,
                StatusCode::UNAUTHORIZED,
                "no credential stays a 401 at {path}, not a 421"
            );
        }
    }
}
