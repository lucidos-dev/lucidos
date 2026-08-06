use std::time::{Duration, Instant};

use crate::workspace::BoxError;

// Header + env-var names mirror the engine's `api::actor` consts. CLI can't
// depend on the engine crate (no `lucidos-common`), so these literals
// must be kept in lockstep with their counterparts there. A rename on
// either side without the matching follow-up silently breaks subprocess
// attribution — `lucidos-e2e/tests/api_support/lucidos_cli_test.rs` is the
// integration backstop.
//
// The token's *value* is opaque here on purpose. The engine mints it bound
// to the spawning thread, and its own prefix is what names that thread, so
// the CLI never has to state who it is and there is no `--from` flag for it
// to get wrong. There used to be a second `x-lucidos-source-thread-id`
// header carrying the thread id; it was unverifiable (any subprocess could
// claim any thread) and the engine no longer reads one.
pub(crate) const HEADER_AGENT_ORIGIN_TOKEN: &str = "x-lucidos-agent-origin-token";
const ENV_AGENT_ORIGIN_TOKEN: &str = "LUCIDOS_AGENT_ORIGIN_TOKEN";

/// The *target workspace assertion*: which workspace this request is meant for.
/// Mirrors `api::actor::HEADER_TARGET_WORKSPACE` under the same lockstep rule as
/// the origin token above. The engine refuses with 409 when it names a
/// workspace other than the one it serves, which is what stops a wrong port
/// from being silently served by whichever engine is listening there.
pub(crate) const HEADER_TARGET_WORKSPACE: &str = "x-lucidos-target-workspace";
const ENV_WORKSPACE: &str = "LUCIDOS_WORKSPACE";

/// Default headers for a request to THIS workspace's engine:
///
/// - the thread-bound origin token, when the matching env var is in scope (that
///   is, inside a Lucidos-spawned subprocess). The engine verifies it and
///   stamps mutating events as Agent-origin instead of "You".
/// - the target workspace assertion, derived from `$LUCIDOS_WORKSPACE`'s
///   basename, so a subcommand that reached the wrong engine is refused rather
///   than served.
///
/// A subcommand that deliberately targets ANOTHER workspace (`lucidos
/// spawn-thread --to`) sets the assertion itself on the request builder, which
/// wins: reqwest fills default headers into vacant entries only, so a
/// per-request header is never overwritten by a default of the same name.
fn default_headers_from_env() -> reqwest::header::HeaderMap {
    let mut h = reqwest::header::HeaderMap::new();
    if let Ok(token) = std::env::var(ENV_AGENT_ORIGIN_TOKEN) {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&token) {
            h.insert(HEADER_AGENT_ORIGIN_TOKEN, v);
        }
    }
    if let Some(name) = self_workspace_name() {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(&name) {
            h.insert(HEADER_TARGET_WORKSPACE, v);
        }
    }
    h
}

/// Basename of `$LUCIDOS_WORKSPACE`, which the engine sets on every subprocess
/// it spawns. `None` outside a spawned subprocess (a terminal user running
/// `lucidos` by hand), where the CLI resolves its workspace by walking up for a
/// ports file instead and has no name to assert at client-construction time.
/// Absent asserts nothing, which is the documented no-op.
fn self_workspace_name() -> Option<String> {
    let raw = std::env::var(ENV_WORKSPACE).ok()?;
    let name = std::path::Path::new(raw.trim())
        .file_name()?
        .to_string_lossy()
        .into_owned();
    if name.is_empty() {
        return None;
    }
    Some(name)
}

/// Blocking HTTP client preconfigured for the local Lucidos engine.
/// Accepts the engine's self-signed cert because the target is `localhost`.
/// Auto-forwards the thread-bound origin token when the matching env var
/// is present (i.e., we're inside a Lucidos-spawned subprocess).
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
/// gate entirely. Same origin-token forwarding as `client()`: the
/// permission server is an engine endpoint and benefits from the same
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

/// Send `req`, fail on non-2xx, and return the response body.
/// Shared error wording via `format_request_error` (timeouts and connect
/// failures get actionable hints instead of reqwest's generic "error sending
/// request"). Use this when the caller prints something OTHER than the raw
/// body (`data write` prints a chat link); use `send_and_print` when the body
/// itself is the output.
pub(crate) fn send_expect_success(
    method: &str,
    url: &str,
    req: reqwest::blocking::RequestBuilder,
) -> Result<String, BoxError> {
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
    Ok(text)
}

