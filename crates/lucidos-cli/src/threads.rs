use crate::http::{client as http_client, send_and_print, send_or_refusal};
use crate::workspace::{BoxError, Workspace};

pub(crate) const ENV_SOURCE_THREAD_ID: &str = "LUCIDOS_THREAD_ID";

const ENV_EVENT_ID: &str = "LUCIDOS_EVENT_ID";

#[derive(Default)]
pub(crate) struct ListFilters<'a> {
    pub active: Option<bool>,
    pub status: &'a [String],
    pub source: Option<&'a str>,
    pub limit: Option<u32>,
    pub parent: Option<String>,
    pub has_draft: Option<bool>,
    pub change_state: Option<String>,
}

pub(crate) fn resolve_parent_filter(
    parent: Option<String>,
    my_children: bool,
    env_thread_id: Option<String>,
) -> Result<Option<String>, BoxError> {
    match (parent, my_children) {
        (Some(_), true) => Err("Pass either --parent <uuid> or --my-children, not both. \
             --my-children is shorthand for --parent with this thread's own id."
            .into()),
        (Some(p), false) => Ok(Some(p)),
        (None, true) => match env_thread_id.map(|t| t.trim().to_string()) {
            Some(t) if !t.is_empty() => Ok(Some(t)),
            _ => Err(format!(
                "--my-children needs a calling thread, and {ENV_SOURCE_THREAD_ID} is not set. \
                 It resolves to the thread this subprocess was spawned for, so it only works \
                 from inside a Lucidos thread. Pass --parent <uuid> instead."
            )
            .into()),
        },
        (None, false) => Ok(None),
    }
}

pub(crate) fn cmd_list(ws: &Workspace, filters: ListFilters<'_>) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/threads/list", ws.base_url());
    send_filtered(&url, &filters)
}

pub(crate) fn cmd_count(ws: &Workspace, filters: ListFilters<'_>) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/threads/count", ws.base_url());
    send_filtered(&url, &filters)
}

fn send_filtered(url: &str, filters: &ListFilters<'_>) -> Result<(), BoxError> {
    send_get(url, &build_query_params(filters))
}

fn send_get(url: &str, params: &[(&'static str, String)]) -> Result<(), BoxError> {
    let mut req = http_client()?.get(url);
    if !params.is_empty() {
        req = req.query(params);
    }
    send_and_print("GET", url, req)
}

pub(crate) fn cmd_drafts(
    ws: &Workspace,
    limit: Option<u32>,
    thread: Option<&str>,
) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/threads/drafts", ws.base_url());
    send_get(&url, &drafts_query_params(limit, thread))
}

pub(crate) fn cmd_held_messages(ws: &Workspace, limit: Option<u32>) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/threads/held-messages", ws.base_url());
    send_get(&url, &limit_param(limit))
}

pub(crate) fn cmd_search(ws: &Workspace, query: &str, limit: Option<u32>) -> Result<(), BoxError> {
    if query.trim().is_empty() {
        return Err("the search query must be non-empty. Say what was discussed.".into());
    }
    let url = format!("{}/api/v1/threads/search", ws.base_url());
    let mut params = vec![("q", query.trim().to_string())];
    params.extend(limit_param(limit));
    send_get(&url, &params)
}

fn limit_param(limit: Option<u32>) -> Vec<(&'static str, String)> {
    limit
        .map(|l| ("limit", l.to_string()))
        .into_iter()
        .collect()
}

fn drafts_query_params(limit: Option<u32>, thread: Option<&str>) -> Vec<(&'static str, String)> {
    let mut params = limit_param(limit);
    if let Some(t) = thread.map(str::trim).filter(|t| !t.is_empty()) {
        params.push(("thread_id", t.to_string()));
    }
    params
}

pub(crate) fn cmd_follow_up(
    ws: &Workspace,
    child_thread_id: &str,
    message: &str,
    event_id: Option<&str>,
    urgent: bool,
) -> Result<(), BoxError> {
    let child = child_thread_id.trim();
    if uuid::Uuid::parse_str(child).is_err() {
        return Err(format!(
            "--thread takes a child thread's uuid, and '{child}' is not one. A follow-up \
             addresses a child by id, never by title: titles are not unique, and a fuzzy \
             match would silently deliver to the wrong child. \
             Run `lucidos threads list --my-children` to find the id."
        )
        .into());
    }
    if message.trim().is_empty() {
        return Err(
            "--message must be non-empty. It lands in the child's conversation \
             as a message from you."
                .into(),
        );
    }

    let url = format!("{}/api/v1/threads/{child}/follow-up", ws.base_url());
    let mut body = serde_json::json!({ "message": message });
    if let Some(id) = event_id.map(str::trim).filter(|s| !s.is_empty()) {
        body["event_id"] = serde_json::Value::String(id.to_string());
    }
    if urgent {
        body["urgent"] = serde_json::Value::Bool(true);
    }
    send_and_print("POST", &url, http_client()?.post(&url).json(&body))
}

