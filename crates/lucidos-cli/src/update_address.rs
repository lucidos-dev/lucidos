//! `lucidos update-address`: read or change the *update address*, the
//! `latest.json` URL update checks read instead of the published release.
//!
//! The setting is the gateway's (`~/.lucidos/updates.toml`), so this talks to
//! the gateway. The file is machine-global and every gateway re-reads it, so
//! any running gateway will do. Plan:
//! `docs/plans/2026-10-21-update-address-setting.md`.

use lucidos_update_address::{UpdateAddress, GATEWAY_STATUS_PATH, RELEASE_CHECK_PATH};
use serde_json::{json, Value};

use crate::gateway;
use crate::workspace::BoxError;

/// What the command was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Show,
    Set(String),
    Reset,
}

pub fn cmd_update_address(action: &Action, port: Option<u16>) -> Result<(), BoxError> {
    // Checked before any gateway is probed, so a typo fails fast and offline.
    let body = request_body(action)?;
    let client = gateway::client()?;
    let base = resolve_base(&client, port)?;
    let token = lucidos_local_token::read().ok_or_else(|| -> BoxError {
        "no local token found. Only a process on the gateway's own machine can \
         change the update address"
            .into()
    })?;

    let release_check = match body {
        None => {
            let status: Value = client
                .get(format!("{base}{GATEWAY_STATUS_PATH}"))
                .header(lucidos_local_token::HEADER_LOCAL_TOKEN, &token)
                .send()?
                .error_for_status()?
                .json()?;
            status.get("release_check").cloned().unwrap_or(Value::Null)
        }
        Some(body) => {
            let response = client
                .put(format!("{base}{RELEASE_CHECK_PATH}"))
                .header(lucidos_local_token::HEADER_LOCAL_TOKEN, &token)
                .json(&body)
                .send()?;
            if !response.status().is_success() {
                let status = response.status();
                let reason = response
                    .json::<Value>()
                    .ok()
                    .and_then(|b| b.get("error").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or_else(|| status.to_string());
                return Err(format!("the gateway refused the update address: {reason}").into());
            }
            response.json()?
        }
    };
    println!("{}", describe(&release_check)?);
    Ok(())
}

/// The PUT body for a change, or `None` for `show`. A `set` value is checked
/// against the shared rule here, so a refused one never reaches the gateway.
fn request_body(action: &Action) -> Result<Option<Value>, BoxError> {
    match action {
        Action::Show => Ok(None),
        Action::Reset => Ok(Some(json!({ "update_address": "" }))),
        Action::Set(raw) => match UpdateAddress::parse(raw)? {
            Some(address) => Ok(Some(json!({ "update_address": address.as_str() }))),
            None => Err("an empty update address means the default. To go back to \
                 the published release, run `lucidos update-address reset`"
                .into()),
        },
    }
}

/// Any answering gateway: the one the caller named, else the first found.
fn resolve_base(client: &reqwest::blocking::Client, port: Option<u16>) -> Result<String, BoxError> {
    if let Some(p) = port {
        return gateway::find_gateways(client, &[p])
            .into_iter()
            .next()
            .map(|g| g.base)
            .ok_or_else(|| gateway::no_gateway_on(p));
    }
    let found = gateway::find_gateways(client, &gateway::candidate_ports());
    gateway::preferred(&found, gateway::dev_port_override())
        .or(found.first())
        .map(|g| g.base.clone())
        .ok_or_else(|| gateway::NO_GATEWAY.into())
}

/// What a person reads, from the gateway's `release_check` object.
///
/// An older gateway sends no `update_address` key at all, and that is an
/// error: it would ignore a write too.
fn describe(release_check: &Value) -> Result<String, BoxError> {
    let Some(address) = release_check.get("update_address") else {
        return Err("this gateway does not support an update address. Update Lucidos first".into());
    };
    let error = release_check
        .get("update_address_error")
        .and_then(Value::as_str);
    Ok(match (address.as_str(), error) {
        (_, Some(error)) => format!(
            "The stored update address is refused, so update checks fail until you fix it.\n\
             {error}\n\
             Run `lucidos update-address set <url>` or `lucidos update-address reset`."
        ),
        (Some(url), None) => format!(
            "Update address: {url}\n\
             Updates come from this address, not the published release.\n\
             Run `lucidos update-address reset` to go back."
        ),
        (None, None) => {
            "No update address is set. Updates come from the published release.".to_string()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn show_sends_no_body() {
        assert_eq!(request_body(&Action::Show).unwrap(), None);
    }

    #[test]
    fn reset_sends_the_empty_address() {
        // `""` is the gateway's reset, the same value Settings' Use default sends.
        assert_eq!(
            request_body(&Action::Reset).unwrap(),
            Some(json!({ "update_address": "" }))
        );
    }

    #[test]
    fn set_sends_the_checked_address() {
        let url = "http://127.0.0.1:5300/api/v1/data/artifacts/releases/rc/latest.json";
        assert_eq!(
            request_body(&Action::Set(format!("  {url} "))).unwrap(),
            Some(json!({ "update_address": url }))
        );
    }

    #[test]
    fn set_refuses_an_address_the_rule_refuses_before_any_request() {
        let err = request_body(&Action::Set("http://example.com/latest.json".into()))
            .unwrap_err()
            .to_string();
        assert!(err.starts_with(lucidos_update_address::RULE), "{err}");
    }

    #[test]
    fn set_refuses_an_empty_address_and_names_reset() {
        let err = request_body(&Action::Set("   ".into()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("update-address reset"), "{err}");
    }

    #[test]
    fn an_unset_address_reads_as_the_published_release() {
        let text = describe(&json!({ "update_address": null, "update_address_error": null }));
        assert!(text.unwrap().contains("published release"));
    }

    #[test]
    fn a_set_address_is_named() {
        let url = "https://example.com/latest.json";
        let text =
            describe(&json!({ "update_address": url, "update_address_error": null })).unwrap();
        assert!(text.contains(url), "{text}");
        assert!(text.contains("reset"), "{text}");
    }

    #[test]
    fn a_refused_address_shows_the_gateway_error() {
        let text = describe(&json!({
            "update_address": "http://example.com/latest.json",
            "update_address_error": "the rule: http://example.com/latest.json",
        }))
        .unwrap();
        assert!(text.contains("refused"), "{text}");
        assert!(text.contains("the rule: http://example.com"), "{text}");
    }

    #[test]
    fn a_gateway_without_the_setting_is_an_error() {
        let err = describe(&json!({ "enabled": true }))
            .unwrap_err()
            .to_string();
        assert!(err.contains("Update Lucidos"), "{err}");
    }
}
