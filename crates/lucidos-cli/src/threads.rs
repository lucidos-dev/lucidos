use crate::http::{client as http_client, send_and_print};
use crate::workspace::{BoxError, Workspace};

pub(crate) struct ListFilters<'a> {
    /// `Some(true)` → only `running` / `waiting_for_user_answer` (agentic loop
    /// mid-flow). `Some(false)` → invert. `None` → no filter.
    pub active: Option<bool>,
    /// Comma-separated source list (`chat`, `trigger`, `coding-agent`).
    /// Legacy `claude_code` is also accepted by the engine.
    pub source: Option<&'a str>,
    /// Server clamps to 1..=1000 (default 100).
    pub limit: Option<u32>,
}

pub(crate) fn cmd_list(ws: &Workspace, filters: ListFilters<'_>) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/threads/list", ws.base_url());
    let params = build_query_params(filters.active, filters.source, filters.limit);
    let mut req = http_client()?.get(&url);
    if !params.is_empty() {
        req = req.query(&params);
    }
    send_and_print("GET", &url, req)
}

pub(crate) fn cmd_count(ws: &Workspace, active: Option<bool>, source: Option<&str>) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/threads/count", ws.base_url());
    let params = build_query_params(active, source, None);
    let mut req = http_client()?.get(&url);
    if !params.is_empty() {
        req = req.query(&params);
    }
    send_and_print("GET", &url, req)
}

fn build_query_params(
    active: Option<bool>,
    source: Option<&str>,
    limit: Option<u32>,
) -> Vec<(&'static str, String)> {
    let mut params: Vec<(&'static str, String)> = Vec::new();
    if let Some(a) = active {
        params.push(("active", a.to_string()));
    }
    if let Some(s) = source {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            params.push(("source", trimmed.to_string()));
        }
    }
    if let Some(l) = limit {
        params.push(("limit", l.to_string()));
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_includes_active_flag_when_set() {
        let params = build_query_params(Some(true), None, None);
        assert_eq!(params, vec![("active", "true".to_string())]);
    }

    #[test]
    fn list_includes_active_false_when_explicitly_inverted() {
        let params = build_query_params(Some(false), None, None);
        assert_eq!(params, vec![("active", "false".to_string())]);
    }

    #[test]
    fn list_includes_source_when_non_empty() {
        let params = build_query_params(None, Some("chat,trigger"), None);
        assert_eq!(params, vec![("source", "chat,trigger".to_string())]);
    }

    #[test]
    fn list_omits_blank_source() {
        let params = build_query_params(None, Some("   "), None);
        assert!(params.is_empty());
    }

    #[test]
    fn list_includes_limit_when_set() {
        let params = build_query_params(None, None, Some(50));
        assert_eq!(params, vec![("limit", "50".to_string())]);
    }

    #[test]
    fn list_returns_no_params_when_all_unset() {
        let params = build_query_params(None, None, None);
        assert!(params.is_empty());
    }

    #[test]
    fn list_composes_all_three() {
        let params = build_query_params(Some(true), Some("trigger"), Some(10));
        assert_eq!(
            params,
            vec![
                ("active", "true".to_string()),
                ("source", "trigger".to_string()),
                ("limit", "10".to_string()),
            ]
        );
    }
}