pub(crate) fn cmd_detach(ws: &Workspace, child_thread_id: &str) -> Result<(), BoxError> {
    let child = child_thread_id.trim();
    if uuid::Uuid::parse_str(child).is_err() {
        return Err(format!(
            "--thread takes a child thread's uuid, and '{child}' is not one. \
             Run `lucidos threads list --my-children` to find the id."
        )
        .into());
    }
    let url = format!("{}/api/v1/threads/{child}/detach", ws.base_url());
    send_and_print("POST", &url, http_client()?.post(&url))
}

pub(crate) fn cmd_child_reports(ws: &Workspace) -> Result<(), BoxError> {
    let thread_id = source_thread_id_from_env().ok_or(
        "LUCIDOS_THREAD_ID is not set, so there is no thread to read child reports for. \
         This subcommand only works from inside a Lucidos coding-agent session.",
    )?;
    let url = format!(
        "{}/api/v1/threads/{thread_id}/child-reports/take",
        ws.base_url()
    );
    send_and_print("POST", &url, http_client()?.post(&url))
}

pub(crate) fn cmd_archive(ws: &Workspace, thread: &str) -> Result<(), BoxError> {
    let target = thread.trim();
    if target.is_empty() || target.contains(['/', '?', '#']) || target.contains(char::is_whitespace)
    {
        return Err(format!(
            "--thread takes `current` or a child thread's uuid, and '{target}' is neither. \
             Run `lucidos threads list --my-children` to find a child's id."
        )
        .into());
    }
    let url = format!("{}/api/v1/threads/{target}/archive", ws.base_url());
    send_and_print("POST", &url, http_client()?.post(&url))
}

fn target_thread(thread: Option<&str>) -> Result<String, BoxError> {
    let target = match thread {
        Some(t) => t.trim().to_string(),
        None => source_thread_id_from_env().ok_or(
            "LUCIDOS_THREAD_ID is not set, so there is no thread to default to. \
             Pass --thread <uuid>.",
        )?,
    };
    if uuid::Uuid::parse_str(&target).is_err() {
        return Err(format!("--thread takes a thread's uuid, and '{target}' is not one.").into());
    }
    Ok(target)
}

pub(crate) fn cmd_context_ceiling(
    ws: &Workspace,
    ceiling: &str,
    thread: Option<&str>,
) -> Result<(), BoxError> {
    let target = target_thread(thread)?;
    let url = format!("{}/api/v1/threads/{target}/context-ceiling", ws.base_url());
    let body = serde_json::json!({ "ceiling": ceiling.trim() });
    send_and_print("PUT", &url, http_client()?.put(&url).json(&body))
}

pub(crate) fn cmd_hold(ws: &Workspace, reason: &str, thread: Option<&str>) -> Result<(), BoxError> {
    let target = target_thread(thread)?;
    let url = format!("{}/api/v1/threads/{target}/hold", ws.base_url());
    let body = serde_json::json!({ "reason": reason.trim() });
    send_and_print("POST", &url, http_client()?.post(&url).json(&body))
}

pub(crate) fn cmd_resume(ws: &Workspace, thread: Option<&str>) -> Result<(), BoxError> {
    let target = target_thread(thread)?;
    let url = format!("{}/api/v1/threads/{target}/hold", ws.base_url());
    send_and_print("DELETE", &url, http_client()?.delete(&url))
}

#[derive(serde::Serialize)]
pub(crate) struct DraftRequest {
    pub text: String,
    pub images: Vec<String>,
    pub mode: String,
    pub repo_id: Option<String>,
    pub app_id: Option<String>,
    pub coding_agent: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub effort: Option<String>,
    pub retry_key: String,
}

pub(crate) fn cmd_create_draft(ws: &Workspace, request: &DraftRequest) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/threads/drafts", ws.base_url());
    send_and_print("POST", &url, http_client()?.post(&url).json(request))
}

