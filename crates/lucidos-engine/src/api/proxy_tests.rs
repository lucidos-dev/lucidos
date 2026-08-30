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
//
// The policy and its table of cases live in `api::browser_origin`, which owns
// them for the whole API surface. Only one thing belongs here: `/proxy/*` still
// applies the gate itself. That is what keeps a credentialed route closed when
// `LUCIDOS_PERMISSIVE_CORS` turns the outer layer off.

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

// Auth header building (Bearer/ApiKey/Basic) moved to AuthLayer impls
// See proxy_static_layers tests for equivalent coverage.

// ---- Path traversal ---------------------------------------------------

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
fn request_path_has_traversal_passes_normal_paths() {
    assert!(!request_path_has_traversal(""));
    assert!(!request_path_has_traversal("/living-room/play"));
    assert!(!request_path_has_traversal("api/v1/items?id=42"));
    // A literal segment that *contains* `..` but isn't `..` is fine.
    assert!(!request_path_has_traversal("foo..bar"));
}

// ---- URL building -----------------------------------------------------

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
    // Pins the shape `QueryParamLayer` relies on when it builds its redacted
    // `log_url_replacement` output (proxy_static_layers.rs): the same key, the
    // value replaced with REDACTED. The layer's own end-to-end redaction is
    // covered by `query_param_layer_publishes_redacted_log_url` there; this
    // only asserts that `append_query_param` places the substituted value where
    // the real one would go.
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

/// Write an `apis.json` holding `body` and return the workspace root.
fn workspace_with_apis_json(tmp: &tempfile::TempDir, body: &str) -> std::path::PathBuf {
    let cfg_dir = tmp.path().join("data/config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("apis.json"), body).unwrap();
    tmp.path().to_path_buf()
}

/// The wedge that took a live workspace offline. One bad entry used to fail
/// the whole file, so every other proxy in it stopped working too.
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

/// A legacy entry the migration could not rewrite gets the translator's
/// words, not serde's "missing field `pipeline`".
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

/// A refused entry must NOT read as "not configured". Only a 404 falls
/// through to the builtin of the same name. A 404 here would send the
/// request to a different backend than the one that was configured.
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

/// An unreadable file answers for every name, builtins included. It may have
/// overridden one, and there is no way left to know which.
#[tokio::test]
async fn an_unparseable_file_refuses_every_name_rather_than_rerouting_it() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = workspace_with_apis_json(&tmp, "{ not json");
    let (status, _) = resolve_proxy_target(&ws, "openai")
        .await
        .expect_err("nothing resolves while the file is unreadable");
    assert_eq!(status, StatusCode::BAD_GATEWAY);
}

/// A name nobody configured is still a plain 404, which is what lets the
/// builtin fallback fire.
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
    // Typo `bearrer` must surface at config-load time, as a refusal of
    // that one provider rather than of the file.
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

/// Write an `apis.json` whose single provider runs `script` and load it.
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

/// Every provider this config refuses, in order. The file itself reads as
/// `data/config/apis.json`, so a file-level refusal is distinguishable from
/// a provider that happens to be named after it.
fn rejected_names(workspace: &std::path::Path) -> Vec<String> {
    load_proxy_config(workspace)
        .rejected
        .iter()
        .map(|r| r.label().to_string())
        .collect()
}

/// Regression: the config load and the spawn must refuse the SAME script
/// paths.
///
/// `scripts/auth/a..b.py` is the value that diverged. The load-time guard
/// rejected only whole `..` segments, so this one loaded cleanly and was then
/// refused at spawn. The weaker of the two checks was the one running first.
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

/// The `data/` root holds gitignored config beside the typed subdirectories, so
/// a path naming it needs no `..` to reach the workspace secrets. The approve
/// route has always required `data/scripts/`; the runner's guard did not, and a
/// first-run workspace auto-approves whatever `apis.json` names.
#[test]
fn a_handshake_script_cannot_name_the_gitignored_data_root() {
    // Both spellings of every one of them. The runner strips a redundant
    // `data/` so an older config resolves, and that must widen nothing: the
    // prefixed form has to be refused for the same reason as the bare one.
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
    // Stripping happens after the traversal check, never before it.
    assert!(
        crate::api::proxy_script_runner::script_path_rejection("data/../.env").is_some(),
        "a '..' value must not be normalized into an accepted path"
    );
}

