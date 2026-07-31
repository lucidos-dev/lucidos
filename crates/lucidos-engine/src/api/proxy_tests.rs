use super::*;
use axum::http::HeaderName;
use axum::routing::any;
use axum::Router;

fn hm(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut h = HeaderMap::new();
    for (n, v) in pairs {
        h.append(
            HeaderName::from_bytes(n.as_bytes()).unwrap(),
            HeaderValue::from_str(v).unwrap(),
        );
    }
    h
}

fn name(s: &str) -> HeaderName {
    HeaderName::from_bytes(s.as_bytes()).unwrap()
}

// ---- Header stripping --------------------------------------------------

#[test]
fn strips_hop_by_hop_headers() {
    for h in [
        "Connection",
        "Keep-Alive",
        "Proxy-Authenticate",
        "Proxy-Authorization",
        "TE",
        "Trailer",
        "Transfer-Encoding",
        "Upgrade",
    ] {
        assert!(
            should_strip_request_header(&name(h)),
            "expected {} to be stripped",
            h
        );
    }
}

#[test]
fn strips_host_cookie_origin_referer() {
    for h in ["Host", "Cookie", "Origin", "Referer"] {
        assert!(
            should_strip_request_header(&name(h)),
            "expected {} to be stripped",
            h
        );
    }
}

#[test]
fn strip_check_is_case_insensitive() {
    assert!(should_strip_request_header(&name("cookie")));
    assert!(should_strip_request_header(&name("HOST")));
    assert!(should_strip_request_header(&name("connection")));
}

#[test]
fn keeps_safe_headers() {
    for h in [
        "Content-Type",
        "Accept",
        "Accept-Language",
        "User-Agent",
        "X-Custom-Header",
    ] {
        assert!(
            !should_strip_request_header(&name(h)),
            "expected {} to be kept",
            h
        );
    }
}

#[test]
fn filter_request_headers_drops_stripped_keeps_others() {
    let input = hm(&[
        ("Content-Type", "application/json"),
        ("Cookie", "session=abc"),
        ("Host", "engine.example.com"),
        ("Origin", "https://engine.example.com"),
        ("Referer", "https://engine.example.com/"),
        ("Accept", "application/json"),
        ("X-Custom", "ok"),
    ]);
    let out = filter_request_headers(&input);
    assert!(out.contains_key("content-type"));
    assert!(out.contains_key("accept"));
    assert!(out.contains_key("x-custom"));
    assert!(!out.contains_key("cookie"));
    assert!(!out.contains_key("host"));
    assert!(!out.contains_key("origin"));
    assert!(!out.contains_key("referer"));
}

// ---- Browser-origin guard ---------------------------------------------

#[test]
fn browser_proxy_guard_allows_non_browser_clients() {
    assert!(browser_proxy_request_allowed(&HeaderMap::new()));
}

#[test]
fn browser_proxy_guard_allows_same_origin_browser_requests() {
    let h = hm(&[
        ("Host", "localhost:5251"),
        ("Origin", "http://localhost:5251"),
        ("Sec-Fetch-Site", "same-origin"),
    ]);
    assert!(browser_proxy_request_allowed(&h));
}

#[test]
fn browser_proxy_guard_rejects_cross_site_fetch_metadata() {
    let h = hm(&[
        ("Host", "localhost:5251"),
        ("Origin", "https://evil.example"),
        ("Sec-Fetch-Site", "cross-site"),
    ]);
    assert!(!browser_proxy_request_allowed(&h));
}

#[test]
fn browser_proxy_guard_rejects_foreign_origin_without_fetch_metadata() {
    // Older browsers may omit Sec-Fetch-* but still send Origin on POSTs.
    // A hostile page can issue the request even though CORS prevents reading
    // the response, so Origin must still match Host for credentialed proxy
    // routes.
    let h = hm(&[
        ("Host", "localhost:5251"),
        ("Origin", "https://evil.example"),
    ]);
    assert!(!browser_proxy_request_allowed(&h));
}

