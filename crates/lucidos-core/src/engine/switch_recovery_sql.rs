pub const SWITCH_TEARDOWN_ABORT_SQL: &str = "event_type = 'ResponseAborted' \
     AND payload->'actor'->>'kind' = 'device' \
     AND payload->>'cause' = 'engine_shutdown'";

/// INVARIANT: SQL list of the events that begin (or restart) a thread's turn.
///
/// Two questions read it, and they must not answer differently about which turn
/// is current. The recovery gates ask through [`after_latest_thread_start_sql`],
/// which is the form to reach for here. `engine::standing_instruction` asks who
/// opened that turn, and selects the newest row of this set directly.
pub(crate) const THREAD_START_EVENTS_SQL: &str = "'MessageReceived',\
    'CodingAgentUserMessageSent','TriggerStarted','ContinuationStarted',\
    'OrphanRecoveryStarted'";

/// INVARIANT: SQL boolean: the event at `seq_expr` is newer than every
/// [`THREAD_START_EVENTS_SQL`] event on the thread at `id_expr`, so it belongs
/// to that thread's **current** turn.
///
/// Every recovery read asking "which turn does this `ResponseAborted` belong
/// to?" goes through here, because the interesting failures are two of them
/// answering differently:
///
/// * Is the *Switch to new version* fingerprint still live, or did a resume
///   already consume it? ([`switch_abort_unsuperseded_sql`], the loop-breaker.)
/// * Does this turn still need an interruption boundary, or did the teardown
///   pre-emit already land one? ([`boundary_abort_already_emitted`].)
///
/// `id_expr` yields the thread's `aggregate_id` (text): `"$1"` for a bound
/// single-thread check, or a column reference for a set-based scan. `seq_expr`
/// yields the event's sequence in the same scope. The subquery aliases its own
/// `events` as `s`, so an unqualified `seq_expr` in an un-aliased outer query is
/// never captured by it.
pub fn after_latest_thread_start_sql(id_expr: &str, seq_expr: &str) -> String {
    format!(
        "{seq} > COALESCE(( \
             SELECT MAX(s.sequence) FROM events s \
             WHERE s.aggregate_id = {id} \
               AND s.event_type IN ({starts}) \
         ), 0)",
        seq = seq_expr,
        id = id_expr,
        starts = THREAD_START_EVENTS_SQL,
    )
}

/// INVARIANT: A switch abort can authorize a resume until a later start or abort
/// supersedes it.
///
/// Coding-agent resume, chat resume and boot settlement share this predicate.
/// `ContinuationStarted` consumes the switch abort even when the resumed turn
/// dies before emitting anything else.
///
/// An archived switch abort cannot authorize a resume after restore. A newer
/// `ResponseAborted` also supersedes it because the boot floor withdraws an
/// unkept promise with an abort.
///
/// The subquery aliases `events` as `ra` to avoid capturing the caller's
/// reference. See [`after_latest_thread_start_sql`].
pub fn switch_abort_unsuperseded_sql(id_expr: &str, seq_expr: &str) -> String {
    format!(
        concat!(
            "{current_turn} AND {seq} > ",
            crate::core::snapshot::boundary::restore_floor_sql!(),
            " AND {seq} = ( \
             SELECT MAX(ra.sequence) FROM events ra \
             WHERE ra.aggregate_id = {id} \
               AND ra.event_type = 'ResponseAborted' \
         )"
        ),
        current_turn = after_latest_thread_start_sql(id_expr, seq_expr),
        seq = seq_expr,
        id = id_expr,
    )
}
