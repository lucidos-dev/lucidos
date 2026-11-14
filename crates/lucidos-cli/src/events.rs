use serde_json::{Map, Value};

use crate::http::{
    client as http_client, client_with_timeout as http_client_with_timeout, send_and_print,
};
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
    /// One thread only. Absent is every thread, which is what this command
    /// always sent before the filter existed.
    pub thread_id: Option<&'a str>,
    pub limit: Option<u32>,
}

pub(crate) fn cmd_query(ws: &Workspace, filters: QueryFilters<'_>) -> Result<(), BoxError> {
    print_liveness_header(ws, filters.thread_id);
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
    /// One thread only. Absent is every thread.
    pub thread_id: Option<&'a str>,
}

pub(crate) fn cmd_count(ws: &Workspace, filters: CountFilters<'_>) -> Result<(), BoxError> {
    print_liveness_header(ws, filters.thread_id);
    let url = format!("{}/api/v1/events/count", ws.base_url());
    let params = build_count_params(&filters);
    let mut req = http_client()?.get(&url);
    if !params.is_empty() {
        req = req.query(&params);
    }
    send_and_print("GET", &url, req)
}

/// Print one thread's liveness on stderr, so stdout stays parseable JSON. A
/// failed read, or an engine without the route, prints nothing: an absent
/// header claims nothing.
fn print_liveness_header(ws: &Workspace, thread_id: Option<&str>) {
    let Some(thread_id) = thread_id else {
        return;
    };
    let url = format!("{}/api/v1/threads/{}/liveness", ws.base_url(), thread_id);
    let liveness = http_client_with_timeout(std::time::Duration::from_secs(5))
        .ok()
        .and_then(|client| client.get(&url).send().ok())
        .filter(|response| response.status().is_success())
        .and_then(|response| response.json::<Value>().ok());
    if let Some(line) = liveness.as_ref().and_then(liveness_header_line) {
        // User-facing CLI output on stderr, beside the JSON on stdout.
        eprintln!("{line}");
    }
}

/// `# thread: working · running · session 2 (handoff, 9 min) · active 4 s ago`.
fn liveness_header_line(liveness: &Value) -> Option<String> {
    let mut parts = vec![
        liveness.get("state")?.as_str()?.to_string(),
        liveness.get("status")?.as_str()?.to_string(),
    ];
    if let Some(session) = liveness.get("session") {
        parts.push(format!(
            "session {} ({}, {})",
            session.get("ordinal")?.as_u64()?,
            session.get("continuity")?.as_str()?,
            age(session.get("age_secs")?.as_i64()?)
        ));
    }
    if liveness.get("handoff").and_then(Value::as_str) == Some("pending") {
        parts.push("handoff pending".to_string());
    }
    if let Some(secs) = liveness
        .get("last_activity_age_secs")
        .and_then(Value::as_i64)
    {
        parts.push(format!("active {} ago", age(secs)));
    }
    Some(format!("# thread: {}", parts.join(" · ")))
}

fn age(secs: i64) -> String {
    match secs {
        ..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min", secs / 60),
        _ => format!("{} h", secs / 3600),
    }
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
    if let Some(id) = filters.thread_id {
        params.push(("thread_id", id.to_string()));
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
    if let Some(id) = filters.thread_id {
        params.push(("thread_id", id.to_string()));
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
    fn the_header_line_reads_like_the_wire_object() {
        let liveness = serde_json::json!({
            "state": "working",
            "status": "running",
            "last_activity_age_secs": 4,
            "session": {"ordinal": 2, "continuity": "handoff", "age_secs": 553},
            "as_of": "2026-11-13T16:43:15Z",
        });
        assert_eq!(
            liveness_header_line(&liveness).as_deref(),
            Some("# thread: working · running · session 2 (handoff, 9 min) · active 4 s ago")
        );
        let stalled = serde_json::json!({
            "state": "stalled", "status": "idle", "handoff": "pending",
            "last_activity_age_secs": 7200, "as_of": "2026-11-13T16:43:15Z",
        });
        assert_eq!(
            liveness_header_line(&stalled).as_deref(),
            Some("# thread: stalled · idle · handoff pending · active 2 h ago")
        );
        assert_eq!(
            liveness_header_line(&serde_json::json!({"error": "x"})),
            None
        );
    }

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
            thread_id: None,
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
            thread_id: None,
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
            thread_id: None,
            limit: None,
        });
        assert_eq!(params.len(), 1);
        assert_eq!(params[0].0, "after_event_id");
        assert_eq!(params[0].1, "22222222-2222-2222-2222-222222222222");
    }

    #[test]
    fn build_count_params_sends_the_thread_filter_when_set() {
        let params = build_count_params(&CountFilters {
            event_type: None,
            since: None,
            until: None,
            thread_id: Some("33333333-3333-3333-3333-333333333333"),
        });
        assert_eq!(
            params,
            vec![(
                "thread_id",
                "33333333-3333-3333-3333-333333333333".to_string()
            )]
        );
    }
}