#[test]
fn browser_proxy_guard_rejects_same_site_but_not_same_origin() {
    let h = hm(&[
        ("Host", "localhost:5251"),
        ("Origin", "http://localhost:5252"),
        ("Sec-Fetch-Site", "same-site"),
    ]);
    assert!(!browser_proxy_request_allowed(&h));
}

#[test]
fn browser_proxy_guard_allows_http2_same_origin_without_host_header() {
    // The bug being fixed: a direct-to-engine HTTP/2 request (iOS PWA, no
    // gateway) carries the authority in the `:authority` pseudo-header, so
    // there's NO Host header, and no x-forwarded-host either. Sec-Fetch-Site
    // = same-origin already proves it's safe, so it must be ALLOWED even with
    // nothing to reconstruct a host from.
    let h = hm(&[
        ("Origin", "https://localhost:5174"),
        ("Sec-Fetch-Site", "same-origin"),
    ]);
    assert!(browser_proxy_request_allowed(&h));
}

#[test]
fn browser_proxy_guard_sec_fetch_same_origin_wins_over_mismatched_origin() {
    // Sec-Fetch-Site is authoritative and unforgeable: when it says
    // same-origin, the request is allowed even if Origin/Host would NOT match
    // (behind a reverse proxy the engine's Host is the internal address, so
    // Origin != Host is normal). No host comparison is performed at all.
    let h = hm(&[
        ("Host", "127.0.0.1:51811"),
        ("Origin", "https://localhost:5251"),
        ("Sec-Fetch-Site", "same-origin"),
    ]);
    assert!(browser_proxy_request_allowed(&h));
}

#[test]
fn browser_proxy_guard_legacy_allows_origin_matching_host() {
    // Pre-Fetch-Metadata browser (no Sec-Fetch-*) still sends Origin on a
    // same-origin POST. Those are HTTP/1.1 with a Host header, so the legacy
    // Origin == Host comparison allows it.
    let h = hm(&[
        ("Host", "localhost:5173"),
        ("Origin", "http://localhost:5173"),
    ]);
    assert!(browser_proxy_request_allowed(&h));
}

#[test]
fn origin_authority_matches_host_normalizes_default_ports() {
    assert!(origin_authority_matches_host(
        "https://localhost",
        "localhost:443"
    ));
    assert!(origin_authority_matches_host(
        "https://localhost",
        "localhost"
    ));
    assert!(origin_authority_matches_host(
        "http://LOCALHOST:80",
        "localhost:80"
    ));
    assert!(origin_authority_matches_host(
        "http://LOCALHOST:80",
        "localhost"
    ));
    assert!(!origin_authority_matches_host(
        "https://localhost",
        "localhost:444"
    ));
}

// Auth header building (Bearer/ApiKey/Basic) moved to AuthLayer impls
// See proxy_static_layers tests for equivalent coverage.

// ---- Path traversal ---------------------------------------------------

#[test]
fn has_traversal_flags_dot_dot_segments() {
    assert!(has_traversal("../etc/passwd"));
    assert!(has_traversal("foo/../bar"));
    assert!(has_traversal("a/b/../../c"));
    assert!(has_traversal("/.."));
}

#[test]
fn has_traversal_flags_backslashes() {
    assert!(has_traversal("foo\\..\\bar"));
    assert!(has_traversal("a\\b"));
}

// ---- Redirect helpers --------------------------------------------

#[test]
fn origin_of_extracts_lowercased_scheme_host_and_default_port() {
    assert_eq!(
        origin_of("https://API.Example.com/path"),
        Some((
            "https".to_string(),
            "api.example.com".to_string(),
            Some(443)
        ))
    );
    assert_eq!(
        origin_of("http://localhost:8080/x"),
        Some(("http".to_string(), "localhost".to_string(), Some(8080)))
    );
}

#[test]
fn origin_of_returns_none_for_unparseable_url() {
    assert!(origin_of("not a url").is_none());
}

#[test]
fn origin_of_treats_explicit_default_port_as_equal_to_implicit() {
    // `https://h` and `https://h:443` are the same origin — port_or_known_default
    // normalizes both to 443 so a redirect that only restates the default port
    // isn't spuriously refused.
    assert_eq!(origin_of("https://h"), origin_of("https://h:443"));
}

