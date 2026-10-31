//! `lucidos tls status`: what the gateway serves over HTTPS, and why not when
//! it does not.
//!
//! The gateway settles its certificates itself. An operator's
//! `LUCIDOS_TLS_CERT`/`KEY` serves every name it covers. The tailnet name gets
//! it only when a phone would accept it there, and otherwise a certificate
//! tailscaled issues. This command only reads the answer.

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
    let operator_beside = body
        .get("operator_serves_other_names")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut lines = vec![match (text("source"), text("https_origin")) {
        (Some("tailnet"), Some(origin)) => {
            format!("HTTPS: automatic, with a certificate from your tailnet. Open {origin}")
        }
        (Some("operator"), Some(origin)) => format!(
            "HTTPS: on every address, with the certificate LUCIDOS_TLS_CERT names. It covers \
             your tailnet name too. Open {origin}"
        ),
        (Some("operator"), None) => {
            "HTTPS: on every address, with the certificate LUCIDOS_TLS_CERT names. Your tailnet \
             name gets no HTTPS."
                .to_string()
        }
        _ => "HTTPS: not available from the gateway itself.".to_string(),
    }];
    if operator_beside {
        lines.push(
            "Every other name gets the certificate LUCIDOS_TLS_CERT names, which a phone would \
             not accept for the tailnet name."
                .to_string(),
        );
    }
    if let Some(expires) = text("expires_at") {
        lines.push(format!("Certificate valid until {expires}."));
    }
    if let Some(error) = text("error") {
        let named = text("name").is_some();
        lines.push(match text("source") {
            Some("none") => format!("Why: {error}"),
            Some("operator") if !named => format!("Why the tailnet name has none: {error}"),
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
    fn an_operator_certificate_for_the_tailnet_name_names_its_origin() {
        let text = describe(&json!({
            "source": "operator",
            "name": "mymac.tailnet-name.ts.net",
            "https_origin": "https://mymac.tailnet-name.ts.net:5252",
        }));
        assert!(text.contains("covers your tailnet name"));
        assert!(text.contains("https://mymac.tailnet-name.ts.net:5252"));
    }

    #[test]
    fn a_tailnet_certificate_beside_an_operator_one_says_which_serves_what() {
        let text = describe(&json!({
            "source": "tailnet",
            "name": "mymac.tailnet-name.ts.net",
            "operator_serves_other_names": true,
            "https_origin": "https://mymac.tailnet-name.ts.net:5252",
        }));
        assert!(text.contains("certificate from your tailnet"));
        assert!(text.contains("Every other name gets the certificate LUCIDOS_TLS_CERT names"));
    }

    #[test]
    fn an_operator_certificate_alone_says_why_the_tailnet_name_has_none() {
        let text = describe(&json!({
            "source": "operator",
            "name": null,
            "error": "MagicDNS is off in this tailnet.",
            "https_origin": null,
        }));
        assert!(text.contains("tailnet name gets no HTTPS"));
        assert!(text.contains("Why the tailnet name has none: MagicDNS is off"));
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
