use serde_json::{Map, Value};

use crate::http::{client as http_client, send_and_print};
use crate::workspace::{BoxError, Workspace};

pub(crate) fn cmd_emit(
    ws: &Workspace,
    event_type: &str,
    payload_raw: &str,
    summary_override: Option<&str>,
) -> Result<(), BoxError> {
    let mut payload: Value =
        serde_json::from_str(payload_raw).map_err(|e| format!("Invalid --payload JSON: {}", e))?;

    let obj = payload
        .as_object_mut()
        .ok_or("payload must be a JSON object")?;

    enforce_summary(obj, summary_override)?;

    let url = format!("{}/api/v1/events/emit", ws.base_url());
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
        Some(Value::String(_)) => {
            Err("payload.summary is empty; pass --summary or set it in --payload".into())
        }
        Some(_) => Err("payload.summary must be a string".into()),
        None => Err(
            "payload missing required `summary` field; pass --summary or include it in --payload"
                .into(),
        ),
    }
}

pub(crate) struct QueryFilters<'a> {
    pub event_type: Option<&'a str>,
    pub since: Option<&'a str>,
    pub until: Option<&'a str>,
    pub before_event_id: Option<&'a str>,
    pub after_event_id: Option<&'a str>,
    pub limit: Option<u32>,
}

pub(crate) fn cmd_query(ws: &Workspace, filters: QueryFilters<'_>) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/events/query", ws.base_url());
    let params = build_query_params(&filters);
    let mut req = http_client()?.get(&url);
    if !params.is_empty() {
        req = req.query(&params);
    }
    send_and_print("GET", &url, req)
}

pub(crate) struct CountFilters<'a> {
    pub event_type: Option<&'a str>,
    pub since: Option<&'a str>,
    pub until: Option<&'a str>,
}

pub(crate) fn cmd_count(ws: &Workspace, filters: CountFilters<'_>) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/events/count", ws.base_url());
    let params = build_count_params(&filters);
    let mut req = http_client()?.get(&url);
    if !params.is_empty() {
        req = req.query(&params);
    }
    send_and_print("GET", &url, req)
}

fn build_count_params(filters: &CountFilters<'_>) -> Vec<(&'static str, String)> {
    let mut params: Vec<(&'static str, String)> = Vec::new();
    if let Some(t) = filters.event_type {
        params.push(("type", t.to_string()));
    }
    if let Some(s) = filters.since {
        params.push(("since", s.to_string()));
    }
    if let Some(u) = filters.until {
        params.push(("until", u.to_string()));
    }
    params
}

/// Build the query-string params for `cmd_query`. Extracted so we can unit
/// test that `--before-event-id` / `--after-event-id` reach the server as
/// `before_event_id` / `after_event_id` (snake_case — the engine doesn't
/// accept the kebab form).
fn build_query_params(filters: &QueryFilters<'_>) -> Vec<(&'static str, String)> {
    let mut params: Vec<(&'static str, String)> = Vec::new();
    if let Some(t) = filters.event_type {
        params.push(("type", t.to_string()));
    }
    if let Some(s) = filters.since {
        params.push(("since", s.to_string()));
    }
    if let Some(u) = filters.until {
        params.push(("until", u.to_string()));
    }
    if let Some(id) = filters.before_event_id {
        params.push(("before_event_id", id.to_string()));
    }
    if let Some(id) = filters.after_event_id {
        params.push(("after_event_id", id.to_string()));
    }
    if let Some(l) = filters.limit {
        params.push(("limit", l.to_string()));
    }
    params
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

    /// `--before-event-id` and `--after-event-id` must reach the engine as
    /// `before_event_id` / `after_event_id` (snake_case). Clap kebab-cases
    /// the flags, so we have to translate ourselves.
    #[test]
    fn build_query_params_serializes_cursor_flags_as_snake_case() {
        let params = build_query_params(&QueryFilters {
            event_type: Some("BrowserLearningObserved"),
            since: None,
            until: None,
            before_event_id: Some("11111111-1111-1111-1111-111111111111"),
            after_event_id: None,
            limit: Some(50),
        });
        assert!(
            params.iter().any(
                |(k, v)| *k == "before_event_id" && v == "11111111-1111-1111-1111-111111111111"
            ),
            "before_event_id missing or wrong: {:?}",
            params
        );
        assert!(
            !params.iter().any(|(k, _)| *k == "after_event_id"),
            "after_event_id should not be present when None"
        );
    }

    #[test]
    fn build_query_params_omits_unset_filters() {
        let params = build_query_params(&QueryFilters {
            event_type: None,
            since: None,
            until: None,
            before_event_id: None,
            after_event_id: None,
            limit: None,
        });
        assert!(params.is_empty(), "expected no params, got {:?}", params);
    }

    #[test]
    fn build_query_params_includes_after_event_id_when_set() {
        let params = build_query_params(&QueryFilters {
            event_type: None,
            since: None,
            until: None,
            before_event_id: None,
            after_event_id: Some("22222222-2222-2222-2222-222222222222"),
            limit: None,
        });
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].0, "after_event_id");
        assert_eq!(params[0].1, "22222222-2222-2222-2222-222222222222");
    }
}
