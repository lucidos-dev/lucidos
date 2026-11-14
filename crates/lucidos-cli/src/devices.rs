//! `lucidos devices`: list or revoke the devices paired with this machine's
//! gateway.
//!
//! `revoke --all` is the way back in for a user whose only paired device lost
//! its credential, such as a desktop window whose cookie was cleared. While a
//! device is paired, a code from `lucidos pair` waits for that device's Allow.
//! A stale row nobody can answer from would block it forever, so revoking it
//! lets the next pairing redeem at once.

use serde_json::Value;

use crate::gateway;
use crate::workspace::BoxError;

/// The gateway's own device list, behind the reserved sigil namespace.
const DEVICES_PATH: &str = "/~/api/v1/auth/devices";

/// What the command was asked to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    List,
    /// One device by id, or every device.
    Revoke(Target),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    One(String),
    All,
}

pub fn cmd_devices(action: &Action, port: Option<u16>) -> Result<(), BoxError> {
    let client = gateway::client()?;
    let base = gateway::the_gateway(
        &client,
        port,
        "Each gateway pairs its own devices, so pass --port to name one",
    )?;
    let token = lucidos_local_token::read().ok_or_else(|| -> BoxError {
        "no local token found. Only a process on the gateway's own machine can \
         manage its paired devices"
            .into()
    })?;
    let devices = list(&client, &base, &token)?;
    match action {
        Action::List => println!("{}", describe(&devices)),
        Action::Revoke(target) => {
            for id in targets(&devices, target)? {
                let response = client
                    .delete(format!("{base}{DEVICES_PATH}/{id}"))
                    .header(lucidos_local_token::HEADER_LOCAL_TOKEN, &token)
                    .send()?;
                if !response.status().is_success() {
                    return Err(format!(
                        "the gateway refused to revoke {id}: {}",
                        response.status()
                    )
                    .into());
                }
                println!("Revoked {id}.");
            }
        }
    }
    Ok(())
}

/// The paired devices, as the gateway lists them.
fn list(
    client: &reqwest::blocking::Client,
    base: &str,
    token: &str,
) -> Result<Vec<Value>, BoxError> {
    let response = client
        .get(format!("{base}{DEVICES_PATH}"))
        .header(lucidos_local_token::HEADER_LOCAL_TOKEN, token)
        .send()?;
    if !response.status().is_success() {
        return Err(format!(
            "the gateway refused to list its devices: {}",
            response.status()
        )
        .into());
    }
    match response.json::<Value>()? {
        Value::Array(devices) => Ok(devices),
        _ => Err("the gateway's device list was not a list".into()),
    }
}

/// Is any device paired with the gateway at `base`? `None` when the gateway
/// would not say, so a caller prints nothing rather than guess.
pub(crate) fn any_paired(
    client: &reqwest::blocking::Client,
    base: &str,
    token: &str,
) -> Option<bool> {
    list(client, base, token).ok().map(|d| !d.is_empty())
}

/// Pure: the ids `target` names among `devices`.
fn targets(devices: &[Value], target: &Target) -> Result<Vec<String>, BoxError> {
    let ids: Vec<String> = devices
        .iter()
        .filter_map(|d| d.get("id").and_then(Value::as_str).map(str::to_string))
        .collect();
    match target {
        Target::All => Ok(ids),
        Target::One(id) if ids.contains(id) => Ok(vec![id.clone()]),
        Target::One(id) => Err(format!(
            "no paired device has the id {id}. `lucidos devices list` shows them"
        )
        .into()),
    }
}

/// Pure: one line per device, for a person reading the terminal.
fn describe(devices: &[Value]) -> String {
    if devices.is_empty() {
        return "No device is paired. `lucidos pair` pairs one.".to_string();
    }
    devices
        .iter()
        .map(|d| {
            let field = |name: &str| d.get(name).and_then(Value::as_str).unwrap_or("-");
            format!(
                "{}  {}  (last seen {})",
                field("id"),
                field("label"),
                field("last_seen_at")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn devices() -> Vec<Value> {
        vec![
            json!({"id": "d1", "label": "My MacBook", "last_seen_at": "2026-01-02"}),
            json!({"id": "d2", "label": "My iPhone"}),
        ]
    }

    #[test]
    fn all_names_every_device_and_one_names_only_itself() {
        assert_eq!(targets(&devices(), &Target::All).unwrap(), ["d1", "d2"]);
        assert_eq!(
            targets(&devices(), &Target::One("d2".into())).unwrap(),
            ["d2"]
        );
    }

    #[test]
    fn an_unknown_id_is_refused_before_anything_is_revoked() {
        let refusal = targets(&devices(), &Target::One("d9".into())).unwrap_err();
        assert!(refusal.to_string().contains("d9"), "{refusal}");
    }

    #[test]
    fn the_list_names_each_device_and_says_when_none_is_paired() {
        let listed = describe(&devices());
        assert!(
            listed.contains("d1  My MacBook  (last seen 2026-01-02)"),
            "{listed}"
        );
        assert!(listed.contains("d2  My iPhone  (last seen -)"), "{listed}");
        assert!(describe(&[]).contains("No device is paired"));
    }
}
