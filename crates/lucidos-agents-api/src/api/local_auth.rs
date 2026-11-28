//! INVARIANT: # A loopback peer address proves nothing
//!
//! [`is_local_process`] reads no peer address, exactly as the gateway's
//! `authorize` does not. `tailscale serve` proxies from this machine, so a
//! phone's request arrives with a loopback source. Trusting the source address
//! would trust the whole tailnet.

//! INVARIANT: # Why the whole surface
//!
//! The engine binds loopback only (ADR 0155, amended). A page on a rebound DNS
//! name still reaches a loopback port. So the Host gate wraps the OUTERMOST
//! router: `/api/v1`, `/app`, `/data` and the frontend fallback.
//! `/data` alone serves the workspace's files off disk.

use crate::engine::thread_events::{ActorMode, MessageOrigin};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use lucidos_local_token as local_token;
use std::sync::OnceLock;

/// Read or mint the local token, and publish it for [`is_local_process`].
///
/// Read ONCE at startup rather than per request: nothing replaces a
/// published token, so no later read could find a fresher value. The hook
/// socket token is only read, never minted. The gateway still sends it on
/// webhook deliveries, so the diagnostic log must redact it.
pub fn publish_this_engines_local_token() {
    let local = token_for_this_engine(
        local_token::LOCAL_TOKEN_FILE,
        "no caller can prove it is a local process",
    );
    let hook_socket = local_token::read_named(local_token::HOOK_SOCKET_TOKEN_FILE);
    crate::diagnostic_log::register_secret_values(local.iter().chain(&hook_socket).cloned());
    publish_local_token(local);
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
/// - An engine that boots before its gateway does. The gateway reads what
///   this one minted.
///
/// A mint that fails is logged with `consequence` and treated as absent. It is
/// never fatal: the loopback engine refuses nothing for want of it.
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

static LOCAL_TOKEN: OnceLock<Option<String>> = OnceLock::new();

fn publish_local_token(token: Option<String>) {
    let _ = LOCAL_TOKEN.set(token);
}

pub(super) fn machine_local_token() -> Option<&'static str> {
    LOCAL_TOKEN.get().and_then(Option::as_deref)
}

/// INVARIANT: Is this caller a process on this machine, running as this user?
///
/// Attribution only: nothing is refused for want of it, and it names a
/// machine rather than a person.
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

/// INVARIANT: Refuse a request addressed to a host other than loopback.
///
/// Layered on the OUTERMOST router, so it runs before routing and before any
/// handler resolves a credential or touches the database.
pub async fn enforce(req: axum::extract::Request, next: axum::middleware::Next) -> Response {
    if let Some(authority) = foreign_authority(&req) {
        crate::log_legacy!(
            "[API] refused {}: a loopback engine does not answer for host {authority:?}",
            req.uri().path()
        );
        return (
            StatusCode::MISDIRECTED_REQUEST,
            "this engine answers only for a loopback host such as 127.0.0.1 or localhost. \
             Reach it through the Lucidos gateway",
        )
            .into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn gated() -> axum::Router {
        axum::Router::new()
            .fallback(|| async { StatusCode::IM_A_TEAPOT })
            .layer(axum::middleware::from_fn(enforce))
    }

    async fn call(path: &str, hdrs: &[(&str, &str)]) -> StatusCode {
        use tower::ServiceExt as _;
        let mut builder = axum::extract::Request::builder().uri(path);
        for (k, v) in hdrs {
            builder = builder.header(*k, *v);
        }
        gated()
            .oneshot(builder.body(axum::body::Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn a_loopback_caller_needs_no_credential() {
        for path in MOUNTS {
            assert_eq!(
                call(path, &[]).await,
                StatusCode::IM_A_TEAPOT,
                "{path} must stay open on a loopback bind"
            );
        }
    }

    #[tokio::test]
    async fn the_door_streams_a_body_instead_of_collecting_it() {
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
            .layer(axum::middleware::from_fn(enforce));

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

    const REBOUND: &[(&str, &str)] = &[
        ("host", "rebind.example:5251"),
        ("sec-fetch-site", "same-origin"),
    ];

    #[tokio::test]
    async fn a_loopback_engine_refuses_a_rebound_page_on_every_mount() {
        for path in MOUNTS.iter().chain(&["/api/v1/devices/register"]) {
            assert_eq!(
                call(path, REBOUND).await,
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
                    call(path, &[("host", host)]).await,
                    StatusCode::IM_A_TEAPOT,
                    "{host} must reach {path}"
                );
            }
        }
    }

    #[tokio::test]
    async fn a_second_host_header_cannot_smuggle_a_foreign_name() {
        let hdrs = &[("host", "localhost:5251"), ("host", "rebind.example:5251")];
        assert_eq!(call("/", hdrs).await, StatusCode::MISDIRECTED_REQUEST);
    }

    #[tokio::test]
    async fn a_foreign_uri_authority_is_refused_beside_a_loopback_host() {
        let uri = "http://rebind.example:5251/api/v1/threads/list";
        assert_eq!(
            call(uri, &[("host", "127.0.0.1:5251")]).await,
            StatusCode::MISDIRECTED_REQUEST
        );
        let loopback_uri = "http://127.0.0.1:5251/api/v1/threads/list";
        assert_eq!(call(loopback_uri, &[]).await, StatusCode::IM_A_TEAPOT);
    }
}