/// Send `req`, fail on non-2xx, and write the response body to stdout.
/// Shared between subcommands that POST/GET JSON to the engine and surface
/// the response body verbatim (`events`, `notify`, …).
pub(crate) fn send_and_print(
    method: &str,
    url: &str,
    req: reqwest::blocking::RequestBuilder,
) -> Result<(), BoxError> {
    println!("{}", send_expect_success(method, url, req)?);
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

    /// Token env var set → the origin-token header is populated. This is
    /// the agent path: the engine verifies the thread-bound token, stamps the
    /// resulting event as `Api { mode: Agent, source_thread_id }` from the
    /// token's own prefix, the UI says "Lucidos Agent" (not "You") and the
    /// popover links back to the spawning thread.
    #[test]
    fn default_headers_from_env_forwards_the_origin_token() {
        let _guard = env_lock().lock().unwrap();
        let token = "00000000-0000-0000-0000-000000000abc.deadbeef";
        // SAFETY: process-wide env mutation gated by env_lock().
        // `LUCIDOS_WORKSPACE` is cleared because the suite itself often runs
        // inside a Lucidos-spawned subprocess, where it IS set; leaving it
        // would make the header count depend on who ran the tests.
        unsafe {
            std::env::set_var(ENV_AGENT_ORIGIN_TOKEN, token);
            std::env::remove_var(ENV_WORKSPACE);
        }
        let headers = default_headers_from_env();
        unsafe {
            std::env::remove_var(ENV_AGENT_ORIGIN_TOKEN);
        }
        assert_eq!(
            headers
                .get(HEADER_AGENT_ORIGIN_TOKEN)
                .and_then(|v| v.to_str().ok()),
            Some(token),
            "agent-origin-token header missing when env var is set"
        );
    }

    /// The token is forwarded VERBATIM. The CLI must not parse, re-derive or
    /// otherwise touch it: its prefix is what authenticates the caller thread,
    /// so any client-side rewriting would be the CLI stating who it is, which
    /// is exactly the capability the thread binding removes.
    #[test]
    fn default_headers_from_env_does_not_rewrite_the_token() {
        let _guard = env_lock().lock().unwrap();
        // Deliberately not a well-formed token. The CLI has no opinion.
        let opaque = "whatever.the.engine.minted";
        // SAFETY: process-wide env mutation gated by env_lock().
        unsafe {
            std::env::set_var(ENV_AGENT_ORIGIN_TOKEN, opaque);
            std::env::remove_var(ENV_WORKSPACE);
        }
        let headers = default_headers_from_env();
        unsafe {
            std::env::remove_var(ENV_AGENT_ORIGIN_TOKEN);
        }
        assert_eq!(
            headers
                .get(HEADER_AGENT_ORIGIN_TOKEN)
                .and_then(|v| v.to_str().ok()),
            Some(opaque)
        );
        assert_eq!(headers.len(), 1, "no second origin header may be sent");
    }

    /// Env var unset means no header. Terminal users running `lucidos ...` by
    /// hand (no subprocess context) get the honest path: the engine has no
    /// evidence of who they are, so it stamps `Api { mode: Human }` and the UI
    /// renders "API caller".
    #[test]
    fn default_headers_from_env_yields_empty_when_env_unset() {
        let _guard = env_lock().lock().unwrap();
        // SAFETY: process-wide env mutation gated by env_lock().
        unsafe {
            std::env::remove_var(ENV_AGENT_ORIGIN_TOKEN);
            std::env::remove_var(ENV_WORKSPACE);
        }
        let headers = default_headers_from_env();
        assert!(
            headers.is_empty(),
            "no origin header may be sent outside a Lucidos-spawned subprocess"
        );
    }

    /// Inside a spawned subprocess the CLI asserts which workspace it is
    /// talking to, so a subcommand that reached the wrong engine (several run
    /// on one machine, each on its own port) is refused with 409 instead of
    /// being served by whichever one was listening.
    #[test]
    fn default_headers_from_env_asserts_the_target_workspace() {
        let _guard = env_lock().lock().unwrap();
        // SAFETY: process-wide env mutation gated by env_lock().
        unsafe {
            std::env::set_var(ENV_WORKSPACE, "/Users/me/workspaces/dev");
            std::env::remove_var(ENV_AGENT_ORIGIN_TOKEN);
        }
        let headers = default_headers_from_env();
        unsafe {
            std::env::remove_var(ENV_WORKSPACE);
        }
        assert_eq!(
            headers
                .get(HEADER_TARGET_WORKSPACE)
                .and_then(|v| v.to_str().ok()),
            Some("dev"),
            "the assertion is the workspace BASENAME, matching what the engine calls itself"
        );
    }

    /// A trailing slash is how a path shows up when it came from a shell
    /// variable, and it must not turn the basename into something the engine
    /// will not recognise.
    #[test]
    fn the_target_workspace_assertion_survives_a_trailing_slash() {
        let _guard = env_lock().lock().unwrap();
        // SAFETY: process-wide env mutation gated by env_lock().
        unsafe {
            std::env::set_var(ENV_WORKSPACE, " /Users/me/workspaces/dev/ ");
        }
        let name = self_workspace_name();
        unsafe {
            std::env::remove_var(ENV_WORKSPACE);
        }
        assert_eq!(name.as_deref(), Some("dev"));
    }

    /// Outside a spawned subprocess there is no workspace to name, and
    /// asserting nothing is the documented no-op: the engine proceeds exactly
    /// as it did before the header existed.
    #[test]
    fn no_workspace_env_asserts_nothing() {
        let _guard = env_lock().lock().unwrap();
        // SAFETY: process-wide env mutation gated by env_lock().
        unsafe {
            std::env::remove_var(ENV_WORKSPACE);
            std::env::remove_var(ENV_AGENT_ORIGIN_TOKEN);
        }
        let headers = default_headers_from_env();
        assert!(
            headers.is_empty(),
            "a terminal user outside a subprocess sends neither header"
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
