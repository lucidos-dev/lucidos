//! `lucidos external-address`: read or change the gateway's *external
//! address*, the origin other devices reach it at through a route the user runs
//! (ADR 0550).
//!
//! Each gateway has its own, so this acts on one gateway, chosen like
//! `lucidos pair` chooses. A set goes through the gateway, which probes the
//! address and refuses it only when another gateway answers there.

use serde_json::{json, Value};

use crate::gateway;
use crate::workspace::BoxError;

/// The gateway control route for the setting.
pub(crate) const EXTERNAL_ADDRESS_PATH: &str = "/~/api/v1/control/external-address";

/// What the command was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Show,
    Set(String),
    Clear,
}

pub fn cmd_external_address(action: &Action, port: Option<u16>) -> Result<(), BoxError> {
    let body = request_body(action)?;
    let client = gateway::client()?;
    let base = gateway::the_gateway(
        &client,
        port,
        "Each gateway has its own external address, so pass --port to name one",
    )?;
    let token = lucidos_local_token::read().ok_or_else(|| -> BoxError {
        "no local token found. Only a process on the gateway's own machine can \
         change the external address"
            .into()
    })?;
    let url = format!("{base}{EXTERNAL_ADDRESS_PATH}");
    let response = match &body {
        None => client.get(&url),
        Some(body) => client.put(&url).json(body),
    }
    .header(lucidos_local_token::HEADER_LOCAL_TOKEN, &token)
    .send()?;
    let status = response.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err("this gateway has no external address setting. Update Lucidos first".into());
    }
    if !status.is_success() {
        let reason = response
            .json::<Value>()
            .ok()
            .and_then(|b| b.get("error").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| status.to_string());
        return Err(format!("the gateway refused the external address: {reason}").into());
    }
    println!("{}", describe(&response.json()?));
    Ok(())
}

/// The external address stored for the gateway at `base`, for `lucidos pair`.
/// `None` when unset, or when the gateway is too old to have one.
pub(crate) fn stored(
    client: &reqwest::blocking::Client,
    base: &str,
    token: &str,
) -> Option<String> {
    client
        .get(format!("{base}{EXTERNAL_ADDRESS_PATH}"))
        .header(lucidos_local_token::HEADER_LOCAL_TOKEN, token)
        .send()
        .ok()
        .filter(|r| r.status().is_success())?
        .json::<Value>()
        .ok()?
        .get("external_address")?
        .as_str()
        .map(str::to_string)
}

/// The PUT body for a change, or `None` for `show`. The gateway validates the
/// value, so its own sentence explains a refusal.
fn request_body(action: &Action) -> Result<Option<Value>, BoxError> {
    match action {
        Action::Show => Ok(None),
        Action::Clear => Ok(Some(json!({ "external_address": "" }))),
        Action::Set(raw) if raw.trim().is_empty() => {
            Err("an empty external address means none. Run `lucidos external-address clear`".into())
        }
        Action::Set(raw) => Ok(Some(json!({ "external_address": raw.trim() }))),
    }
}

/// What a person reads, from the gateway's answer.
fn describe(body: &Value) -> String {
    let address = body.get("external_address").and_then(Value::as_str);
    let env_var = body
        .get("env_var")
        .and_then(Value::as_str)
        .unwrap_or("LUCIDOS_EXTERNAL_ADDRESS");
    let mut lines = vec![match address {
        Some(origin) => format!("External address: {origin}"),
        None => "No external address is set. Pairing QRs use the tailnet or the LAN.".to_string(),
    }];
    if body.get("locked").and_then(Value::as_bool) == Some(true) {
        lines.push(format!(
            "Set by {env_var} in the gateway's environment, so it cannot be changed here."
        ));
    }
    if let Some(verdict) = body.get("probe").and_then(|p| p.get("verdict")) {
        let detail = body["probe"]
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or_default();
        lines.push(match verdict.as_str() {
            Some("verified") => "Checked: it reaches this gateway.".to_string(),
            Some("unreachable") => format!(
                "Saved, but it did not answer from this machine ({detail}). That is \
                 normal behind hairpin NAT or a client-only VPN. Test it from a phone."
            ),
            _ => format!("Saved, but it could not be checked ({detail})."),
        });
    }
    if address.is_none() {
        let examples: Vec<&str> = body
            .get("suggestions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|s| s.get("origin").and_then(Value::as_str))
            .collect();
        if !examples.is_empty() {
            lines.push(format!(
                "This machine's addresses: {}. Run `lucidos external-address set <origin>`.",
                examples.join(", ")
            ));
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_sends_no_body_and_clear_sends_the_empty_address() {
        assert_eq!(request_body(&Action::Show).unwrap(), None);
        assert_eq!(
            request_body(&Action::Clear).unwrap(),
            Some(json!({ "external_address": "" }))
        );
    }

    #[test]
    fn set_sends_the_trimmed_value_and_refuses_an_empty_one() {
        assert_eq!(
            request_body(&Action::Set(" https://lucidos.example.com ".into())).unwrap(),
            Some(json!({ "external_address": "https://lucidos.example.com" }))
        );
        let err = request_body(&Action::Set("  ".into()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("external-address clear"), "{err}");
    }

    #[test]
    fn an_unset_address_names_this_machines_addresses() {
        let text = describe(&json!({
            "external_address": null,
            "locked": false,
            "suggestions": [{ "interface": "wg0", "origin": "http://10.8.0.2:5252" }],
            "probe": null,
        }));
        assert!(text.contains("No external address"), "{text}");
        assert!(text.contains("http://10.8.0.2:5252"), "{text}");
    }

    #[test]
    fn a_locked_address_names_the_variable() {
        let text = describe(&json!({
            "external_address": "https://lucidos.example.com",
            "locked": true,
            "env_var": "LUCIDOS_EXTERNAL_ADDRESS",
            "probe": null,
        }));
        assert!(text.contains("https://lucidos.example.com"), "{text}");
        assert!(text.contains("LUCIDOS_EXTERNAL_ADDRESS"), "{text}");
    }

    #[test]
    fn each_saved_verdict_reads_differently() {
        let with = |probe: Value| {
            describe(&json!({ "external_address": "https://lucidos.example.com", "probe": probe }))
        };
        assert!(with(json!({ "verdict": "verified" })).contains("reaches this gateway"));
        let unreachable = with(json!({ "verdict": "unreachable", "detail": "timed out" }));
        assert!(unreachable.contains("did not answer") && unreachable.contains("timed out"));
        let unknown = with(json!({ "verdict": "unknown", "detail": "HTTP 401" }));
        assert!(unknown.contains("could not be checked") && unknown.contains("HTTP 401"));
    }
}
