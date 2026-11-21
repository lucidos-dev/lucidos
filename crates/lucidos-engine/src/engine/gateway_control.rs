//! The engine's loopback hop to its gateway's control API: the listing at
//! `/~/api/v1/control/workspaces`, and the workspace broker's mutating calls.
//!
//! A call over loopback carries no browser fetch metadata, so `control_authz`
//! (`lucidos-gateway/src/control.rs`) allows it. Same posture as
//! [`crate::boot_report`] and `api::history::restart_via_gateway`: the
//! loopback client, which trusts only the loopback CA. Callers own the
//! deadline and the projection: the listing names every workspace on the
//! machine, so no caller passes it through raw.

/// The raw body of the gateway's `GET /~/api/v1/control{path}`, or why not.
/// Unbounded: wrap it in the caller's deadline.
pub async fn fetch_control(gateway_port: &str, path: &str) -> Result<String, String> {
    send_control(
        gateway_port,
        reqwest::Method::GET,
        path,
        &serde_json::Value::Null,
    )
    .await
    .map_err(|e| format!("{e:?}"))
}

/// Why a mutating control call did not succeed. Neither arm ever holds the
/// gateway's raw body: only redacted error text, a known reason slug and the
/// gateway's explicit unchanged marker.
#[derive(Debug, PartialEq)]
pub enum ControlRefusal {
    /// The gateway answered with this status and message.
    Refused {
        status: u16,
        message: String,
        unchanged: bool,
        reason: Option<&'static str>,
    },
    /// No gateway answered, or its answer was unreadable.
    Unreachable(String),
}

/// The longest gateway message passed back, in characters.
const MESSAGE_MAX_CHARS: usize = 400;
/// The largest gateway body read, in bytes.
const BODY_MAX_BYTES: usize = 64 * 1024;

/// POST `body` to `/~/api/v1/control{path}` and return the success body.
/// Never retried: once a request may have reached the gateway, a retry could
/// act twice. Unbounded: wrap it in a deadline.
pub async fn post_control(
    gateway_port: &str,
    path: &str,
    body: &serde_json::Value,
) -> Result<String, ControlRefusal> {
    send_control(gateway_port, reqwest::Method::POST, path, body).await
}

/// Send one control request. Never retry an uncertain outcome or follow redirects.
pub async fn send_control(
    gateway_port: &str,
    method: reqwest::Method,
    path: &str,
    body: &serde_json::Value,
) -> Result<String, ControlRefusal> {
    let client = crate::gateway_auth::client_builder()
        .retry(reqwest::retry::never())
        .build()
        .map_err(|e| {
            ControlRefusal::Unreachable(format!("could not build the gateway client ({e})"))
        })?;
    let token = lucidos_local_token::read();
    send_with(
        &client,
        token.as_deref(),
        gateway_port,
        (method, path),
        body,
    )
    .await
}

async fn send_with(
    client: &reqwest::Client,
    token: Option<&str>,
    gateway_port: &str,
    (method, path): (reqwest::Method, &str),
    body: &serde_json::Value,
) -> Result<String, ControlRefusal> {
    let unreachable = |why: String| ControlRefusal::Unreachable(redact(&why, token));
    let url = format!(
        "{}/~/api/v1/control{path}",
        lucidos_local_token::client::origin(gateway_port)
    );
    let request = client.request(method.clone(), &url);
    let request = if method == reqwest::Method::GET {
        request
    } else {
        request.json(body)
    };
    let mut response = request.send().await.map_err(|e| {
        unreachable(format!(
            "the gateway on :{gateway_port} did not answer ({e})"
        ))
    })?;
    let status = response.status();
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| unreachable(format!("the gateway answer was unreadable ({e})")))?
    {
        if bytes.len() + chunk.len() > BODY_MAX_BYTES {
            return Err(unreachable("the gateway answer was too large".into()));
        }
        bytes.extend_from_slice(&chunk);
    }
    if status.is_success() {
        return Ok(redact(&String::from_utf8_lossy(&bytes), token));
    }
    let fields = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default();
    let owner_device = lucidos_gateway_constants::DEVICE_SESSION_OWNER_DEVICE;
    Err(ControlRefusal::Refused {
        unchanged: fields["changed"] == false,
        // Only a known slug crosses the boundary; never gateway credentials.
        reason: (fields["reason"] == owner_device).then_some(owner_device),
        status: status.as_u16(),
        message: redact(&refusal_message(&bytes, status), token),
    })
}

/// The gateway's `{"error": …}` text, capped, or the status when it sent none.
fn refusal_message(bytes: &[u8], status: reqwest::StatusCode) -> String {
    let message = serde_json::from_slice::<serde_json::Value>(bytes)
        .ok()
        .and_then(|body| body.get("error")?.as_str().map(str::to_owned))
        .unwrap_or_else(|| format!("the gateway answered {status}"));
    match message.char_indices().nth(MESSAGE_MAX_CHARS) {
        Some((end, _)) => format!("{}\u{2026}", &message[..end]),
        None => message,
    }
}

/// `text` with every occurrence of the local token replaced.
fn redact(text: &str, token: Option<&str>) -> String {
    match token {
        Some(token) if !token.is_empty() => text.replace(token, "[redacted]"),
        _ => text.to_owned(),
    }
}

#[cfg(test)]
async fn post_with(
    client: &reqwest::Client,
    token: Option<&str>,
    gateway_port: &str,
    path: &str,
    body: &serde_json::Value,
) -> Result<String, ControlRefusal> {
    send_with(
        client,
        token,
        gateway_port,
        (reqwest::Method::POST, path),
        body,
    )
    .await
}

#[cfg(test)]
#[path = "gateway_control_tests.rs"]
mod tests;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use axum::{http::StatusCode, routing::get, Router};

    /// Serve `body` at the control-listing path on a loopback port, over TLS
    /// from the test loopback CA, and hand back the port. Stands in for the
    /// gateway.
    pub async fn stand_in_gateway(body: &'static str, status: StatusCode) -> u16 {
        let app = Router::<()>::new().route(
            "/~/api/v1/control/workspaces",
            get(move || async move { (status, body) }),
        );
        lucidos_local_token::test_support::serve_https(app).port()
    }
}
