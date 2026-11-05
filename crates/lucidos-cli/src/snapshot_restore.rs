//! Agent restore admission: prove the issuing workspace before gateway routing.
use crate::workspace::BoxError;
use serde_json::{json, Value};

fn validate_target(target: &str, expected: &str) -> Result<(), BoxError> {
    if !lucidos_gateway_constants::is_valid_workspace_id(target) {
        return Err("snapshot_target_invalid".into());
    }
    if expected != target {
        return Err("snapshot_restore_binding_mismatch".into());
    }
    Ok(())
}

fn verified_source<'a>(
    proof: &'a Value,
    thread: uuid::Uuid,
    target: &str,
) -> Result<&'a str, BoxError> {
    let source = proof["workspace_id"]
        .as_str()
        .ok_or("snapshot_restore_caller_unverified")?;
    if !lucidos_gateway_constants::is_valid_workspace_id(source)
        || proof["source_thread_id"]
            .as_str()
            .and_then(|id| id.parse::<uuid::Uuid>().ok())
            != Some(thread)
    {
        return Err("snapshot_restore_caller_unverified".into());
    }
    if source == target {
        return Err("snapshot_restore_own_workspace_refused".into());
    }
    Ok(source)
}

pub(super) fn run(
    handle: &str,
    target: &str,
    expected: &str,
    reason: Option<&str>,
    port: Option<u16>,
    json_output: bool,
    wait: bool,
) -> Result<u8, BoxError> {
    validate_target(target, expected)?;
    let path = format!("{}/restore", super::handle_path(handle)?);
    // No owner-device impersonation: a terminal without an issuing thread must
    // use the owner UI. The source engine verifies the opaque origin credential.
    let thread = std::env::var("LUCIDOS_THREAD_ID")
        .map_err(|_| "snapshot_restore_caller_unverified")?
        .parse::<uuid::Uuid>()?;
    let origin = std::env::var("LUCIDOS_AGENT_ORIGIN_TOKEN")
        .map_err(|_| "snapshot_restore_caller_unverified")?;
    let local = lucidos_local_token::read().ok_or("snapshot_restore_caller_unverified")?;
    let workspace = crate::workspace::resolve_from_env()?;
    let socket = lucidos_engine_socket::path_for(&workspace.root)?;
    // Deliberately no basename assertion: the socket and engine-issued proof
    // establish the source identity, never a directory name supplied by a caller.
    let client = reqwest::blocking::Client::builder()
        .unix_socket(socket)
        .no_proxy()
        .retry(reqwest::retry::never())
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let response = client
        .get("http://localhost/api/v1/snapshots/restore-caller")
        .header(crate::http::HEADER_AGENT_ORIGIN_TOKEN, &origin)
        .header(lucidos_local_token::HEADER_LOCAL_TOKEN, &local)
        .send()
        .map_err(|_| "snapshot_restore_caller_unverified")?;
    if !response.status().is_success() {
        return Err("snapshot_restore_caller_unverified".into());
    }
    let proof: Value = response
        .json()
        .map_err(|_| "snapshot_restore_caller_unverified")?;
    let source = verified_source(&proof, thread, target)?;
    // Self-restore is refused before even probing a gateway.
    let base = crate::gateway::the_gateway(
        &crate::gateway::probe_client()?,
        port,
        "Pass --port to name the gateway that owns the target workspace.",
    )?;
    let client = crate::http::client()?;
    let (mut code, mut result) = send_once(&client, &base, &path, target, source, reason)?;
    if wait && code == 0 {
        let id = result["restore_id"]
            .as_str()
            .ok_or("snapshot_restore_id_missing")?
            .parse()?;
        eprintln!("Restore {id} admitted; waiting for its gateway journal.");
        (code, result) = wait_restore(
            &client,
            &base,
            target,
            id,
            std::time::Duration::from_secs(1),
        )?;
    }
    if json_output {
        println!("{}", serde_json::to_string(&result)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    Ok(code)
}

fn wait_restore(
    client: &reqwest::blocking::Client,
    base: &str,
    target: &str,
    id: uuid::Uuid,
    interval: std::time::Duration,
) -> Result<(u8, Value), BoxError> {
    loop {
        let status = super::read_restore_status(client, base, id, target)?;
        if let Some(code) = super::restore_terminal_code(&status)? {
            return Ok((code, status));
        }
        std::thread::sleep(interval);
    }
}

fn send_once(
    client: &reqwest::blocking::Client,
    base: &str,
    path: &str,
    target: &str,
    source: &str,
    reason: Option<&str>,
) -> Result<(u8, Value), BoxError> {
    let response = client
        .post(format!("{base}/{target}/api/v1{path}"))
        .header(crate::http::HEADER_TARGET_WORKSPACE, target)
        .json(&json!({"expected_workspace_id":target, "source_workspace":source, "reason":reason}))
        .send()
        .map_err(|_| "snapshot_restore_delivery_unknown")?;
    let status = response.status();
    let body = response
        .json()
        .map_err(|_| "snapshot_restore_delivery_unknown")?;
    let code = if status.is_success() {
        0
    } else if status == reqwest::StatusCode::CONFLICT {
        75
    } else if status.is_client_error() {
        2
    } else {
        4
    };
    Ok((code, body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    #[test]
    fn restore_posts_once_for_acceptance_refusal_redirect_and_lost_reply() {
        for (status, expected) in [
            (202, Some(0)),
            (409, Some(75)),
            (403, Some(2)),
            (503, Some(4)),
            (307, Some(4)),
            (0, None),
        ] {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut bytes = Vec::new();
                let (headers, body) = loop {
                    let mut chunk = [0; 8192];
                    let n = stream.read(&mut chunk).unwrap();
                    assert!(n > 0);
                    bytes.extend_from_slice(&chunk[..n]);
                    if let Some(end) = bytes.windows(4).position(|b| b == b"\r\n\r\n") {
                        let headers = String::from_utf8(bytes[..end].to_vec())
                            .unwrap()
                            .to_lowercase();
                        let length = headers
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .unwrap()
                            .parse::<usize>()
                            .unwrap();
                        if bytes.len() >= end + 4 + length {
                            break (
                                headers,
                                serde_json::from_slice::<Value>(&bytes[end + 4..]).unwrap(),
                            );
                        }
                    }
                };
                assert!(headers.starts_with(
                    "post /target/api/v1/snapshots/snap_00000000000000000000000000/restore "
                ));
                assert!(headers.contains("x-lucidos-target-workspace: target"));
                assert_eq!(
                    body,
                    json!({"expected_workspace_id":"target", "source_workspace":"source", "reason":"test"})
                );
                if status != 0 {
                    let body = "{}";
                    write!(stream, "HTTP/1.1 {status} Test\r\nLocation: /retry\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                }
                drop(stream);
                listener
            });
            let client = reqwest::blocking::Client::builder()
                .no_proxy()
                .retry(reqwest::retry::never())
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap();
            let result = send_once(
                &client,
                &base,
                "/snapshots/snap_00000000000000000000000000/restore",
                "target",
                "source",
                Some("test"),
            );
            match expected {
                Some(code) => assert_eq!(result.unwrap().0, code),
                None => assert_eq!(
                    result.unwrap_err().to_string(),
                    "snapshot_restore_delivery_unknown"
                ),
            }
            let listener = server.join().unwrap();
            listener.set_nonblocking(true).unwrap();
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
    }

    #[test]
    fn proof_requires_matching_thread_and_refuses_own_workspace() {
        let thread = uuid::Uuid::new_v4();
        let proof = json!({"workspace_id":"source", "source_thread_id":thread.to_string()});
        assert_eq!(verified_source(&proof, thread, "target").unwrap(), "source");
        assert!(verified_source(&proof, thread, "source").is_err());
        assert!(verified_source(&proof, uuid::Uuid::new_v4(), "target").is_err());
        assert!(verified_source(
            &json!({"workspace_id":"../source", "source_thread_id":thread.to_string()}),
            thread,
            "target"
        )
        .is_err());
        assert!(verified_source(&json!({}), thread, "target").is_err());
    }

    #[test]
    fn target_requires_explicit_matching_slug() {
        assert!(validate_target("target", "target").is_ok());
        assert!(validate_target("target", "other").is_err());
        assert!(validate_target("/tmp/target", "/tmp/target").is_err());
    }

    #[test]
    fn wait_reads_only_the_exact_gateway_journal_until_terminal() {
        for (phase, code) in [
            ("completed", 0),
            ("rolled-back", 4),
            ("needs-attention", 4),
            ("refused", 4),
            ("failed-unchanged", 4),
        ] {
            let id = uuid::Uuid::new_v4();
            let (base, server) = status_server(id, "target", &["files-restored", phase], id);
            let client = reqwest::blocking::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap();
            let (actual, result) =
                wait_restore(&client, &base, "target", id, std::time::Duration::ZERO).unwrap();
            assert_eq!(actual, code);
            assert_eq!(result["phase"], phase);
            server.join().unwrap();
        }
    }

    #[test]
    fn wait_refuses_a_replaced_or_foreign_journal() {
        for (id, workspace) in [
            (uuid::Uuid::new_v4(), "target"),
            (uuid::Uuid::nil(), "other"),
        ] {
            let (base, server) = status_server(id, workspace, &["completed"], uuid::Uuid::nil());
            let client = reqwest::blocking::Client::builder()
                .no_proxy()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap();
            assert!(wait_restore(
                &client,
                &base,
                "target",
                uuid::Uuid::nil(),
                std::time::Duration::ZERO
            )
            .is_err());
            server.join().unwrap();
        }
    }

    fn status_server(
        id: uuid::Uuid,
        workspace: &str,
        phases: &[&str],
        requested_id: uuid::Uuid,
    ) -> (String, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let workspace = workspace.to_owned();
        let phases: Vec<String> = phases.iter().map(|p| (*p).to_owned()).collect();
        let server = std::thread::spawn(move || {
            for phase in phases {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(std::time::Instant::now() < deadline, "journal read missing");
                            std::thread::sleep(std::time::Duration::from_millis(1));
                        }
                        Err(error) => panic!("accept status request: {error}"),
                    }
                };
                // macOS hands the accepted stream the listener's non-blocking
                // flag, so a request still in flight would read as WouldBlock.
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let mut bytes = [0; 4096];
                    let n = stream.read(&mut bytes).unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&bytes[..n]);
                }
                let request = String::from_utf8(request).unwrap();
                assert_eq!(request.lines().next().unwrap(), format!("GET /~/api/v1/control/workspaces/target/snapshot-restore-status?restore_id={requested_id} HTTP/1.1"));
                let body =
                    json!({"restore":{"restore_id":id.to_string(),"workspace_id":workspace,"phase":phase}})
                        .to_string();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        (base, server)
    }
}
