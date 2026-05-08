//! `lucidos proxy` — call backends configured in `data/config/apis.json`
//! through the engine, so scripts never see credentials.
//!
//! curl-style ergonomics:
//! - body to stdout, status discarded by default (exit 0 even on 4xx/5xx)
//! - `--fail` mirrors `curl --fail`: nonzero exit on 4xx/5xx, suppress body
//! - `--include` mirrors `curl -i`: prepend status line + headers to stdout
//! - `-H` repeated for headers, `-X` for method, `-d` / stdin for body

use std::io::{self, Read, Write};

use crate::http::client;
use crate::workspace::{BoxError, Workspace};

#[derive(Debug, Clone)]
pub(crate) struct ProxyArgs {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) method: String,
    pub(crate) headers: Vec<String>,
    pub(crate) body: BodySource,
    pub(crate) include: bool,
    pub(crate) fail: bool,
    pub(crate) credentials: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum BodySource {
    None,
    Inline(String),
    Stdin,
}

pub(crate) fn run(ws: &Workspace, args: ProxyArgs) -> Result<u8, BoxError> {
    if args.credentials {
        return run_credentials(ws, &args.name, &args.path);
    }
    let body_bytes = read_body(&args.body)?;
    let url = build_request_url(&ws.base_url(), &args.name, &args.path)?;
    let method = parse_method(&args.method)?;
    let headers = parse_headers(&args.headers)?;

    let client = client()?;
    let mut req = client.request(method, &url);
    for (name, value) in &headers {
        req = req.header(name, value);
    }
    if let Some(body) = body_bytes {
        req = req.body(body);
    }

    let resp = req
        .send()
        .map_err(|e| format!("Failed to send request to {}: {}", url, e))?;
    let status = resp.status();
    let resp_headers = resp.headers().clone();
    let body = resp
        .bytes()
        .map_err(|e| format!("Failed to read response body: {}", e))?;

    let stdout = io::stdout();
    let mut out = stdout.lock();

    if args.include {
        writeln!(
            out,
            "HTTP/1.1 {} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("")
        )?;
        for (name, value) in &resp_headers {
            if let Ok(v) = value.to_str() {
                writeln!(out, "{}: {}", name, v)?;
            }
        }
        writeln!(out)?;
    }

    if args.fail && (status.is_client_error() || status.is_server_error()) {
        let _ = writeln!(
            io::stderr(),
            "lucidos proxy: HTTP {} {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or("")
        );
        return Ok(22); // curl uses exit 22 for HTTP errors
    }

    out.write_all(&body)?;
    Ok(0)
}

fn run_credentials(ws: &Workspace, name: &str, path: &str) -> Result<u8, BoxError> {
    if !path.is_empty() {
        return Err(format!(
            "--credentials cannot be combined with a request path (got {:?})",
            path
        )
        .into());
    }
    let url = build_credentials_url(&ws.base_url(), name)?;
    let client = client()?;
    let resp = client
        .get(&url)
        .send()
        .map_err(|e| format!("Failed to fetch credentials for {}: {}", name, e))?;
    let status = resp.status();
    let body = resp
        .bytes()
        .map_err(|e| format!("Failed to read response body: {}", e))?;
    if status.is_client_error() || status.is_server_error() {
        let body_text = String::from_utf8_lossy(&body);
        let _ = writeln!(
            io::stderr(),
            "lucidos proxy: HTTP {} {} — {}",
            status.as_u16(),
            status.canonical_reason().unwrap_or(""),
            body_text.trim(),
        );
        return Ok(22);
    }
    let stdout = io::stdout();
    let mut out = stdout.lock();
    out.write_all(&body)?;
    Ok(0)
}

fn read_body(source: &BodySource) -> Result<Option<Vec<u8>>, BoxError> {
    match source {
        BodySource::None => Ok(None),
        BodySource::Inline(s) => Ok(Some(s.as_bytes().to_vec())),
        BodySource::Stdin => {
            let mut buf = Vec::new();
            io::stdin()
                .read_to_end(&mut buf)
                .map_err(|e| format!("Failed to read stdin: {}", e))?;
            Ok(Some(buf))
        }
    }
}

/// Build the engine URL: `<base>/api/v1/proxy/<name><path>`.
/// Names are restricted to ASCII alphanumeric + `-` + `_` so they're safe
/// to splice into a path segment without URL-encoding (the same charset that
/// the JSON config file would realistically use).
pub(crate) fn build_request_url(
    base_url: &str,
    name: &str,
    path: &str,
) -> Result<String, BoxError> {
    validate_name(name)?;
    let normalized_path = if path.is_empty() {
        String::new()
    } else if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };
    Ok(format!(
        "{}/api/v1/proxy/{}{}",
        base_url.trim_end_matches('/'),
        name,
        normalized_path
    ))
}

/// Build the engine URL for the credential-bundle endpoint:
/// `<base>/api/v1/proxy-credentials/<name>`.
pub(crate) fn build_credentials_url(base_url: &str, name: &str) -> Result<String, BoxError> {
    validate_name(name)?;
    Ok(format!(
        "{}/api/v1/proxy-credentials/{}",
        base_url.trim_end_matches('/'),
        name,
    ))
}