#[test]
fn origin_of_distinguishes_scheme_downgrade() {
    // The security-critical case: a `https → http` redirect to the SAME host is
    // a different origin, so the auth pipeline must refuse to re-sign (the
    // credential would otherwise go out over plaintext).
    assert_ne!(origin_of("https://h/x"), origin_of("http://h/x"));
}

#[test]
fn origin_of_distinguishes_port() {
    // Same host, different port (e.g. an internal admin service) is a different
    // origin — credentials bound to :443 must not follow a redirect to :8080.
    assert_ne!(origin_of("https://h:443/x"), origin_of("https://h:8080/x"));
}

#[test]
fn resolve_redirect_handles_absolute_target() {
    let url = resolve_redirect_location("https://example.com/a/b", "https://example.com/c/d?q=1")
        .unwrap();
    assert_eq!(url.as_str(), "https://example.com/c/d?q=1");
}

#[test]
fn resolve_redirect_handles_relative_target() {
    let url = resolve_redirect_location("https://example.com/a/b", "/c/d").unwrap();
    assert_eq!(url.as_str(), "https://example.com/c/d");
}

#[test]
fn resolve_redirect_handles_protocol_relative_target() {
    let url = resolve_redirect_location("https://example.com/a", "//other.com/x").unwrap();
    assert_eq!(url.as_str(), "https://other.com/x");
}

#[test]
fn is_redirect_status_covers_30x_we_follow() {
    assert!(is_redirect_status(StatusCode::MOVED_PERMANENTLY));
    assert!(is_redirect_status(StatusCode::FOUND));
    assert!(is_redirect_status(StatusCode::SEE_OTHER));
    assert!(is_redirect_status(StatusCode::TEMPORARY_REDIRECT));
    assert!(is_redirect_status(StatusCode::PERMANENT_REDIRECT));
    // 304 is not really a redirect — we don't follow.
    assert!(!is_redirect_status(StatusCode::NOT_MODIFIED));
    assert!(!is_redirect_status(StatusCode::OK));
    assert!(!is_redirect_status(StatusCode::BAD_GATEWAY));
}

#[test]
fn has_traversal_passes_normal_paths() {
    assert!(!has_traversal(""));
    assert!(!has_traversal("/Spisestua/play"));
    assert!(!has_traversal("api/v1/items?id=42"));
    // A literal segment that *contains* `..` but isn't `..` is fine.
    assert!(!has_traversal("foo..bar"));
}

// ---- URL building -----------------------------------------------------

#[test]
fn build_url_handles_no_trailing_no_leading() {
    assert_eq!(
        build_target_url("http://localhost:5005", "Spisestua/play", None),
        "http://localhost:5005/Spisestua/play"
    );
}

#[test]
fn build_url_handles_trailing_and_leading_slashes() {
    assert_eq!(
        build_target_url("http://localhost:5005/", "/Spisestua/play", None),
        "http://localhost:5005/Spisestua/play"
    );
}

#[test]
fn build_url_empty_path_omits_slash() {
    assert_eq!(
        build_target_url("http://localhost:5005", "", None),
        "http://localhost:5005"
    );
}

#[test]
fn build_url_includes_query_string() {
    assert_eq!(
        build_target_url(
            "http://api.example.com",
            "/v1/items",
            Some("limit=10&page=2")
        ),
        "http://api.example.com/v1/items?limit=10&page=2"
    );
}

#[test]
fn build_url_ignores_empty_query() {
    assert_eq!(
        build_target_url("http://api.example.com", "/v1/items", Some("")),
        "http://api.example.com/v1/items"
    );
}

// ---- Query-param appending --------------------------------------------

#[test]
fn append_query_param_to_url_with_no_existing_query() {
    let url = append_query_param("https://api.example.com/v1/x", "api-key", "secret-123");
    assert_eq!(url, "https://api.example.com/v1/x?api-key=secret-123");
}

