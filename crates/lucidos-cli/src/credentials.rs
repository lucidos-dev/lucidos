//! `lucidos credentials`, the read and write side of a *credential scope*.
//!
//! A credential is presented only to a base URL it declares, and a provider
//! often needs several: one Binance key pair signs both `api.binance.com` and
//! `fapi.binance.com`.
//!
//! Deliberately CLI-only. Nothing here is an LLM tool or an SDK namespace: the
//! chat agent asks for a credential through `request_credential`, and Settings
//! is where a person edits one.

use crate::http::{client, send_expect_json};
use crate::workspace::{BoxError, Workspace};

/// Every credential the engine reports, secrets excluded, as it wrote them.
///
/// Values rather than a parsed struct. `--json` then prints the engine's own
/// shape, and the human view reads fields off the same fetch.
fn fetch_credentials(
    ws: &Workspace,
    name: Option<&str>,
) -> Result<Vec<serde_json::Value>, BoxError> {
    let url = format!("{}/api/v1/credentials", ws.base_url());
    let parsed = send_expect_json("GET", &url, client()?.get(&url))?;
    Ok(parsed["credentials"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|c| name.is_none_or(|n| c["service_name"] == n))
        .collect())
}

fn field<'a>(row: &'a serde_json::Value, key: &str) -> &'a str {
    row[key].as_str().unwrap_or("?")
}

fn base_urls_of(row: &serde_json::Value) -> Vec<&str> {
    row["base_urls"]
        .as_array()
        .map(|urls| urls.iter().filter_map(|u| u.as_str()).collect())
        .unwrap_or_default()
}

/// `lucidos credentials list`, one line per credential with the hosts it covers.
///
/// `--json` prints the engine's own array instead, for a script that wants to
/// read the set rather than a person.
pub(crate) fn cmd_list(ws: &Workspace, name: Option<&str>, json: bool) -> Result<(), BoxError> {
    let rows = fetch_credentials(ws, name)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        match name {
            Some(n) => println!("No credential is named {}.", n),
            None => println!("No credentials are stored."),
        }
        return Ok(());
    }
    for row in &rows {
        // An empty set is a real state, and it means the credential is refused
        // everywhere. Saying so beats printing a blank column.
        let urls = base_urls_of(row);
        let scope = match urls.is_empty() {
            true => "(no base URL, so it is sent nowhere)".to_string(),
            false => urls.join("  "),
        };
        println!(
            "{:<24} {:<16} {}",
            field(row, "service_name"),
            field(row, "auth_type"),
            scope
        );
    }
    Ok(())
}

/// `lucidos credentials set-base-urls`, replacing the whole scope.
///
/// Replace rather than append, so the command states the resulting set and a
/// reader of the shell history sees exactly what the credential now covers.
/// Widening one is therefore `list` and then `set-base-urls` with every host.
pub(crate) fn cmd_set_base_urls(
    ws: &Workspace,
    name: &str,
    auth_type: Option<&str>,
    urls: &[String],
) -> Result<(), BoxError> {
    let id = resolve_id(ws, name, auth_type)?;
    let url = format!("{}/api/v1/credential-base-urls", ws.base_url());
    let req = client()?
        .put(&url)
        .query(&[("id", &id)])
        .json(&serde_json::json!({ "base_urls": urls }));
    let parsed = send_expect_json("PUT", &url, req)?;
    // The route answers 200 with `{success:false, error}` for a refusal, so a
    // transport-level success is not yet an outcome.
    if parsed["success"] != serde_json::Value::Bool(true) {
        let reason = parsed["error"].as_str().unwrap_or("the engine refused it");
        return Err(format!("Could not set the base URLs for {}: {}", name, reason).into());
    }
    match urls.is_empty() {
        true => println!("{} now has no base URL, so it is sent nowhere.", name),
        false => println!("{} now covers {}", name, urls.join(", ")),
    }
    Ok(())
}

/// The row `name` addresses, or an error naming the ambiguity.
///
/// A name is not a unique handle: an `oauth_client` registration may share one
/// with an API key. So two matches ask for `--auth-type` rather than picking.
fn resolve_id(ws: &Workspace, name: &str, auth_type: Option<&str>) -> Result<String, BoxError> {
    let matches: Vec<serde_json::Value> = fetch_credentials(ws, Some(name))?
        .into_iter()
        .filter(|row| auth_type.is_none_or(|t| field(row, "auth_type") == t))
        .collect();
    match matches.as_slice() {
        [] => Err(match auth_type {
            Some(t) => format!("No credential named {} has auth type {}.", name, t).into(),
            None => format!("No credential is named {}.", name).into(),
        }),
        [only] => Ok(field(only, "id").to_string()),
        rows => {
            let types = rows
                .iter()
                .map(|row| field(row, "auth_type"))
                .collect::<Vec<_>>()
                .join(", ");
            Err(format!(
                "Two credentials are named {}, of types {}. \
                 Add --auth-type to say which one.",
                name, types
            )
            .into())
        }
    }
}
