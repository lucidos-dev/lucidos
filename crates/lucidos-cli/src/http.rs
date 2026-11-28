use std::time::{Duration, Instant};

use crate::workspace::BoxError;

pub(crate) const HEADER_AGENT_ORIGIN_TOKEN: &str = "x-lucidos-agent-origin-token";
const ENV_AGENT_ORIGIN_TOKEN: &str = "LUCIDOS_AGENT_ORIGIN_TOKEN";

pub(crate) fn has_origin_token() -> bool {
    std::env::var(ENV_AGENT_ORIGIN_TOKEN).is_ok_and(|token| !token.is_empty())
}

pub(crate) fn contains_engine_token(value: &str) -> bool {
    [std::env::var(ENV_AGENT_ORIGIN_TOKEN).ok(), local_token()]
        .into_iter()
        .flatten()
        .any(|token| !token.is_empty() && value.contains(&token))
}

pub(crate) const HEADER_TARGET_WORKSPACE: &str = "x-lucidos-target-workspace";
const ENV_WORKSPACE: &str = "LUCIDOS_WORKSPACE";

use lucidos_local_token::{read as local_token, HEADER_LOCAL_TOKEN};

fn default_headers_from_env() -> reqwest::header::HeaderMap {
    headers_from(
        std::env::var(ENV_AGENT_ORIGIN_TOKEN).ok().as_deref(),
        self_workspace_name().as_deref(),
        local_token().as_deref(),
    )
}

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

pub(crate) fn client() -> Result<reqwest::blocking::Client, BoxError> {
    engine_client_builder()
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e).into())
}

fn engine_client_builder() -> reqwest::blocking::ClientBuilder {
    lucidos_local_token::client::blocking_client_builder()
        .default_headers(default_headers_from_env())
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::none())
}

pub(crate) fn client_with_timeout(
    timeout: std::time::Duration,
) -> Result<reqwest::blocking::Client, BoxError> {
    engine_client_builder()
        .timeout(timeout)
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e).into())
}

pub(crate) fn permission_prompt_client() -> Result<reqwest::blocking::Client, BoxError> {
    engine_client_builder()
        .timeout(None)
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e).into())
}

const DEFAULT_CONNECT_WAIT: Duration = Duration::from_secs(30);

