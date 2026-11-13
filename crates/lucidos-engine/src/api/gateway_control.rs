//! The engine's loopback hop to its gateway's control API: the listing at
//! `/~/api/v1/control/workspaces`, and the workspace broker's mutating calls.
//!
//! A call over loopback carries no browser fetch metadata, so `control_authz`
//! (`lucidos-gateway/src/control.rs`) allows it. Same posture as
//! [`crate::boot_report`] and `api::history::restart_via_gateway`: scheme via
//! [`crate::net_config::gateway_scheme_order`], the self-signed dev cert accepted,
//! no ambient proxy. Callers own the deadline and the projection: the listing
//! names every workspace on the machine, so no caller passes it through raw.

/// The raw body of the gateway's `GET /~/api/v1/control{path}`, or why not.
/// Unbounded: wrap it in the caller's deadline.
pub(super) async fn fetch_control(gateway_port: &str, path: &str) -> Result<String, String> {
    let client = crate::gateway_auth::client_builder()
        .build()
        .map_err(|e| format!("could not build the gateway client ({e})"))?;
    // The gateway's scheme first, the other second, so a dev/packaged
    // mismatch still connects (the class that broke `restart_via_gateway`).
    for scheme in crate::net_config::gateway_scheme_order() {
        let url = format!("{scheme}://127.0.0.1:{gateway_port}/~/api/v1/control{path}");
        let Ok(resp) = client.get(&url).send().await else {
            continue; // unreachable on this scheme, so try the other protocol
        };
        if !resp.status().is_success() {
            // The gateway answered and refused; the other scheme won't differ.
            return Err(format!("gateway control {path} returned {}", resp.status()));
        }
        return resp
            .text()
            .await
            .map_err(|e| format!("gateway control {path} body was unreadable ({e})"));
    }
    Err(format!("gateway on :{gateway_port} unreachable"))
}

/// Why a mutating control call did not succeed. Neither arm ever holds the
/// gateway's raw body: only its `error` text, with the local token redacted.
#[derive(Debug, PartialEq)]
pub(super) enum ControlRefusal {
    /// The gateway answered with this status and message.
    Refused { status: u16, message: String },
    /// No gateway answered, or its answer was unreadable.
    Unreachable(String),
}

/// The longest gateway message passed back, in characters.
const MESSAGE_MAX_CHARS: usize = 400;
/// The largest gateway body read, in bytes.
const BODY_MAX_BYTES: usize = 64 * 1024;

/// POST `body` to `/~/api/v1/control{path}` and return the success body.
/// Only a connect failure tries the other scheme: once a request may have
/// reached the gateway, a retry could act twice. Unbounded: wrap it in a
/// deadline.
pub(super) async fn post_control(
    gateway_port: &str,
    path: &str,
    body: &serde_json::Value,
) -> Result<String, ControlRefusal> {
    let client = crate::gateway_auth::client_builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
        .build()
        .map_err(|e| {
            ControlRefusal::Unreachable(format!("could not build the gateway client ({e})"))
        })?;
    let token = lucidos_local_token::read();
    let schemes = crate::net_config::gateway_scheme_order();
    post_with(&client, token.as_deref(), schemes, gateway_port, path, body).await
}

async fn post_with(
    client: &reqwest::Client,
    token: Option<&str>,
    schemes: [&str; 2],
    gateway_port: &str,
    path: &str,
    body: &serde_json::Value,
) -> Result<String, ControlRefusal> {
    let unreachable = |why: String| ControlRefusal::Unreachable(redact(&why, token));
    for scheme in schemes {
        let url = format!("{scheme}://127.0.0.1:{gateway_port}/~/api/v1/control{path}");
        let mut response = match client.post(&url).json(body).send().await {
            Ok(response) => response,
            Err(e) if e.is_connect() => continue,
            Err(e) => return Err(unreachable(format!("the gateway call failed ({e})"))),
        };
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
            return Ok(String::from_utf8_lossy(&bytes).into_owned());
        }
        return Err(ControlRefusal::Refused {
            status: status.as_u16(),
            message: redact(&refusal_message(&bytes, status), token),
        });
    }
    Err(unreachable(format!(
        "gateway on :{gateway_port} unreachable"
    )))
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
#[path = "gateway_control_tests.rs"]
mod tests;

#[cfg(test)]
pub(super) mod test_support {
    use axum::{http::StatusCode, routing::get, Router};

    /// Serve `body` at the control-listing path on a loopback port, over plain
    /// HTTP, and hand back the port. Stands in for the gateway.
    ///
    /// Plain HTTP on purpose. With TLS configured in the env,
    /// `gateway_scheme_order()` tries `https` first. The stub then answers only on the fallback, which
    /// makes the fallback loop load-bearing for the test.
    pub(in crate::api) async fn stand_in_gateway(body: &'static str, status: StatusCode) -> u16 {
        let app = Router::<()>::new().route(
            "/~/api/v1/control/workspaces",
            get(move || async move { (status, body) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        port
    }
}
