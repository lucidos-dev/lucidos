use crate::http::{client, send_expect_json};
use crate::workspace::{BoxError, Workspace};

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
        let names = env_var_names_of(row);
        if !names.is_empty() {
            println!("    names: {}", names.join(" "));
        }
    }
    Ok(())
}

fn env_var_names_of(row: &serde_json::Value) -> Vec<&str> {
    row["env_var_names"]
        .as_array()
        .map(|names| names.iter().filter_map(|n| n.as_str()).collect())
        .unwrap_or_default()
}

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
