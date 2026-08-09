use crate::http::{client as http_client, send_and_print};
use crate::workspace::{BoxError, Workspace};

/// Env var carrying the calling thread's id. Read only to resolve
/// `--my-children` into a `parent` filter, which is a convenience over a
/// public read. It is NOT how the engine learns who the caller is: that comes
/// from the thread-bound origin token the HTTP client forwards, which is why
/// `follow-up` has no `--from` flag to get wrong.
const ENV_SOURCE_THREAD_ID: &str = "LUCIDOS_THREAD_ID";

/// Env var carrying the event that spawned this subprocess. Defaults
/// `follow-up --event-id`, matching how `spawn-thread` defaults
/// `--caller-event-id`.
const ENV_EVENT_ID: &str = "LUCIDOS_EVENT_ID";

pub(crate) struct ListFilters<'a> {
    /// `Some(true)` selects the UNION of `running` and
    /// `waiting_for_user_answer`, `Some(false)` inverts it, `None` filters
    /// nothing. For "is the workspace busy?" callers want `status` with
    /// `running` alone: a thread awaiting a user answer is blocked on the
    /// human, not working.
    pub active: Option<bool>,
    /// Exactly these statuses, from repeated `--status` flags and/or
    /// comma-separated values. Empty means no status narrowing. Clap already
    /// refuses this together with `active`; the engine validates the values and
    /// is the single place that owns the wording for a bad one.
    pub status: &'a [String],
    /// Comma-separated source list (`chat`, `trigger`, `coding-agent`).
    /// Legacy `claude_code` is also accepted by the engine.
    pub source: Option<&'a str>,
    /// Server clamps to 1..=1000 (default 100). `None` on the `count` path,
    /// which has no page to size.
    pub limit: Option<u32>,
    /// Restrict to the direct children of this thread. Same name as the
    /// `parent` query param and as the `my_children` filter the LLM tool
    /// resolves from its own ambient thread.
    pub parent: Option<String>,
}

/// Resolve `--parent <uuid>` / `--my-children` into the single `parent` value
/// the query param takes.
///
/// `--my-children` is sugar for `--parent $LUCIDOS_THREAD_ID`, so outside a
/// Lucidos-spawned subprocess it has nothing to resolve to. That is an error
/// rather than a silent unfiltered list: "show me my children" answered with
/// every thread in the workspace is the wrong answer, not a broader one.
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
    send_filtered("GET", &url, &filters)
}

/// Same filters as `cmd_list`, against the count endpoint. `limit` is carried
/// in the shared `ListFilters` and left `None` here, mirroring how the store
/// and HTTP layers reuse one filter struct for both queries.
pub(crate) fn cmd_count(ws: &Workspace, filters: ListFilters<'_>) -> Result<(), BoxError> {
    let url = format!("{}/api/v1/threads/count", ws.base_url());
    send_filtered("GET", &url, &filters)
}

fn send_filtered(method: &str, url: &str, filters: &ListFilters<'_>) -> Result<(), BoxError> {
    let params = build_query_params(
        filters.active,
        filters.status,
        filters.source,
        filters.limit,
        filters.parent.as_deref(),
    );
    let mut req = http_client()?.get(url);
    if !params.is_empty() {
        req = req.query(&params);
    }
    send_and_print(method, url, req)
}

/// `lucidos threads follow-up`: send a message to one of the calling thread's
/// own child threads.
///
/// The caller is never stated. It comes off the thread-bound origin token the
/// HTTP client forwards, and the engine looks the relationship up from the
/// child's projection row, so this command cannot address a thread the calling
/// thread did not spawn. That is also why there is no `--from`.
///
/// Prints the ack body verbatim, like every other write verb in this CLI
/// (`changes apply`, `notify`, `threads list` / `count`). The ack already
/// carries a `detail` sentence saying what happened to the message, so there
/// is nothing to synthesize, and reformatting it here would put a second copy
/// of the ack's wording one repo away from the first.
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
    // Only sent when set, so the engine's `#[serde(default)]` is what decides
    // the default and the CLI never has to restate it.
    if urgent {
        body["urgent"] = serde_json::Value::Bool(true);
    }
    send_and_print("POST", &url, http_client()?.post(&url).json(&body))
}

/// Default `--event-id` from the spawning event, the way `spawn-thread`
/// defaults `--caller-event-id`.
pub(crate) fn event_id_from_env() -> Option<String> {
    std::env::var(ENV_EVENT_ID)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Read the calling thread's id for `--my-children`.
pub(crate) fn source_thread_id_from_env() -> Option<String> {
    std::env::var(ENV_SOURCE_THREAD_ID).ok()
}

fn build_query_params(
    active: Option<bool>,
    status: &[String],
    source: Option<&str>,
    limit: Option<u32>,
    parent: Option<&str>,
) -> Vec<(&'static str, String)> {
    let mut params: Vec<(&'static str, String)> = Vec::new();
    if let Some(a) = active {
        params.push(("active", a.to_string()));
    }
    // Repeated `--status a --status b` and `--status a,b` are the same
    // request: both arrive as one comma-separated param. The engine owns
    // validation, so a bad value produces one wording rather than two.
    if !status.is_empty() {
        params.push(("status", status.join(",")));
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
    if let Some(p) = parent {
        let trimmed = p.trim();
        if !trimmed.is_empty() {
            params.push(("parent", trimmed.to_string()));
        }
    }
    params
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No status flags, so the status axis contributes nothing.
    const NO_STATUS: &[String] = &[];

    fn statuses(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
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

    /// Repeated flags and one comma-separated flag are the same request, so
    /// neither form is a second thing the engine has to know about.
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

    /// The status axis composes with the others exactly as `active` does. Clap
    /// refuses `--active --status`, so the two never both appear here.
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

    /// "Show me my children" answered with every thread in the workspace is
    /// the wrong answer, not a broader one, so an unresolvable `--my-children`
    /// is an error rather than a silent no-op.
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
