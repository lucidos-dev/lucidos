use uuid::Uuid;

/// Canonical "thread is parked on an unanswered `AskUserQuestion`" predicate, as
/// a correlated SQL `EXISTS(...)` body. `id_expr` is a SQL expression yielding
/// the thread's `aggregate_id` (text): `"$1"` for the single-thread bool check,
/// or a column reference such as `"pt.aggregate_id"` for a set-based sweep.
///
/// This is the SINGLE source of truth for the preserve guard. Every restart
/// abort or cleanup path resolves through this fragment, so "parked on a
/// question means never aborted" cannot drift between paths. A terminal after
/// the `UserQuestionAsked` flips it to false. A path that wrongly emitted one
/// would then defeat every OTHER path's guard on the next boot.
///
/// "Parked" means the question is still the last thing that happened: no answer,
/// no terminal, AND no agent progression. The progression half comes from
/// [`crate::engine::thread_events::ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES`],
/// the constant the frontend
/// mirrors to strike the card through. So "the card is dead" and "the thread is
/// no longer preserved" cannot disagree.
pub fn unanswered_question_exists_sql(id_expr: &str) -> String {
    format!(
        "EXISTS ( SELECT 1 FROM events uqa WHERE {} )",
        unanswered_question_predicate_sql(id_expr),
    )
}

/// The agent permission prompts: each request type, paired with the type that
/// resolves it by `request_id`. Each one blocks a live agent call.
pub const PERMISSION_PROMPT_EVENT_TYPES: [(&str, &str); 3] = [
    (
        "CodingAgentPermissionRequest",
        "CodingAgentPermissionResolved",
    ),
    ("CommandPermissionRequested", "CommandPermissionResolved"),
    ("McpPermissionRequested", "McpPermissionResolved"),
];

/// SQL `EXISTS(...)`: the thread `id_expr` names has an agent permission
/// prompt nobody resolved.
pub fn unresolved_permission_prompt_exists_sql(id_expr: &str) -> String {
    unresolved_permission_prompt_exists_before_sql(id_expr, "")
}

fn unresolved_permission_prompt_exists_before_sql(id_expr: &str, fence: &str) -> String {
    let arms = PERMISSION_PROMPT_EVENT_TYPES
        .map(|(request, resolved)| {
            format!(
                "(p.event_type = '{request}' AND NOT EXISTS ( \
                   SELECT 1 FROM events r WHERE r.event_type = '{resolved}' \
                   AND r.payload->>'request_id' = p.payload->>'request_id'))"
            )
        })
        .join(" OR ");
    format!(
        "EXISTS ( SELECT 1 FROM events p WHERE p.aggregate_id = {id_expr} {fence} AND ({arms}) )"
    )
}

/// Whether a permission card prevents dispatch of typed inputs.
pub async fn open_permission_card_exists(
    conn: &mut sqlx::PgConnection,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    open_permission_card_before(conn, thread_id, None).await
}