/// A refused config has to say which provider and which value. The operator
/// meets this as a line in the startup log, with no request to inspect.
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
    // Unparseable, so there is no entry to blame and the file answers for
    // itself. Still a rejection rather than an error: the engine boots.
    let load = load_proxy_config(&ws);
    assert!(load.providers.is_empty());
    assert_eq!(load.rejected.len(), 1);
    assert!(load.rejected[0].provider.is_none());
    assert_eq!(load.rejected[0].label(), "data/config/apis.json");
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
        Transport::Verified,
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
async fn upstream_does_not_see_host_header_from_engine() {
    // We forward the request with no Host header — reqwest sets one from
    // the URL (i.e. the upstream's host, not the engine's). This proves
    // a Host: engine.example.com sent by the browser doesn't bleed through.
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
    )
    .await;
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
        Transport::Verified,
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
        Transport::Verified,
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
        Transport::Verified,
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
        Transport::Verified,
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
    // Port 1 is privileged and nothing in this suite binds it, so the connect
    // fails fast with ECONNREFUSED rather than hanging until the client's
    // connect timeout.
    let resp = forward_request(
        Method::GET,
        "http://127.0.0.1:1/nope",
        "http://127.0.0.1:1/nope",
        HeaderMap::new(),
        Vec::new(),
        Bytes::new(),
        Transport::Verified,
    )
    .await;
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
}

/// An `apis.json` entry may name an OAuth client registration explicitly (a
/// `script_handshake` layer whose script runs its own exchange).
/// `CredentialStore::get` is deliberately blind to `oauth_client`, so without
/// the typed second attempt the layer 502s on a credential that plainly exists.
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

