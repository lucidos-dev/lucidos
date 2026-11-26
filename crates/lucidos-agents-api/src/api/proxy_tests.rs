use super::*;
use axum::http::HeaderName;
use axum::routing::any;
use axum::Router;

fn test_timeout() -> std::time::Duration {
    crate::api::proxy_timeout::default_wait()
}

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
fn strips_our_own_namespace_and_the_headers_the_gateway_owns() {
    for h in [
        "x-lucidos-local-token",
        "x-lucidos-hook-socket-token",
        "x-lucidos-agent-origin-token",
        "x-lucidos-device-id",
        "x-lucidos-target-workspace",
        "X-Lucidos-Anything-Added-Later",
        "x-forwarded-prefix",
        "x-forwarded-host",
    ] {
        assert!(
            should_strip_request_header(&name(h)),
            "expected {h} to be stripped"
        );
    }
    for h in [
        lucidos_local_token::HEADER_LOCAL_TOKEN,
        lucidos_local_token::HEADER_HOOK_SOCKET_TOKEN,
    ] {
        assert!(
            should_strip_request_header(&name(h)),
            "expected {h} to be stripped"
        );
    }
}

#[test]
fn strips_the_framing_headers_the_caller_sent() {
    for h in ["Content-Length", "content-length", "Transfer-Encoding"] {
        assert!(
            should_strip_request_header(&name(h)),
            "expected {h} to be stripped"
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

#[test]
fn a_credentialed_proxy_route_applies_the_same_origin_gate_itself() {
    assert!(browser_proxy_request_allowed(&HeaderMap::new()));
    let foreign = hm(&[
        ("Host", "localhost:5251"),
        ("Origin", "https://evil.example"),
        ("Sec-Fetch-Site", "cross-site"),
    ]);
    assert!(!browser_proxy_request_allowed(&foreign));
}

#[test]
fn request_path_has_traversal_flags_dot_dot_segments() {
    assert!(request_path_has_traversal("../etc/passwd"));
    assert!(request_path_has_traversal("foo/../bar"));
    assert!(request_path_has_traversal("a/b/../../c"));
    assert!(request_path_has_traversal("/.."));
}

#[test]
fn request_path_has_traversal_flags_backslashes() {
    assert!(request_path_has_traversal("foo\\..\\bar"));
    assert!(request_path_has_traversal("a\\b"));
}

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
    assert_eq!(origin_of("https://h"), origin_of("https://h:443"));
}

#[test]
fn origin_of_distinguishes_scheme_downgrade() {
    assert_ne!(origin_of("https://h/x"), origin_of("http://h/x"));
}

#[test]
fn origin_of_distinguishes_port() {
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
    assert!(!is_redirect_status(StatusCode::NOT_MODIFIED));
    assert!(!is_redirect_status(StatusCode::OK));
    assert!(!is_redirect_status(StatusCode::BAD_GATEWAY));
}

#[test]
fn only_the_30x_rfc_9110_rewrites_turn_the_next_hop_into_a_get() {
    for method in [Method::POST, Method::PUT, Method::DELETE, Method::PATCH] {
        assert!(redirect_downgrades_to_get(StatusCode::SEE_OTHER, &method));
    }
    assert!(!redirect_downgrades_to_get(
        StatusCode::SEE_OTHER,
        &Method::HEAD
    ));
    for status in [StatusCode::MOVED_PERMANENTLY, StatusCode::FOUND] {
        assert!(redirect_downgrades_to_get(status, &Method::POST));
        assert!(!redirect_downgrades_to_get(status, &Method::PUT));
        assert!(!redirect_downgrades_to_get(status, &Method::GET));
    }
    for status in [
        StatusCode::TEMPORARY_REDIRECT,
        StatusCode::PERMANENT_REDIRECT,
    ] {
        assert!(!redirect_downgrades_to_get(status, &Method::POST));
    }
}

#[test]
fn request_path_has_traversal_passes_normal_paths() {
    assert!(!request_path_has_traversal(""));
    assert!(!request_path_has_traversal("/living-room/play"));
    assert!(!request_path_has_traversal("api/v1/items?id=42"));
    assert!(!request_path_has_traversal("foo..bar"));
}

#[test]
fn build_url_handles_no_trailing_no_leading() {
    assert_eq!(
        build_target_url("http://localhost:5005", "living-room/play", None),
        "http://localhost:5005/living-room/play"
    );
}

#[test]
fn build_url_handles_trailing_and_leading_slashes() {
    assert_eq!(
        build_target_url("http://localhost:5005/", "/living-room/play", None),
        "http://localhost:5005/living-room/play"
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
    let base = "https://api.example.com/v1/x";
    let real = append_query_param(base, "api-key", "actual-secret-value");
    let redacted = append_query_param(base, "api-key", "REDACTED");
    assert!(real.contains("actual-secret-value"));
    assert!(!redacted.contains("actual-secret-value"));
    assert_eq!(redacted, "https://api.example.com/v1/x?api-key=REDACTED");
}

#[test]
fn hmac_sha256_known_vector() {
    let key = [0x0bu8; 20];
    let sig = compute_hmac_hex(HmacAlgorithm::Sha256, &key, b"Hi There");
    assert_eq!(
        sig,
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
}

#[test]
fn hmac_sha512_known_vector() {
    let key = [0x0bu8; 20];
    let sig = compute_hmac_hex(HmacAlgorithm::Sha512, &key, b"Hi There");
    assert_eq!(
        sig,
        "87aa7cdea5ef619d4ff0b4241a1d6cb02379f4e2ce4ec2787ad0b30545e17cdedaa833b7d6b8a702038b274eaea3f4e4be9d914eeb61f1702e696c203a126854"
    );
}

#[test]
fn hmac_signature_matches_binance_known_example() {
    let secret = b"NhqPtmdSJYdKjVHjA7PZj4Mge3R5YNiP1e3UZjInClVN65XAbvqqM6A7H5fATj0j";
    let query = "symbol=LTCBTC&side=BUY&type=LIMIT&timeInForce=GTC&quantity=1&price=0.1&recvWindow=5000&timestamp=1499827319559";
    let sig = compute_hmac_hex(HmacAlgorithm::Sha256, secret, query.as_bytes());
    assert_eq!(
        sig,
        "c8db56825ae71d6d79447849e617115f4a920fa2acdcab2b053c4b2838bd6b71"
    );
}

fn workspace_with_apis_json(tmp: &tempfile::TempDir, body: &str) -> std::path::PathBuf {
    let cfg_dir = tmp.path().join("data/config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("apis.json"), body).unwrap();
    tmp.path().to_path_buf()
}

#[test]
fn one_refused_provider_does_not_take_its_neighbours_down() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace_with_apis_json(
        &tmp,
        r#"{
          "good": {"base_url": "https://good.test"},
          "broken": {"base_url": "https://x", "auth": {"pipeline": [
            {"type": "bearrer", "credential": "k"}
          ]}},
          "also-good": {"base_url": "https://also.test", "auth": {"pipeline": [
            {"type": "static_credential", "kind": "bearer", "credential": "k"}
          ]}}
        }"#,
    );
    let load = load_proxy_config(&ws);
    assert_eq!(load.providers.len(), 2, "both good entries must load");
    assert!(load.providers.contains_key("good"));
    assert!(load.providers.contains_key("also-good"));
    assert_eq!(rejected_names(&ws), vec!["broken".to_string()]);
}

#[test]
fn a_refused_legacy_entry_is_reported_in_words_a_user_can_act_on() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace_with_apis_json(
        &tmp,
        r#"{"old": {"base_url": "https://x", "auth": {"type": "credential_bundle"}}}"#,
    );
    let reason = &load_proxy_config(&ws).rejected[0].reason;
    assert!(reason.contains("credential_bundle"), "{reason}");
    assert!(
        !reason.contains("pipeline"),
        "serde's words leaked: {reason}"
    );
}

#[tokio::test]
async fn a_refused_provider_is_a_502_and_never_reaches_the_builtin() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace_with_apis_json(
        &tmp,
        r#"{"openai": {"base_url": "https://x", "auth": {"pipeline": [
            {"type": "bearrer", "credential": "k"}
        ]}}}"#,
    );
    let (status, msg) = resolve_proxy_target(&ws, "openai")
        .await
        .expect_err("a refused entry must not resolve");
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(msg.contains("openai"), "names the provider: {msg}");
    assert!(msg.contains("unusable"), "says what is wrong: {msg}");
}

#[tokio::test]
async fn an_unparseable_file_refuses_every_name_rather_than_rerouting_it() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace_with_apis_json(&tmp, "{ not json");
    let (status, _) = resolve_proxy_target(&ws, "openai")
        .await
        .expect_err("nothing resolves while the file is unreadable");
    assert_eq!(status, StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn an_unconfigured_name_is_still_a_404() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace_with_apis_json(&tmp, r#"{"other": {"base_url": "https://x"}}"#);
    let (status, _) = resolve_proxy_target(&ws, "openai").await.unwrap_err();
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[test]
fn load_config_returns_empty_when_file_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let load = load_proxy_config(tmp.path());
    assert!(load.providers.is_empty());
    assert!(load.rejected.is_empty());
}

#[test]
fn load_config_parses_basic_shape() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace_with_apis_json(&tmp, r#"{"sonos": {"base_url": "http://localhost:5005"}}"#);
    let cfg = load_proxy_config(&ws).providers;
    assert_eq!(cfg.len(), 1);
    let sonos = cfg.get("sonos").unwrap();
    assert_eq!(sonos.base_url, "http://localhost:5005");
    assert!(sonos.auth.is_none());
}

#[test]
fn load_config_parses_pipeline_shape_with_static_credential_layer() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace_with_apis_json(
        &tmp,
        r#"{
                "comfort": {
                    "base_url": "https://accsmart.panasonic.com",
                    "auth": {"pipeline": [
                        {"type": "static_credential", "kind": "bearer", "credential": "comfort-cloud"}
                    ]}
                }
            }"#,
    );
    let cfg = load_proxy_config(&ws).providers;
    let comfort = cfg.get("comfort").unwrap();
    let pipeline = comfort.auth.as_ref().unwrap();
    assert_eq!(pipeline.pipeline.len(), 1);
}

#[test]
fn load_config_rejects_unknown_layer_type_in_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace_with_apis_json(
        &tmp,
        r#"{"foo": {"base_url": "https://x", "auth": {"pipeline": [
                {"type": "bearrer", "credential": "k"}
            ]}}}"#,
    );
    assert_eq!(rejected_names(&ws), vec!["foo".to_string()]);
}

#[test]
fn load_config_rejects_script_handshake_with_traversal_path_in_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace_with_apis_json(
        &tmp,
        r#"{"x": {"base_url": "https://x", "auth": {"pipeline": [
                {"type": "script_handshake", "credential": "x", "script": "../../../etc/passwd"}
            ]}}}"#,
    );
    assert_eq!(rejected_names(&ws), vec!["x".to_string()]);
}

#[test]
fn load_config_rejects_script_handshake_with_absolute_path_in_pipeline() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace_with_apis_json(
        &tmp,
        r#"{"x": {"base_url": "https://x", "auth": {"pipeline": [
                {"type": "script_handshake", "credential": "x", "script": "/etc/passwd"}
            ]}}}"#,
    );
    assert_eq!(rejected_names(&ws), vec!["x".to_string()]);
}

fn load_config_with_script(script: &str) -> Result<(), String> {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace_with_apis_json(
        &tmp,
        &format!(
            r#"{{"acme": {{"base_url": "https://x", "auth": {{"pipeline": [
                {{"type": "script_handshake", "credential": "x", "script": "{script}"}}
            ]}}}}}}"#
        ),
    );
    match load_proxy_config(&ws).rejected.into_iter().next() {
        Some(rejected) => Err(rejected.reason),
        None => Ok(()),
    }
}

fn rejected_names(workspace: &std::path::Path) -> Vec<String> {
    load_proxy_config(workspace)
        .rejected
        .iter()
        .map(|r| r.label().to_string())
        .collect()
}

#[test]
fn the_config_load_and_the_spawn_guard_refuse_the_same_script_paths() {
    for script in [
        "scripts/auth/ok.py",
        "scripts/auth/a..b.py",
        "../../../etc/passwd",
        "/etc/passwd",
        ".env",
        "config/apis.json",
    ] {
        let refused_at_spawn =
            crate::api::proxy_script_runner::script_path_rejection(script).is_some();
        let refused_at_load = load_config_with_script(script).is_err();
        assert_eq!(
            refused_at_load, refused_at_spawn,
            "'{script}' must get the same verdict from both guards"
        );
    }
}

