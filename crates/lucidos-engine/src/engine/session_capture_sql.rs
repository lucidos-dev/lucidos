/// A SQL predicate over unaliased `events` columns: a model call of a
/// thread's own turn that reports its input size. A sub-agent's call and an
/// auxiliary call on the thread do not count.
pub(crate) const MAIN_SESSION_CAPTURE: &str = "event_type = 'ContextCaptured' \
     AND payload->>'parent_tool_use_id' IS NULL \
     AND payload->'usage'->>'input_tokens' IS NOT NULL \
     AND COALESCE(payload->>'purpose', 'turn') = 'turn'";

/// A SQL predicate over unaliased `events` columns: a [`MAIN_SESSION_CAPTURE`]
/// in `thread`'s current *agent session*. Size, warmth and the estimates read
/// only these, so a fresh session starts from nothing.
pub(crate) fn current_session_capture_sql(thread: &str) -> String {
    format!(
        "{MAIN_SESSION_CAPTURE} AND {}",
        super::session_scope::in_current_session_sql(thread, "sequence")
    )
}

/// A SQL scalar: when the latest capture of `thread`'s current session that
/// matches `predicate` began, which is its `created` less its `duration_ms`.
/// `thread` is a SQL expression, such as `$1` or `ts.thread_id`.
fn latest_capture_started(thread: &str, predicate: &str) -> String {
    latest_capture_started_in_any_session(
        thread,
        &format!(
            "{predicate} AND {}",
            super::session_scope::in_current_session_sql(thread, "e.sequence")
        ),
    )
}

/// When the capture `e` began: its `created` less its `duration_ms`.
pub(crate) const CAPTURE_STARTED: &str =
    "e.created - make_interval(secs => COALESCE((e.payload->>'duration_ms')::float8, 0) / 1000)";

/// [`latest_capture_started`] over every session of `thread`.
fn latest_capture_started_in_any_session(thread: &str, predicate: &str) -> String {
    format!(
        "(SELECT {CAPTURE_STARTED} \
            FROM events e \
           WHERE e.thread_id = {thread} AND {predicate} \
           ORDER BY e.sequence DESC LIMIT 1)"
    )
}

/// A SQL scalar: when the latest main-session turn request of `thread` began.
pub(crate) fn last_turn_started_sql(thread: &str) -> String {
    latest_capture_started(thread, MAIN_SESSION_CAPTURE)
}

/// A SQL predicate over the capture `e`: a *real* request, a main-session turn
/// request that no keep-alive anchor started. A resume beat's hidden turn does
/// not count, and a copy beat is no turn capture.
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

/// A SQL scalar: when the latest *real* request of `thread` began. The
/// per-thread keep-alive switch holds until this moves (ADR 0554). It reads
/// every session: a new session must not undo a request that re-armed it.
pub(crate) fn last_real_request_started_sql(thread: &str) -> String {
    latest_capture_started_in_any_session(thread, &real_request_sql())
}

/// A SQL scalar: when the first *real* request of `thread` after the event
/// at `sequence` began, in any session. NULL until one has. A card's wait
/// ends there, since that request reads the cache.
pub(crate) fn first_real_request_started_after_sql(thread: &str, sequence: &str) -> String {
    format!(
        "(SELECT {CAPTURE_STARTED} \
            FROM events e \
           WHERE e.thread_id = {thread} AND e.sequence > {sequence} AND {} \
           ORDER BY e.sequence LIMIT 1)",
        real_request_sql()
    )
}

/// A SQL scalar: when the *cache* of `thread` last refreshed. That is the later
/// start of its last main-session turn and its last keep-alive copy beat. The
/// keep-alive, the cache-warm line and the switch estimate all read it, so a
/// beaten thread is warm to all three (ADR 0547).
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