#[test]
fn append_query_param_to_url_with_existing_query() {
    let url = append_query_param(
        "https://api.example.com/v1/x?limit=10&page=2",
        "api-key",
        "secret-123",
    );
    assert_eq!(
        url,
        "https://api.example.com/v1/x?limit=10&page=2&api-key=secret-123"
    );
}

#[test]
fn append_query_param_url_encodes_value_with_special_chars() {
    let url = append_query_param("https://x/y", "k", "a&b=c d");
    assert_eq!(url, "https://x/y?k=a%26b%3Dc%20d");
}

#[test]
fn append_query_param_redacted_form_does_not_contain_credential() {
    // Mirrors what apply_auth does for ProxyAuth::QueryParam to build
    // log_url: same key, value replaced with REDACTED. Guards against
    // the credential leaking into log lines.
    let base = "https://api.example.com/v1/x";
    let real = append_query_param(base, "api-key", "actual-secret-value");
    let redacted = append_query_param(base, "api-key", "REDACTED");
    assert!(real.contains("actual-secret-value"));
    assert!(!redacted.contains("actual-secret-value"));
    assert_eq!(redacted, "https://api.example.com/v1/x?api-key=REDACTED");
}

// ---- HMAC signing ------------------------------------------------------

#[test]
fn hmac_sha256_known_vector() {
    // RFC 4231 test case 1: key = 0x0b * 20, data = "Hi There"
    let key = [0x0bu8; 20];
    let sig = compute_hmac_hex(HmacAlgorithm::Sha256, &key, b"Hi There");
    assert_eq!(
        sig,
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
}

#[test]
fn hmac_sha512_known_vector() {
    // RFC 4231 test case 1
    let key = [0x0bu8; 20];
    let sig = compute_hmac_hex(HmacAlgorithm::Sha512, &key, b"Hi There");
    assert_eq!(
        sig,
        "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cdedaa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854"
    );
}

// Query-string assembly (sign_query_string) used to live here as a
// standalone helper; with HmacSignedLayer doing the assembly inline,
// the equivalent tests now live in proxy_hmac_layer (sign_with*).

#[test]
fn hmac_signature_matches_binance_known_example() {
    // Binance's published worked example (SIGNED endpoint test):
    //   secret = "NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j"
    //   query  = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC&quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559"
    //   sig    = "c8db56825ae71d6d79447849e617115f4a920fa2acdcab2b053c4b2838bd6b71"
    let secret = b"NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j";
    let query = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC&quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559";
    let sig = compute_hmac_hex(HmacAlgorithm::Sha256, secret, query.as_bytes());
    assert_eq!(
        sig,
        "c8db56825ae71d6d79447849e617115f4a920fa2acdcab2b053c4b2838bd6b71"
    );
}

// ---- Config loader ----------------------------------------------------

#[test]
fn load_config_returns_empty_when_file_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = load_proxy_config(tmp.path()).unwrap();
    assert!(cfg.is_empty());
}

#[test]
fn load_config_parses_basic_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_dir = tmp.path().join("data/config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("apis.json"),
        r#"{"sonos": {"base_url": "http://localhost:5005"}}"#,
    )
    .unwrap();
    let cfg = load_proxy_config(tmp.path()).unwrap();
    assert_eq!(cfg.len(), 1);
    let sonos = cfg.get("sonos").unwrap();
    assert_eq!(sonos.base_url, "http://localhost:5005");
    assert!(sonos.auth.is_none());
}

// Legacy `auth.type` (Bearer/ApiKey/Basic/QueryParam/HmacSigned/
// ScriptHandshake) parsing tests + ScriptHandshake-cache integration
// were deleted with the legacy `ProxyAuth` enum. Equivalent coverage:
//   - on-disk pipeline shape: proxy_pipeline_config tests
//   - per-layer behavior: proxy_static_layers / proxy_hmac_layer /
//     proxy_script_layer / proxy_wasm_signer tests
//   - 401 retry decision: proxy_pipeline retry-truth-table tests
//   - removed credential_bundle: proxy_migration negative-guard test
//   - upgrade migration of legacy apis.json: proxy_migration tests

