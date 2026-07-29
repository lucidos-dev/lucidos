use std::time::{Duration, Instant};

use crate::workspace::BoxError;

// Header + env-var names mirror the engine's `api::actor` consts. CLI can't
// depend on the engine crate (no `lucidos-common`), so these four literals
// must be kept in lockstep with their counterparts there. A rename on
// either side without the matching follow-up silently breaks subprocess
// attribution — `lucidos-e2e/tests/api_support/lucidos_cli_test.rs` is the
// integration backstop.
pub(crate) const HEADER_AGENT_ORIGIN_TOKEN: &str = "x-lucidos-agent-origin-token";
pub(crate) const HEADER_SOURCE_THREAD_ID: &str = "x-lucidos-source-thread-id";
const ENV_AGENT_ORIGIN_TOKEN: &str = "LUCIDOS_AGENT_ORIGIN_TOKEN";
const ENV_SOURCE_THREAD_ID: &str = "LUCIDOS_THREAD_ID";

/// Forward the subprocess-origin headers when the matching env vars are in
/// scope (i.e. we're inside a Lucidos-spawned subprocess). Engine recognises
/// the token and stamps mutating events as Agent-origin instead of "You".
fn default_headers_from_env() -> reqwest::header::HeaderMap {
    let mut h = reqwest::header::HeaderMap::new();
    if let Ok(token) = std::env::var(ENV_AGENT_ORIGIN_TOKEN) {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&token) {
            h.insert(HEADER_AGENT_ORIGIN_TOKEN, v);
        }
    }
    if let Ok(thread_id) = std::env::var(ENV_SOURCE_THREAD_ID) {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&thread_id) {
            h.insert(HEADER_SOURCE_THREAD_ID, v);
        }
    }
    h
}

/// Blocking HTTP client preconfigured for the local Lucidos engine.
/// Accepts the engine's self-signed cert because the target is `localhost`.
/// Auto-forwards the subprocess-origin headers when the matching env vars
/// are present (i.e., we're inside a Lucidos-spawned subprocess).
pub(crate) fn client() -> Result<reqwest::blocking::Client, BoxError> {
    reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .default_headers(default_headers_from_env())
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e).into())
}

/// HTTP client for the MCP permission server. Disables reqwest's default 30s
/// blocking timeout because `/api/v1/internal/permission-prompt` waits for the
/// user's click. With the default timeout, every prompt fails after 30s and
/// CC pivots to a `Bash` heredoc (in `--allowedTools`) that bypasses the
/// gate entirely. Same subprocess-origin header forwarding as `client()` —
/// the permission server is an engine endpoint and benefits from the same
/// honest attribution.
pub(crate) fn permission_prompt_client() -> Result<reqwest::blocking::Client, BoxError> {
    reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .default_headers(default_headers_from_env())
        .timeout(None)
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e).into())
}

/// Turn a `reqwest::Error` from `.send()` into a message the user can act on.
/// The default `Display` impl is "error sending request for url (<url>)" with
/// no hint about *what* failed — so a timeout, a refused connection, and a
/// TLS handshake fault all look the same. We classify the common cases, add
/// an actionable hint, and always include elapsed wall clock so a hang is
/// distinguishable from an instant failure.
///
/// Note on ordering: `reqwest::Error::is_timeout()` is also true for
/// *connect-phase* timeouts (DNS/TCP that never completed), so we check
/// `is_connect()` first — otherwise a host-unreachable hang would tell the
/// user the engine accepted the connection when no socket ever opened.
pub(crate) fn format_request_error(
    method: &str,
    url: &str,
    err: &reqwest::Error,
    elapsed: Duration,
) -> String {
    let elapsed_s = elapsed.as_secs_f64();
    let cause = root_cause(err);
    if err.is_connect() {
        format!(
            "{method} {url} could not connect after {elapsed_s:.1}s ({cause}) — \
             is the engine running? Try `./scripts/status.sh`."
        )
    } else if err.is_timeout() {
        format!(
            "{method} {url} timed out after {elapsed_s:.1}s ({cause}) — \
             engine accepted the request but never responded. \
             See the engine log under `.lucidos/engine.log` for a stalled upstream call."
        )
    } else {
        format!("{method} {url} failed after {elapsed_s:.1}s: {cause}")
    }
}