fn validate_name(name: &str) -> Result<(), BoxError> {
    if name.is_empty() {
        return Err("proxy name is empty".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "proxy name {:?} must contain only ASCII letters, digits, '-', or '_'",
            name
        )
        .into());
    }
    Ok(())
}

fn parse_method(s: &str) -> Result<reqwest::Method, BoxError> {
    reqwest::Method::from_bytes(s.to_uppercase().as_bytes())
        .map_err(|e| format!("Invalid HTTP method {:?}: {}", s, e).into())
}

/// Parse `-H "Foo: bar"` repeated arguments into header pairs.
pub(crate) fn parse_headers(raw: &[String]) -> Result<Vec<(String, String)>, BoxError> {
    let mut out = Vec::with_capacity(raw.len());
    for entry in raw {
        let (name, value) = entry
            .split_once(':')
            .ok_or_else(|| format!("Invalid header {:?} (expected 'Name: value')", entry))?;
        let name = name.trim().to_string();
        let value = value.trim().to_string();
        if name.is_empty() {
            return Err(format!("Invalid header {:?} (empty name)", entry).into());
        }
        out.push((name, value));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_url_basic() {
        let url = build_request_url("https://localhost:8443", "sonos", "/Spisestua/play").unwrap();
        assert_eq!(
            url,
            "https://localhost:8443/api/v1/proxy/sonos/Spisestua/play"
        );
    }

    #[test]
    fn build_url_adds_leading_slash() {
        let url = build_request_url("https://localhost:8443", "sonos", "play").unwrap();
        assert_eq!(url, "https://localhost:8443/api/v1/proxy/sonos/play");
    }

    #[test]
    fn build_url_handles_empty_path() {
        let url = build_request_url("https://localhost:8443", "sonos", "").unwrap();
        assert_eq!(url, "https://localhost:8443/api/v1/proxy/sonos");
    }

    #[test]
    fn build_url_strips_trailing_slash_from_base() {
        let url = build_request_url("https://localhost:8443/", "sonos", "/x").unwrap();
        assert_eq!(url, "https://localhost:8443/api/v1/proxy/sonos/x");
    }

    #[test]
    fn build_url_rejects_empty_name() {
        assert!(build_request_url("https://x", "", "/y").is_err());
    }

    #[test]
    fn build_url_rejects_slash_in_name() {
        assert!(build_request_url("https://x", "foo/bar", "/y").is_err());
    }

    #[test]
    fn build_url_rejects_whitespace_in_name() {
        assert!(build_request_url("https://x", "weird name", "/y").is_err());
    }

    #[test]
    fn build_url_accepts_dashes_and_underscores() {
        let url = build_request_url("https://x", "my-api_v2", "/y").unwrap();
        assert_eq!(url, "https://x/api/v1/proxy/my-api_v2/y");
    }

    #[test]
    fn parse_headers_splits_on_first_colon() {
        let h = parse_headers(&["Authorization: Bearer abc:def".to_string()]).unwrap();
        assert_eq!(
            h,
            vec![("Authorization".to_string(), "Bearer abc:def".to_string())]
        );
    }

    #[test]
    fn parse_headers_trims_whitespace() {
        let h = parse_headers(&["  X-Foo  :  bar  ".to_string()]).unwrap();
        assert_eq!(h, vec![("X-Foo".to_string(), "bar".to_string())]);
    }

    #[test]
    fn parse_headers_rejects_missing_colon() {
        assert!(parse_headers(&["nocolon".to_string()]).is_err());
    }

    #[test]
    fn parse_headers_rejects_empty_name() {
        assert!(parse_headers(&[": value".to_string()]).is_err());
    }

    #[test]
    fn parse_headers_accepts_empty_value() {
        let h = parse_headers(&["X-Empty:".to_string()]).unwrap();
        assert_eq!(h, vec![("X-Empty".to_string(), "".to_string())]);
    }

    #[test]
    fn build_credentials_url_basic() {
        let url = build_credentials_url("https://localhost:8443", "comfort_creds").unwrap();
        assert_eq!(
            url,
            "https://localhost:8443/api/v1/proxy-credentials/comfort_creds"
        );
    }

    #[test]
    fn build_credentials_url_strips_trailing_slash_from_base() {
        let url = build_credentials_url("https://localhost:8443/", "x").unwrap();
        assert_eq!(url, "https://localhost:8443/api/v1/proxy-credentials/x");
    }

    #[test]
    fn build_credentials_url_rejects_empty_name() {
        assert!(build_credentials_url("https://x", "").is_err());
    }

    #[test]
    fn build_credentials_url_rejects_slash_in_name() {
        assert!(build_credentials_url("https://x", "foo/bar").is_err());
    }

    #[test]
    fn build_credentials_url_accepts_dashes_and_underscores() {
        let url = build_credentials_url("https://x", "my-creds_v2").unwrap();
        assert_eq!(url, "https://x/api/v1/proxy-credentials/my-creds_v2");
    }
}
