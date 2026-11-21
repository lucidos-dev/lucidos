/// A SQL predicate: `thread` has a `SessionHandedOff` that nothing has ended.
/// `thread` is a SQL expression, such as `$1` or `c.thread_id`, and may name
/// any outer alias: the subqueries use their own.
pub fn pending_handoff_sql(thread: &str) -> String {
    format!(
        concat!(
            "EXISTS (SELECT 1 FROM events pending_handoff \
                  WHERE pending_handoff.thread_id = {thread} \
                    AND pending_handoff.event_type = 'SessionHandedOff' \
                    AND pending_handoff.sequence > ",
            crate::core::snapshot::boundary::restore_floor_sql!(),
            " \
                    AND NOT EXISTS (SELECT 1 FROM events handoff_end \
                          WHERE handoff_end.thread_id = {thread} \
                            AND {}))"
        ),
        ends_handoff_sql("handoff_end", "pending_handoff"),
        thread = thread
    )
}

/// `SessionHandedOff` and every type `ends_handoff_sql` matches. The partial
/// index `idx_events_handoff_lifecycle` lists the same types, so the planner
/// can answer `pending_handoff_threads_sql` from it.
pub const HANDOFF_LIFECYCLE_EVENT_TYPES: [&str; 5] = [
    "SessionHandedOff",
    "SessionStarted",
    "ResponseCanceled",
    "ResponseFailed",
    "SessionHandoffWithdrawn",
];

/// A SQL query: the `thread_id` of every thread with a pending handoff, the
/// set `pending_handoff_sql` tests one thread against. It costs one index
/// probe per `SessionHandedOff`, whatever the thread count. The `LIMIT 1`
/// keeps the planner from turning the end probe into a scan of every end event.
/// It never yields NULL, so a caller may test membership with `NOT IN`.
pub fn pending_handoff_threads_sql() -> String {
    pending_handoff_rows_sql("pending_handoff.thread_id")
}

/// The same set as [`pending_handoff_threads_sql`], selecting `columns` of
/// each pending `SessionHandedOff` row, aliased `pending_handoff`. A thread
/// with two pending handoffs yields two rows.
pub fn pending_handoff_rows_sql(columns: &str) -> String {
    let lifecycle = HANDOFF_LIFECYCLE_EVENT_TYPES
        .iter()
        .map(|event_type| format!("'{event_type}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        concat!(
            "SELECT {columns} FROM events pending_handoff \
               LEFT JOIN LATERAL (SELECT 1 AS ended FROM events handoff_end \
                     WHERE handoff_end.thread_id = pending_handoff.thread_id \
                       AND handoff_end.event_type IN ({lifecycle}) \
                       AND {} LIMIT 1) handoff_ended ON TRUE \
              WHERE pending_handoff.event_type = 'SessionHandedOff' \
                AND pending_handoff.thread_id IS NOT NULL \
                AND pending_handoff.sequence > ",
            crate::core::snapshot::boundary::restore_floor_sql!(),
            " \
                AND handoff_ended.ended IS NULL"
        ),
        ends_handoff_sql("handoff_end", "pending_handoff"),
        columns = columns,
        lifecycle = lifecycle
    )
}

/// A SQL predicate: the `events` row `alias` ends the handoff recorded by the
/// `SessionHandedOff` row `handoff`, on the same thread. Four things end one:
///
/// - its successor session's `SessionStarted`, whose `continuity` is
///   `handoff`. A recovery or conflict-resolution session records none, and
///   a lane run records it only when it consumes the handoff (ADR 0675);
/// - a user's Stop, or an Apply, Discard or Archive that stopped a turn;
/// - a failed turn that began after it: the successor session never started;
/// - a person's *Return to the old session*, `SessionHandoffWithdrawn`.
pub fn ends_handoff_sql(alias: &str, handoff: &str) -> String {
    format!(
        "{alias}.sequence > {handoff}.sequence AND ( \
           ({alias}.event_type = 'SessionStarted' \
              AND {alias}.payload->'continuity'->>'kind' = 'handoff') \
           OR {alias}.event_type = 'SessionHandoffWithdrawn' \
           OR ({alias}.event_type = 'ResponseCanceled' \
              AND {alias}.payload->>'cause' IN ('user_stop', 'user_action')) \
           OR ({alias}.event_type = 'ResponseFailed' \
              AND EXISTS (SELECT 1 FROM events {alias}_turn \
                   WHERE {alias}_turn.thread_id = {alias}.thread_id \
                     AND {alias}_turn.event_type = 'MessageReceived' \
                     AND {alias}_turn.sequence > {handoff}.sequence \
                     AND {alias}_turn.sequence < {alias}.sequence)))"
    )
}
