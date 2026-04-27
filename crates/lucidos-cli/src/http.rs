use crate::workspace::BoxError;

/// Blocking HTTP client preconfigured for the local Lucidos engine.
/// Accepts the engine's self-signed cert because the target is `localhost`.
pub(crate) fn client() -> Result<reqwest::blocking::Client, BoxError> {
    reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e).into())
}

/// HTTP client for the MCP permission server. Disables reqwest's default 30s
/// blocking timeout because `/api/internal/permission-prompt` waits for the
/// user's click. With the default timeout, every prompt fails after 30s and
/// CC pivots to a `Bash` heredoc (in `--allowedTools`) that bypasses the
/// gate entirely.
pub(crate) fn permission_prompt_client() -> Result<reqwest::blocking::Client, BoxError> {
    reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(true)
        .timeout(None)
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e).into())
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
            stream.read(&mut buf).expect("read request");
            thread::sleep(delay);
            let body = b"{}";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            stream.write_all(response.as_bytes()).expect("write headers");
            stream.write_all(body).expect("write body");
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
}