/// Optional sequence fence to distinguish the card that held the input from
/// a card raised after the input began dispatch.
pub async fn open_permission_card_before(
    conn: &mut sqlx::PgConnection,
    thread_id: Uuid,
    before: Option<i64>,
) -> Result<bool, sqlx::Error> {
    let prompts = unresolved_permission_prompt_exists_before_sql(
        "$1",
        "AND ($2::bigint IS NULL OR p.sequence < $2)",
    );
    let active_question = unanswered_question_predicate_sql("$1");
    sqlx::query_scalar(&format!(
        "SELECT {prompts}
          OR EXISTS (SELECT 1 FROM events uqa
               WHERE {active_question} AND ($2::bigint IS NULL OR uqa.sequence < $2)
                 AND uqa.payload->'permission_request' ? 'prepared')
          OR EXISTS (SELECT 1 FROM events q
               WHERE q.aggregate_id = $1 AND q.event_type = 'UserQuestionAsked'
                 AND q.payload ? 'permission_request'
                 AND NOT q.payload->'permission_request' ? 'prepared'
                 AND ($2::bigint IS NULL OR q.sequence < $2)
                 AND NOT EXISTS (SELECT 1 FROM events a WHERE a.aggregate_id = $1
                    AND a.event_type = 'UserQuestionAnswered'
                    AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id'))"
    ))
    .bind(thread_id.to_string())
    .bind(before)
    .fetch_one(conn)
    .await
}

/// SQL `EXISTS(...)`: the thread has an *open card*. That is a live
/// `UserQuestionAsked` by the park predicate, or an unresolved agent
/// permission prompt. The single definition behind "one open card per
/// thread": the EventBus gate and the card queue both read it.
pub fn open_card_exists_sql(id_expr: &str) -> String {
    format!(
        "({} OR {})",
        unanswered_question_exists_sql(id_expr),
        unresolved_permission_prompt_exists_sql(id_expr)
    )
}

/// Whether `thread_id` has an *open card*, read on `conn`. Pass the append
/// transaction from the EventBus, so a batch sees its own earlier cards.
pub async fn open_card_exists(
    conn: &mut sqlx::PgConnection,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(&format!("SELECT {}", open_card_exists_sql("$1")))
        .bind(thread_id.to_string())
        .fetch_one(conn)
        .await
}

/// The predicate itself, over one `events uqa` row.
///
/// Split out so the yes/no guard above and [`newest_open_question`] below
/// cannot drift on what "still parked" means. They differ in what they select
/// and in nothing else.
pub fn unanswered_question_predicate_sql(id_expr: &str) -> String {
    format!(
        "uqa.aggregate_id = {id} AND uqa.event_type = 'UserQuestionAsked' \
           AND NOT EXISTS ( \
               SELECT 1 FROM events later \
               WHERE later.aggregate_id = {id} AND later.sequence > uqa.sequence \
                 AND {closes} \
           )",
        id = id_expr,
        closes = closes_question_sql("uqa", "later"),
    )
}

/// SQL over two `events` aliases: `later` closes the `question` card. The
/// turn moved on, or `later` is that card's own answer or tool result. An
/// answer to another card leaves this one open.
pub fn closes_question_sql(question: &str, later: &str) -> String {
    format!(
        "({later}.event_type IN ({moved_on}) OR {own_result} \
          OR ({later}.event_type = 'UserQuestionAnswered' \
              AND {later}.payload->>'tool_use_id' = {question}.payload->>'tool_use_id'))",
        moved_on = turn_moved_on_event_types_sql(),
        own_result =
            crate::engine::thread_events::ThreadEvent::question_own_result_sql(question, later),
    )
}

/// Turn ends that are NOT in
/// [`crate::engine::thread_events::ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES`].
/// They are absent there because a coding-agent turn ends on `CodingAgentIdled`.
/// The preserve guard must still treat them as park-ending: either one means the
/// turn that owned the question is over.
const TURN_ENDED_EVENT_TYPES: &[&str] = &["ResponseGenerated", "SessionEnded"];

/// Every event type after which the turn that asked a question has moved on.
/// An answer is not one: answering one card says nothing about another.
fn turn_moved_on_event_types() -> impl Iterator<Item = &'static str> {
    crate::engine::thread_events::ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES
        .iter()
        .chain(TURN_ENDED_EVENT_TYPES.iter())
        .copied()
}

/// Every event type that ends a question park: the turn moved on, or an answer
/// landed. `UserQuestionAnswered` is absent from the overtaken list because
/// that check pairs it by `tool_use_id` rather than by type.
fn park_ending_event_types() -> impl Iterator<Item = &'static str> {
    turn_moved_on_event_types().chain(std::iter::once("UserQuestionAnswered"))
}

/// The turn-moved-on event types as a SQL `IN (...)` body. The card queue
/// reads it to retire a queued agent card whose turn is over.
pub fn turn_moved_on_event_types_sql() -> &'static str {
    static SQL: std::sync::LazyLock<String> =
        std::sync::LazyLock::new(|| quoted_list(turn_moved_on_event_types()));
    &SQL
}

fn quoted_list(types: impl Iterator<Item = &'static str>) -> String {
    types
        .map(|t| format!("'{t}'"))
        .collect::<Vec<_>>()
        .join(",")
}

/// The two event types that carry a card's payload: a posted card and a
/// *queued card*, as a SQL `IN (...)` body. A lookup by `tool_use_id` that
/// must also find a card the owner has not seen yet reads both.
pub const CARD_EVENT_TYPES_SQL: &str = "'UserQuestionAsked','UserQuestionQueued'";