#[test]
fn load_config_parses_pipeline_shape_with_static_credential_layer() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_dir = tmp.path().join("data/config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("apis.json"),
        r#"{
                "comfort": {
                    "base_url": "https://accsmart.panasonic.com",
                    "auth": {"pipeline": [
                        {"type": "static_credential", "kind": "bearer", "credential": "comfort-cloud"}
                    ]}
                }
            }"#,
    )
    .unwrap();
    let cfg = load_proxy_config(tmp.path()).unwrap();
    let comfort = cfg.get("comfort").unwrap();
    let pipeline = comfort.auth.as_ref().unwrap();
    assert_eq!(pipeline.pipeline.len(), 1);
}

#[test]
fn load_config_rejects_unknown_layer_type_in_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_dir = tmp.path().join("data/config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("apis.json"),
        r#"{"foo": {"base_url": "https://x", "auth": {"pipeline": [
                {"type": "bearrer", "credential": "k"}
            ]}}}"#,
    )
    .unwrap();
    // Typo `bearrer` must surface at config-load time.
    assert!(load_proxy_config(tmp.path()).is_err());
}

#[test]
fn load_config_rejects_script_handshake_with_traversal_path_in_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_dir = tmp.path().join("data/config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("apis.json"),
        r#"{"x": {"base_url": "https://x", "auth": {"pipeline": [
                {"type": "script_handshake", "credential": "x", "script": "../../../etc/passwd"}
            ]}}}"#,
    )
    .unwrap();
    assert!(load_proxy_config(tmp.path()).is_err());
}

#[test]
fn load_config_rejects_script_handshake_with_absolute_path_in_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_dir = tmp.path().join("data/config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join("apis.json"),
        r#"{"x": {"base_url": "https://x", "auth": {"pipeline": [
                {"type": "script_handshake", "credential": "x", "script": "/etc/passwd"}
            ]}}}"#,
    )
    .unwrap();
    assert!(load_proxy_config(tmp.path()).is_err());
}

#[test]
fn load_config_invalid_json_returns_err() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg_dir = tmp.path().join("data/config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("apis.json"), "{ not json").unwrap();
    assert!(load_proxy_config(tmp.path()).is_err());
}

// ---- Integration tests with a tiny upstream server --------------------
//
// For these, we spin up an axum server on 127.0.0.1:0 (random port) and
// exercise `forward_request` directly — no AppState/database needed.

use std::sync::Arc;
use std::sync::Mutex;

/// Records what the upstream observed for assertion.
#[derive(Default, Clone)]
struct UpstreamRecord {
    method: String,
    path: String,
    query: String,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
}

type RecordSlot = Arc<Mutex<Option<UpstreamRecord>>>;

/// Spawn an upstream that records the incoming request and replies with
/// `status` and `body`. Returns `(base_url, slot)`.
async fn spawn_recording_upstream(status: u16, body: &'static str) -> (String, RecordSlot) {
    let slot: RecordSlot = Arc::new(Mutex::new(None));
    let slot_clone = slot.clone();
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let shutdown_from_handler = shutdown.clone();
    let app = Router::new().fallback(any(move |req: axum::extract::Request| {
        let slot = slot_clone.clone();
        let shutdown = shutdown_from_handler.clone();
        async move {
            let method = req.method().to_string();
            let path = req.uri().path().to_string();
            let query = req.uri().query().unwrap_or("").to_string();
            let headers: Vec<(String, String)> = req
                .headers()
                .iter()
                .map(|(n, v)| (n.as_str().to_string(), v.to_str().unwrap_or("").to_string()))
                .collect();
            let req_body = axum::body::to_bytes(req.into_body(), 1024 * 1024)
                .await
                .unwrap_or_default()
                .to_vec();
            *slot.lock().unwrap() = Some(UpstreamRecord {
                method,
                path,
                query,
                body: req_body,
                headers,
            });
            shutdown.notify_one();
            (
                StatusCode::from_u16(status).unwrap_or(StatusCode::OK),
                body.to_string(),
            )
        }
    }));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown.notified().await;
            })
            .await
            .unwrap();
    });
    (format!("http://{}", addr), slot)
}

