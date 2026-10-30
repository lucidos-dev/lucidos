//! A minimal, bounded LocalAPI client: one `GET` per connection, over HTTP/1.0.
//!
//! HTTP/1.0 keeps the reply unchunked, and Go's server closes the connection
//! after it, so reading to the end is the whole protocol. No `Origin` or
//! `Referer` is sent: LocalAPI refuses a request that carries either.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use crate::discovery::Endpoint;
use crate::Answer;

const STATUS_PATH: &str = "/localapi/v0/status?peers=false";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);
/// The whole status exchange, from the request to the last byte.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_RESPONSE: usize = 1 << 20;

/// This node's status.
pub(crate) fn query(endpoint: &Endpoint) -> Answer {
    get(endpoint, STATUS_PATH, EXCHANGE_TIMEOUT)
}

/// `GET path`, cut off once `timeout` has passed since the request.
pub(crate) fn get(endpoint: &Endpoint, path: &str, timeout: Duration) -> Answer {
    match endpoint {
        Endpoint::Tcp { port, token } => {
            let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, *port));
            match TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT) {
                Ok(stream) => exchange(stream, &request(path, Some(token)), timeout),
                Err(_) => Answer::Silent,
            }
        }
        #[cfg(unix)]
        Endpoint::Unix(socket) => match std::os::unix::net::UnixStream::connect(socket) {
            Ok(stream) => exchange(stream, &request(path, None), timeout),
            Err(_) => Answer::Silent,
        },
        #[cfg(not(unix))]
        Endpoint::Unix(_) => Answer::Silent,
    }
}

trait Timeouts {
    fn set_timeouts(&self, timeout: Duration) -> std::io::Result<()>;
}

impl Timeouts for TcpStream {
    fn set_timeouts(&self, timeout: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(timeout))?;
        self.set_write_timeout(Some(timeout))
    }
}

#[cfg(unix)]
impl Timeouts for std::os::unix::net::UnixStream {
    fn set_timeouts(&self, timeout: Duration) -> std::io::Result<()> {
        self.set_read_timeout(Some(timeout))?;
        self.set_write_timeout(Some(timeout))
    }
}

fn exchange<S: Read + Write + Timeouts>(mut stream: S, request: &str, timeout: Duration) -> Answer {
    let deadline = Instant::now() + timeout;
    if stream.set_timeouts(timeout).is_err() {
        return Answer::Silent;
    }
    if stream.write_all(request.as_bytes()).is_err() {
        return Answer::Silent;
    }
    let mut raw = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let read = if left.is_zero() || stream.set_timeouts(left).is_err() {
            Err(std::io::ErrorKind::TimedOut.into())
        } else {
            stream.read(&mut buf)
        };
        match read {
            Ok(0) => break,
            Ok(n) if raw.len() + n <= MAX_RESPONSE => raw.extend_from_slice(&buf[..n]),
            Ok(_) => return Answer::Garbled("the reply was too large".into()),
            // A daemon that takes the connection and says nothing is wedged.
            Err(_) if raw.is_empty() => return Answer::Silent,
            Err(e) => return Answer::Garbled(format!("the reply broke off ({e})")),
        }
    }
    answer_of(&raw)
}

fn request(path: &str, token: Option<&str>) -> String {
    let auth = token
        .map(|t| {
            format!(
                "Authorization: Basic {}\r\n",
                base64(format!(":{t}").as_bytes())
            )
        })
        .unwrap_or_default();
    format!("GET {path} HTTP/1.0\r\nHost: local-tailscaled.sock\r\n{auth}\r\n")
}

/// The body of a `200` reply, or the status line and body of any other.
fn answer_of(raw: &[u8]) -> Answer {
    let text = String::from_utf8_lossy(raw);
    let Some((head, body)) = text.split_once("\r\n\r\n") else {
        return Answer::Garbled("the reply had no HTTP header".into());
    };
    let status_line = head.lines().next().unwrap_or_default();
    match status_line.split_whitespace().nth(1) {
        Some("200") => Answer::Body(body.to_string()),
        Some(_) => Answer::Refused {
            status_line: status_line.to_string(),
            body: body.to_string(),
        },
        None => Answer::Garbled("the reply had no HTTP status".into()),
    }
}