/// SQL over one `events {alias}` row: a *queued card* still waiting, never
/// posted and never answered. The card queue posts these, and a producer
/// that dedups counts them as raised.
pub fn waiting_queued_card_sql(alias: &str) -> String {
    format!(
        "({alias}.event_type = 'UserQuestionQueued' \
          AND NOT EXISTS (SELECT 1 FROM events settled \
            WHERE settled.aggregate_id = {alias}.aggregate_id \
              AND settled.event_type IN ('UserQuestionAsked', 'UserQuestionAnswered') \
              AND settled.payload->>'tool_use_id' = {alias}.payload->>'tool_use_id'))"
    )
}

/// Whether an event of this type ends the thread's question park whatever
/// question is open. A `CodingAgentToolResult` is absent: it ends the park only
/// when it pairs with the question's own `tool_use_id`. For a consumer that
/// follows events one by one.
pub fn ends_question_park(event_type: &str) -> bool {
    park_ending_event_types().any(|t| t == event_type)
}

/// True when the thread's most recent `UserQuestionAsked` has no later answer,
/// terminal, or agent progression: it is parked waiting for the user, and the
/// card on screen is still live. Such a thread is a stable, resumable
/// checkpoint, so recovery must preserve it across a restart with no abort and
/// no idle. Answering resumes it through the no-live-subprocess
/// `ContinuationRequested` path. A pending question survives a user switch and a
/// crash alike.
///
/// Shared predicate for BOTH sides of that invariant. The teardown emit consults
/// it to skip the boundary `ResponseAborted`, because a question-parked session
/// is still MID-TURN and the `is_in_flight()` filter cannot exclude it. This
/// recovery pass consults it to skip the abort and idle pair. One definition
/// keeps "no boundary lands at teardown" and "recovery preserves" from drifting
/// apart: a `ResponseAborted` is park-ending, so a teardown that emitted one
/// would defeat the guard on the very next boot.
pub async fn thread_has_unanswered_question(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    thread_parked_on_question(pool, thread_id)
        .await
        .unwrap_or_else(|e| {
            log_legacy!(
                "[Recovery] Could not read whether thread {} is parked on a question: {}",
                thread_id,
                e
            );
            false
        })
}

/// [`thread_has_unanswered_question`] for a caller that must not read a failed
/// lookup as "not parked". Apply Now refuses on an `Err`, because applying over
/// a live question card is the direction that loses the user's answer.
pub async fn thread_parked_on_question(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    // `$1` is bound as the thread id (text). The shared fragment keeps this
    // per-thread check and every set-based sweep on one definition.
    let sql = format!("SELECT {}", unanswered_question_exists_sql("$1"));
    sqlx::query_scalar::<_, bool>(&sql)
        .bind(thread_id.to_string())
        .fetch_one(pool)
        .await
}

/// True when an engine teardown must leave this thread exactly as it is because
/// its session is parked on an unanswered `AskUserQuestion`. Thin wrapper over
/// [`thread_has_unanswered_question`], so the two teardown sites cannot diverge
/// and the skip is always logged. Callers still cancel the agent runtime, so no
/// subprocess outlives the engine.
///
/// The card must stay answerable across the restart, which needs the
/// `UserQuestionAsked` to still be the thread's newest event at the next boot.
/// Two teardown sites would break that, and both consult this:
///
/// * `shutdown_agent_sessions` sends the graceful interrupt, which cancels the
///   question and records a rejection the user never made.
/// * the stop and chat-cancel arms of `run_session` emit a terminal and flush
///   buffered agent text.
///
/// Gated on `is_shutdown`, so a user Stop, Apply, Discard or Archive outside a
/// teardown is untouched: each deliberately ends the turn and cancel-stamps the
/// card itself. The gate is a *window*, not an actor test, so a raw Stop inside
/// the teardown window is swallowed too.
pub async fn preserve_question_park_at_shutdown(
    pool: &sqlx::PgPool,
    site: &'static str,
    thread_id: Uuid,
    is_shutdown: bool,
) -> bool {
    if !is_shutdown || !thread_has_unanswered_question(pool, thread_id).await {
        return false;
    }
    log_legacy!(
        "[Shutdown] {}: preserving session {}, parked on an unanswered question \
         (no interrupt, no terminal, no text flush)",
        site,
        thread_id
    );
    true
}