#[test]
fn a_handshake_script_cannot_name_the_gitignored_data_root() {
    for script in [
        ".env",
        "postgres/pg_hba.conf",
        "config/apis.json",
        "secrets",
        "data/.env",
        "data/postgres/pg_hba.conf",
        "data/config/apis.json",
        "data/data/scripts/auth/ok.py",
    ] {
        let reason = crate::api::proxy_script_runner::script_path_rejection(script)
            .unwrap_or_else(|| panic!("'{script}' must be refused"));
        assert!(
            reason.contains("data/scripts/"),
            "'{script}' refused for the wrong reason: {reason}"
        );
    }
    for script in ["scripts/auth/ok.py", "data/scripts/auth/ok.py"] {
        assert!(
            crate::api::proxy_script_runner::script_path_rejection(script).is_none(),
            "'{script}' is a real handshake script and must still be allowed"
        );
    }
    assert!(
        crate::api::proxy_script_runner::script_path_rejection("data/../.env").is_some(),
        "a '..' value must not be normalized into an accepted path"
    );
}

#[test]
fn a_refused_script_path_names_the_provider_and_the_value() {
    let err = load_config_with_script("scripts/auth/a..b.py")
        .expect_err("a '..' substring must be refused");
    assert!(err.contains("acme"), "names the provider: {err}");
    assert!(
        err.contains("scripts/auth/a..b.py"),
        "names the value: {err}"
    );
}

#[test]
fn load_config_invalid_json_is_rejected_as_the_file_itself() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace_with_apis_json(&tmp, "{ not json");
    let load = load_proxy_config(&ws);
    assert!(load.providers.is_empty());
    assert_eq!(load.rejected.len(), 1);
    assert!(load.rejected[0].provider.is_none());
    assert_eq!(load.rejected[0].label(), "data/config/apis.json");
}

use std::sync::Arc;
use std::sync::Mutex;

#[derive(Default, Clone)]
struct UpstreamRecord {
    method: String,
    path: String,
    query: String,
    body: Vec<u8>,
    headers: Vec<(String, String)>,
}

type RecordSlot = Arc<Mutex<Option<UpstreamRecord>>>;

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
        Transport::Verified,
        test_timeout(),
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
    let _ = forward_request(
        Method::GET,
        &url,
        &url,
        headers,
        Vec::new(),
        Bytes::new(),
        Transport::Verified,
        test_timeout(),
    )
    .await;
    let recorded = slot.lock().unwrap().clone().unwrap();
    let observed: Vec<&str> = recorded.headers.iter().map(|(n, _)| n.as_str()).collect();
    assert!(!observed.iter().any(|n| n.eq_ignore_ascii_case("cookie")));
    assert!(!observed.iter().any(|n| n.eq_ignore_ascii_case("origin")));
    assert!(!observed.iter().any(|n| n.eq_ignore_ascii_case("referer")));
    assert!(observed.iter().any(|n| n.eq_ignore_ascii_case("x-keep-me")));
}

#[tokio::test]
async fn the_engines_own_trust_headers_never_reach_an_upstream() {
    let (base, slot) = spawn_recording_upstream(200, "ok").await;
    let url = format!("{}/x", base);
    let headers = hm(&[
        ("x-lucidos-local-token", "full-authority-machine-token"),
        ("x-lucidos-hook-socket-token", "hook-socket-scope-token"),
        ("x-lucidos-agent-origin-token", "thread-bound-token"),
        ("x-lucidos-device-id", "device-1"),
        ("x-forwarded-prefix", "/dev/"),
        ("x-forwarded-host", "laptop.example.ts.net"),
        ("X-Keep-Me", "yes"),
    ]);
    let _ = forward_request(
        Method::GET,
        &url,
        &url,
        headers,
        Vec::new(),
        Bytes::new(),
        Transport::Verified,
        test_timeout(),
    )
    .await;
    let recorded = slot.lock().unwrap().clone().unwrap();
    for (name, value) in &recorded.headers {
        let ours = name.to_ascii_lowercase().starts_with("x-lucidos-")
            && !name.eq_ignore_ascii_case(PROXY_ORIGIN_HEADER);
        assert!(
            !name.eq_ignore_ascii_case("x-forwarded-prefix")
                && !name.eq_ignore_ascii_case("x-forwarded-host")
                && !ours,
            "the upstream saw {name}: {value}"
        );
    }
    assert!(recorded
        .headers
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case("x-keep-me")));
}

#[tokio::test]
async fn every_forward_carries_the_proxy_origin_marker() {
    let (base, slot) = spawn_recording_upstream(200, "ok").await;
    let url = format!("{}/x", base);
    let _ = forward_request(
        Method::GET,
        &url,
        &url,
        HeaderMap::new(),
        Vec::new(),
        Bytes::new(),
        Transport::Verified,
        test_timeout(),
    )
    .await;
    let recorded = slot.lock().unwrap().clone().unwrap();
    assert!(
        recorded
            .headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case(PROXY_ORIGIN_HEADER)),
        "the forward went out unmarked: {:?}",
        recorded.headers
    );
}

async fn spawn_guarded_engine() -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let hits_in_route = hits.clone();
    let app = Router::new()
        .fallback(any(move || {
            let hits = hits_in_route.clone();
            async move {
                hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                "shell-only data"
            }
        }))
        .layer(axum::middleware::from_fn(refuse_proxied_request));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{}", addr), hits)
}

