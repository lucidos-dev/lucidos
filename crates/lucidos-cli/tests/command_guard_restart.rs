#[path = "support/connect.rs"]
mod connect;

use connect::{serve_once, unused_address};
use std::io::Write;
use std::net::{SocketAddr, TcpListener};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const TOKEN: &str = "test-origin-token";
const PREFIX: &str = "Refused by the command guard: could not verify standing denies";

fn guard(address: SocketAddr, hook: &str) -> Child {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lucidos"))
        .arg(hook)
        .env("LUCIDOS_THREAD_ID", "00000000-0000-4000-8000-000000000001")
        .env("LUCIDOS_API_BASE_URL", format!("http://{address}"))
        .env("LUCIDOS_AGENT_ORIGIN_TOKEN", TOKEN)
        .env("LUCIDOS_CLI_CONNECT_WAIT_SECS", "0")
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"tool_input":{"command":"git status"}}"#)
        .unwrap();
    child
}

fn refusal(output: Output) -> String {
    assert_eq!(output.status.code(), Some(2), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(!stderr.contains(TOKEN), "{stderr}");
    stderr
}

#[test]
fn both_hooks_recover_when_the_engine_starts_listening() {
    for hook in ["cc-bash-guard", "command-deny-hook"] {
        let address = unused_address();
        let child = guard(address, hook);
        thread::sleep(Duration::from_millis(600));
        serve_once(
            TcpListener::bind(address).unwrap(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nnull",
        );
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{output:?}");
    }
}

#[test]
fn unavailable_engine_refuses_within_the_hook_budget() {
    let start = Instant::now();
    let stderr = refusal(
        guard(unused_address(), "cc-bash-guard")
            .wait_with_output()
            .unwrap(),
    );
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(stderr.starts_with(PREFIX), "{stderr}");
    assert!(stderr.contains("could not connect after 3."), "{stderr}");
    assert!(stderr.contains("may be restarting"), "{stderr}");
}

fn scripted_refusal(status: &str, body: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let child = guard(listener.local_addr().unwrap(), "cc-bash-guard");
    let check = listener.try_clone().unwrap();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    serve_once(listener, response.as_bytes());
    let stderr = refusal(child.wait_with_output().unwrap());
    assert_eq!(
        check.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
    stderr
}

#[test]
fn http_failures_report_status_and_only_safe_reason_slugs() {
    let stderr = scripted_refusal(
        "401 Unauthorized",
        r#"{"error":"private response","reason":"origin_token_stale"}"#,
    );
    assert!(stderr.contains("401 Unauthorized"), "{stderr}");
    assert!(stderr.contains("origin_token_stale"), "{stderr}");
    assert!(!stderr.contains("private response"), "{stderr}");

    let stderr = scripted_refusal(
        "500 Internal Server Error",
        r#"{"reason":"unsafe body detail"}"#,
    );
    assert!(stderr.contains("500 Internal Server Error"), "{stderr}");
    assert!(!stderr.contains("unsafe body detail"), "{stderr}");
    assert!(
        scripted_refusal("401 Unauthorized", r#"{"reason":"test-origin-token"}"#)
            .contains("401 Unauthorized")
    );
}

#[test]
fn success_requires_a_valid_decision_and_preserves_refusal_text() {
    let stderr = scripted_refusal("200 OK", "malformed response");
    assert!(stderr.contains("could not parse"), "{stderr}");
    assert!(!stderr.contains("malformed response"), "{stderr}");
    assert_eq!(
        scripted_refusal("200 OK", r#""refusal text""#),
        "refusal text\n"
    );
}