pub(crate) fn cmd_submit_draft(
    ws: &Workspace,
    thread: &str,
    revision: i64,
    epoch: i64,
    retry_key: &str,
) -> Result<(), BoxError> {
    let target = thread.trim();
    if uuid::Uuid::parse_str(target).is_err() {
        return Err(format!(
            "--thread takes the draft's thread uuid, and '{target}' is not one. \
             Run `lucidos threads drafts` to find it."
        )
        .into());
    }
    let retry_key = retry_key.trim();
    let body = serde_json::json!({
        "expected_revision": revision,
        "expected_epoch": epoch,
        "retry_key": retry_key,
    });
    let url = format!("{}/api/v1/threads/{target}/submit-draft", ws.base_url());
    let answer = send_or_refusal("POST", &url, http_client()?.post(&url).json(&body))
        .map_err(|error| naming_the_retry_key(error, retry_key))??;
    println!("{answer}");
    Ok(())
}

fn naming_the_retry_key(error: BoxError, retry_key: &str) -> BoxError {
    format!(
        "{error}\nThis send's retry key is {retry_key}. If it may have landed, run the same \
         command with `--retry-key {retry_key}` to read its receipt."
    )
    .into()
}

pub(crate) fn event_id_from_env() -> Option<String> {
    std::env::var(ENV_EVENT_ID)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

pub(crate) fn source_thread_id_from_env() -> Option<String> {
    std::env::var(ENV_SOURCE_THREAD_ID).ok()
}

pub(crate) fn build_query_params(filters: &ListFilters<'_>) -> Vec<(&'static str, String)> {
    let mut params: Vec<(&'static str, String)> = Vec::new();
    if let Some(a) = filters.active {
        params.push(("active", a.to_string()));
    }
    if !filters.status.is_empty() {
        params.push(("status", filters.status.join(",")));
    }
    if let Some(s) = filters.source {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            params.push(("source", trimmed.to_string()));
        }
    }
    params.extend(limit_param(filters.limit));
    if let Some(p) = filters.parent.as_deref() {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            params.push(("parent", trimmed.to_string()));
        }
    }
    if let Some(d) = filters.has_draft {
        params.push(("has_draft", d.to_string()));
    }
    if let Some(state) = filters.change_state.as_deref() {
        params.push(("change_state", state.to_string()));
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_submit_draft_names_its_retry_key() {
        let error = naming_the_retry_key("connection reset".into(), "key-1").to_string();
        assert!(error.starts_with("connection reset"), "{error}");
        assert!(error.contains("--retry-key key-1"), "{error}");
    }

    const NO_STATUS: &[String] = &[];

    fn statuses(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    fn build_query_params(
        active: Option<bool>,
        status: &[String],
        source: Option<&str>,
        limit: Option<u32>,
        parent: Option<&str>,
    ) -> Vec<(&'static str, String)> {
        super::build_query_params(&ListFilters {
            active,
            status,
            source,
            limit,
            parent: parent.map(str::to_string),
            ..ListFilters::default()
        })
    }

    #[test]
    fn the_draft_and_change_state_filters_go_out_as_given() {
        let params = super::build_query_params(&ListFilters {
            has_draft: Some(true),
            change_state: Some("unproposed".to_string()),
            ..ListFilters::default()
        });
        assert_eq!(
            params,
            vec![
                ("has_draft", "true".to_string()),
                ("change_state", "unproposed".to_string()),
            ]
        );
    }

    #[test]
    fn drafts_names_one_thread_only_when_asked() {
        assert!(drafts_query_params(None, None).is_empty());
        assert!(drafts_query_params(None, Some("  ")).is_empty());
        let one = "12121212-1212-1212-1212-121212121212";
        assert_eq!(
            drafts_query_params(Some(5), Some(one)),
            vec![("limit", "5".to_string()), ("thread_id", one.to_string())]
        );
    }

    #[test]
    fn list_includes_active_flag_when_set() {
        let params = build_query_params(Some(true), NO_STATUS, None, None, None);
        assert_eq!(params, vec![("active", "true".to_string())]);
    }

    #[test]
    fn list_includes_active_false_when_explicitly_inverted() {
        let params = build_query_params(Some(false), NO_STATUS, None, None, None);
        assert_eq!(params, vec![("active", "false".to_string())]);
    }

    #[test]
    fn list_includes_status_when_set() {
        let params = build_query_params(None, &statuses(&["running"]), None, None, None);
        assert_eq!(params, vec![("status", "running".to_string())]);
    }

    #[test]
    fn repeated_and_comma_separated_status_flags_produce_one_param() {
        let repeated = build_query_params(
            None,
            &statuses(&["running", "waiting_for_user_answer"]),
            None,
            None,
            None,
        );
        let joined = build_query_params(
            None,
            &statuses(&["running,waiting_for_user_answer"]),
            None,
            None,
            None,
        );
        assert_eq!(repeated, joined);
        assert_eq!(
            repeated,
            vec![("status", "running,waiting_for_user_answer".to_string())]
        );
    }

    #[test]
    fn list_includes_source_when_non_empty() {
        let params = build_query_params(None, NO_STATUS, Some("chat,trigger"), None, None);
        assert_eq!(params, vec![("source", "chat,trigger".to_string())]);
    }

    #[test]
    fn list_omits_blank_source() {
        let params = build_query_params(None, NO_STATUS, Some("   "), None, None);
        assert!(params.is_empty());
    }

    #[test]
    fn list_includes_limit_when_set() {
        let params = build_query_params(None, NO_STATUS, None, Some(50), None);
        assert_eq!(params, vec![("limit", "50".to_string())]);
    }

    #[test]
    fn list_returns_no_params_when_all_unset() {
        let params = build_query_params(None, NO_STATUS, None, None, None);
        assert!(params.is_empty());
    }

    #[test]
    fn list_includes_parent_when_set() {
        let parent = "11111111-1111-1111-1111-111111111111";
        let params = build_query_params(None, NO_STATUS, None, None, Some(parent));
        assert_eq!(params, vec![("parent", parent.to_string())]);
    }

    #[test]
    fn list_omits_blank_parent() {
        let params = build_query_params(None, NO_STATUS, None, None, Some("  "));
        assert!(params.is_empty());
    }

    #[test]
    fn list_composes_every_filter() {
        let parent = "22222222-2222-2222-2222-222222222222";
        let params = build_query_params(
            Some(true),
            NO_STATUS,
            Some("trigger"),
            Some(10),
            Some(parent),
        );
        assert_eq!(
            params,
            vec![
                ("active", "true".to_string()),
                ("source", "trigger".to_string()),
                ("limit", "10".to_string()),
                ("parent", parent.to_string()),
            ]
        );
    }

    #[test]
    fn status_composes_with_the_other_filters() {
        let parent = "99999999-9999-9999-9999-999999999999";
        let params = build_query_params(
            None,
            &statuses(&["running"]),
            Some("coding-agent"),
            Some(10),
            Some(parent),
        );
        assert_eq!(
            params,
            vec![
                ("status", "running".to_string()),
                ("source", "coding-agent".to_string()),
                ("limit", "10".to_string()),
                ("parent", parent.to_string()),
            ]
        );
    }

    #[test]
    fn my_children_resolves_to_the_calling_thread() {
        let me = "33333333-3333-3333-3333-333333333333".to_string();
        assert_eq!(
            resolve_parent_filter(None, true, Some(me.clone())).unwrap(),
            Some(me)
        );
    }

    #[test]
    fn my_children_without_a_calling_thread_is_an_actionable_error() {
        for env in [None, Some(String::new()), Some("   ".to_string())] {
            let err = resolve_parent_filter(None, true, env)
                .expect_err("must not silently list the whole workspace");
            let msg = err.to_string();
            assert!(msg.contains("--parent"), "must name the way out: {msg}");
        }
    }

    #[test]
    fn parent_and_my_children_together_is_refused() {
        let explicit = Some("44444444-4444-4444-4444-444444444444".to_string());
        let mine = Some("55555555-5555-5555-5555-555555555555".to_string());
        assert!(
            resolve_parent_filter(explicit, true, mine).is_err(),
            "two answers to one filter must not silently pick one"
        );
    }

    #[test]
    fn parent_alone_ignores_the_environment() {
        let explicit = "66666666-6666-6666-6666-666666666666".to_string();
        let mine = Some("77777777-7777-7777-7777-777777777777".to_string());
        assert_eq!(
            resolve_parent_filter(Some(explicit.clone()), false, mine).unwrap(),
            Some(explicit)
        );
    }

    #[test]
    fn no_filter_flags_yields_no_parent() {
        let mine = Some("88888888-8888-8888-8888-888888888888".to_string());
        assert_eq!(resolve_parent_filter(None, false, mine).unwrap(), None);
    }
}