/// Standard base64 with padding: all Basic auth needs, without a dependency.
fn base64(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn base64_matches_the_rfc_4648_vectors() {
        for (plain, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            (":s3cret", "OnMzY3JldA=="),
        ] {
            assert_eq!(base64(plain.as_bytes()), encoded);
        }
    }

    #[test]
    fn the_request_is_http_1_0_with_basic_auth_and_no_origin() {
        let req = request(STATUS_PATH, Some("s3cret"));
        assert!(req.starts_with("GET /localapi/v0/status?peers=false HTTP/1.0\r\n"));
        assert!(req.contains("Host: local-tailscaled.sock\r\n"));
        assert!(req.contains("Authorization: Basic OnMzY3JldA==\r\n"));
        assert!(!req.to_ascii_lowercase().contains("origin"));
        assert!(!request(STATUS_PATH, None).contains("Authorization"));
    }

    #[test]
    fn only_a_200_yields_a_body() {
        assert!(matches!(
            answer_of(b"HTTP/1.0 200 OK\r\nContent-Type: application/json\r\n\r\n{}"),
            Answer::Body(b) if b == "{}"
        ));
        assert!(matches!(
            answer_of(b"HTTP/1.0 403 Forbidden\r\n\r\ncert access denied"),
            Answer::Refused { status_line, body }
                if status_line == "HTTP/1.0 403 Forbidden" && body == "cert access denied"
        ));
        assert!(matches!(answer_of(b"garbage"), Answer::Garbled(_)));
    }

    #[test]
    fn a_get_sends_its_own_path() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let n = conn.read(&mut buf).unwrap();
            conn.write_all(b"HTTP/1.0 200 OK\r\n\r\npem").unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        let endpoint = Endpoint::Tcp {
            port,
            token: "t".into(),
        };
        let answer = get(&endpoint, "/localapi/v0/cert/n?type=pair", EXCHANGE_TIMEOUT);
        assert!(matches!(answer, Answer::Body(b) if b == "pem"));
        assert!(seen
            .join()
            .unwrap()
            .starts_with("GET /localapi/v0/cert/n?type=pair HTTP/1.0\r\n"));
    }

    /// Serve one connection with `reply`, after reading the request.
    fn serve_once(reply: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut buf = [0u8; 1024];
            let _ = conn.read(&mut buf);
            let _ = conn.write_all(reply.as_bytes());
        });
        port
    }

    #[test]
    fn a_tcp_endpoint_returns_the_status_body() {
        let port = serve_once("HTTP/1.0 200 OK\r\n\r\n{\"BackendState\":\"Stopped\"}");
        let answer = query(&Endpoint::Tcp {
            port,
            token: "t".into(),
        });
        assert!(matches!(answer, Answer::Body(b) if b.contains("Stopped")));
    }

    #[test]
    fn a_closed_port_is_no_answer() {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let answer = query(&Endpoint::Tcp {
            port,
            token: "t".into(),
        });
        assert!(matches!(answer, Answer::Silent));
    }

    #[test]
    fn a_server_that_never_replies_is_cut_off() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hold = std::thread::spawn(move || {
            let conn = listener.accept();
            std::thread::sleep(EXCHANGE_TIMEOUT + Duration::from_secs(2));
            drop(conn);
        });
        let started = Instant::now();
        let answer = query(&Endpoint::Tcp {
            port,
            token: "t".into(),
        });
        assert!(matches!(answer, Answer::Silent));
        assert!(started.elapsed() < EXCHANGE_TIMEOUT + Duration::from_secs(1));
        drop(hold);
    }

    #[cfg(unix)]
    #[test]
    fn a_missing_unix_socket_is_no_answer() {
        let answer = query(&Endpoint::Unix("/nonexistent/tailscaled.sock".into()));
        assert!(matches!(answer, Answer::Silent));
    }
}
