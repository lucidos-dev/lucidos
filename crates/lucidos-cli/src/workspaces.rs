//! Read-only gateway cleanup listing for agents and terminal users.

use crate::{gateway, workspace::BoxError};

fn pending_message(result: &serde_json::Value) -> Option<&'static str> {
    if result
        .get("scanned_at")
        .is_none_or(serde_json::Value::is_null)
    {
        Some("Workspace review is pending. Run this command again after the gateway finishes scanning; empty rows do not yet mean there are no workspaces.")
    } else {
        None
    }
}

pub fn review(port: Option<u16>) -> Result<(), BoxError> {
    let client = gateway::client()?;
    let base = gateway::the_gateway(
        &client,
        port,
        "Pass --port to select the gateway whose workspaces you want to review",
    )?;
    let token = lucidos_local_token::read().ok_or("no local token found")?;
    let result: serde_json::Value = client
        .get(format!("{base}/~/api/v1/control/cleanup"))
        .header(lucidos_local_token::HEADER_LOCAL_TOKEN, token)
        .send()?
        .error_for_status()?
        .json()?;
    if let Some(message) = pending_message(&result) {
        eprintln!("{message}");
    }
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_empty_scan_is_pending_but_completed_empty_scan_is_not() {
        assert!(pending_message(
            &serde_json::json!({"scanned_at": null, "refreshing": true, "rows": []})
        )
        .is_some());
        assert!(pending_message(&serde_json::json!({"scanned_at": "2026-01-01T00:00:00Z", "refreshing": false, "rows": []})).is_none());
    }
}
