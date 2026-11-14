//! Exercise restart recovery through the installed command's HTTP path.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::{Command, Output};
use std::thread;
use std::time::{Duration, Instant};

fn unused_address() -> std::net::SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

fn command(address: std::net::SocketAddr) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lucidos"));
    command
        .env("LUCIDOS_API_BASE_URL", format!("http://{address}"))
        .env_remove("LUCIDOS_CLI_CONNECT_WAIT_SECS");
    command
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn serve_once(listener: TcpListener, response: &'static [u8]) -> String {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut stream = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(Instant::now() < deadline, "CLI never connected");
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept: {error}"),
        }
    };
    // On macOS an accepted stream inherits the listener's non-blocking mode.
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut request = Vec::new();
    let mut buffer = [0; 4096];
    loop {
        let read = stream.read(&mut buffer).unwrap();
        assert!(read > 0, "request ended early");
        request.extend_from_slice(&buffer[..read]);
        if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
            let length: usize = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length: "))
                .unwrap_or("0")
                .parse()
                .unwrap();
            if request.len() >= end + 4 + length {
                break;
            }
        }
    }
    stream.write_all(response).unwrap();
    String::from_utf8(request).unwrap()
}

#[test]
fn commands_survive_connection_refused_until_the_engine_returns() {
    for args in [
        vec!["model-prices"],
        vec![
            "events",
            "emit",
            "SnapshotCompleted",
            "--payload",
            "{}",
            "--summary",
            "snapshot ready",
        ],
    ]
    .into_iter()
    .chain(["PUT", "PATCH", "DELETE"].map(|method| {
        vec![
            "proxy",
            "snapshot",
            "-X",
            method,
            "-d",
            "update",
            "-H",
            "X-Retry-Test: preserved",
        ]
    })) {
        let address = unused_address();
        let mut command = command(address);
        command.args(&args);
        let child = thread::spawn(move || command.output().unwrap());
        // Nothing is listening while the command makes its initial attempt.
        thread::sleep(Duration::from_millis(600));
        let request = serve_once(
            TcpListener::bind(address).unwrap(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
        );
        let output = child.join().unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
        if args[0] == "events" {
            assert_eq!(output.stdout, b"{}\n");
            assert!(request.starts_with("POST /api/v1/events/emit "));
            assert!(request.contains("snapshot ready"));
        } else if args[0] == "proxy" {
            assert_eq!(output.stdout, b"{}");
            assert!(request.starts_with(&format!("{} /api/v1/proxy/snapshot ", args[3])));
            assert!(request.ends_with("update"));
            assert!(request
                .to_ascii_lowercase()
                .contains("x-retry-test: preserved"));
        } else {
            assert_eq!(output.stdout, b"{}\n");
            assert!(request.starts_with("GET /api/v1/model-prices "));
        }
    }
}

#[test]
fn exhausted_window_reports_the_elapsed_wait() {
    let start = Instant::now();
    let output = command(unused_address())
        .env("LUCIDOS_CLI_CONNECT_WAIT_SECS", "1")
        .arg("model-prices")
        .output()
        .unwrap();
    let elapsed = start.elapsed();
    assert!(!output.status.success());
    assert!(elapsed >= Duration::from_secs(1), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    let error = stderr(&output);
    assert!(error.contains("could not connect after 1."), "{error}");
    assert!(error.contains("Is the engine running?"), "{error}");
}

#[test]
fn zero_disables_the_connect_wait() {
    let start = Instant::now();
    let output = command(unused_address())
        .env("LUCIDOS_CLI_CONNECT_WAIT_SECS", "0")
        .arg("model-prices")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(start.elapsed() < Duration::from_secs(2));
    assert!(stderr(&output).contains("could not connect after 0."));
}

#[test]
fn git_refresh_does_not_wait_for_engine_recovery() {
    let repo = tempfile::tempdir().unwrap();
    for args in [
        vec!["init", "--initial-branch=retry-test"],
        vec![
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
    ] {
        let output = Command::new("git")
            .current_dir(repo.path())
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
    }
    let start = Instant::now();
    let output = command(unused_address())
        .current_dir(repo.path())
        .env("LUCIDOS_CLI_CONNECT_WAIT_SECS", "10")
        .env("LUCIDOS_THREAD_ID", "00000000-0000-4000-8000-000000000001")
        .arg("coding-agent-diff-hook")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("could not connect"));
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn established_connections_and_http_errors_are_never_replayed() {
    for response in [
        &b"HTTP/1.1 503 Unavailable\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}"[..],
        &b"HTTP/1.1 307 Temporary Redirect\r\nLocation: /elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"[..],
        &b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{}"[..],
        &b""[..],
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let check = listener.try_clone().unwrap();
        let server = thread::spawn(move || serve_once(listener, response));
        let output = command(address)
            .env("LUCIDOS_CLI_CONNECT_WAIT_SECS", "1")
            .args(["events", "emit", "SnapshotCompleted", "--payload", "{}", "--summary", "snapshot ready"])
            .output()
            .unwrap();
        server.join().unwrap();
        assert!(!output.status.success(), "{output:?}");
        assert_eq!(check.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock);
    }
}