/// Walk the error's `source()` chain to the leaf and render that. Avoids the
/// double-URL noise from `reqwest::Error`'s `Display`, which embeds the URL
/// the helper already prints (`"error sending request for url (...)"`).
fn root_cause(err: &reqwest::Error) -> String {
    use std::error::Error;
    let mut last = err.to_string();
    let mut source: Option<&(dyn Error + 'static)> = err.source();
    while let Some(s) = source {
        last = s.to_string();
        source = s.source();
    }
    last
}

/// Send `req`, fail on non-2xx, and write the response body to stdout.
/// Shared between subcommands that POST/GET JSON to the engine and surface
/// the response body verbatim (`events`, `notify`, …) — keeps error wording
/// consistent via `format_request_error` (timeouts and connect failures get
/// actionable hints instead of reqwest's generic "error sending request").
pub(crate) fn send_and_print(
    method: &str,
    url: &str,
    req: reqwest::blocking::RequestBuilder,
) -> Result<(), BoxError> {
    let start = Instant::now();
    let resp = req
        .send()
        .map_err(|e| format_request_error(method, url, &e, start.elapsed()))?;
    let status = resp.status();
    let text = resp
        .text()
        .map_err(|e| format!("Failed to read response body: {}", e))?;
    if !status.is_success() {
        return Err(format!("{} {} returned {}: {}", method, url, status, text).into());
    }
    println!("{}", text);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    fn spawn_delayed_server(delay: Duration) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 8192];
            // Best-effort: drain whatever the client sent (typically a single
            // GET line + headers). We don't care about the contents.
            let _ = stream.read(&mut buf);
            thread::sleep(delay);
            let body = b"{}";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write headers");
            stream.write_all(body).expect("write body");
        });
        port
    }

    /// Accept the connection, drain the request, then hold the socket open
    /// indefinitely without writing a response. Simulates the engine hang the
    /// CLI experienced on `proxy comfort-cloud` — TCP succeeds, HTTP stalls.
    fn spawn_silent_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            // Hold the connection — never respond. Drop after a long sleep
            // so the test thread doesn't leak the socket beyond the suite.
            thread::sleep(Duration::from_secs(30));
            drop(stream);
        });
        port
    }

    #[test]
    fn permission_prompt_client_handles_slow_responses() {
        let port = spawn_delayed_server(Duration::from_millis(200));
        let resp = permission_prompt_client()
            .expect("build")
            .post(format!("http://127.0.0.1:{port}/x"))
            .body("{}")
            .send()
            .expect("must not fail");
        assert_eq!(resp.status().as_u16(), 200);
    }

    /// Shared serialization across the env-mutating tests below — they touch
    /// the same two process-wide env vars and would race each other without
    /// a single lock.
    fn env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        &LOCK
    }

    /// Both env vars set → both headers populated. This is the agent path:
    /// engine recognises the token, stamps the resulting event as
    /// `Api { mode: Agent, source_thread_id }`, the UI says "Lucidos Agent"
    /// (not "You") and the popover links back to the spawning thread.
    #[test]
    fn default_headers_from_env_populates_both_headers_when_env_set() {
        let _guard = env_lock().lock().unwrap();
        let tid = "00000000-0000-0000-0000-000000000abc";
        // SAFETY: process-wide env mutation gated by env_lock().
        unsafe {
            std::env::set_var(ENV_AGENT_ORIGIN_TOKEN, "header-test-token");
            std::env::set_var(ENV_SOURCE_THREAD_ID, tid);
        }
        let headers = default_headers_from_env();
        unsafe {
            std::env::remove_var(ENV_AGENT_ORIGIN_TOKEN);
            std::env::remove_var(ENV_SOURCE_THREAD_ID);
        }
        assert_eq!(
            headers
                .get(HEADER_AGENT_ORIGIN_TOKEN)
                .and_then(|v| v.to_str().ok()),
            Some("header-test-token"),
            "agent-origin-token header missing when env var is set"
        );
        assert_eq!(
            headers
                .get(HEADER_SOURCE_THREAD_ID)
                .and_then(|v| v.to_str().ok()),
            Some(tid),
            "source-thread-id header missing when env var is set"
        );
    }

    /// Neither env var set → neither header populated. Terminal users running
    /// `lucidos ...` by hand (no subprocess context) get the honest path: the
    /// engine stamps `Api { mode: Human }`, the UI says "You".
    #[test]
    fn default_headers_from_env_yields_empty_when_env_unset() {
        let _guard = env_lock().lock().unwrap();
        // SAFETY: process-wide env mutation gated by env_lock().
        unsafe {
            std::env::remove_var(ENV_AGENT_ORIGIN_TOKEN);
            std::env::remove_var(ENV_SOURCE_THREAD_ID);
        }
        let headers = default_headers_from_env();
        assert!(
            headers.get(HEADER_AGENT_ORIGIN_TOKEN).is_none(),
            "agent-origin-token must be absent when env var unset"
        );
        assert!(
            headers.get(HEADER_SOURCE_THREAD_ID).is_none(),
            "source-thread-id must be absent when env var unset"
        );
    }

    /// Token set, thread id absent → token flows alone. This is the
    /// scheduled-script path: subprocess runs without a thread context, so
    /// the request still stamps `Api { mode: Agent }` but with
    /// `source_thread_id: None`. Without this, schedule-driven applies would
    /// fall through to `Api { mode: Human }`.
    #[test]
    fn default_headers_from_env_includes_token_without_thread_id() {
        let _guard = env_lock().lock().unwrap();
        // SAFETY: process-wide env mutation gated by env_lock().
        unsafe {
            std::env::set_var(ENV_AGENT_ORIGIN_TOKEN, "scheduled-token");
            std::env::remove_var(ENV_SOURCE_THREAD_ID);
        }
        let headers = default_headers_from_env();
        unsafe {
            std::env::remove_var(ENV_AGENT_ORIGIN_TOKEN);
        }
        assert_eq!(
            headers
                .get(HEADER_AGENT_ORIGIN_TOKEN)
                .and_then(|v| v.to_str().ok()),
            Some("scheduled-token"),
            "token header missing"
        );
        assert!(
            headers.get(HEADER_SOURCE_THREAD_ID).is_none(),
            "thread-id header must be absent when env var unset"
        );
    }

    #[test]
    fn format_request_error_classifies_timeout() {
        let port = spawn_silent_server();
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(150))
            .build()
            .expect("build");
        let url = format!("http://127.0.0.1:{port}/x");
        let err = client.get(&url).send().expect_err("must timeout");
        assert!(err.is_timeout(), "expected timeout, got: {err:?}");

        let msg = format_request_error("GET", &url, &err, Duration::from_millis(150));
        assert!(msg.contains("timed out"), "msg must mention timeout: {msg}");
        assert!(msg.contains("GET"), "msg must include method: {msg}");
        assert!(msg.contains(&url), "msg must include url: {msg}");
        assert!(
            msg.contains("never responded"),
            "msg must explain the hang: {msg}"
        );
        assert!(
            msg.contains("engine.log"),
            "msg must point at engine log: {msg}"
        );
    }

    #[test]
    fn format_request_error_classifies_connection_refused() {
        // Bind + immediately drop to grab a port the kernel will refuse
        // connections on. Tight race window but reliable on localhost.
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let port = listener.local_addr().unwrap().port();
            drop(listener);
            port
        };
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(500))
            .build()
            .expect("build");
        let url = format!("http://127.0.0.1:{port}/x");
        let err = client.get(&url).send().expect_err("must fail");
        assert!(
            err.is_connect() || err.is_request(),
            "expected connect/request error, got: {err:?}"
        );

        let msg = format_request_error("GET", &url, &err, Duration::from_millis(5));
        assert!(
            msg.contains("could not connect") || msg.contains("connect"),
            "msg must mention connect: {msg}"
        );
        assert!(
            msg.contains("engine running"),
            "msg must hint at engine status: {msg}"
        );
        // Regression guard for the double-URL bug: reqwest's default Display
        // embeds the URL in "error sending request for url (URL)"; if we
        // included the raw err instead of root_cause(), the URL would
        // appear twice.
        let occurrences = msg.matches(url.as_str()).count();
        assert_eq!(
            occurrences, 1,
            "url must appear exactly once (not embedded in reqwest's err display): {msg}"
        );
    }

    #[test]
    fn format_request_error_always_includes_method_and_url() {
        // Regression guard: pre-fix `proxy.rs` formatted the error as
        // "Failed to send request to <URL>: <reqwest err>" with the HTTP
        // method dropped on the floor. Any error path — classified or
        // fallback — must keep the method.
        let port = spawn_silent_server();
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_millis(120))
            .build()
            .expect("build");
        let url = format!("http://127.0.0.1:{port}/x");
        let err = client.get(&url).send().expect_err("must fail");
        let msg = format_request_error("POST", &url, &err, Duration::from_millis(118));
        assert!(msg.contains("POST"), "msg must include method: {msg}");
        assert!(msg.contains(&url), "msg must include url: {msg}");
        assert!(
            !msg.starts_with("error sending request"),
            "msg must not be raw reqwest default: {msg}"
        );
    }
}