#[tokio::test]
async fn a_forward_aimed_at_an_engine_never_reaches_its_routes() {
    let (base, hits) = spawn_guarded_engine().await;
    let url = format!("{}/api/v1/oauth/google/access-token", base);
    let resp = forward_request(
        Method::GET,
        &url,
        &url,
        HeaderMap::new(),
        Vec::new(),
        Bytes::new(),
        Transport::Verified,
        test_timeout(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn the_engine_refuses_a_marked_request_and_serves_an_unmarked_one() {
    let (base, hits) = spawn_guarded_engine().await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let marked = client
        .get(format!("{base}/api/v1/health"))
        .header(PROXY_ORIGIN_HEADER, "anything")
        .send()
        .await
        .unwrap();
    assert_eq!(marked.status().as_u16(), 403);
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    let plain = client
        .get(format!("{base}/api/v1/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(plain.status().as_u16(), 200);
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
}

#[tokio::test]
async fn upstream_does_not_see_host_header_from_engine() {
    let (base, slot) = spawn_recording_upstream(200, "ok").await;
    let url = format!("{}/x", base);
    let headers = hm(&[("Host", "engine.example.com")]);
    let _ = forward_request(
        Method::GET,
        &url,
        &url,
        headers,
        Vec::new(),
        Bytes::new(),
        Transport::Verified,
        test_timeout(),
    )
    .await;
    let recorded = slot.lock().unwrap().clone().unwrap();
    let host = recorded
        .headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("host"))
        .map(|(_, v)| v.as_str())
        .unwrap_or("");
    assert_ne!(host, "engine.example.com");
}

#[tokio::test]
async fn forwards_arbitrary_auth_headers_to_upstream() {
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
        Transport::Verified,
        test_timeout(),
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

fn recorded_values<'a>(recorded: &'a UpstreamRecord, header: &str) -> Vec<&'a str> {
    recorded
        .headers
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case(header))
        .map(|(_, v)| v.as_str())
        .collect()
}

#[tokio::test]
async fn an_injected_header_replaces_the_callers_own() {
    let (base, slot) = spawn_recording_upstream(200, "ok").await;
    let url = format!("{}/x", base);
    let caller = hm(&[
        ("Authorization", "Bearer app-picked"),
        ("X-Api-Key", "app-picked"),
        ("X-Keep-Me", "yes"),
    ]);
    let auth_vec: Vec<(HeaderName, HeaderValue)> = vec![
        (
            HeaderName::from_static("authorization"),
            HeaderValue::from_static("Bearer engine-credential"),
        ),
        (
            HeaderName::from_static("x-api-key"),
            HeaderValue::from_static("engine-key"),
        ),
    ];
    let _ = forward_request(
        Method::GET,
        &url,
        &url,
        caller,
        auth_vec,
        Bytes::new(),
        Transport::Verified,
        test_timeout(),
    )
    .await;
    let recorded = slot.lock().unwrap().clone().unwrap();
    assert_eq!(
        recorded_values(&recorded, "authorization"),
        vec!["Bearer engine-credential"]
    );
    assert_eq!(recorded_values(&recorded, "x-api-key"), vec!["engine-key"]);
    assert_eq!(recorded_values(&recorded, "x-keep-me"), vec!["yes"]);
}

#[tokio::test]
async fn a_stale_caller_content_length_does_not_frame_the_body() {
    let (base, slot) = spawn_recording_upstream(200, "ok").await;
    let url = format!("{}/sign", base);
    let signed = r#"{"amount":1,"signature":"0abcdef"}"#;
    let caller = hm(&[("Content-Length", "3")]);
    let resp = forward_request(
        Method::POST,
        &url,
        &url,
        caller,
        Vec::new(),
        Bytes::copy_from_slice(signed.as_bytes()),
        Transport::Verified,
        test_timeout(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let recorded = slot.lock().unwrap().clone().expect("upstream not called");
    assert_eq!(String::from_utf8_lossy(&recorded.body), signed);
    let real_length = signed.len().to_string();
    assert_eq!(
        recorded_values(&recorded, "content-length"),
        vec![real_length.as_str()]
    );
}

const UNSENDABLE_SECRET: &str = "tok-abc";

struct UnsendableHeaderLayer;

#[async_trait::async_trait]
impl crate::proxy_auth_layer::AuthLayer for UnsendableHeaderLayer {
    fn output_namespace(&self) -> &str {
        "unsendable"
    }
    fn scope_bindings(&self) -> Vec<crate::proxy_auth_layer::ScopeBinding> {
        Vec::new()
    }
    async fn apply(
        &self,
        _input: &crate::proxy_auth_layer::LayerInput<'_>,
    ) -> Result<crate::proxy_auth_layer::AuthMutation, (StatusCode, String)> {
        Ok(crate::proxy_auth_layer::AuthMutation {
            add_headers: vec![(
                HeaderName::from_static("authorization"),
                format!("Bearer {UNSENDABLE_SECRET}\n"),
            )],
            ..Default::default()
        })
    }
}

#[tokio::test]
async fn a_produced_header_value_the_codec_refuses_is_refused_not_dropped() {
    let (base, slot) = spawn_recording_upstream(200, "ok").await;
    let tmp = tempfile::tempdir().unwrap();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(tmp.path(), &pool, &bus);
    let layers: Vec<Arc<dyn crate::proxy_auth_layer::AuthLayer>> =
        vec![Arc::new(UnsendableHeaderLayer)];
    let scoped = ScopedPipeline::bind(&ctx, "comfort", base, layers, false)
        .await
        .expect("a loopback upstream binds");

    let err = forward_with_redirects(
        "comfort",
        &scoped,
        &CallBudget::start(test_timeout()),
        &Method::GET,
        "v1/items",
        None,
        &HeaderMap::new(),
        &Bytes::new(),
    )
    .await
    .expect_err("a credential the codec cannot send must not be dropped");

    assert_eq!(err.0, StatusCode::BAD_GATEWAY);
    assert!(err.1.contains("authorization"), "{}", err.1);
    assert!(
        !err.1.contains(UNSENDABLE_SECRET),
        "the message carried the credential: {}",
        err.1
    );
    assert!(
        slot.lock().unwrap().is_none(),
        "the upstream saw a request that had lost its credential"
    );
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
        Transport::Verified,
        test_timeout(),
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
        Transport::Verified,
        test_timeout(),
    )
    .await;
    let recorded = slot.lock().unwrap().clone().unwrap();
    assert_eq!(recorded.query, "limit=10&api-key=secret-123");
}

#[tokio::test]
async fn upstream_5xx_passes_through() {
    for status in [500, 503, 429] {
        let (base, _slot) = spawn_recording_upstream(status, "down").await;
        let url = format!("{}/x", base);
        let resp = forward_request(
            Method::GET,
            &url,
            &url,
            HeaderMap::new(),
            Vec::new(),
            Bytes::new(),
            Transport::Verified,
            test_timeout(),
        )
        .await;
        assert_eq!(resp.status().as_u16(), status);
        assert!(resp
            .extensions()
            .get::<crate::api::internal_faults::ExternalFailure>()
            .is_some());
        assert_eq!(body_text(resp).await, "down");
    }
}

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
    let base = spawn_redirecting_upstream(302, "https://example.invalid/never-fetched").await;
    let url = format!("{}/start", base);
    let resp = forward_request(
        Method::GET,
        &url,
        &url,
        HeaderMap::new(),
        Vec::new(),
        Bytes::new(),
        Transport::Verified,
        test_timeout(),
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
async fn a_refused_redirect_does_not_echo_the_location() {
    for location in [
        "http://[::1?token=redirect-secret",
        "data:text/plain?token=redirect-secret",
    ] {
        let base = spawn_redirecting_upstream(302, location).await;
        let tmp = tempfile::tempdir().unwrap();
        let pool = unreachable_pool();
        let bus = crate::test_support::offline_event_bus();
        let ctx = offline_scope_ctx(tmp.path(), &pool, &bus);
        let scoped = ScopedPipeline::bind(&ctx, "comfort", base, Vec::new(), false)
            .await
            .expect("a loopback upstream binds");

        let err = forward_with_redirects(
            "comfort",
            &scoped,
            &CallBudget::start(test_timeout()),
            &Method::GET,
            "start",
            None,
            &HeaderMap::new(),
            &Bytes::new(),
        )
        .await
        .expect_err("a Location with no usable host must be refused");

        assert_eq!(err.0, StatusCode::BAD_GATEWAY, "{location}");
        assert!(
            !err.1.contains("redirect-secret"),
            "the 502 body echoed the Location: {}",
            err.1
        );
    }
}

async fn spawn_upstream_with_headers(headers: &'static [(&'static str, &'static str)]) -> String {
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let shutdown_from_handler = shutdown.clone();
    let app = Router::new().fallback(any(move || {
        let shutdown = shutdown_from_handler.clone();
        async move {
            shutdown.notify_one();
            let mut resp = "upstream body".into_response();
            resp.headers_mut().clear();
            for (n, v) in headers {
                resp.headers_mut()
                    .append(name(n), HeaderValue::from_static(v));
            }
            resp
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
    format!("http://{}", addr)
}

async fn proxied_with(headers: &'static [(&'static str, &'static str)]) -> Response {
    let url = format!("{}/x", spawn_upstream_with_headers(headers).await);
    forward_request(
        Method::GET,
        &url,
        &url,
        HeaderMap::new(),
        Vec::new(),
        Bytes::new(),
        Transport::Verified,
        test_timeout(),
    )
    .await
}

fn header_values(resp: &Response, header: &str) -> Vec<String> {
    resp.headers()
        .get_all(header)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect()
}

const ORIGIN_SCOPED_RESPONSE_HEADERS: &[(&str, &str)] = &[
    ("set-cookie", "lucidos_device_x=attacker; Path=/"),
    ("set-cookie", "theme=dark; Path=/"),
    ("set-cookie2", "legacy=1"),
    ("clear-site-data", "\"cookies\", \"storage\""),
    (
        "strict-transport-security",
        "max-age=63072000; includeSubDomains",
    ),
    ("content-security-policy", "script-src *"),
    ("content-security-policy-report-only", "default-src 'none'"),
    ("service-worker-allowed", "/"),
    ("public-key-pins", "pin-sha256=\"AAAA\"; max-age=1"),
    (
        "public-key-pins-report-only",
        "pin-sha256=\"AAAA\"; max-age=1",
    ),
    ("expect-ct", "max-age=86400, enforce"),
    ("alt-svc", "h3=\":443\"; ma=2592000"),
    ("access-control-allow-origin", "*"),
    ("access-control-allow-credentials", "true"),
    ("access-control-expose-headers", "*"),
    ("timing-allow-origin", "*"),
    ("www-authenticate", "Basic realm=\"Lucidos\""),
    ("permissions-policy", "camera=*"),
    ("feature-policy", "camera *"),
    ("document-policy", "force-load-at-top"),
    ("cross-origin-opener-policy", "unsafe-none"),
    ("cross-origin-embedder-policy", "require-corp"),
    ("cross-origin-resource-policy", "cross-origin"),
    ("origin-agent-cluster", "?1"),
    ("origin-trial", "token"),
    ("accept-ch", "Sec-CH-UA-Model"),
    ("critical-ch", "Sec-CH-UA-Model"),
    ("nel", "{\"report_to\":\"default\",\"max_age\":31536000}"),
    ("report-to", "{\"group\":\"default\"}"),
    (
        "reporting-endpoints",
        "default=\"https://attacker.invalid/r\"",
    ),
    ("refresh", "0; url=https://attacker.invalid/"),
    ("set-login", "logged-in"),
    ("sec-session-registration", "(ES256); path=\"/r\""),
    ("speculation-rules", "\"/rules.json\""),
    ("x-frame-options", "ALLOWALL"),
    ("x-xss-protection", "0"),
    ("x-content-security-policy", "script-src *"),
    ("x-webkit-csp", "script-src *"),
    ("x-dns-prefetch-control", "on"),
    ("x-permitted-cross-domain-policies", "all"),
    ("x-lucidos-local-token", "forged"),
    ("x-forwarded-host", "attacker.invalid"),
];

#[tokio::test]
async fn origin_scoped_upstream_headers_never_reach_the_lucidos_origin() {
    let resp = proxied_with(ORIGIN_SCOPED_RESPONSE_HEADERS).await;
    assert_eq!(resp.status(), StatusCode::OK);
    for (header, value) in ORIGIN_SCOPED_RESPONSE_HEADERS {
        assert!(
            !header_values(&resp, header).contains(&value.to_string()),
            "the proxy forwarded the upstream's {header}"
        );
    }
    assert_eq!(body_text(resp).await, "upstream body");
}

const API_RESPONSE_HEADERS: &[(&str, &str)] = &[
    ("content-type", "application/json"),
    ("content-language", "en"),
    ("content-disposition", "attachment; filename=\"export.csv\""),
    ("cache-control", "no-cache"),
    ("etag", "\"v1\""),
    ("last-modified", "Wed, 21 Oct 2026 07:28:00 GMT"),
    ("vary", "Accept"),
    ("location", "/v1/items/42"),
    (
        "link",
        "<https://api.example.com/items?page=2>; rel=\"next\"",
    ),
    ("retry-after", "30"),
    ("ratelimit-remaining", "10"),
    ("request-id", "req_123"),
    ("x-request-id", "abc"),
    ("x-ratelimit-remaining", "99"),
    ("x-total-count", "1234"),
    ("anthropic-ratelimit-requests-remaining", "49"),
    ("openai-processing-ms", "120"),
];

#[tokio::test]
async fn api_response_headers_still_reach_the_caller() {
    let resp = proxied_with(API_RESPONSE_HEADERS).await;
    for (header, value) in API_RESPONSE_HEADERS {
        assert_eq!(
            header_values(&resp, header),
            vec![value.to_string()],
            "the proxy dropped or rewrote {header}"
        );
    }
}

#[tokio::test]
async fn every_proxied_response_refuses_mime_sniffing() {
    let resp = proxied_with(&[("content-type", "application/json")]).await;
    assert_eq!(header_values(&resp, "x-content-type-options"), ["nosniff"]);
    assert!(header_values(&resp, "content-security-policy").is_empty());
}

#[tokio::test]
async fn an_upstream_document_renders_sandboxed_without_script() {
    const DOCUMENTS: &[&[(&str, &str)]] = &[
        &[
            ("content-type", "text/html; charset=utf-8"),
            ("content-security-policy", "script-src *"),
        ],
        &[("content-type", "image/svg+xml")],
        &[("content-type", "application/xml")],
        &[
            ("content-type", "application/json"),
            ("content-type", "text/html"),
        ],
        &[("content-type", "application/json, text/html")],
    ];
    for headers in DOCUMENTS {
        let resp = proxied_with(headers).await;
        assert_eq!(
            header_values(&resp, "content-security-policy"),
            [PROXIED_DOCUMENT_CSP],
            "{headers:?} must render in an opaque origin with no script"
        );
        assert_eq!(header_values(&resp, "x-content-type-options"), ["nosniff"]);
    }
}

#[tokio::test]
async fn an_untyped_upstream_body_is_sandboxed_too() {
    let resp = proxied_with(&[]).await;
    assert_eq!(
        header_values(&resp, "content-security-policy"),
        [PROXIED_DOCUMENT_CSP]
    );
}

async fn spawn_redirect_then_record(status: u16, location: &'static str) -> (String, RecordSlot) {
    let slot: RecordSlot = Arc::new(Mutex::new(None));
    let slot_clone = slot.clone();
    let hops = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let shutdown_from_handler = shutdown.clone();
    let app = Router::new().fallback(any(move |req: axum::extract::Request| {
        let slot = slot_clone.clone();
        let hops = hops.clone();
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
            if hops.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                let mut resp = (
                    StatusCode::from_u16(status).unwrap_or(StatusCode::FOUND),
                    "redirect",
                )
                    .into_response();
                resp.headers_mut().insert(
                    axum::http::header::LOCATION,
                    HeaderValue::from_str(location).unwrap(),
                );
                return resp;
            }
            *slot.lock().unwrap() = Some(UpstreamRecord {
                method,
                path,
                query,
                body: req_body,
                headers,
            });
            shutdown.notify_one();
            (StatusCode::OK, "landed").into_response()
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

async fn unauthenticated_pipeline(base_url: &str) -> ScopedPipeline {
    let tmp = tempfile::tempdir().unwrap();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(tmp.path(), &pool, &bus);
    ScopedPipeline::bind(&ctx, "backend", base_url.to_string(), Vec::new(), false)
        .await
        .expect("a loopback upstream binds")
}

#[tokio::test]
async fn a_redirect_under_a_base_path_does_not_double_the_prefix() {
    let (upstream, slot) = spawn_redirect_then_record(302, "/v1/items/").await;
    let scoped = unauthenticated_pipeline(&format!("{upstream}/v1")).await;

    let (resp, _) = forward_with_redirects(
        "backend",
        &scoped,
        &CallBudget::start(test_timeout()),
        &Method::GET,
        "items",
        None,
        &HeaderMap::new(),
        &Bytes::new(),
    )
    .await
    .expect("a same-origin redirect inside the prefix is followed");

    assert_eq!(resp.status(), StatusCode::OK);
    let recorded = slot.lock().unwrap().clone().expect("second hop not made");
    assert_eq!(recorded.path, "/v1/items/");
}

#[tokio::test]
async fn a_redirect_out_of_the_base_path_is_refused() {
    let (upstream, slot) = spawn_redirect_then_record(302, "/admin").await;
    let scoped = unauthenticated_pipeline(&format!("{upstream}/v1")).await;

    let err = forward_with_redirects(
        "backend",
        &scoped,
        &CallBudget::start(test_timeout()),
        &Method::GET,
        "items",
        None,
        &HeaderMap::new(),
        &Bytes::new(),
    )
    .await
    .expect_err("a Location outside the base path must not be followed");

    assert_eq!(err.0, StatusCode::BAD_GATEWAY);
    assert!(err.1.contains("/admin"), "{}", err.1);
    assert!(
        slot.lock().unwrap().is_none(),
        "the proxy fetched a resource outside its base path"
    );
}

#[tokio::test]
async fn a_303_is_replayed_as_a_bodyless_get() {
    let (upstream, slot) = spawn_redirect_then_record(303, "/jobs/42").await;
    let scoped = unauthenticated_pipeline(&upstream).await;

    let (resp, _) = forward_with_redirects(
        "backend",
        &scoped,
        &CallBudget::start(test_timeout()),
        &Method::POST,
        "jobs",
        None,
        &HeaderMap::new(),
        &Bytes::copy_from_slice(br#"{"run":true}"#),
    )
    .await
    .expect("a same-origin 303 is followed");

    assert_eq!(resp.status(), StatusCode::OK);
    let recorded = slot.lock().unwrap().clone().expect("second hop not made");
    assert_eq!(recorded.path, "/jobs/42");
    assert_eq!(recorded.method, "GET");
    assert!(
        recorded.body.is_empty(),
        "the 303 hop replayed the original body"
    );
}

#[tokio::test]
async fn upstream_unreachable_returns_502() {
    let resp = forward_request(
        Method::GET,
        "http://127.0.0.1:1/nope",
        "http://127.0.0.1:1/nope",
        HeaderMap::new(),
        Vec::new(),
        Bytes::new(),
        Transport::Verified,
        test_timeout(),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn fetch_required_credential_resolves_an_oauth_client_by_name() {
    use crate::test_support::{seed_credential, setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    seed_credential(
        &pool,
        "acme",
        "https://api.acme.test",
        crate::core::AuthType::OauthClient,
        "{\"client_id\":\"cid\"}",
    )
    .await;

    let cred = fetch_required_credential(&pool, "acme")
        .await
        .expect("an explicitly named oauth client must resolve");
    assert_eq!(cred.auth_type, crate::core::AuthType::OauthClient);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[test]
fn an_explicitly_named_oauth_client_still_injects_its_env_vars() {
    use crate::core::credentials::{
        credential_env_vars_for, named_credential_env_vars, SecretMentions,
    };
    use crate::core::{AuthType, Credential};

    let mut cred = Credential::fixture("acme", AuthType::OauthClient, "{\"client_id\":\"cid\"}");
    cred.base_urls = vec!["https://api.acme.test".to_string()];
    let no_references =
        crate::core::secret_reference::tests::ScriptedResolver::answering(Vec::new());

    assert_eq!(
        futures::executor::block_on(credential_env_vars_for(cred.clone(), &no_references)).unwrap(),
        vec![(
            "CRED_ACME".to_string(),
            "{\"client_id\":\"cid\"}".to_string()
        )]
    );
    assert!(
        futures::executor::block_on(named_credential_env_vars(
            vec![cred],
            &SecretMentions::scan("echo $CRED_ACME"),
            &no_references
        ))
        .env_vars
        .is_empty(),
        "a subprocess naming it still does not get it"
    );
}

#[tokio::test]
async fn fetch_required_credential_does_not_case_fold_a_miss() {
    use crate::test_support::{seed_credential, setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    seed_credential(
        &pool,
        "stripe",
        "https://api.stripe.test",
        crate::core::AuthType::OauthClient,
        "{\"client_id\":\"cid\"}",
    )
    .await;

    let err = fetch_required_credential(&pool, "Stripe")
        .await
        .expect_err("a differently-cased name is a different credential");
    assert!(
        err.1.contains("'Stripe'"),
        "the error must name the credential the config asked for: {}",
        err.1
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

const PREFIXED_BASE: &str = "https://upstream.example/safe-prefix";

#[test]
fn a_normal_path_stays_contained() {
    assert_eq!(
        build_contained_target_url(PREFIXED_BASE, "items/42", None).unwrap(),
        "https://upstream.example/safe-prefix/items/42"
    );
    assert_eq!(
        build_contained_target_url(PREFIXED_BASE, "/items", Some("limit=10")).unwrap(),
        "https://upstream.example/safe-prefix/items?limit=10"
    );
    assert!(build_contained_target_url(PREFIXED_BASE, "", None).is_ok());
    assert!(build_contained_target_url(PREFIXED_BASE, "/", None).is_ok());
}

#[test]
fn every_dot_segment_spelling_is_refused_by_containment() {
    for path in [
        "%2e%2e/admin",
        "%2E%2E/admin",
        ".%2e/admin",
        "%2e./admin",
        "../admin",
        "a/../../admin",
        "%2e%2e/%2e%2e/etc",
    ] {
        let result = build_contained_target_url(PREFIXED_BASE, path, None);
        assert!(
            result.is_err(),
            "path escaped the configured prefix: {path} -> {result:?}"
        );
    }
}

#[test]
fn a_dot_segment_that_stays_inside_the_prefix_is_allowed() {
    assert!(build_contained_target_url(PREFIXED_BASE, "a/%2e%2e/b", None).is_ok());
}

#[test]
fn a_prefix_less_base_url_still_forwards_everything() {
    let base = "http://localhost:5005";
    assert_eq!(
        build_contained_target_url(base, "living-room/play", None).unwrap(),
        "http://localhost:5005/living-room/play"
    );
    assert!(build_contained_target_url(base, "%2e%2e/anything", None).is_ok());
}

#[test]
fn a_sibling_prefix_is_not_containment() {
    let result = build_contained_target_url(PREFIXED_BASE, "%2e%2e/safe-prefix-evil/x", None);
    assert!(result.is_err(), "sibling prefix accepted: {result:?}");
}

#[test]
fn containment_fails_closed_on_an_unparseable_base_url() {
    assert!(build_contained_target_url("not a url", "/x", None).is_err());
    assert!(build_contained_target_url("", "/x", None).is_err());
}

#[test]
fn the_returned_url_is_the_unmodified_concatenation() {
    let path = "items/42";
    let query = Some("a=1&b=%20");
    assert_eq!(
        build_contained_target_url(PREFIXED_BASE, path, query).unwrap(),
        build_target_url(PREFIXED_BASE, path, query)
    );
}

#[test]
fn request_path_has_traversal_flags_encoded_dot_segments() {
    assert!(request_path_has_traversal("%2e%2e/admin"));
    assert!(request_path_has_traversal("%2E%2E/admin"));
    assert!(request_path_has_traversal(".%2e/admin"));
    assert!(request_path_has_traversal("%2e./admin"));
    assert!(request_path_has_traversal("a/%2e%2e/b"));
}

#[test]
fn request_path_has_traversal_leaves_harmless_encodings_alone() {
    assert!(!request_path_has_traversal("./items"));
    assert!(!request_path_has_traversal("%2e/items"));
    assert!(!request_path_has_traversal("%252e%252e/admin"));
    assert!(!request_path_has_traversal("foo%2e%2ebar"));
    assert!(!request_path_has_traversal("%2e%2e%2e"));
}

#[test]
fn the_raw_concatenation_really_does_escape_the_prefix() {
    let raw = build_target_url(PREFIXED_BASE, "%2e%2e/admin", None);
    assert_eq!(raw, "https://upstream.example/safe-prefix/%2e%2e/admin");
    let parsed = reqwest::Url::parse(&raw).expect("parses");
    assert_eq!(parsed.path(), "/admin", "the prefix survived normalization");
    assert_eq!(
        parsed.host_str(),
        Some("upstream.example"),
        "still same origin"
    );
}

#[test]
fn an_encoded_separator_cannot_smuggle_a_parent_segment() {
    for path in [
        "%2e%2e%2fadmin",
        "..%2fadmin",
        "%2E%2E%2Fadmin",
        "..%5cadmin",
        "a/..%2f..%2fadmin",
        "%2e%2e%252fadmin",
        "%2e%2e%25252fadmin",
        "..%252fadmin",
    ] {
        let result = build_contained_target_url(PREFIXED_BASE, path, None);
        assert!(
            result.is_err(),
            "encoded separator escaped the prefix: {path} -> {result:?}"
        );
    }
}

#[test]
fn a_legitimate_encoded_slash_inside_a_segment_still_forwards() {
    let ok = build_contained_target_url(PREFIXED_BASE, "contents/src%2fmain.rs", None);
    assert!(ok.is_ok(), "legitimate encoded slash refused: {ok:?}");
    assert_eq!(
        ok.unwrap(),
        "https://upstream.example/safe-prefix/contents/src%2fmain.rs",
        "an accepted path must forward byte-identical"
    );
    for path in [
        "fetch/https%3A%2F%2Fexample.com%2Fa",
        "report%252e%252epdf",
        "items/50%25-off",
        "a..b/c",
    ] {
        let result = build_contained_target_url(PREFIXED_BASE, path, None);
        assert!(
            result.is_ok(),
            "ordinary path refused: {path} -> {result:?}"
        );
    }
}

#[test]
fn a_deeply_nested_path_is_refused_without_peeling_every_layer() {
    let hostile = format!("%{}", "25".repeat(30_000));
    let result = build_contained_target_url(PREFIXED_BASE, &hostile, None);
    assert!(result.is_err(), "a hostile nesting must be refused");
}

#[test]
fn a_same_origin_redirect_is_re_anchored_under_the_prefix() {
    let next = redirect_path_within_base(PREFIXED_BASE, "/safe-prefix/next").unwrap();
    assert_eq!(next, "/next");
    assert_eq!(
        build_contained_target_url(PREFIXED_BASE, &next, None).unwrap(),
        "https://upstream.example/safe-prefix/next"
    );
    assert_eq!(
        redirect_path_within_base(PREFIXED_BASE, "/safe-prefix").unwrap(),
        ""
    );
    assert_eq!(
        redirect_path_within_base(PREFIXED_BASE, "/safe-prefix/").unwrap(),
        "/"
    );
    assert!(redirect_path_within_base(PREFIXED_BASE, "/admin").is_none());
    assert!(redirect_path_within_base(PREFIXED_BASE, "/safe-prefix-evil/x").is_none());
    assert_eq!(
        redirect_path_within_base("http://localhost:5005", "/living-room/play").unwrap(),
        "/living-room/play"
    );
}

#[test]
fn handshake_script_paths_are_workspace_relative_and_deduped() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    std::fs::create_dir_all(ws.join("data/config")).unwrap();
    std::fs::write(
        ws.join("data/config/apis.json"),
        r#"{
          "one": {"base_url": "https://a.test", "auth": {"pipeline": [
            {"type": "script_handshake", "script": "scripts/auth/shared.py"}
          ]}},
          "two": {"base_url": "https://b.test", "auth": {"pipeline": [
            {"type": "static_credential", "kind": "bearer", "credential": "k"},
            {"type": "script_handshake", "script": "scripts/auth/shared.py"},
            {"type": "script_handshake", "script": "scripts/auth/other.py"}
          ]}},
          "three": {"base_url": "https://c.test"}
        }"#,
    )
    .unwrap();

    let paths = load_proxy_config(ws).handshake_script_paths();
    assert_eq!(
        paths,
        vec![
            "data/scripts/auth/other.py".to_string(),
            "data/scripts/auth/shared.py".to_string(),
        ]
    );
}

#[test]
fn the_data_prefixed_spelling_seeds_the_same_scripts() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    std::fs::create_dir_all(ws.join("data/config")).unwrap();
    std::fs::write(
        ws.join("data/config/apis.json"),
        r#"{
          "storage": {"base_url": "https://storage.test", "auth": {"pipeline": [
            {"type": "script_handshake",
             "script": "data/scripts/auth/exchange.py",
             "credential": "web-api-key",
             "oauth_providers": ["google"]}
          ]}},
          "calendar": {"base_url": "https://calendar.test", "auth": {"pipeline": [
            {"type": "script_handshake", "script": "scripts/auth/exchange.py"}
          ]}}
        }"#,
    )
    .unwrap();

    let seeds = load_proxy_config(ws).handshake_seed_entries();
    assert_eq!(
        seeds.len(),
        1,
        "both spellings name one script, so they seed one line: {seeds:?}"
    );
    assert_eq!(seeds[0].path, "data/scripts/auth/exchange.py");
    assert_eq!(seeds[0].base_url, None);
    assert_eq!(
        seeds[0].injects,
        Some(
            ["c:web-api-key", "o:google"]
                .into_iter()
                .map(str::to_string)
                .collect()
        )
    );
}

#[test]
fn a_single_entry_seeds_its_scope_and_its_injected_set() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    std::fs::create_dir_all(ws.join("data/config")).unwrap();
    std::fs::write(
        ws.join("data/config/apis.json"),
        r#"{
          "storage": {"base_url": "https://storage.test", "auth": {"pipeline": [
            {"type": "script_handshake",
             "script": "data/scripts/auth/exchange.py",
             "credential": "web-api-key",
             "oauth_providers": ["google"]}
          ]}}
        }"#,
    )
    .unwrap();

    let seeds = load_proxy_config(ws).handshake_seed_entries();
    assert_eq!(seeds[0].base_url.as_deref(), Some("https://storage.test"));
    assert_eq!(
        seeds[0].injects,
        Some(
            ["c:web-api-key", "o:google"]
                .into_iter()
                .map(str::to_string)
                .collect()
        )
    );
}

#[tokio::test]
async fn a_credential_is_refused_for_a_provider_outside_its_scope() {
    use crate::test_support::{seed_credential, setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    seed_credential(
        &pool,
        "scoped-key",
        "https://api.example.test",
        crate::core::AuthType::Bearer,
        "sk-live-secret",
    )
    .await;

    check_credential_scope(&pool, "scoped-key", "https://api.example.test")
        .await
        .expect("its own API is in scope");
    check_credential_scope(&pool, "scoped-key", "https://api.example.test/v2")
        .await
        .expect("a deeper path on the same host is in scope");

    for elsewhere in [
        "https://evil.test",
        "https://api.example.test.evil.test",
        "http://api.example.test",
    ] {
        let err = check_credential_scope(&pool, "scoped-key", elsewhere)
            .await
            .expect_err("out of scope must be refused");
        assert_eq!(err.0, StatusCode::BAD_GATEWAY);
        assert!(err.1.contains("scoped-key"), "{}", err.1);
        assert!(err.1.contains("will not be sent"), "{}", err.1);
        let remedy = err.1.rsplit_once(". ").expect("a remedy sentence").1;
        assert!(
            !remedy.contains(elsewhere),
            "the remedy names the requested host, a grant to paste: {remedy}"
        );
    }

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn one_credential_reaches_every_host_of_the_provider_it_declares() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    let bus = crate::test_support::offline_event_bus();
    CredentialStore::upsert(
        &pool,
        &bus,
        "binance",
        &[
            "https://api.binance.com".to_string(),
            "https://fapi.binance.com".to_string(),
        ],
        crate::core::AuthType::ApiKey,
        "the-hmac-key",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    for host in ["https://api.binance.com", "https://fapi.binance.com"] {
        check_credential_scope(&pool, "binance", host)
            .await
            .unwrap_or_else(|e| panic!("{host} is declared, so it is in scope: {}", e.1));
    }

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_multi_scope_credential_is_still_refused_outside_its_set() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    let bus = crate::test_support::offline_event_bus();
    CredentialStore::upsert(
        &pool,
        &bus,
        "binance",
        &[
            "https://api.binance.com".to_string(),
            "https://fapi.binance.com".to_string(),
        ],
        crate::core::AuthType::ApiKey,
        "the-hmac-key",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    for elsewhere in [
        "https://evil.test",
        "https://dapi.binance.com",
        "https://binance.com",
        "https://api.binance.com.evil.test",
        "http://api.binance.com",
    ] {
        let err = check_credential_scope(&pool, "binance", elsewhere)
            .await
            .expect_err("a host outside the declared set must be refused");
        assert_eq!(err.0, StatusCode::BAD_GATEWAY);
        assert!(err.1.contains("will not be sent"), "{}", err.1);
        assert!(err.1.contains("https://api.binance.com"), "{}", err.1);
        assert!(err.1.contains("https://fapi.binance.com"), "{}", err.1);
        assert!(err.1.contains("Settings"), "{}", err.1);
        assert!(err.1.contains("set-base-urls"), "{}", err.1);
        assert!(
            !err.1.contains(&format!("--url {elsewhere}")),
            "the refusal must not pre-fill a grant for the rejected host: {}",
            err.1
        );
    }

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_unscoped_credential_is_not_sent_anywhere() {
    use crate::test_support::{seed_credential, setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    seed_credential(
        &pool,
        "unscoped-key",
        "",
        crate::core::AuthType::Bearer,
        "sk-live-secret",
    )
    .await;

    let err = check_credential_scope(&pool, "unscoped-key", "https://api.example.test")
        .await
        .expect_err("an unscoped credential must be refused");
    assert!(err.1.contains("no base URL"), "{}", err.1);
    assert!(err.1.contains("Settings"), "{}", err.1);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[test]
fn credential_scopes_collect_every_naming_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    std::fs::create_dir_all(ws.join("data/config")).unwrap();
    std::fs::write(
        ws.join("data/config/apis.json"),
        r#"{
          "one": {"base_url": "https://a.test", "auth": {"pipeline": [
            {"type": "static_credential", "kind": "bearer", "credential": "only-here"},
            {"type": "script_handshake", "credential": "shared", "script": "scripts/auth/x.py"}
          ]}},
          "two": {"base_url": "https://b.test", "auth": {"pipeline": [
            {"type": "hmac_signed", "key_credential": "shared",
             "secret_credential": "sec", "algorithm": "sha256",
             "signed_payload": "query_string"},
            {"type": "wasm_signer", "module": "m",
             "credential_handles": [{"name": "h", "credential": "wasm-cred"}]}
          ]}}
        }"#,
    )
    .unwrap();

    let scopes = load_proxy_config(ws).credential_scopes();
    assert_eq!(
        scopes
            .get("only-here")
            .map(|s| s.iter().collect::<Vec<_>>()),
        Some(vec![&"https://a.test".to_string()])
    );
    assert_eq!(scopes.get("shared").map(|s| s.len()), Some(2));
    assert_eq!(
        scopes.get("sec").map(|s| s.iter().collect::<Vec<_>>()),
        Some(vec![&"https://b.test".to_string()])
    );
    assert_eq!(
        scopes
            .get("wasm-cred")
            .map(|s| s.iter().collect::<Vec<_>>()),
        Some(vec![&"https://b.test".to_string()])
    );
}

async fn mark_undeclared(pool: &sqlx::PgPool, service_name: &str) {
    sqlx::query("UPDATE credentials SET scope_declared = false WHERE service_name = $1")
        .bind(service_name)
        .execute(pool)
        .await
        .expect("mark the credential undeclared");
}

fn configured(
    entries: &[(&str, &[&str])],
) -> std::collections::BTreeMap<String, std::collections::BTreeSet<String>> {
    entries
        .iter()
        .map(|(name, urls)| {
            (
                name.to_string(),
                urls.iter().map(|u| u.to_string()).collect(),
            )
        })
        .collect()
}

async fn stored_scope(pool: &sqlx::PgPool, service_name: &str) -> Vec<String> {
    CredentialStore::get(pool, service_name)
        .await
        .unwrap()
        .expect("the row exists")
        .base_urls
}

#[tokio::test]
async fn inferring_a_scope_runs_once() {
    use crate::test_support::{seed_credential, setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    let bus = crate::test_support::offline_event_bus();
    seed_credential(
        &pool,
        "legacy-key",
        "",
        crate::core::AuthType::Bearer,
        "sk-legacy",
    )
    .await;
    mark_undeclared(&pool, "legacy-key").await;

    let scoped = CredentialStore::infer_undeclared_scopes(
        &pool,
        &bus,
        &configured(&[("legacy-key", &["https://api.first.test"])]),
    )
    .await
    .unwrap();
    assert_eq!(
        scoped,
        vec![(
            "legacy-key".to_string(),
            "https://api.first.test".to_string()
        )],
        "an undeclared, unscoped credential takes the inferred scope"
    );

    let rescoped = CredentialStore::infer_undeclared_scopes(
        &pool,
        &bus,
        &configured(&[("legacy-key", &["https://api.other.test"])]),
    )
    .await
    .unwrap();
    assert!(rescoped.is_empty(), "a second start infers nothing");
    assert_eq!(
        stored_scope(&pool, "legacy-key").await,
        vec!["https://api.first.test".to_string()],
        "the inference fills the set once, and never appends to it"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn inferring_a_scope_never_fills_a_scope_the_user_emptied() {
    use crate::test_support::{seed_credential, setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    let bus = crate::test_support::offline_event_bus();
    seed_credential(
        &pool,
        "narrowed-key",
        "https://api.example.test",
        crate::core::AuthType::Bearer,
        "sk-live-secret",
    )
    .await;
    let id = CredentialStore::get(&pool, "narrowed-key")
        .await
        .unwrap()
        .expect("the row exists")
        .id;
    CredentialStore::set_base_urls(&pool, &bus, id, &[], None)
        .await
        .unwrap()
        .expect("the row exists");

    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("data/config")).unwrap();
    std::fs::write(
        tmp.path().join("data/config/apis.json"),
        r#"{"app-entry": {"base_url": "https://attacker.test", "auth": {"pipeline": [
            {"type": "static_credential", "kind": "bearer", "credential": "narrowed-key"}
        ]}}}"#,
    )
    .unwrap();
    let scopes = load_proxy_config(tmp.path()).credential_scopes();

    let scoped = CredentialStore::infer_undeclared_scopes(&pool, &bus, &scopes)
        .await
        .unwrap();
    assert!(scoped.is_empty(), "nothing is inferred: {scoped:?}");
    assert!(
        stored_scope(&pool, "narrowed-key").await.is_empty(),
        "the empty scope the user chose survives the start"
    );
    let err = check_credential_scope(&pool, "narrowed-key", "https://attacker.test")
        .await
        .expect_err("an unscoped credential is refused");
    assert_eq!(err.0, StatusCode::BAD_GATEWAY);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn inferring_a_scope_never_reaches_a_type_that_is_not_sent() {
    use crate::test_support::{seed_credential, setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    let bus = crate::test_support::offline_event_bus();
    for (name, auth_type) in [
        ("deploys-github", crate::core::AuthType::Secret),
        ("dropbox", crate::core::AuthType::OauthClient),
    ] {
        seed_credential(&pool, name, "", auth_type, "value").await;
        mark_undeclared(&pool, name).await;
    }

    let scoped = CredentialStore::infer_undeclared_scopes(
        &pool,
        &bus,
        &configured(&[
            ("deploys-github", &["https://attacker.test"]),
            ("dropbox", &["https://attacker.test"]),
        ]),
    )
    .await
    .unwrap();
    assert!(scoped.is_empty(), "nothing is inferred: {scoped:?}");
    for name in ["deploys-github", "dropbox"] {
        let row = sqlx::query_scalar::<_, Vec<String>>(
            "SELECT base_urls FROM credentials WHERE service_name = $1",
        )
        .bind(name)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(row.is_empty(), "{name} stays unscoped");
    }

    pool.close().await;
    teardown_test_db(&db).await;
}

fn offline_scope_ctx<'a>(
    workspace: &'a std::path::Path,
    pool: &'a sqlx::PgPool,
    bus: &'a crate::engine::event_bus::EventBus,
) -> ScopeContext<'a> {
    ScopeContext {
        pool,
        workspace_path: workspace,
        event_bus: bus,
    }
}

fn unreachable_pool() -> sqlx::PgPool {
    sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgres://nobody:nobody@127.0.0.1:1/nobody")
        .expect("connect_lazy never errors on parse-only failure")
}

fn handshake_layer(script: &str) -> Vec<Arc<dyn crate::proxy_auth_layer::AuthLayer>> {
    handshake_layer_injecting(script, None, &[])
}

fn handshake_layer_injecting(
    script: &str,
    credential: Option<&str>,
    oauth_providers: &[&str],
) -> Vec<Arc<dyn crate::proxy_auth_layer::AuthLayer>> {
    vec![Arc::new(
        crate::api::proxy_script_layer::ScriptHandshakeLayer::new(
            "script_handshake".into(),
            "comfort".into(),
            credential.map(str::to_string),
            script.into(),
            "https://accsmart.panasonic.test".into(),
            oauth_providers.iter().map(|p| p.to_string()).collect(),
            unreachable_pool(),
            Arc::new(std::path::PathBuf::from("/ws")),
            Arc::new(crate::proxy_token_cache::ProxyTokenCache::new()),
            Arc::new(crate::api::proxy_script_layer::DbOAuthLookup::new(
                unreachable_pool(),
            )),
        ),
    )]
}

fn approve(workspace: &std::path::Path, script: &str) {
    const BODY: &[u8] = b"print(1)";
    let rel = crate::core::handshake_approvals::config_path_key(script);
    let abs = workspace.join(&rel);
    std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
    std::fs::write(&abs, BODY).unwrap();
    crate::core::handshake_approvals::record(workspace, &rel, BODY).unwrap();
}

#[tokio::test]
async fn a_credential_less_handshake_is_refused_off_its_bound_host() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(ws, &pool, &bus);
    approve(ws, "scripts/auth/comfort.py");

    ScopedPipeline::bind(
        &ctx,
        "comfort",
        "https://accsmart.panasonic.test".to_string(),
        handshake_layer("scripts/auth/comfort.py"),
        false,
    )
    .await
    .expect("the first call binds the script to its own upstream");
    assert_eq!(
        crate::core::handshake_approvals::scope_for(ws, "data/scripts/auth/comfort.py").as_deref(),
        Some("https://accsmart.panasonic.test")
    );

    let err = ScopedPipeline::bind(
        &ctx,
        "comfort",
        "https://evil.test".to_string(),
        handshake_layer("scripts/auth/comfort.py"),
        false,
    )
    .await
    .expect_err("a rewritten base_url must not receive the minted token");
    assert_eq!(err.0, StatusCode::BAD_GATEWAY);
    assert!(err.1.contains("data/scripts/auth/comfort.py"), "{}", err.1);
    assert!(
        err.1.contains("https://accsmart.panasonic.test"),
        "{}",
        err.1
    );
}

#[tokio::test]
async fn a_handshake_credential_scoped_to_its_token_endpoint_still_works() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(ws, &pool, &bus);
    approve(ws, "scripts/auth/google-oauth-bearer.py");

    ScopedPipeline::bind(
        &ctx,
        "google-calendar",
        "https://www.googleapis.test".to_string(),
        handshake_layer_injecting(
            "scripts/auth/google-oauth-bearer.py",
            Some("oauth:google"),
            &["google"],
        ),
        false,
    )
    .await
    .expect("a handshake credential does not travel to the entry's base_url");

    assert_eq!(
        crate::core::handshake_approvals::injects_for(
            ws,
            "data/scripts/auth/google-oauth-bearer.py"
        ),
        Some(
            ["c:oauth:google", "o:google"]
                .into_iter()
                .map(str::to_string)
                .collect()
        ),
        "the first call records what the script may be handed"
    );
}

#[tokio::test]
async fn a_handshake_script_is_refused_a_secret_it_was_not_approved_with() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(ws, &pool, &bus);
    approve(ws, "scripts/auth/exchange.py");

    let bind = |layers| {
        ScopedPipeline::bind(
            &ctx,
            "storage",
            "https://storage.test".to_string(),
            layers,
            false,
        )
    };
    bind(handshake_layer_injecting(
        "scripts/auth/exchange.py",
        Some("firebase-web-api-key"),
        &["google"],
    ))
    .await
    .expect("the first call binds what the entry then asked for");

    for (credential, providers) in [
        (Some("openrouter"), &["google"][..]),
        (Some("firebase-web-api-key"), &["google", "microsoft"][..]),
        (None, &["google"][..]),
    ] {
        let err = bind(handshake_layer_injecting(
            "scripts/auth/exchange.py",
            credential,
            providers,
        ))
        .await
        .expect_err("a rewritten entry must not change what the script receives");
        assert_eq!(err.0, StatusCode::BAD_GATEWAY);
        assert!(err.1.contains("c:firebase-web-api-key"), "{}", err.1);
        assert!(err.1.contains("approved to receive"), "{}", err.1);
    }
}

#[tokio::test]
async fn a_credential_name_that_would_corrupt_the_record_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(ws, &pool, &bus);
    approve(ws, "scripts/auth/x.py");

    for bad in ["my key", "a,b"] {
        let err = ScopedPipeline::bind(
            &ctx,
            "p",
            "https://api.test".to_string(),
            handshake_layer_injecting("scripts/auth/x.py", Some(bad), &[]),
            false,
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, StatusCode::BAD_GATEWAY);
        assert!(err.1.contains("Rename it in Settings"), "{}", err.1);
    }
    assert!(
        crate::core::handshake_approvals::injects_for(ws, "data/scripts/auth/x.py").is_none(),
        "a refused set must not reach the record"
    );
}

#[tokio::test]
async fn a_second_first_call_is_checked_against_the_scope_that_won() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(ws, &pool, &bus);
    approve(ws, "scripts/auth/x.py");
    crate::core::handshake_approvals::bind_scope_if_absent(
        ws,
        "data/scripts/auth/x.py",
        "https://first.test",
    )
    .unwrap();

    ScopedPipeline::bind(
        &ctx,
        "p",
        "https://second.test".to_string(),
        handshake_layer("scripts/auth/x.py"),
        false,
    )
    .await
    .expect_err("the losing racer must be refused, not passed");
}

#[tokio::test]
async fn a_deeper_path_on_the_bound_host_still_passes() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(ws, &pool, &bus);
    approve(ws, "scripts/auth/x.py");
    crate::core::handshake_approvals::bind_scope_if_absent(
        ws,
        "data/scripts/auth/x.py",
        "https://api.test",
    )
    .unwrap();

    ScopedPipeline::bind(
        &ctx,
        "p",
        "https://api.test/v2".to_string(),
        handshake_layer("scripts/auth/x.py"),
        false,
    )
    .await
    .expect("a deeper path on the bound host is the same API");
}

#[tokio::test]
async fn an_unapproved_script_is_left_to_the_runner() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(ws, &pool, &bus);

    ScopedPipeline::bind(
        &ctx,
        "p",
        "https://anywhere.test".to_string(),
        handshake_layer("scripts/auth/never-approved.py"),
        false,
    )
    .await
    .expect("the gate has no approval to scope, so it says nothing");
    assert!(
        crate::core::handshake_approvals::entries(ws).is_empty(),
        "binding must never invent an approval"
    );
}

#[tokio::test]
async fn the_builtin_local_key_is_refused_outside_its_credential_scope() {
    use crate::test_support::{seed_credential, setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    let bus = crate::test_support::offline_event_bus();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = offline_scope_ctx(tmp.path(), &pool, &bus);

    seed_credential(
        &pool,
        "local",
        "http://localhost:1234/v1",
        crate::core::AuthType::Bearer,
        "local-key",
    )
    .await;

    let local_bearer = || -> Vec<Arc<dyn crate::proxy_auth_layer::AuthLayer>> {
        vec![Arc::new(
            crate::api::proxy_static_layers::StaticHeaderLayer::bearer(
                "local".into(),
                "local-key".into(),
                crate::proxy_auth_layer::ScopeBinding::StoredCredential("local".into()),
            ),
        )]
    };

    let err = ScopedPipeline::bind(
        &ctx,
        "local",
        "https://evil.test".to_string(),
        local_bearer(),
        false,
    )
    .await
    .expect_err("the local key must not follow a rewritten preference");
    assert_eq!(err.0, StatusCode::BAD_GATEWAY);
    assert!(err.1.contains("local"), "{}", err.1);
    assert!(err.1.contains("will not be sent"), "{}", err.1);

    ScopedPipeline::bind(
        &ctx,
        "local",
        "http://localhost:1234/v1".to_string(),
        local_bearer(),
        false,
    )
    .await
    .expect("the scoped local backend keeps working");

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_gate_binds_one_credential_for_two_declared_hosts() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    let bus = crate::test_support::offline_event_bus();
    let tmp = tempfile::tempdir().unwrap();
    let ctx = offline_scope_ctx(tmp.path(), &pool, &bus);

    CredentialStore::upsert(
        &pool,
        &bus,
        "binance",
        &[
            "https://api.binance.com".to_string(),
            "https://fapi.binance.com".to_string(),
        ],
        crate::core::AuthType::ApiKey,
        "the-hmac-key",
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let signed = || -> Vec<Arc<dyn crate::proxy_auth_layer::AuthLayer>> {
        vec![Arc::new(
            crate::api::proxy_static_layers::StaticHeaderLayer::bearer(
                "binance".into(),
                "the-hmac-key".into(),
                crate::proxy_auth_layer::ScopeBinding::StoredCredential("binance".into()),
            ),
        )]
    };

    for (entry, host) in [
        ("binance", "https://api.binance.com"),
        ("binance-futures", "https://fapi.binance.com"),
    ] {
        ScopedPipeline::bind(&ctx, entry, host.to_string(), signed(), false)
            .await
            .unwrap_or_else(|e| panic!("{entry} must reach {host}: {}", e.1));
    }

    let err = ScopedPipeline::bind(
        &ctx,
        "binance-elsewhere",
        "https://evil.test".to_string(),
        signed(),
        false,
    )
    .await
    .expect_err("a third host was never declared");
    assert_eq!(err.0, StatusCode::BAD_GATEWAY);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_env_supplied_local_key_follows_the_env_base_url() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(tmp.path(), &pool, &bus);

    let env_keyed = || -> Vec<Arc<dyn crate::proxy_auth_layer::AuthLayer>> {
        vec![Arc::new(
            crate::api::proxy_static_layers::StaticHeaderLayer::bearer(
                "local".into(),
                "env-key".into(),
                crate::proxy_auth_layer::ScopeBinding::Pinned {
                    what: "LUCIDOS_LOCAL_API_KEY".into(),
                    base_url: "http://localhost:11434/v1".into(),
                },
            ),
        )]
    };

    ScopedPipeline::bind(
        &ctx,
        "local",
        "http://localhost:11434/v1".to_string(),
        env_keyed(),
        false,
    )
    .await
    .expect("the env base URL is where an env key belongs");

    let err = ScopedPipeline::bind(
        &ctx,
        "local",
        "https://evil.test".to_string(),
        env_keyed(),
        false,
    )
    .await
    .expect_err("a preference cannot redirect an env-supplied key");
    assert!(err.1.contains("LUCIDOS_LOCAL_API_KEY"), "{}", err.1);
}

fn layers_pinned_to(base_url: &str) -> Vec<Arc<dyn crate::proxy_auth_layer::AuthLayer>> {
    vec![Arc::new(
        crate::api::proxy_static_layers::StaticHeaderLayer::bearer(
            "k".into(),
            "sk-live".into(),
            crate::proxy_auth_layer::ScopeBinding::Pinned {
                what: "the test key".into(),
                base_url: base_url.to_string(),
            },
        ),
    )]
}

#[tokio::test]
async fn a_credential_over_plaintext_needs_the_opt_in() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(tmp.path(), &pool, &bus);

    let public = "http://api.example.test";
    let err = ScopedPipeline::bind(
        &ctx,
        "p",
        public.to_string(),
        layers_pinned_to(public),
        false,
    )
    .await
    .expect_err("a credential over plaintext must be refused by default");
    assert_eq!(err.0, StatusCode::BAD_GATEWAY);
    assert!(err.1.contains("insecure_transport"), "{}", err.1);

    ScopedPipeline::bind(
        &ctx,
        "p",
        public.to_string(),
        layers_pinned_to(public),
        true,
    )
    .await
    .expect("the opt-in is what makes it allowed");

    for loopback in [
        "http://localhost:5005",
        "http://127.0.0.1:11434/v1",
        "http://[::1]:8080",
    ] {
        ScopedPipeline::bind(
            &ctx,
            "p",
            loopback.to_string(),
            layers_pinned_to(loopback),
            false,
        )
        .await
        .unwrap_or_else(|e| panic!("loopback must keep working: {loopback} gave {}", e.1));
    }

    ScopedPipeline::bind(&ctx, "p", public.to_string(), Vec::new(), false)
        .await
        .expect("an uncredentialed plaintext entry is unchanged");
}

#[tokio::test]
async fn a_base_url_with_no_host_is_refused_at_the_gate() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(tmp.path(), &pool, &bus);

    for bad in ["", "not a url", "file:///etc/passwd"] {
        let err = ScopedPipeline::bind(&ctx, "p", bad.to_string(), Vec::new(), false)
            .await
            .expect_err("a base_url with no host must be refused");
        assert_eq!(err.0, StatusCode::BAD_GATEWAY, "{bad}");
    }
}

#[test]
fn the_load_names_every_entry_it_will_not_vouch_for() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace_with_apis_json(
        &tmp,
        r#"{
          "opted-in":  {"base_url": "https://self-signed.test", "insecure_transport": true},
          "plaintext": {"base_url": "http://api.public.test", "auth": {"pipeline": [
            {"type": "static_credential", "kind": "bearer", "credential": "k"}
          ]}},
          "loopback":  {"base_url": "http://localhost:5005", "auth": {"pipeline": [
            {"type": "static_credential", "kind": "bearer", "credential": "k"}
          ]}},
          "fine":      {"base_url": "https://api.good.test", "auth": {"pipeline": [
            {"type": "static_credential", "kind": "bearer", "credential": "k"}
          ]}}
        }"#,
    );

    let load = load_proxy_config(&ws);
    let named: Vec<&str> = load.insecure.iter().map(|i| i.provider.as_str()).collect();
    assert_eq!(named, vec!["opted-in", "plaintext"]);
    assert!(load.insecure[0].reason.contains("insecure_transport is on"));
    assert!(load.insecure[1].reason.contains("not loopback"));
}

async fn spawn_self_signed_upstream() -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (cert_pem, key_pem) = crate::test_support::self_signed_pem();
    let cert = openssl::x509::X509::from_pem(&cert_pem).unwrap();
    let key = openssl::pkey::PKey::private_key_from_pem(&key_pem).unwrap();
    const BUNDLE_PASS: &str = "handshake-test";
    let pkcs12 = openssl::pkcs12::Pkcs12::builder()
        .name("test")
        .pkey(&key)
        .cert(&cert)
        .build2(BUNDLE_PASS)
        .unwrap()
        .to_der()
        .unwrap();
    let identity = native_tls::Identity::from_pkcs12(&pkcs12, BUNDLE_PASS).unwrap();
    let acceptor: tokio_native_tls::TlsAcceptor =
        native_tls::TlsAcceptor::new(identity).unwrap().into();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        for _ in 0..2 {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(stream).await else {
                    return;
                };
                let mut buf = [0u8; 1024];
                let _ = tls.read(&mut buf).await;
                let _ = tls
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok")
                    .await;
                let _ = tls.shutdown().await;
            });
        }
    });
    format!("https://127.0.0.1:{}", addr.port())
}

#[tokio::test]
async fn a_provider_without_the_opt_in_refuses_an_invalid_certificate() {
    let base = spawn_self_signed_upstream().await;
    let url = format!("{base}/v1/items");

    let refused = forward_request(
        Method::GET,
        &url,
        &url,
        HeaderMap::new(),
        Vec::new(),
        Bytes::new(),
        Transport::Verified,
        test_timeout(),
    )
    .await;
    assert_eq!(
        refused.status(),
        StatusCode::BAD_GATEWAY,
        "a self-signed certificate must not be accepted by default"
    );

    let accepted = forward_request(
        Method::GET,
        &url,
        &url,
        HeaderMap::new(),
        Vec::new(),
        Bytes::new(),
        Transport::Unverified,
        test_timeout(),
    )
    .await;
    assert_eq!(
        accepted.status(),
        StatusCode::OK,
        "the self-signed dev workflow keeps working behind the opt-in"
    );
    assert_eq!(body_text(accepted).await, "ok");
}

#[tokio::test]
async fn the_transport_follows_the_entry_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(tmp.path(), &pool, &bus);

    let verified = ScopedPipeline::bind(&ctx, "p", "https://a.test".into(), Vec::new(), false)
        .await
        .unwrap();
    let unverified = ScopedPipeline::bind(&ctx, "p", "https://a.test".into(), Vec::new(), true)
        .await
        .unwrap();
    assert_eq!(verified.transport, Transport::Verified);
    assert_eq!(unverified.transport, Transport::Unverified);
}

async fn spawn_slow_upstream(delay: std::time::Duration) -> String {
    let app = Router::new().fallback(any(move || async move {
        tokio::time::sleep(delay).await;
        "ok"
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn an_upstream_slower_than_the_timeout_answers_504_without_the_url() {
    let base = spawn_slow_upstream(std::time::Duration::from_secs(3)).await;
    let url = format!("{base}/v1/slow?key=secret-value");
    let log_url = redacted_query_log_url(&format!("{base}/v1/slow"), "key");
    let resp = forward_request(
        Method::GET,
        &url,
        &log_url,
        HeaderMap::new(),
        Vec::new(),
        Bytes::new(),
        Transport::Verified,
        std::time::Duration::from_millis(300),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::GATEWAY_TIMEOUT);
    let body = body_text(resp).await;
    assert!(body.contains("upstream timeout"), "{body}");
    assert!(
        !body.contains("secret-value"),
        "the credential leaked: {body}"
    );
    assert!(!body.contains("/v1/slow"), "the URL leaked: {body}");
}

#[tokio::test]
async fn an_upstream_within_the_timeout_answers_normally() {
    let base = spawn_slow_upstream(std::time::Duration::from_millis(1500)).await;
    let url = format!("{base}/v1/slow");
    let resp = forward_request(
        Method::GET,
        &url,
        &url,
        HeaderMap::new(),
        Vec::new(),
        Bytes::new(),
        Transport::Verified,
        std::time::Duration::from_secs(5),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_text(resp).await, "ok");
}

#[tokio::test]
async fn an_entry_timeout_reaches_the_forward() {
    let base = spawn_slow_upstream(std::time::Duration::from_millis(1500)).await;
    let tmp = tempfile::tempdir().unwrap();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(tmp.path(), &pool, &bus);
    let scoped = ScopedPipeline::bind(&ctx, "slow", base.clone(), Vec::new(), false)
        .await
        .expect("an unauthenticated loopback entry binds");
    for (entry_secs, expected) in [(5.0, StatusCode::OK), (1.0, StatusCode::GATEWAY_TIMEOUT)] {
        let timeout = crate::api::proxy_timeout::effective(Some(entry_secs), Some("600")).unwrap();
        let resp = dispatch_scoped(
            "slow",
            &scoped,
            timeout,
            Method::GET,
            "v1/slow".to_string(),
            None,
            HeaderMap::new(),
            Bytes::new(),
        )
        .await
        .expect("dispatch");
        assert_eq!(resp.status(), expected, "timeout_secs = {entry_secs}");
    }
}

#[test]
fn an_entry_timeout_out_of_range_refuses_only_that_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace_with_apis_json(
        &tmp,
        r#"{
          "patient": {"base_url": "https://patient.test", "timeout_secs": 300},
          "too-long": {"base_url": "https://long.test", "timeout_secs": 601},
          "zero": {"base_url": "https://zero.test", "timeout_secs": 0}
        }"#,
    );
    let load = load_proxy_config(&ws);
    assert_eq!(load.providers["patient"].timeout_secs, Some(300.0));
    assert_eq!(
        rejected_names(&ws),
        vec!["too-long".to_string(), "zero".to_string()]
    );
    let reason = &load
        .rejected
        .iter()
        .find(|r| r.provider.as_deref() == Some("too-long"))
        .unwrap()
        .reason;
    assert!(reason.contains("'timeout_secs'"), "{reason}");
    assert!(reason.contains("between 1 and 600"), "{reason}");
}

#[tokio::test]
async fn a_slow_redirect_chain_is_cut_by_the_call_cap() {
    let app = Router::new().fallback(any(|| async {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        (
            StatusCode::FOUND,
            [(axum::http::header::LOCATION, "/hop")],
            "",
        )
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let tmp = tempfile::tempdir().unwrap();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(tmp.path(), &pool, &bus);
    let scoped = ScopedPipeline::bind(&ctx, "chain", base, Vec::new(), false)
        .await
        .expect("an unauthenticated loopback entry binds");

    let started = std::time::Instant::now();
    let budget = CallBudget::with_cap(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(1000),
    );
    let status = match forward_with_redirects(
        "chain",
        &scoped,
        &budget,
        &Method::GET,
        "hop",
        None,
        &HeaderMap::new(),
        &Bytes::new(),
    )
    .await
    {
        Ok((resp, _)) => resp.status(),
        Err((status, _)) => status,
    };
    assert_eq!(status, StatusCode::GATEWAY_TIMEOUT);
    assert!(
        started.elapsed() < std::time::Duration::from_millis(1800),
        "the cap must stop the chain near 1 s, took {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn a_body_that_outlasts_the_timeout_answers_504() {
    let app = Router::new().fallback(any(|| async {
        let chunks = futures::StreamExt::then(futures::stream::iter(0..3), |i| async move {
            if i > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            }
            Ok::<_, std::io::Error>(Bytes::from_static(b"data: chunk\n\n"))
        });
        axum::response::Response::builder()
            .status(StatusCode::OK)
            .header("content-type", "text/event-stream")
            .body(Body::from_stream(chunks))
            .unwrap()
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/v1/stream", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let resp = forward_request(
        Method::POST,
        &url,
        &url,
        HeaderMap::new(),
        Vec::new(),
        Bytes::new(),
        Transport::Verified,
        std::time::Duration::from_millis(300),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::GATEWAY_TIMEOUT);
    let body = body_text(resp).await;
    assert!(body.starts_with("upstream timeout"), "{body}");
}

struct SlowLayer;

#[async_trait::async_trait]
impl crate::proxy_auth_layer::AuthLayer for SlowLayer {
    fn output_namespace(&self) -> &str {
        "slow"
    }
    fn scope_bindings(&self) -> Vec<crate::proxy_auth_layer::ScopeBinding> {
        Vec::new()
    }
    async fn apply(
        &self,
        _input: &crate::proxy_auth_layer::LayerInput<'_>,
    ) -> Result<crate::proxy_auth_layer::AuthMutation, (StatusCode, String)> {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        Ok(Default::default())
    }
}

#[tokio::test]
async fn a_slow_pipeline_step_is_cut_by_the_call_cap() {
    let (base, slot) = spawn_recording_upstream(200, "ok").await;
    let tmp = tempfile::tempdir().unwrap();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(tmp.path(), &pool, &bus);
    let layers: Vec<Arc<dyn crate::proxy_auth_layer::AuthLayer>> = vec![Arc::new(SlowLayer)];
    let scoped = ScopedPipeline::bind(&ctx, "slow-login", base, layers, false)
        .await
        .expect("a loopback upstream binds");
    let budget = CallBudget::with_cap(
        std::time::Duration::from_secs(5),
        std::time::Duration::from_millis(500),
    );
    let started = std::time::Instant::now();
    let err = forward_with_redirects(
        "slow-login",
        &scoped,
        &budget,
        &Method::GET,
        "v1/items",
        None,
        &HeaderMap::new(),
        &Bytes::new(),
    )
    .await
    .expect_err("the call runs out of time inside the pipeline");
    assert_eq!(err.0, StatusCode::GATEWAY_TIMEOUT, "{}", err.1);
    assert!(
        started.elapsed() < std::time::Duration::from_millis(1500),
        "the cap must stop the slow step, took {:?}",
        started.elapsed()
    );
    assert!(
        slot.lock().unwrap().is_none(),
        "no request may reach the upstream"
    );
}

const ECHOED_TOKEN: &str = "echo-bearer-token-1234";
const ECHOED_QUERY_KEY: &str = "qk+echo/secret=";
const ECHOED_QUERY_KEY_ENCODED: &str = "qk%2Becho%2Fsecret%3D";

async fn credentialed_pipeline(base_url: &str) -> ScopedPipeline {
    use crate::api::proxy_static_layers::{QueryParamLayer, StaticHeaderLayer};
    use crate::proxy_auth_layer::{AuthLayer, ScopeBinding};
    let pinned = |what: &str| ScopeBinding::Pinned {
        what: what.to_string(),
        base_url: base_url.to_string(),
    };
    let layers: Vec<Arc<dyn AuthLayer>> = vec![
        Arc::new(StaticHeaderLayer::bearer(
            "bearer".into(),
            ECHOED_TOKEN.into(),
            pinned("the bearer token"),
        )),
        Arc::new(QueryParamLayer::new(
            "query".into(),
            "api_key".into(),
            ECHOED_QUERY_KEY.into(),
            pinned("the query key"),
        )),
    ];
    let tmp = tempfile::tempdir().unwrap();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(tmp.path(), &pool, &bus);
    ScopedPipeline::bind(&ctx, "echo", base_url.to_string(), layers, false)
        .await
        .expect("a loopback upstream binds")
}

async fn spawn_echoing_upstream() -> String {
    let shutdown = Arc::new(tokio::sync::Notify::new());
    let shutdown_from_handler = shutdown.clone();
    let app = Router::new().fallback(any(move |req: axum::extract::Request| {
        let shutdown = shutdown_from_handler.clone();
        async move {
            shutdown.notify_one();
            let auth = req
                .headers()
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_string();
            let raw_query = req.uri().query().unwrap_or_default().to_string();
            let parsed =
                reqwest::Url::parse(&format!("http://upstream.test/?{raw_query}")).unwrap();
            let key = parsed
                .query_pairs()
                .find(|(name, _)| name == "api_key")
                .map(|(_, value)| value.into_owned())
                .unwrap_or_default();
            let body = format!(r#"{{"error":"bad page","auth":"{auth}","key":"{key}"}}"#);
            let mut resp = (StatusCode::BAD_REQUEST, body).into_response();
            resp.headers_mut().insert(
                axum::http::header::LINK,
                HeaderValue::from_str(&format!(
                    "<https://upstream.test/items?{raw_query}&page=2>; rel=\"next\""
                ))
                .unwrap(),
            );
            resp.headers_mut().insert(
                "x-echo-authorization",
                HeaderValue::from_str(&auth).unwrap(),
            );
            resp
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
    format!("http://{}", addr)
}

async fn forward_items(
    scoped: &ScopedPipeline,
    headers: &HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    forward_with_redirects(
        "echo",
        scoped,
        &CallBudget::start(test_timeout()),
        &Method::GET,
        "items",
        None,
        headers,
        &Bytes::new(),
    )
    .await
    .map(|(resp, _)| resp)
}

fn assert_carries_no_injected_credential(what: &str, text: &str) {
    for secret in [ECHOED_TOKEN, ECHOED_QUERY_KEY, ECHOED_QUERY_KEY_ENCODED] {
        assert!(
            !text.contains(secret),
            "{what} carried the injected credential {secret:?}: {text}"
        );
    }
}

#[tokio::test]
async fn an_upstream_echoing_the_injected_credential_returns_none_of_it() {
    let base = spawn_echoing_upstream().await;
    let scoped = credentialed_pipeline(&base).await;

    let resp = forward_items(&scoped, &HeaderMap::new())
        .await
        .expect("the echo reply is forwarded");

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    for (name, value) in resp.headers() {
        let value = String::from_utf8_lossy(value.as_bytes());
        assert_carries_no_injected_credential(&format!("header {name}"), &value);
    }
    let link = header_values(&resp, "link").join(", ");
    assert!(
        link.contains("page=2"),
        "the rest of the Link survives: {link}"
    );
    assert!(link.contains(crate::core::REDACTED_MARKER), "{link}");
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let body = String::from_utf8(body.to_vec()).unwrap();
    assert_carries_no_injected_credential("the body", &body);
    assert!(
        body.contains("bad page"),
        "the rest of the body survives: {body}"
    );
    assert!(body.contains(crate::core::REDACTED_MARKER), "{body}");
}

#[tokio::test]
async fn a_reply_without_the_credential_passes_byte_identical() {
    let (base, _slot) = spawn_recording_upstream(200, "plain upstream body").await;
    let scoped = credentialed_pipeline(&base).await;

    let resp = forward_items(&scoped, &HeaderMap::new())
        .await
        .expect("the reply is forwarded");

    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(header_values(&resp, "content-length"), vec!["19"]);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"plain upstream body");
}

#[tokio::test]
async fn a_refused_redirect_does_not_quote_the_injected_credential() {
    let location = format!("data:text/plain,{ECHOED_TOKEN}");
    let base = spawn_redirecting_upstream(302, Box::leak(location.into_boxed_str())).await;
    let scoped = credentialed_pipeline(&base).await;

    let err = forward_items(&scoped, &HeaderMap::new())
        .await
        .expect_err("a Location with no host is refused");

    assert_eq!(err.0, StatusCode::BAD_GATEWAY);
    assert!(!err.1.contains("data:text/plain,"), "{}", err.1);
    assert_carries_no_injected_credential("the refusal", &err.1);
}

#[tokio::test]
async fn a_credentialed_call_asks_for_an_uncompressed_reply() {
    let (base, slot) = spawn_recording_upstream(200, "ok").await;
    let scoped = credentialed_pipeline(&base).await;

    forward_items(&scoped, &hm(&[("accept-encoding", "gzip, br")]))
        .await
        .expect("the reply is forwarded");

    let recorded = slot.lock().unwrap().clone().expect("no request recorded");
    let accept_encoding: Vec<&str> = recorded
        .headers
        .iter()
        .filter(|(name, _)| name == "accept-encoding")
        .map(|(_, value)| value.as_str())
        .collect();
    assert_eq!(accept_encoding, vec!["identity"]);
}

#[tokio::test]
async fn an_encoded_reply_to_a_credentialed_call_is_refused() {
    let base = spawn_upstream_with_headers(&[("content-encoding", "gzip")]).await;
    let scoped = credentialed_pipeline(&base).await;

    let err = forward_items(&scoped, &HeaderMap::new())
        .await
        .expect_err("an encoded body cannot be checked for the credential");

    assert_eq!(err.0, StatusCode::BAD_GATEWAY);
    assert!(err.1.contains("gzip"), "{}", err.1);
}

#[tokio::test]
async fn an_unauthenticated_call_keeps_the_upstream_encoding() {
    let base = spawn_upstream_with_headers(&[("content-encoding", "gzip")]).await;
    let scoped = unauthenticated_pipeline(&base).await;

    let resp = forward_items(&scoped, &HeaderMap::new())
        .await
        .expect("an unauthenticated reply is forwarded");

    assert_eq!(header_values(&resp, "content-encoding"), vec!["gzip"]);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(&body[..], b"upstream body");
}

#[test]
fn a_lan_or_tailnet_address_is_not_loopback() {
    for base in [
        "http://192.168.1.20:8080",
        "http://10.0.0.5",
        "http://172.16.0.9",
        "http://100.64.0.7",
        "http://169.254.10.1",
        "http://0.0.0.0:80",
        "http://device.local",
        "http://[fd00::1]:80",
    ] {
        assert!(plaintext_to_a_public_host(base), "{base} read as loopback");
    }
    for base in [
        "http://localhost",
        "http://LOCALHOST:5005",
        "http://127.0.0.1",
        "http://127.0.0.2:80",
        "http://[::1]:8080",
        "https://192.168.1.20",
    ] {
        assert!(!plaintext_to_a_public_host(base), "{base} read as public");
    }
}

#[tokio::test]
async fn a_caller_cannot_remove_or_forge_the_proxy_origin_marker() {
    let (base, slot) = spawn_recording_upstream(200, "ok").await;
    let url = format!("{}/x", base);
    let _ = forward_request(
        Method::GET,
        &url,
        &url,
        hm(&[(PROXY_ORIGIN_HEADER, "forged-by-caller")]),
        Vec::new(),
        Bytes::new(),
        Transport::Verified,
        test_timeout(),
    )
    .await;
    let recorded = slot.lock().unwrap().clone().unwrap();
    assert_eq!(recorded_values(&recorded, PROXY_ORIGIN_HEADER), vec!["1"]);
}

#[test]
fn the_upstream_document_sandbox_is_stricter_than_the_file_sandbox() {
    let tokens = |csp: &str| {
        csp.split_whitespace()
            .map(str::to_string)
            .collect::<std::collections::BTreeSet<_>>()
    };
    let upstream = tokens(PROXIED_DOCUMENT_CSP);
    let files = tokens(crate::api::file_response::DOCUMENT_SANDBOX_CSP);
    assert!(upstream.contains("sandbox"));
    assert!(!upstream.contains("allow-scripts"));
    assert!(upstream.is_subset(&files));
    assert!(upstream.len() < files.len());
}

#[test]
fn a_containment_refusal_never_quotes_the_query_string() {
    let query = Some("api_key=query-secret");
    let deep = format!("%{}", "25".repeat(30_000));
    for path in ["%2e%2e/admin", "%2e%2e%2fadmin", deep.as_str()] {
        let err = build_contained_target_url(PREFIXED_BASE, path, query).unwrap_err();
        assert!(!err.contains("query-secret"), "{err}");
    }
    let err = build_contained_target_url("not a url", "/x", query).unwrap_err();
    assert!(!err.contains("query-secret"), "{err}");
}

#[test]
fn the_log_url_masks_a_credential_the_url_already_carries() {
    let masked = redacted_query_log_url(
        "https://h.test/items?page=2&api_key=real-secret&x=1&api_key=other-secret",
        "api_key",
    );
    assert_eq!(
        masked,
        "https://h.test/items?page=2&api_key=REDACTED&x=1&api_key=REDACTED&api_key=REDACTED"
    );
    assert_eq!(
        redacted_query_log_url("https://h.test/items", "api_key"),
        "https://h.test/items?api_key=REDACTED"
    );
}

#[tokio::test]
async fn the_forwarded_query_carries_the_real_credential_value() {
    let (base, slot) = spawn_recording_upstream(200, "ok").await;
    let scoped = credentialed_pipeline(&base).await;
    forward_items(&scoped, &HeaderMap::new())
        .await
        .expect("the reply is forwarded");
    let recorded = slot.lock().unwrap().clone().expect("no request recorded");
    assert_eq!(
        recorded.query,
        format!("api_key={ECHOED_QUERY_KEY_ENCODED}")
    );
}

#[tokio::test]
async fn a_redirect_to_another_port_of_the_same_host_is_refused() {
    let (other, slot) = spawn_recording_upstream(200, "landed").await;
    let location: &'static str = Box::leak(format!("{other}/x").into_boxed_str());
    let base = spawn_redirecting_upstream(302, location).await;
    let scoped = unauthenticated_pipeline(&base).await;

    let err = forward_with_redirects(
        "backend",
        &scoped,
        &CallBudget::start(test_timeout()),
        &Method::GET,
        "start",
        None,
        &HeaderMap::new(),
        &Bytes::new(),
    )
    .await
    .expect_err("a different origin must not be followed");

    assert_eq!(err.0, StatusCode::BAD_GATEWAY);
    assert!(err.1.contains("different origin"), "{}", err.1);
    assert!(
        slot.lock().unwrap().is_none(),
        "the proxy followed a redirect across ports"
    );
}

struct CountingLayer(Arc<std::sync::atomic::AtomicUsize>);

#[async_trait::async_trait]
impl crate::proxy_auth_layer::AuthLayer for CountingLayer {
    fn output_namespace(&self) -> &str {
        "counting"
    }
    fn scope_bindings(&self) -> Vec<crate::proxy_auth_layer::ScopeBinding> {
        Vec::new()
    }
    async fn apply(
        &self,
        _input: &crate::proxy_auth_layer::LayerInput<'_>,
    ) -> Result<crate::proxy_auth_layer::AuthMutation, (StatusCode, String)> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(Default::default())
    }
}

#[tokio::test]
async fn the_engine_caps_redirects_and_runs_the_pipeline_on_every_hop() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let upstream_hits = Arc::new(AtomicUsize::new(0));
    let hits_in_route = upstream_hits.clone();
    let app = Router::new().fallback(any(move || {
        let hits = hits_in_route.clone();
        async move {
            hits.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::FOUND,
                [(axum::http::header::LOCATION, "/hop")],
                "",
            )
        }
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let layer_runs = Arc::new(AtomicUsize::new(0));
    let layers: Vec<Arc<dyn crate::proxy_auth_layer::AuthLayer>> =
        vec![Arc::new(CountingLayer(layer_runs.clone()))];
    let tmp = tempfile::tempdir().unwrap();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(tmp.path(), &pool, &bus);
    let scoped = ScopedPipeline::bind(&ctx, "loop", base, layers, false)
        .await
        .expect("a loopback upstream binds");

    let err = forward_with_redirects(
        "loop",
        &scoped,
        &CallBudget::start(test_timeout()),
        &Method::GET,
        "hop",
        None,
        &HeaderMap::new(),
        &Bytes::new(),
    )
    .await
    .expect_err("an endless redirect chain must stop");

    assert_eq!(err.0, StatusCode::BAD_GATEWAY);
    assert!(err.1.contains("exceeded"), "{}", err.1);
    assert_eq!(upstream_hits.load(Ordering::SeqCst), MAX_REDIRECT_HOPS);
    assert_eq!(layer_runs.load(Ordering::SeqCst), MAX_REDIRECT_HOPS);
}

#[tokio::test]
async fn a_base_url_that_would_start_a_second_record_is_refused_but_padding_is_not() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(ws, &pool, &bus);
    approve(ws, "scripts/auth/x.py");
    let recorded = || crate::core::handshake_approvals::scope_for(ws, "data/scripts/auth/x.py");

    let err = ScopedPipeline::bind(
        &ctx,
        "p",
        "https://api.test/a\nb".to_string(),
        handshake_layer("scripts/auth/x.py"),
        false,
    )
    .await
    .expect_err("a newline in the base_url must not reach the record");
    assert_eq!(err.0, StatusCode::BAD_GATEWAY);
    assert!(err.1.contains("whitespace"), "{}", err.1);
    assert!(recorded().is_none());

    ScopedPipeline::bind(
        &ctx,
        "p",
        " https://api.test/v1 \n".to_string(),
        handshake_layer("scripts/auth/x.py"),
        false,
    )
    .await
    .expect("padding is trimmed before it is judged and recorded");
    assert!(recorded().is_some());
}

#[test]
fn a_rejected_entry_contributes_no_handshake_script() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace_with_apis_json(
        &tmp,
        r#"{
          "bad":  {"base_url": "https://a.test", "auth": {"pipeline": [
            {"type": "script_handshake", "script": "../escape.py"}
          ]}},
          "good": {"base_url": "https://b.test", "auth": {"pipeline": [
            {"type": "script_handshake", "script": "scripts/auth/ok.py"}
          ]}}
        }"#,
    );
    assert_eq!(rejected_names(&ws), vec!["bad".to_string()]);
    assert_eq!(
        load_proxy_config(&ws).handshake_script_paths(),
        vec!["data/scripts/auth/ok.py".to_string()]
    );
}

async fn engine_router(engine: &Arc<crate::engine::LucidosEngine>, pool: &sqlx::PgPool) -> Router {
    let scheduler = crate::scheduler::SchedulerManager::new(engine.clone(), pool.clone())
        .await
        .unwrap();
    crate::api::create_router(
        engine.clone(),
        pool.clone(),
        engine.event_store().clone(),
        crate::memory::EmbedderSlot::empty(),
        None,
        engine.workspace_path().to_path_buf(),
        Arc::new(tokio::sync::Mutex::new(scheduler)),
        chrono::Utc::now(),
        &crate::net_config::BindChoice::Loopback,
        Arc::new(crate::api::hooks::NoEventHooks),
    )
}

#[tokio::test]
async fn a_marked_request_is_refused_before_any_route_or_fallback_answers() {
    use crate::test_support::{boot_mock_engine, in_engine_child, setup_test_db, teardown_test_db};
    use tower::ServiceExt;
    if !in_engine_child(
        "api::proxy::tests::a_marked_request_is_refused_before_any_route_or_fallback_answers",
    )
    .await
    {
        return;
    }
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let app = engine_router(&engine, &pool).await;
    for path in [
        "/api/v1/health",
        "/app/some-app/index.html",
        "/data/config/apis.json",
        "/a-frontend-page",
    ] {
        let request = axum::http::Request::builder()
            .uri(path)
            .header(PROXY_ORIGIN_HEADER, "1")
            .body(Body::empty())
            .unwrap();
        let resp = app.clone().oneshot(request).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{path}");
        let body = body_text(resp).await;
        assert!(
            body.contains("may not call a Lucidos engine"),
            "{path}: {body}"
        );
    }
    drop(app);
    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_credentialed_proxy_route_refuses_a_foreign_page_even_with_permissive_cors() {
    use crate::test_support::{boot_mock_engine, in_engine_child, setup_test_db, teardown_test_db};
    use tower::ServiceExt;
    if !in_engine_child(
        "api::proxy::tests::a_credentialed_proxy_route_refuses_a_foreign_page_even_with_permissive_cors",
    )
    .await
    {
        return;
    }
    std::env::set_var("LUCIDOS_PERMISSIVE_CORS", "1");
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let app = engine_router(&engine, &pool).await;
    let path = "/api/v1/proxy/openai/v1/models";
    let request = axum::http::Request::builder()
        .method("GET")
        .uri(path)
        .header("host", "localhost:5251")
        .header("origin", "https://evil.example")
        .header("sec-fetch-site", "cross-site")
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(request).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN, "{path}");
    let body = body_text(resp).await;
    assert!(body.contains("credentialed proxy routes"), "{path}: {body}");
    drop(app);
    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_refused_entry_never_falls_through_to_the_builtin_provider() {
    use crate::test_support::{boot_mock_engine, in_engine_child, setup_test_db, teardown_test_db};
    if !in_engine_child(
        "api::proxy::tests::a_refused_entry_never_falls_through_to_the_builtin_provider",
    )
    .await
    {
        return;
    }
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let config_dir = engine.workspace_path().join("data/config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(
        config_dir.join("apis.json"),
        r#"{"openai": {"base_url": "https://x", "auth": {"pipeline": [
            {"type": "bearrer", "credential": "k"}
        ]}}}"#,
    )
    .unwrap();
    let err = match resolve_named_proxy(&engine, engine.workspace_path(), "openai").await {
        Ok(_) => panic!("a refused entry resolved, so it reached the builtin"),
        Err(err) => err,
    };
    assert_eq!(err.0, StatusCode::BAD_GATEWAY);
    assert!(err.1.contains("unusable"), "{}", err.1);
    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn only_the_entry_flag_reaches_the_unverified_client() {
    use crate::test_support::{boot_mock_engine, in_engine_child, setup_test_db, teardown_test_db};
    if !in_engine_child("api::proxy::tests::only_the_entry_flag_reaches_the_unverified_client")
        .await
    {
        return;
    }
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = boot_mock_engine(&db).await;
    let entry = |base: &str, insecure_transport: bool| ProxyConfig {
        base_url: base.to_string(),
        auth: None,
        insecure_transport,
        timeout_secs: None,
    };

    let refused = spawn_self_signed_upstream().await;
    let from_entry = dispatch_proxy_request(
        &engine,
        "dev",
        &entry(&refused, false),
        Method::GET,
        "v1/items".into(),
        None,
        HeaderMap::new(),
        Bytes::new(),
    )
    .await
    .unwrap();
    assert_eq!(from_entry.status(), StatusCode::BAD_GATEWAY);
    let from_builtin = dispatch_resolved(
        &engine,
        "openai",
        ResolvedProxy::Builtin {
            base_url: refused.clone(),
            layers: Vec::new(),
        },
        Method::GET,
        "v1/items".into(),
        None,
        HeaderMap::new(),
        Bytes::new(),
        crate::api::proxy_cost::CostOwner::default(),
    )
    .await
    .unwrap();
    assert_eq!(from_builtin.status(), StatusCode::BAD_GATEWAY);

    let accepted = spawn_self_signed_upstream().await;
    let opted_in = dispatch_proxy_request(
        &engine,
        "dev",
        &entry(&accepted, true),
        Method::GET,
        "v1/items".into(),
        None,
        HeaderMap::new(),
        Bytes::new(),
    )
    .await
    .unwrap();
    assert_eq!(opted_in.status(), StatusCode::OK);
    drop(engine);
    pool.close().await;
    teardown_test_db(&db).await;
}
