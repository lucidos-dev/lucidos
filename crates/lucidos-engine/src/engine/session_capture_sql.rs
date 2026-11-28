pub(crate) const MAIN_SESSION_CAPTURE: &str = "event_type = 'ContextCaptured' \
     AND payload->>'parent_tool_use_id' IS NULL \
     AND payload->'usage'->>'input_tokens' IS NOT NULL \
     AND COALESCE(payload->>'purpose', 'turn') = 'turn'";

pub(crate) fn current_session_capture_sql(thread: &str) -> String {
    format!(
        "{MAIN_SESSION_CAPTURE} AND {}",
        super::session_scope::in_current_session_sql(thread, "sequence")
    )
}

fn latest_capture_started(thread: &str, predicate: &str) -> String {
    latest_capture_started_in_any_session(
        thread,
        &format!(
            "{predicate} AND {}",
            super::session_scope::in_current_session_sql(thread, "e.sequence")
        ),
    )
}

pub(crate) const CAPTURE_STARTED: &str =
    "e.created - make_interval(secs => COALESCE((e.payload->>'duration_ms')::float8, 0) / 1000)";

fn latest_capture_started_in_any_session(thread: &str, predicate: &str) -> String {
    format!(
        "(SELECT {CAPTURE_STARTED} \
            FROM events e \
           WHERE e.thread_id = {thread} AND {predicate} \
           ORDER BY e.sequence DESC LIMIT 1)"
    )
}

pub(crate) fn last_turn_started_sql(thread: &str) -> String {
    latest_capture_started(thread, MAIN_SESSION_CAPTURE)
}

fn real_request_sql() -> String {
    format!(
        "{MAIN_SESSION_CAPTURE} AND NOT EXISTS ( \
           SELECT 1 FROM events a \
            WHERE a.id = NULLIF(e.payload->>'request_event_id', '')::uuid \
              AND a.event_type = 'PromptInjected' \
              AND a.payload->'origin'->'reason'->>'kind' = '{}')",
        crate::engine::thread_events::CACHE_KEEP_ALIVE_REASON_KIND
    )
}

pub(crate) fn last_real_request_started_sql(thread: &str) -> String {
    latest_capture_started_in_any_session(thread, &real_request_sql())
}

pub(crate) fn first_real_request_started_after_sql(thread: &str, sequence: &str) -> String {
    format!(
        "(SELECT {CAPTURE_STARTED} \
            FROM events e \
           WHERE e.thread_id = {thread} AND e.sequence > {sequence} AND {} \
           ORDER BY e.sequence LIMIT 1)",
        real_request_sql()
    )
}

pub(crate) fn last_cache_refresh_started_sql(thread: &str) -> String {
    let beat = latest_capture_started(
        thread,
        &format!(
            "event_type = 'ContextCaptured' AND payload->>'purpose' = '{}'",
            crate::engine::ContextPurpose::CacheKeepAlive.wire()
        ),
    );
    format!("GREATEST({}, {beat})", last_turn_started_sql(thread))
}
