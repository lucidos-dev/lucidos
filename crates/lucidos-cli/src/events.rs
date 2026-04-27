use serde_json::{Map, Value};

use crate::http::client as http_client;
use crate::workspace::{BoxError, Workspace};

/// Send `req`, fail on non-2xx, and write the response body to stdout.
fn send_and_print(method: &str, url: &str, req: reqwest::blocking::RequestBuilder) -> Result<(), BoxError> {
    let resp = req
        .send()
        .map_err(|e| format!("{} {} failed: {}", method, url, e))?;
    let status = resp.status();
    let text = resp
        .text()
        .map_err(|e| format!("Failed to read response body: {}", e))?;
    if !status.is_success() {
        return Err(format!("{} {} returned {}: {}", method, url, status, text).into());
    }
    println!("{}", text);
    Ok(())
}

pub(crate) fn cmd_emit(
    ws: &Workspace,
    event_type: &str,
    payload_raw: &str,
    summary_override: Option<&str>,
) -> Result<(), BoxError> {
    let mut payload: Value = serde_json::from_str(payload_raw)
        .map_err(|e| format!("Invalid --payload JSON: {}", e))?;

    let obj = payload
        .as_object_mut()
        .ok_or("payload must be a JSON object")?;

    enforce_summary(obj, summary_override)?;

    let url = format!("{}/api/events/emit", ws.base_url());
    let body = serde_json::json!({
        "event_type": event_type,
        "payload": payload,
    });
    send_and_print("POST", &url, http_client()?.post(&url).json(&body))
}

fn enforce_summary(
    payload: &mut Map<String, Value>,
    summary_override: Option<&str>,
) -> Result<(), BoxError> {
    if let Some(s) = summary_override {
        payload.insert("summary".into(), Value::String(s.to_string()));
        return Ok(());
    }
    match payload.get("summary") {
        Some(Value::String(s)) if !s.is_empty() => Ok(()),
        Some(Value::String(_)) => Err("payload.summary is empty; pass --summary or set it in --payload".into()),
        Some(_) => Err("payload.summary must be a string".into()),
        None => Err("payload missing required `summary` field; pass --summary or include it in --payload".into()),
    }
}

pub(crate) fn cmd_query(
    ws: &Workspace,
    event_type: Option<&str>,
    since: Option<&str>,
    until: Option<&str>,
    limit: Option<u32>,
) -> Result<(), BoxError> {
    let url = format!("{}/api/events/query", ws.base_url());
    let mut params: Vec<(&str, String)> = Vec::new();
    if let Some(t) = event_type {
        params.push(("type", t.to_string()));
    }
    if let Some(s) = since {
        params.push(("since", s.to_string()));
    }
    if let Some(u) = until {
        params.push(("until", u.to_string()));
    }
    if let Some(l) = limit {
        params.push(("limit", l.to_string()));
    }
    let mut req = http_client()?.get(&url);
    if !params.is_empty() {
        req = req.query(&params);
    }
    send_and_print("GET", &url, req)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforce_summary_accepts_existing_string() {
        let mut obj = Map::new();
        obj.insert("summary".into(), Value::String("ok".into()));
        enforce_summary(&mut obj, None).unwrap();
    }

    #[test]
    fn enforce_summary_rejects_missing() {
        let mut obj = Map::new();
        let err = enforce_summary(&mut obj, None).unwrap_err();
        assert!(err.to_string().contains("summary"));
    }

    #[test]
    fn enforce_summary_rejects_empty_string() {
        let mut obj = Map::new();
        obj.insert("summary".into(), Value::String("".into()));
        assert!(enforce_summary(&mut obj, None).is_err());
    }

    #[test]
    fn enforce_summary_rejects_non_string() {
        let mut obj = Map::new();
        obj.insert("summary".into(), Value::Bool(true));
        assert!(enforce_summary(&mut obj, None).is_err());
    }

    #[test]
    fn override_injects_summary_when_missing() {
        let mut obj = Map::new();
        enforce_summary(&mut obj, Some("from-cli")).unwrap();
        assert_eq!(obj["summary"], Value::String("from-cli".into()));
    }

    #[test]
    fn override_replaces_existing_summary() {
        let mut obj = Map::new();
        obj.insert("summary".into(), Value::String("old".into()));
        enforce_summary(&mut obj, Some("new")).unwrap();
        assert_eq!(obj["summary"], Value::String("new".into()));
    }
}
