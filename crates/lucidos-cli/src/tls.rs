//! `lucidos tls status`: what the gateway serves over HTTPS, and why not when
//! it does not.
//!
//! The gateway settles its certificate itself: an operator's
//! `LUCIDOS_TLS_CERT`/`KEY`, else one tailscaled issues for this machine's
//! tailnet name. This command only reads the answer.

use serde_json::Value;

use crate::gateway;
use crate::workspace::BoxError;

/// The gateway control route for the TLS state.
const TLS_PATH: &str = "/~/api/v1/control/tls";

pub fn cmd_tls_status(port: Option<u16>) -> Result<(), BoxError> {
    let client = gateway::client()?;
    let base = gateway::the_gateway(
        &client,
        port,
        "Each gateway has its own certificate, so pass --port to name one",
    )?;
    let token = lucidos_local_token::read().ok_or_else(|| -> BoxError {
        "no local token found. Only a process on the gateway's own machine can read \
         its TLS state"
            .into()
    })?;
    let response = client
        .get(format!("{base}{TLS_PATH}"))
        .header(lucidos_local_token::HEADER_LOCAL_TOKEN, &token)
        .send()?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err("this gateway does not report its TLS state. Update Lucidos first".into());
    }
    if !status.is_success() {
        return Err(format!("the gateway refused the TLS read: {status}").into());
    }
    println!("{}", describe(&response.json()?));
    Ok(())
}

/// What a person reads, from the gateway's answer.
fn describe(body: &Value) -> String {
    let text = |key: &str| body.get(key).and_then(Value::as_str);
    let mut lines = vec![match (text("source"), text("https_origin")) {
        (Some("tailnet"), Some(origin)) => {
            format!("HTTPS: automatic, with a certificate from your tailnet. Open {origin}")
        }
        (Some("operator"), _) => {
            "HTTPS: on every address, with the certificate LUCIDOS_TLS_CERT names.".to_string()
        }
        _ => "HTTPS: not available from the gateway itself.".to_string(),
    }];
    if let Some(expires) = text("expires_at") {
        lines.push(format!("Certificate valid until {expires}."));
    }
    if let Some(error) = text("error") {
        lines.push(match text("source") {
            Some("none") => format!("Why: {error}"),
            _ => format!(
                "The last certificate update hit a problem, and the current certificate still serves: {error}"
            ),
        });
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_tailnet_certificate_names_its_origin_and_expiry() {
        let text = describe(&json!({
            "source": "tailnet",
            "name": "mac.tailnet-name.ts.net",
            "expires_at": "2027-01-27T00:00:00Z",
            "error": null,
            "https_origin": "https://mac.tailnet-name.ts.net:5252",
        }));
        assert!(text.contains("automatic"));
        assert!(text.contains("https://mac.tailnet-name.ts.net:5252"));
        assert!(text.contains("2027-01-27"));
    }

    #[test]
    fn no_https_always_says_why() {
        let text = describe(&json!({
            "source": "none",
            "error": "MagicDNS is off in this tailnet.",
            "https_origin": null,
        }));
        assert!(text.contains("not available"));
        assert!(text.contains("Why: MagicDNS is off"));
    }

    #[test]
    fn a_failed_update_says_the_current_certificate_still_serves() {
        let text = describe(&json!({
            "source": "tailnet",
            "error": "tailscaled did not answer",
            "https_origin": "https://mac.tailnet-name.ts.net:5252",
        }));
        assert!(text.contains("still serves: tailscaled did not answer"));
    }
}
