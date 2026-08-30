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

/// The *local token*: proof that this caller is a process on this machine.
///
/// Unlike the two headers above, this one is NOT hand-mirrored. It comes from
/// `lucidos-local-token`, a crate with no dependencies. The gateway, the
/// engine, the CLI and the app all take it, so the header name and the file
/// path have exactly one home. Four hand-copies would drift, and a stale copy
/// here is not a missing feature. It is a CLI that silently cannot
/// authenticate.
use lucidos_local_token::{read as local_token, HEADER_LOCAL_TOKEN};

/// Default headers for a request to THIS workspace's engine:
///
/// - the thread-bound origin token, when the matching env var is in scope (that
///   is, inside a Lucidos-spawned subprocess). The engine verifies it and
///   stamps mutating events as Agent-origin instead of "You".
/// - the target workspace assertion, derived from `$LUCIDOS_WORKSPACE`'s
///   basename, so a subcommand that reached the wrong engine is refused rather
///   than served.
/// - the local token, when this machine has a gateway that minted one. It is
///   what proves the caller is local, since a loopback peer address does not.
///
/// A subcommand that deliberately targets ANOTHER workspace (`lucidos
/// spawn-thread --to`) sets the assertion itself on the request builder, which
/// wins: reqwest fills default headers into vacant entries only, so a
/// per-request header is never overwritten by a default of the same name.
fn default_headers_from_env() -> reqwest::header::HeaderMap {
    headers_from(
        std::env::var(ENV_AGENT_ORIGIN_TOKEN).ok().as_deref(),
        self_workspace_name().as_deref(),
        local_token().as_deref(),
    )
}

/// The pure half: exactly the headers these three inputs imply.
///
/// Split out because the local token comes from a FILE, not the environment.
/// Every machine that has ever run a gateway has one. A test that set the two
/// env vars and then asserted on the whole map was therefore asserting on
/// whoever ran it. It failed on a developer machine and passed on a fresh one.
fn headers_from(
    origin_token: Option<&str>,
    workspace_name: Option<&str>,
    local: Option<&str>,
) -> reqwest::header::HeaderMap {
    let mut h = reqwest::header::HeaderMap::new();
    if let Some(token) = origin_token {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(token) {
            h.insert(HEADER_AGENT_ORIGIN_TOKEN, v);
        }
    }
    if let Some(name) = workspace_name {
        if let Ok(v) = reqwest::header::HeaderValue::from_str(name) {
            h.insert(HEADER_TARGET_WORKSPACE, v);
        }
    }
    if let Some(token) = local {
        if let Ok(mut v) = reqwest::header::HeaderValue::from_str(token) {
            v.set_sensitive(true);
            h.insert(HEADER_LOCAL_TOKEN, v);
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
///
/// `no_proxy()` + `danger_accept_invalid_certs(true)` is the loopback pair
/// `.claude/rules/rust.md` prescribes, and the one `pair.rs` and every engine
/// and gateway client already use. Without it an `HTTPS_PROXY` in the
/// environment, which a corporate machine exports globally, routes every
/// subcommand's call to its own engine through that proxy.
pub(crate) fn client() -> Result<reqwest::blocking::Client, BoxError> {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .default_headers(default_headers_from_env())
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e).into())
}

/// Like [`client`] but with an explicit response deadline, replacing reqwest's
/// 30s default.
///
/// For a caller whose request is incidental to its real job, where a hung
/// engine must cost seconds rather than half a minute. `build-slot` announces
/// contention this way while a build waits. It exists so that caller does not
/// hand-roll a third builder and drop the two default headers with it: an
/// unattributed call is stamped `Api { mode: Human }` (ADR 0050), and one with
/// no workspace assertion is served by whichever engine holds the port.
pub(crate) fn client_with_timeout(
    timeout: std::time::Duration,
) -> Result<reqwest::blocking::Client, BoxError> {
    reqwest::blocking::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
        .default_headers(default_headers_from_env())
        .timeout(timeout)
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
        .no_proxy()
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
    } else if is_tls_against_plain_http(&cause) {
        format!(
            "{method} {url} failed after {elapsed_s:.1}s: {cause}. That is a TLS \
             handshake meeting a plain-http socket. The scheme comes from PROTO= \
             in the target's .lucidos/ports, and an absent line is read as https."
        )
    } else {
        format!("{method} {url} failed after {elapsed_s:.1}s: {cause}")
    }
}