/// Read the full body of a `Response` into a `String`.
async fn body_text(resp: Response) -> String {
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024)
        .await
        .unwrap();
    String::from_utf8_lossy(&bytes).to_string()
}

async fn run_method_test(method: Method, body: &str) {
    let (base, slot) = spawn_recording_upstream(200, "ok").await;
    let url = format!("{}/path/sub", base);
    let resp = forward_request(
        method.clone(),
        &url,
        &url,
        HeaderMap::new(),
        Vec::new(),
        Bytes::copy_from_slice(body.as_bytes()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let recorded = slot.lock().unwrap().clone().expect("upstream not called");
    assert_eq!(recorded.method, method.as_str());
    assert_eq!(recorded.path, "/path/sub");
    assert_eq!(recorded.body, body.as_bytes());
}

#[tokio::test]
async fn forwards_get() {
    run_method_test(Method::GET, "").await;
}

#[tokio::test]
async fn forwards_post_with_body() {
    run_method_test(Method::POST, r#"{"hello":"world"}"#).await;
}

#[tokio::test]
async fn forwards_put_with_body() {
    run_method_test(Method::PUT, "raw bytes").await;
}

#[tokio::test]
async fn forwards_delete() {
    run_method_test(Method::DELETE, "").await;
}

#[tokio::test]
async fn forwards_patch_with_body() {
    run_method_test(Method::PATCH, "patch-body").await;
}

#[tokio::test]
async fn upstream_does_not_see_stripped_headers() {
    let (base, slot) = spawn_recording_upstream(200, "ok").await;
    let url = format!("{}/x", base);
    let headers = hm(&[
        ("Cookie", "session=abc"),
        ("Origin", "https://engine.local"),
        ("Referer", "https://engine.local/"),
        ("X-Keep-Me", "yes"),
    ]);
    let _ = forward_request(Method::GET, &url, &url, headers, Vec::new(), Bytes::new()).await;
    let recorded = slot.lock().unwrap().clone().unwrap();
    let observed: Vec<&str> = recorded.headers.iter().map(|(n, _)| n.as_str()).collect();
    assert!(!observed.iter().any(|n| n.eq_ignore_ascii_case("cookie")));
    assert!(!observed.iter().any(|n| n.eq_ignore_ascii_case("origin")));
    assert!(!observed.iter().any(|n| n.eq_ignore_ascii_case("referer")));
    assert!(observed.iter().any(|n| n.eq_ignore_ascii_case("x-keep-me")));
}

#[tokio::test]
async fn upstream_does_not_see_host_header_from_engine() {
    // We forward the request with no Host header — reqwest sets one from
    // the URL (i.e. the upstream's host, not the engine's). This proves
    // a Host: engine.example.com sent by the browser doesn't bleed through.
    let (base, slot) = spawn_recording_upstream(200, "ok").await;
    let url = format!("{}/x", base);
    let headers = hm(&[("Host", "engine.example.com")]);
    let _ = forward_request(Method::GET, &url, &url, headers, Vec::new(), Bytes::new()).await;
    let recorded = slot.lock().unwrap().clone().unwrap();
    let host = recorded
        .headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("host"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    // The upstream's Host header (set by reqwest) should be the upstream's
    // address — it must not be `engine.example.com`.
    assert_ne!(host, "engine.example.com");
}

#[tokio::test]
async fn forwards_arbitrary_auth_headers_to_upstream() {
    // Smoke-tests forward_request's auth_headers parameter. The actual
    // header construction lives in BearerLayer / ApiKeyLayer (with
    // their own tests); this confirms forward_request actually puts
    // them on the wire.
    let (base, slot) = spawn_recording_upstream(200, "ok").await;
    let url = format!("{}/x", base);
    let auth_vec: Vec<(HeaderName, HeaderValue)> = vec![
        (
            HeaderName::from_static("authorization"),
            HeaderValue::from_static("Bearer tok-xyz"),
        ),
        (
            HeaderName::from_static("x-api-key"),
            HeaderValue::from_static("secret-key"),
        ),
    ];
    let _ = forward_request(
        Method::GET,
        &url,
        &url,
        HeaderMap::new(),
        auth_vec,
        Bytes::new(),
    )
    .await;
    let recorded = slot.lock().unwrap().clone().unwrap();
    let authz = recorded
        .headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("authorization"))
        .map(|(_, v)| v.as_str());
    let key = recorded
        .headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("x-api-key"))
        .map(|(_, v)| v.as_str());
    assert_eq!(authz, Some("Bearer tok-xyz"));
    assert_eq!(key, Some("secret-key"));
}

#[tokio::test]
async fn forwards_query_param_auth_to_upstream() {
    let (base, slot) = spawn_recording_upstream(200, "ok").await;
    let url = append_query_param(&format!("{}/v1/items", base), "api-key", "secret-123");
    let _ = forward_request(
        Method::GET,
        &url,
        &url,
        HeaderMap::new(),
        Vec::new(),
        Bytes::new(),
    )
    .await;
    let recorded = slot.lock().unwrap().clone().unwrap();
    assert_eq!(recorded.path, "/v1/items");
    assert_eq!(recorded.query, "api-key=secret-123");
}

#[tokio::test]
async fn forwards_query_param_auth_preserves_existing_query() {
    let (base, slot) = spawn_recording_upstream(200, "ok").await;
    let url = append_query_param(
        &format!("{}/v1/items?limit=10", base),
        "api-key",
        "secret-123",
    );
    let _ = forward_request(
        Method::GET,
        &url,
        &url,
        HeaderMap::new(),
        Vec::new(),
        Bytes::new(),
    )
    .await;
    let recorded = slot.lock().unwrap().clone().unwrap();
    assert_eq!(recorded.query, "limit=10&api-key=secret-123");
}

#[tokio::test]
async fn upstream_5xx_passes_through() {
    let (base, _slot) = spawn_recording_upstream(503, "down").await;
    let url = format!("{}/x", base);
    let resp = forward_request(
        Method::GET,
        &url,
        &url,
        HeaderMap::new(),
        Vec::new(),
        Bytes::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body_text(resp).await, "down");
}

/// Spawn an upstream that responds with `status` + `Location` header.
/// Used by Phase-8 redirect tests below.
async fn spawn_redirecting_upstream(status: u16, location: &'static str) -> String {
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let shutdown_from_handler = shutdown.clone();
    let app = Router::new().route(
        "/*path",
        any(move || {
            let location = location;
            let shutdown = shutdown_from_handler.clone();
            async move {
                let mut resp = (
                    StatusCode::from_u16(status).unwrap_or(StatusCode::FOUND),
                    "redirect",
                )
                    .into_response();
                resp.headers_mut().insert(
                    axum::http::header::LOCATION,
                    HeaderValue::from_str(location).unwrap(),
                );
                shutdown.notify_one();
                resp
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                shutdown.notified().await;
            })
            .await
            .unwrap();
    });
    format!("http://{}", addr)
}

#[tokio::test]
async fn forward_request_does_not_auto_follow_30x() {
    // The shared CLIENT sets `redirect(Policy::none())` so signed
    // proxy requests never silently follow upstream redirects without
    // re-running the auth pipeline. Verify by pointing forward_request
    // at a 302 upstream and asserting we get the 302 back instead of
    // the redirect target's body.
    let base = spawn_redirecting_upstream(302, "https://example.invalid/never-fetched").await;
    let url = format!("{}/start", base);
    let resp = forward_request(
        Method::GET,
        &url,
        &url,
        HeaderMap::new(),
        Vec::new(),
        Bytes::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FOUND);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some("https://example.invalid/never-fetched")
    );
}

#[tokio::test]
async fn upstream_unreachable_returns_502() {
    // Bind a port and drop the listener — the OS keeps the address
    // briefly unreachable, but on most OSes the connect call fails fast
    // with ECONNREFUSED. We pick port 1 (privileged, never bound by us).
    let resp = forward_request(
        Method::GET,
        "http://127.0.0.1:1/nope",
        "http://127.0.0.1:1/nope",
        HeaderMap::new(),
        Vec::new(),
        Bytes::new(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
}