/// The live-config regression. A `data/config/apis.json` written before
/// 2026-08-05 spells the credential `oauth:<provider>`, and the prefix migration
/// renames the row to `<provider>`. `data/config/` is user data no DB migration
/// can rewrite, so without this tolerance every request through that API starts
/// 502ing the moment the engine restarts. Temporary measure
/// registered in `docs/temporary-measures.md` under "`oauth:` prefix stripped
/// from a caller-supplied credential name".
#[tokio::test]
async fn fetch_required_credential_tolerates_a_legacy_oauth_prefixed_name() {
    use crate::test_support::{seed_credential, setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    seed_credential(
        &pool,
        "ghealth",
        "https://healthcare.googleapis.test",
        crate::core::AuthType::OauthClient,
        "{\"client_id\":\"cid\"}",
    )
    .await;

    let cred = fetch_required_credential(&pool, "oauth:ghealth")
        .await
        .expect("the pre-migration spelling must still resolve");
    assert_eq!(cred.service_name, "ghealth");

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The tolerance must not invent a credential. A name that matches nothing under
/// either spelling still fails, and the error names what was asked for.
#[tokio::test]
async fn fetch_required_credential_still_reports_a_genuinely_missing_one() {
    use crate::test_support::{setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;

    let err = fetch_required_credential(&pool, "oauth:nothing-here")
        .await
        .expect_err("nothing to resolve");
    assert!(
        err.1.contains("oauth:nothing-here"),
        "the error must name the credential the config asked for: {}",
        err.1
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// An explicitly named `oauth_client` DOES inject. The list-taking
/// `credential_env_vars` skips the type because the blanket fan-out would
/// broadcast a `client_secret` into every subprocess for no reader; a layer the
/// user pointed at one credential is the opposite case, and injecting nothing
/// there would be a configured secret silently going missing.
#[test]
fn an_explicitly_named_oauth_client_still_injects_its_env_vars() {
    use crate::core::credentials::{credential_env_vars, credential_env_vars_for};
    use crate::core::{AuthType, Credential};

    let cred = Credential {
        id: uuid::Uuid::new_v4(),
        service_name: "acme".to_string(),
        base_urls: vec!["https://api.acme.test".to_string()],
        auth_type: AuthType::OauthClient,
        auth_value: "{\"client_id\":\"cid\"}".to_string(),
        auth_header: "Authorization".to_string(),
        env_var_name: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    assert_eq!(
        credential_env_vars_for(cred.clone()),
        vec![(
            "CRED_ACME".to_string(),
            "{\"client_id\":\"cid\"}".to_string()
        )]
    );
    assert!(
        credential_env_vars(vec![cred]).is_empty(),
        "the blanket fan-out still skips it"
    );
}

/// The tolerance must not become a case-insensitive match on every miss. A
/// config naming `Stripe` where no such credential exists must stay a miss, even
/// when an unrelated `stripe` OAuth registration is present: resolving it would
/// silently send a `{client_id, ...}` blob as the API's auth header.
#[tokio::test]
async fn fetch_required_credential_does_not_case_fold_a_non_prefixed_miss() {
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

    fetch_required_credential(&pool, "Stripe")
        .await
        .expect_err("a differently-cased name is a different credential");

    pool.close().await;
    teardown_test_db(&db).await;
}

// ---- Upstream base-path containment -----------------------------------
//
// A double-encoded path used to escape the operator's `base_url` prefix while
// staying on the configured origin. The request then reached an endpoint
// outside the prefix, with the proxy's credentials attached. The mechanism was
// decode-once (axum) then normalize (url), and the guard here is containment
// on the parsed path. See the plan doc named on `build_contained_target_url`.

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
    // The prefix itself, with and without a trailing slash.
    assert!(build_contained_target_url(PREFIXED_BASE, "", None).is_ok());
    assert!(build_contained_target_url(PREFIXED_BASE, "/", None).is_ok());
}

#[test]
fn every_dot_segment_spelling_is_refused_by_containment() {
    // What axum hands the handler after decoding `%252e%252e` once, plus the
    // mixed and literal forms the URL parser treats the same way.
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
    // Containment is the property, not a ban on the spelling. `/safe-prefix/a/../b`
    // normalizes to `/safe-prefix/b`, which never left.
    assert!(build_contained_target_url(PREFIXED_BASE, "a/%2e%2e/b", None).is_ok());
}

#[test]
fn a_prefix_less_base_url_still_forwards_everything() {
    // No prefix means nothing to escape, so ordinary traffic is untouched.
    let base = "http://localhost:5005";
    assert_eq!(
        build_contained_target_url(base, "living-room/play", None).unwrap(),
        "http://localhost:5005/living-room/play"
    );
    assert!(build_contained_target_url(base, "%2e%2e/anything", None).is_ok());
}

#[test]
fn a_sibling_prefix_is_not_containment() {
    // `/safe-prefix-evil` shares a string prefix but not a segment boundary.
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
    // Signing layers hash the URL they are handed, so an accepted path must
    // come back byte-identical to what `build_target_url` produced.
    let path = "items/42";
    let query = Some("a=1&b=%20");
    assert_eq!(
        build_contained_target_url(PREFIXED_BASE, path, query).unwrap(),
        build_target_url(PREFIXED_BASE, path, query)
    );
}

// ---- Encoded dot segments at the edge guard ---------------------------

#[test]
fn request_path_has_traversal_flags_encoded_dot_segments() {
    // What the handler sees after axum decodes `%252e%252e` exactly once, and
    // what the `proxy_request` LLM tool sees with no decode at all.
    assert!(request_path_has_traversal("%2e%2e/admin"));
    assert!(request_path_has_traversal("%2E%2E/admin"));
    assert!(request_path_has_traversal(".%2e/admin"));
    assert!(request_path_has_traversal("%2e./admin"));
    assert!(request_path_has_traversal("a/%2e%2e/b"));
}

#[test]
fn request_path_has_traversal_leaves_harmless_encodings_alone() {
    // A single dot segment cannot leave a prefix.
    assert!(!request_path_has_traversal("./items"));
    assert!(!request_path_has_traversal("%2e/items"));
    // Still-encoded input: `%252e%252e` is a literal segment upstream, not a
    // parent one, so rejecting it here would refuse a legitimate resource name.
    assert!(!request_path_has_traversal("%252e%252e/admin"));
    // A segment that merely contains the spelling.
    assert!(!request_path_has_traversal("foo%2e%2ebar"));
    assert!(!request_path_has_traversal("%2e%2e%2e"));
}

#[test]
fn the_raw_concatenation_really_does_escape_the_prefix() {
    // The reason containment exists, pinned so the two stay comparable. Nothing
    // in `build_target_url` is wrong on its own; the escape happens when the
    // URL parser normalizes what it produced. If this ever stops escaping, the
    // parser changed and the guard above should be re-read, not deleted.
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
    // The URL parser does not treat `%2f` as a separator, so these parse as one
    // contained segment and would forward verbatim. An upstream that decodes
    // encoded slashes then reads `../admin` and leaves the prefix anyway.
    for path in [
        "%2e%2e%2fadmin",
        "..%2fadmin",
        "%2E%2E%2Fadmin",
        "..%5cadmin",
        "a/..%2f..%2fadmin",
        // Nested, for an upstream stack that decodes more than once. Each extra
        // `%25` is one more layer, so the probe iterates to a fixed point
        // instead of chasing a depth.
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
    // The reason the guard checks the decoded READING instead of banning `%2f`.
    // Some APIs put an encoded path inside one segment.
    let ok = build_contained_target_url(PREFIXED_BASE, "contents/src%2fmain.rs", None);
    assert!(ok.is_ok(), "legitimate encoded slash refused: {ok:?}");
    assert_eq!(
        ok.unwrap(),
        "https://upstream.example/safe-prefix/contents/src%2fmain.rs",
        "an accepted path must forward byte-identical"
    );
    // Deep decoding must not turn ordinary content into a false rejection.
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
fn decoded_readings_peels_one_layer_per_round_and_terminates() {
    assert_eq!(
        decoded_readings("%2e%2e%252fadmin"),
        vec!["%2e%2e%2fadmin", "%2e%2e/admin"]
    );
    assert_eq!(decoded_readings("..%5cadmin"), vec!["../admin"]);
    // Nothing to decode means no extra probe to run.
    assert!(decoded_readings("items/42").is_empty());
}

#[test]
fn a_same_origin_redirect_is_re_anchored_under_the_prefix() {
    // Worth pinning, because it is easy to read the redirect loop as an escape.
    // The loop sets `current_path` from the Location's parsed path with the
    // leading slash trimmed, then the next hop concatenates it back onto
    // `base_url`. So `Location: /admin` arrives here as `admin` and resolves to
    // `/safe-prefix/admin`, which never left. Containment runs per hop anyway,
    // so the invariant does not rest on that re-anchoring staying this way.
    let resolved = build_contained_target_url(PREFIXED_BASE, "admin", None);
    assert_eq!(
        resolved.unwrap(),
        "https://upstream.example/safe-prefix/admin"
    );
    assert!(build_contained_target_url(PREFIXED_BASE, "safe-prefix/next", None).is_ok());
}

/// The startup seed keys approvals the way the runner does, so the two
/// spellings have to agree: config says `scripts/auth/x.py`, the record says
/// `data/scripts/auth/x.py`.
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

/// The shape a packaged 0.32.0 install carried, reconstructed. Its config
/// spelled every script `data/scripts/auth/*.py`, from before resolution moved
/// under `data/`. Prefixing `data/` again keyed the seed at `data/data/...`, so
/// the boot pass approved nothing and wrote the once-only marker anyway.
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
    // Two hosts disagree, so neither is an answer and the first call binds.
    assert_eq!(seeds[0].base_url, None);
    // The injected set does NOT disagree. No secret moves through an entry
    // that injects nothing, so it holds no opinion, and the one entry that
    // does inject decides.
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

/// One entry naming a script IS an answer, for both columns at once.
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

/// The credential-theft half of ADR 0144. `apis.json` is writable over the
/// API, so `base_url` is caller data: an entry naming a real credential and
/// pointing somewhere else must not get that credential attached.
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
    // A path under the scoped prefix is still the same API.
    check_credential_scope(&pool, "scoped-key", "https://api.example.test/v2")
        .await
        .expect("a deeper path on the same host is in scope");

    for elsewhere in [
        "https://evil.test",
        // The classic near-miss a raw string prefix would wave through.
        "https://api.example.test.evil.test",
        "http://api.example.test",
    ] {
        let err = check_credential_scope(&pool, "scoped-key", elsewhere)
            .await
            .expect_err("out of scope must be refused");
        assert_eq!(err.0, StatusCode::BAD_GATEWAY);
        assert!(err.1.contains("scoped-key"), "{}", err.1);
        assert!(err.1.contains("will not be sent"), "{}", err.1);
    }

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The defect a set of base URLs exists for. Binance signs spot calls at
/// `api.binance.com` and futures calls at `fapi.binance.com` with ONE HMAC
/// pair, and Helius has the same shape. A single scope could not say so, so the
/// second entry answered 502 on every call.
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

/// And the other half: declaring a second host widens the credential by that
/// host and by nothing else. A sibling of a declared host is not declared.
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
        // Undeclared sibling hosts of a declared one. Nothing is inferred from
        // how a host is spelled, so neither is in scope.
        "https://dapi.binance.com",
        "https://binance.com",
        // The near-miss a raw string prefix would wave through.
        "https://api.binance.com.evil.test",
        "http://api.binance.com",
    ] {
        let err = check_credential_scope(&pool, "binance", elsewhere)
            .await
            .expect_err("a host outside the declared set must be refused");
        assert_eq!(err.0, StatusCode::BAD_GATEWAY);
        assert!(err.1.contains("will not be sent"), "{}", err.1);
        // The refusal names the whole DECLARED set, so the user can see what
        // it does cover, and points at the two surfaces that change it.
        assert!(err.1.contains("https://api.binance.com"), "{}", err.1);
        assert!(err.1.contains("https://fapi.binance.com"), "{}", err.1);
        assert!(err.1.contains("Settings"), "{}", err.1);
        assert!(err.1.contains("set-base-urls"), "{}", err.1);
        // But it never hands back a command with the REQUESTED host filled in.
        // That host is `apis.json` data an app UI can write, so a ready-made
        // grant for it would finish the theft the gate exists to refuse.
        assert!(
            !err.1.contains(&format!("--url {elsewhere}")),
            "the refusal must not pre-fill a grant for the rejected host: {}",
            err.1
        );
    }

    pool.close().await;
    teardown_test_db(&db).await;
}

/// An unscoped credential goes nowhere, and the refusal says how to fix it.
/// The startup pass gives a scope to any row `apis.json` explains, so one that
/// still has none is a row nothing accounts for.
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

/// What the startup scope inference reads. A credential used by exactly one
/// entry has an answer; one used by two entries that disagree does not.
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
    // Named by both entries, so nothing here decides its scope.
    assert_eq!(scopes.get("shared").map(|s| s.len()), Some(2));
    // The HMAC secret and the WASM handle are credentials too.
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

/// The inference is once. A row that already carries a scope is never
/// rewritten, so a user's own correction survives every later start.
#[tokio::test]
async fn inferring_a_scope_leaves_an_existing_one_alone() {
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

    assert!(
        CredentialStore::infer_scope_if_empty(&pool, &bus, "legacy-key", "https://api.first.test")
            .await
            .unwrap(),
        "an unscoped credential takes the inferred scope"
    );
    assert!(
        !CredentialStore::infer_scope_if_empty(&pool, &bus, "legacy-key", "https://api.other.test")
            .await
            .unwrap(),
        "a scoped credential is never re-scoped"
    );
    let stored = CredentialStore::get(&pool, "legacy-key").await.unwrap();
    assert_eq!(
        stored.unwrap().base_urls,
        vec!["https://api.first.test".to_string()],
        "the inference fills the set once, and never appends to it"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A `secret` keeps its empty scope through the inference. For that type the
/// empty set is the answer, not a legacy gap: the value is signed with and
/// never sent, and `apis.json` is writable over the API. So an entry naming a
/// webhook signing secret must not be able to hand it to a host.
#[tokio::test]
async fn inferring_a_scope_never_reaches_a_secret() {
    use crate::test_support::{seed_credential, setup_test_db, teardown_test_db};
    let (pool, db) = setup_test_db().await;
    let bus = crate::test_support::offline_event_bus();
    seed_credential(
        &pool,
        "deploys-github",
        "",
        crate::core::AuthType::Secret,
        "whsec_shared",
    )
    .await;

    assert!(
        !CredentialStore::infer_scope_if_empty(
            &pool,
            &bus,
            "deploys-github",
            "https://attacker.test"
        )
        .await
        .unwrap(),
        "a secret is skipped, so nothing is inferred and nothing is announced"
    );
    assert!(
        CredentialStore::get(&pool, "deploys-github")
            .await
            .unwrap()
            .expect("the row exists")
            .base_urls
            .is_empty(),
        "the scope stays empty, so the gate still refuses it everywhere"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

// ---- The scope gate (ADR 0144 decision 4) ------------------------------

/// A gate context over a temp workspace, an offline bus, and a pool that is
/// never reached. Enough for every binding but `StoredCredential`.
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

/// A credential-less `script_handshake` layer, the arm that used to skip the
/// check entirely.
fn handshake_layer(script: &str) -> Vec<Arc<dyn crate::api::proxy_auth_layer::AuthLayer>> {
    handshake_layer_injecting(script, None, &[])
}

/// The same layer, with the secrets an `apis.json` entry asks us to inject.
fn handshake_layer_injecting(
    script: &str,
    credential: Option<&str>,
    oauth_providers: &[&str],
) -> Vec<Arc<dyn crate::api::proxy_auth_layer::AuthLayer>> {
    vec![Arc::new(
        crate::api::proxy_script_layer::ScriptHandshakeLayer::new(
            "script_handshake".into(),
            "comfort".into(),
            credential.map(str::to_string),
            script.into(),
            oauth_providers.iter().map(|p| p.to_string()).collect(),
            unreachable_pool(),
            Arc::new(std::path::PathBuf::from("/ws")),
            Arc::new(crate::api::proxy_token_cache::ProxyTokenCache::new()),
            Arc::new(crate::api::proxy_script_layer::DbOAuthLookup::new(
                unreachable_pool(),
            )),
        ),
    )]
}

/// Write `script` and record it as approved, which is what an in-process file
/// tool does. The bytes never run here, but they have to be on disk: binding a
/// scope verifies that the file is still the approved one.
fn approve(workspace: &std::path::Path, script: &str) {
    const BODY: &[u8] = b"print(1)";
    let rel = crate::core::handshake_approvals::config_path_key(script);
    let abs = workspace.join(&rel);
    std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
    std::fs::write(&abs, BODY).unwrap();
    crate::core::handshake_approvals::record(workspace, &rel, BODY).unwrap();
}

/// FINDING 1. A handshake script mints a live token, and `apis.json` is
/// writable over the API. Rewriting `base_url` used to deliver that token to
/// the attacker's host, because the base-URL check ran only when the layer
/// named a credential.
#[tokio::test]
async fn a_credential_less_handshake_is_refused_off_its_bound_host() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(ws, &pool, &bus);
    approve(ws, "scripts/auth/comfort.py");

    // First use binds the script to the host its entry then named.
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

    // Now the attacker rewrites base_url and keeps the script.
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

/// The regression a packaged 0.32.0 install hit, at the gate that caused it.
///
/// A handshake credential is presented by the SCRIPT, to the provider's own
/// token endpoint, and never to the entry's `base_url`. Checking it against
/// `base_url` refused every ordinary OAuth handshake: a `google` OAuth client
/// is scoped to `oauth2.googleapis.com` and the calendar API lives on
/// `www.googleapis.com`.
///
/// The pool here is unreachable on purpose. Passing proves the gate no longer
/// asks the credential store a question it cannot answer.
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

/// What replaces the credential-scope check on this arm.
///
/// `apis.json` is writable over the API. An entry naming an approved script can
/// be rewritten to ask for a secret the user never meant it to see. The record
/// decides, not the entry.
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

    // Swapping the credential, and adding a provider, are the same attack.
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

/// The record is one line per script, comma joined inside a whitespace split.
/// A credential named with either character would re-cut that line, so the call
/// is refused before anything is written rather than after.
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

/// The bind is once, and a concurrent first request is checked against
/// whichever host won rather than waved through.
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

/// A path under the bound host is the same API, so a deeper base_url is fine.
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

/// An unapproved script gets no scope invented for it. The runner refuses it
/// by hash, and that message is the one worth showing.
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

/// FINDING 2. `local_base_url` is an ordinary settable preference, so the
/// builtin `local` provider's upstream is caller data. Its key follows the
/// credential's own scope, and the gate is what refuses the rewrite. The
/// binding `resolve_local` hands over is pinned in `proxy_builtin`'s own tests.
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

    let local_bearer = || -> Vec<Arc<dyn crate::api::proxy_auth_layer::AuthLayer>> {
        vec![Arc::new(
            crate::api::proxy_static_layers::StaticHeaderLayer::bearer(
                "local".into(),
                "local-key".into(),
                crate::api::proxy_auth_layer::ScopeBinding::StoredCredential("local".into()),
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

    // The backend it was actually scoped to still works, key and all.
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

/// The reported defect, at the chokepoint rather than at the predicate. Two
/// `apis.json` entries name one credential, and the pipeline binds for both.
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

    let signed = || -> Vec<Arc<dyn crate::api::proxy_auth_layer::AuthLayer>> {
        vec![Arc::new(
            crate::api::proxy_static_layers::StaticHeaderLayer::bearer(
                "binance".into(),
                "the-hmac-key".into(),
                crate::api::proxy_auth_layer::ScopeBinding::StoredCredential("binance".into()),
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

/// A key the environment supplied has no row to scope it, so process env has
/// to name the host as well. A rewritten preference cannot speak for it.
#[tokio::test]
async fn an_env_supplied_local_key_follows_the_env_base_url() {
    let tmp = tempfile::tempdir().unwrap();
    let pool = unreachable_pool();
    let bus = crate::test_support::offline_event_bus();
    let ctx = offline_scope_ctx(tmp.path(), &pool, &bus);

    let env_keyed = || -> Vec<Arc<dyn crate::api::proxy_auth_layer::AuthLayer>> {
        vec![Arc::new(
            crate::api::proxy_static_layers::StaticHeaderLayer::bearer(
                "local".into(),
                "env-key".into(),
                crate::api::proxy_auth_layer::ScopeBinding::Pinned {
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

// ---- Transport policy --------------------------------------------------

/// A layer whose pin covers whatever it is asked about, so these cases test the
/// transport rule and nothing else.
fn layers_pinned_to(base_url: &str) -> Vec<Arc<dyn crate::api::proxy_auth_layer::AuthLayer>> {
    vec![Arc::new(
        crate::api::proxy_static_layers::StaticHeaderLayer::bearer(
            "k".into(),
            "sk-live".into(),
            crate::api::proxy_auth_layer::ScopeBinding::Pinned {
                what: "the test key".into(),
                base_url: base_url.to_string(),
            },
        ),
    )]
}

/// FINDING 3, plaintext half. Anyone on the path reads a credential sent over
/// plain http to a public host. So it needs the same explicit opt-in an invalid
/// certificate does.
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

    // Opted in, the operator has said they accept it.
    ScopedPipeline::bind(
        &ctx,
        "p",
        public.to_string(),
        layers_pinned_to(public),
        true,
    )
    .await
    .expect("the opt-in is what makes it allowed");

    // Loopback is exempt: no attacker sits on the path to this machine. This is
    // the documented local-device workflow and the builtin `local` provider.
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

    // A keyless entry has nothing to leak, so plain http stays fine.
    ScopedPipeline::bind(&ctx, "p", public.to_string(), Vec::new(), false)
        .await
        .expect("an uncredentialed plaintext entry is unchanged");
}

/// Nothing downstream can judge a base URL with no host. The gate refuses it
/// rather than comparing scopes against a string that parses to nothing.
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

/// The boot read names both shapes so a user hears about them once, in one
/// place, rather than on the first 502.
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

/// A self-signed certificate and the private key for it, as PEM.
///
/// Built with the `openssl` crate rather than a new dev-dependency. The SAN is
/// what makes the opted-in half meaningful. rustls refuses a certificate with
/// no SAN before it weighs the signature, so without one both clients fail and
/// the test proves nothing.
fn self_signed_pem() -> (Vec<u8>, Vec<u8>) {
    use openssl::asn1::Asn1Time;
    use openssl::hash::MessageDigest;
    use openssl::pkey::PKey;
    use openssl::rsa::Rsa;
    use openssl::x509::extension::SubjectAlternativeName;
    use openssl::x509::{X509NameBuilder, X509};

    let key = PKey::from_rsa(Rsa::generate(2048).unwrap()).unwrap();
    let mut subject = X509NameBuilder::new().unwrap();
    subject.append_entry_by_text("CN", "localhost").unwrap();
    let subject = subject.build();

    let mut builder = X509::builder().unwrap();
    builder.set_version(2).unwrap();
    builder.set_subject_name(&subject).unwrap();
    builder.set_issuer_name(&subject).unwrap();
    builder.set_pubkey(&key).unwrap();
    builder
        .set_not_before(&Asn1Time::days_from_now(0).unwrap())
        .unwrap();
    builder
        .set_not_after(&Asn1Time::days_from_now(1).unwrap())
        .unwrap();
    let san = SubjectAlternativeName::new()
        .ip("127.0.0.1")
        .dns("localhost")
        .build(&builder.x509v3_context(None, None))
        .unwrap();
    builder.append_extension(san).unwrap();
    builder.sign(&key, MessageDigest::sha256()).unwrap();

    (
        builder.build().to_pem().unwrap(),
        key.private_key_to_pem_pkcs8().unwrap(),
    )
}

/// Serve one HTTPS request from a self-signed certificate, then stop. Returns
/// the base URL. Deliberately hand-rolled: the assertion is about the TLS
/// handshake, so the HTTP half only has to be enough for a 200.
async fn spawn_self_signed_upstream() -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (cert_pem, key_pem) = self_signed_pem();
    let cert = openssl::x509::X509::from_pem(&cert_pem).unwrap();
    let key = openssl::pkey::PKey::private_key_from_pem(&key_pem).unwrap();
    // A non-empty passphrase: macOS Security.framework refuses an empty one
    // with -25293, and the bundle never leaves this process.
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
        // Two accepts: the refused client also opens a connection, and which
        // test runs first is not ours to decide.
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

/// FINDING 3, certificate half. The shared client accepted any certificate for
/// every provider, so an on-path attacker read the API key for an ordinary
/// public API. Validation is the default now, and the exception is per
/// provider.
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
    )
    .await;
    assert_eq!(
        accepted.status(),
        StatusCode::OK,
        "the self-signed dev workflow keeps working behind the opt-in"
    );
    assert_eq!(body_text(accepted).await, "ok");
}

/// The opt-in is the only route to the unverified client, so the flag on the
/// entry is what decides which one a request uses.
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