/// Does this root cause say "we spoke TLS and got plain HTTP back"?
///
/// Named because the raw text is unreadable. Rustls reports these two when it
/// reads an HTTP status line as a TLS record header. Neither points at the
/// scheme, the port, or the file that chose them.
///
/// Deliberately narrow. A certificate complaint is a different fault with a
/// different remedy, and blaming `PROTO=` for one would send the reader to the
/// wrong file.
fn is_tls_against_plain_http(cause: &str) -> bool {
    let lower = cause.to_ascii_lowercase();
    lower.contains("record overflow") || lower.contains("invalidcontenttype")
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

/// [`send_expect_success`], then parse the body as JSON.
///
/// The engine answers every `/api/v1` route with JSON, so a body that will not
/// parse means something else replied. One message for that, rather than a copy
/// per subcommand.
pub(crate) fn send_expect_json(
    method: &str,
    url: &str,
    req: reqwest::blocking::RequestBuilder,
) -> Result<serde_json::Value, BoxError> {
    let body = send_expect_success(method, url, req)?;
    serde_json::from_str(&body)
        .map_err(|e| format!("Unexpected response from {}: {}", url, e).into())
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

    #[test]
    fn a_tls_handshake_on_a_plain_socket_is_recognised_and_named() {
        // "record overflow" is what a caller saw when a workspace's ports file
        // had lost its PROTO line. It names neither the scheme nor the file.
        assert!(is_tls_against_plain_http(
            "received corrupt message of type InvalidContentType: record overflow"
        ));
        assert!(is_tls_against_plain_http("Record overflow"));
        assert!(!is_tls_against_plain_http("connection refused"));
        assert!(!is_tls_against_plain_http("operation timed out"));
        // A certificate complaint is a different fault, and PROTO= is not it.
        assert!(!is_tls_against_plain_http(
            "invalid peer certificate: Expired"
        ));
    }

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

    /// Accept one request, hand its raw text back, and answer 200.
    fn spawn_capturing_server() -> (u16, std::sync::mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 8192];
            let n = stream.read(&mut buf).unwrap_or(0);
            let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
            );
        });
        (port, rx)
    }

    /// The deadline-carrying constructor sends the SAME two headers as the
    /// other two, over the wire.
    ///
    /// It exists because a caller that wants a shorter timeout is exactly the
    /// caller tempted to hand-roll its own builder. `build-slot`'s contention
    /// announcement did, and shipped unattributed until review caught it: with
    /// no origin token the engine stamps `Api { mode: Human }` (ADR 0050), and
    /// with no workspace assertion a wrong port is served rather than refused.
    #[test]
    fn the_timeout_client_sends_the_same_headers_as_the_others() {
        let _guard = env_lock().lock().unwrap();
        let token = "00000000-0000-0000-0000-000000000abc.deadbeef";
        // SAFETY: process-wide env mutation gated by env_lock().
        unsafe {
            std::env::set_var(ENV_AGENT_ORIGIN_TOKEN, token);
            std::env::set_var(ENV_WORKSPACE, "/Users/me/workspaces/dev");
        }
        let (port, rx) = spawn_capturing_server();
        let sent = client_with_timeout(Duration::from_secs(5))
            .expect("build")
            .post(format!("http://127.0.0.1:{port}/x"))
            .body("{}")
            .send()
            .is_ok();
        unsafe {
            std::env::remove_var(ENV_AGENT_ORIGIN_TOKEN);
            std::env::remove_var(ENV_WORKSPACE);
        }
        assert!(sent, "the request must reach the server");

        let request = rx.recv_timeout(Duration::from_secs(5)).expect("captured");
        let lower = request.to_lowercase();
        assert!(
            lower.contains(HEADER_AGENT_ORIGIN_TOKEN) && request.contains(token),
            "origin token missing from the wire: {request}"
        );
        assert!(
            lower.contains(HEADER_TARGET_WORKSPACE) && request.contains("dev"),
            "workspace assertion missing from the wire: {request}"
        );
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
        let token = "00000000-0000-0000-0000-000000000abc.deadbeef";
        let headers = headers_from(Some(token), None, None);
        assert_eq!(
            headers
                .get(HEADER_AGENT_ORIGIN_TOKEN)
                .and_then(|v| v.to_str().ok()),
            Some(token),
            "agent-origin-token header missing when the token is in scope"
        );
    }

    /// The env var the token actually comes from, which the pure test above
    /// cannot prove. Asserts the one header rather than the whole map, so a
    /// machine that has a local token file does not fail it.
    #[test]
    fn the_origin_token_header_is_read_from_its_own_env_var() {
        let _guard = env_lock().lock().unwrap();
        let token = "00000000-0000-0000-0000-000000000abc.deadbeef";
        // SAFETY: process-wide env mutation gated by env_lock().
        unsafe {
            std::env::set_var(ENV_AGENT_ORIGIN_TOKEN, token);
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
            "the env var name drifted from what the CLI reads"
        );
    }

    /// The local token rides along whenever this machine has one. It is what
    /// proves the caller is local, since a loopback peer address does not.
    #[test]
    fn the_local_token_rides_along_and_is_marked_sensitive() {
        let headers = headers_from(None, None, Some("abc123"));
        let value = headers.get(HEADER_LOCAL_TOKEN).expect("local token header");
        assert_eq!(value.to_str().ok(), Some("abc123"));
        assert!(
            value.is_sensitive(),
            "a bearer secret must not reach a debug log"
        );
    }

    /// The token is forwarded VERBATIM. The CLI must not parse, re-derive or
    /// otherwise touch it: its prefix is what authenticates the caller thread,
    /// so any client-side rewriting would be the CLI stating who it is, which
    /// is exactly the capability the thread binding removes.
    #[test]
    fn default_headers_from_env_does_not_rewrite_the_token() {
        // Deliberately not a well-formed token. The CLI has no opinion.
        let opaque = "whatever.the.engine.minted";
        let headers = headers_from(Some(opaque), None, None);
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
        let headers = headers_from(None, None, None);
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
        let headers = headers_from(None, None, None);
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