fn connect_wait() -> Duration {
    std::env::var("LUCIDOS_CLI_CONNECT_WAIT_SECS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_CONNECT_WAIT)
}

fn retryable_connect_error(err: &reqwest::Error) -> bool {
    use std::error::Error;
    use std::io::ErrorKind;

    if !err.is_connect() || err.is_timeout() {
        return false;
    }
    let mut source = err.source();
    while let Some(error) = source {
        if let Some(io) = error.downcast_ref::<std::io::Error>() {
            return matches!(
                io.kind(),
                ErrorKind::ConnectionRefused
                    | ErrorKind::NetworkUnreachable
                    | ErrorKind::HostUnreachable
                    | ErrorKind::AddrNotAvailable
            );
        }
        source = error.source();
    }
    false
}

fn is_boot_rejection(status: reqwest::StatusCode, headers: &reqwest::header::HeaderMap) -> bool {
    status == reqwest::StatusCode::SERVICE_UNAVAILABLE
        && headers
            .get("x-lucidos-engine-booting")
            .is_some_and(|value| value == "1")
}

pub(crate) trait ConnectRetry {
    fn send_with_connect_retry(self) -> Result<reqwest::blocking::Response, reqwest::Error>
    where
        Self: Sized,
    {
        self.send_with_connect_retry_within(connect_wait())
    }

    fn send_with_connect_retry_within(
        self,
        wait: Duration,
    ) -> Result<reqwest::blocking::Response, reqwest::Error>;
}

impl ConnectRetry for reqwest::blocking::RequestBuilder {
    fn send_with_connect_retry_within(
        self,
        wait: Duration,
    ) -> Result<reqwest::blocking::Response, reqwest::Error> {
        let start = Instant::now();
        let mut delay = Duration::from_millis(100);
        let mut request = self;
        loop {
            let next = request.try_clone();
            let result = request.send();
            let retryable = match &result {
                Err(error) => retryable_connect_error(error),
                Ok(response) => is_boot_rejection(response.status(), response.headers()),
            };
            let remaining = wait.saturating_sub(start.elapsed());
            let Some(next) = next.filter(|_| retryable && !remaining.is_zero()) else {
                return result;
            };
            std::thread::sleep(delay.min(remaining));
            if start.elapsed() >= wait {
                return result;
            }
            delay = (delay * 2).min(Duration::from_secs(1));
            request = next;
        }
    }
}

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
            "{method} {url} could not connect after {elapsed_s:.1}s ({cause}). \
             Is the engine running? It refuses connections while it is stopped \
             or restarting, so retry once it is back."
        )
    } else if err.is_timeout() {
        format!(
            "{method} {url} timed out after {elapsed_s:.1}s ({cause}). \
             The request outcome is unknown. The engine may still finish it."
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

fn is_tls_against_plain_http(cause: &str) -> bool {
    let lower = cause.to_ascii_lowercase();
    lower.contains("record overflow") || lower.contains("invalidcontenttype")
}

pub(crate) fn root_cause(err: &reqwest::Error) -> String {
    use std::error::Error;
    let mut last = err.to_string();
    let mut source: Option<&(dyn Error + 'static)> = err.source();
    while let Some(s) = source {
        last = s.to_string();
        source = s.source();
    }
    last
}

const HEADER_CHILD_REPORTS_WAITING: &str = "lucidos-child-reports-waiting";

pub(crate) fn child_reports_note(headers: &reqwest::header::HeaderMap) -> Option<String> {
    let waiting: u64 = headers
        .get(HEADER_CHILD_REPORTS_WAITING)?
        .to_str()
        .ok()?
        .parse()
        .ok()?;
    let reports = match waiting {
        0 => return None,
        1 => "A child thread's report is".to_string(),
        n => format!("{n} child thread reports are"),
    };
    Some(format!(
        "lucidos: {reports} waiting for you. Read it now with `lucidos threads child-reports`."
    ))
}

pub(crate) fn send_expect_success(
    method: &str,
    url: &str,
    req: reqwest::blocking::RequestBuilder,
) -> Result<String, BoxError> {
    send_or_refusal(method, url, req)?
}

pub(crate) fn send_or_refusal(
    method: &str,
    url: &str,
    req: reqwest::blocking::RequestBuilder,
) -> Result<Result<String, BoxError>, BoxError> {
    Ok(send_or_refusal_with_headers(method, url, req)?.map(|(_, text)| text))
}

pub(crate) fn send_expect_success_with_headers(
    method: &str,
    url: &str,
    req: reqwest::blocking::RequestBuilder,
) -> Result<(reqwest::header::HeaderMap, String), BoxError> {
    send_or_refusal_with_headers(method, url, req)?
}

fn send_or_refusal_with_headers(
    method: &str,
    url: &str,
    req: reqwest::blocking::RequestBuilder,
) -> Result<Result<(reqwest::header::HeaderMap, String), BoxError>, BoxError> {
    let start = Instant::now();
    let resp = req
        .send_with_connect_retry()
        .map_err(|e| format_request_error(method, url, &e, start.elapsed()))?;
    let status = resp.status();
    if let Some(note) = child_reports_note(resp.headers()) {
        eprintln!("{note}");
    }
    let headers = resp.headers().clone();
    let text = resp
        .text()
        .map_err(|e| format!("Failed to read response body: {}", e))?;
    if status.is_success() {
        return Ok(Ok((headers, text)));
    }
    let error: BoxError = format!("{} {} returned {}: {}", method, url, status, text).into();
    if status.is_client_error() || is_boot_rejection(status, &headers) {
        Ok(Err(error))
    } else {
        Err(error)
    }
}

pub(crate) fn send_expect_json(
    method: &str,
    url: &str,
    req: reqwest::blocking::RequestBuilder,
) -> Result<serde_json::Value, BoxError> {
    let body = send_expect_success(method, url, req)?;
    serde_json::from_str(&body)
        .map_err(|e| format!("Unexpected response from {}: {}", url, e).into())
}

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
    use std::io::{BufRead, BufReader, Read, Write};

    fn waiting(value: &str) -> reqwest::header::HeaderMap {
        let mut h = reqwest::header::HeaderMap::new();
        h.insert(HEADER_CHILD_REPORTS_WAITING, value.parse().unwrap());
        h
    }

    #[test]
    fn a_waiting_child_report_is_noted() {
        let one = child_reports_note(&waiting("1")).expect("one waits");
        assert!(one.contains("A child thread's report is waiting"), "{one}");
        assert!(one.contains("lucidos threads child-reports"), "{one}");
        let two = child_reports_note(&waiting("2")).expect("two wait");
        assert!(two.contains("2 child thread reports are waiting"), "{two}");
        assert_eq!(child_reports_note(&waiting("0")), None);
        assert_eq!(child_reports_note(&waiting("x")), None);
        assert_eq!(child_reports_note(&reqwest::header::HeaderMap::new()), None);
    }
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn tls_handshake_keeps_the_commands_response_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0; 4096];
            assert!(stream.read(&mut buffer).unwrap() > 0);
            thread::sleep(Duration::from_millis(1200));
            stream.write_all(b"HTTP/1.1 200 OK\r\n\r\n").unwrap();
        });
        let error = client_with_timeout(Duration::from_secs(5))
            .unwrap()
            .post(format!("https://{address}"))
            .send_with_connect_retry()
            .unwrap_err();
        assert!(!error.is_timeout(), "{error:?}");
        assert!(!retryable_connect_error(&error));
        server.join().unwrap();
    }

    #[test]
    fn response_timeout_is_not_retried() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let check = listener.try_clone().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0; 4096];
            assert!(stream.read(&mut buffer).unwrap() > 0);
            thread::sleep(Duration::from_millis(300));
        });
        let error = client_with_timeout(Duration::from_millis(100))
            .unwrap()
            .post(format!("http://{address}"))
            .body("mutation")
            .send_with_connect_retry()
            .unwrap_err();
        assert!(error.is_timeout(), "{error:?}");
        assert!(!retryable_connect_error(&error));
        server.join().unwrap();
        check.set_nonblocking(true).unwrap();
        assert_eq!(
            check.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[test]
    fn tls_failure_on_an_established_socket_is_not_retried() {
        let (port, captured) = spawn_capturing_server();
        let error = client()
            .unwrap()
            .post(format!("https://127.0.0.1:{port}"))
            .send_with_connect_retry()
            .unwrap_err();
        captured.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(error.is_connect(), "{error:?}");
        assert!(!retryable_connect_error(&error));
    }

    #[test]
    fn a_tls_handshake_on_a_plain_socket_is_recognised_and_named() {
        assert!(is_tls_against_plain_http(
            "received corrupt message of type InvalidContentType: record overflow"
        ));
        assert!(is_tls_against_plain_http("Record overflow"));
        assert!(!is_tls_against_plain_http("connection refused"));
        assert!(!is_tls_against_plain_http("operation timed out"));
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

    fn spawn_silent_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            thread::sleep(Duration::from_secs(30));
            drop(stream);
        });
        port
    }

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

    fn spawn_status_server(status_line: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf = [0u8; 8192];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(
                format!("HTTP/1.1 {status_line}\r\nContent-Length: 2\r\n\r\n{{}}").as_bytes(),
            );
        });
        port
    }

    const BOOT_RESPONSE: &str = "503 Service Unavailable\r\nX-Lucidos-Engine-Booting: 1";

    #[test]
    fn a_boot_rejection_retries_a_clonable_post() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            for status in [BOOT_RESPONSE, "200 OK"] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut request = BufReader::new(&mut stream);
                let mut line = String::new();
                request.read_line(&mut line).unwrap();
                assert!(line.starts_with("POST "));
                loop {
                    line.clear();
                    assert_ne!(request.read_line(&mut line).unwrap(), 0);
                    if line == "\r\n" {
                        break;
                    }
                }
                let mut body = [0; 8];
                request.read_exact(&mut body).unwrap();
                assert_eq!(&body, b"mutation");
                write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .unwrap();
            }
        });
        let response = client_with_timeout(Duration::from_secs(2))
            .unwrap()
            .post(url)
            .body("mutation")
            .send_with_connect_retry_within(Duration::from_secs(2))
            .unwrap();
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        server.join().unwrap();
    }

    #[test]
    fn boot_retry_preserves_terminal_responses() {
        for (status, wait, streaming) in [
            ("503 Service Unavailable", Duration::from_secs(1), false),
            (
                "503 Service Unavailable\r\nX-Lucidos-Engine-Booting: 0",
                Duration::from_secs(1),
                false,
            ),
            (BOOT_RESPONSE, Duration::from_millis(10), false),
            (BOOT_RESPONSE, Duration::from_secs(1), true),
        ] {
            let url = format!("http://127.0.0.1:{}/x", spawn_status_server(status));
            let mut request = client_with_timeout(Duration::from_secs(2))
                .unwrap()
                .post(url);
            if streaming {
                request = request.body(reqwest::blocking::Body::sized(std::io::empty(), 0));
                assert!(request.try_clone().is_none());
            }
            let response = request.send_with_connect_retry_within(wait).unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
            assert_eq!(response.text().unwrap(), "{}");
        }
    }

    #[test]
    fn an_unreplayed_boot_rejection_is_a_definitive_refusal() {
        let url = format!("http://127.0.0.1:{}/x", spawn_status_server(BOOT_RESPONSE));
        let request = client_with_timeout(Duration::from_secs(2))
            .unwrap()
            .post(&url)
            .body(reqwest::blocking::Body::sized(std::io::empty(), 0));
        let refusal = send_or_refusal("POST", &url, request).unwrap().unwrap_err();
        assert!(refusal.to_string().contains("503"));
    }

    #[test]
    fn a_client_error_is_a_refusal_and_a_server_error_is_unknown() {
        let send = |status_line| {
            let url = format!("http://127.0.0.1:{}/x", spawn_status_server(status_line));
            send_or_refusal("POST", &url, reqwest::blocking::Client::new().post(&url))
        };
        assert_eq!(send("200 OK").unwrap().unwrap(), "{}");
        assert!(send("409 Conflict").unwrap().is_err());
        assert!(send("500 Internal Server Error").is_err());
    }

    #[test]
    fn a_stale_origin_token_refusal_keeps_the_server_body_verbatim() {
        let body = r#"{"error":"The engine may have restarted since it was issued. Run it again.","reason":"origin_token_stale"}"#;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 8192];
            assert!(stream.read(&mut buf).unwrap() > 0);
            write!(
                stream,
                "HTTP/1.1 401 Unauthorized\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let url = format!("http://127.0.0.1:{port}/api/v1/data/artifacts/report.json");
        let error = send_expect_success("PUT", &url, reqwest::blocking::Client::new().put(&url))
            .unwrap_err()
            .to_string();
        server.join().unwrap();
        assert_eq!(
            error,
            format!("PUT {url} returned 401 Unauthorized: {body}")
        );
    }

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

    fn env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        &LOCK
    }

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

    #[test]
    fn default_headers_from_env_does_not_rewrite_the_token() {
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

    #[test]
    fn default_headers_from_env_yields_empty_when_env_unset() {
        let headers = headers_from(None, None, None);
        assert!(
            headers.is_empty(),
            "no origin header may be sent outside a Lucidos-spawned subprocess"
        );
    }

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
            msg.contains("outcome is unknown"),
            "msg must explain what is known after the deadline: {msg}"
        );
        assert!(
            msg.contains("may still finish"),
            "msg must describe the unknown outcome: {msg}"
        );
        assert!(!msg.contains("engine.log"));
    }

    #[test]
    fn format_request_error_classifies_connection_refused() {
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
        assert!(
            !msg.contains("scripts/"),
            "the hint must work without a checkout: {msg}"
        );
        assert!(msg.contains("restarting"), "msg must name a restart: {msg}");
        let occurrences = msg.matches(url.as_str()).count();
        assert_eq!(
            occurrences, 1,
            "url must appear exactly once (not embedded in reqwest's err display): {msg}"
        );
    }

    #[test]
    fn format_request_error_always_includes_method_and_url() {
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
