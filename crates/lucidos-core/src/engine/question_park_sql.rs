use uuid::Uuid;

pub fn unanswered_question_exists_sql(id_expr: &str) -> String {
    format!(
        "EXISTS ( SELECT 1 FROM events uqa WHERE {} )",
        unanswered_question_predicate_sql(id_expr),
    )
}

pub const PERMISSION_PROMPT_EVENT_TYPES: [(&str, &str); 3] = [
    (
        "CodingAgentPermissionRequest",
        "CodingAgentPermissionResolved",
    ),
    ("CommandPermissionRequested", "CommandPermissionResolved"),
    ("McpPermissionRequested", "McpPermissionResolved"),
];

pub fn unresolved_permission_prompt_exists_sql(id_expr: &str) -> String {
    format!(
        "EXISTS ( SELECT 1 FROM events p WHERE p.aggregate_id = {id_expr} AND {} )",
        unresolved_permission_prompt_sql("p")
    )
}

fn unresolved_permission_prompt_sql(alias: &str) -> String {
    let arms = PERMISSION_PROMPT_EVENT_TYPES
        .map(|(request, resolved)| {
            format!(
                "({alias}.event_type = '{request}' AND NOT EXISTS ( \
                   SELECT 1 FROM events r WHERE r.event_type = '{resolved}' \
                   AND r.payload->>'request_id' = {alias}.payload->>'request_id'))"
            )
        })
        .join(" OR ");
    format!("({arms})")
}

pub fn engine_raised_card_sql(alias: &str) -> String {
    format!(
        "({alias}.payload ? 'permission_request' \
          AND NOT {alias}.payload->'permission_request' ? 'prepared')"
    )
}

pub fn card_waits_for_turn_end_sql(alias: &str) -> String {
    format!(
        "({engine_raised} AND COALESCE(({alias}.payload->'permission_request'->>'holds_a_turn')::boolean, \
          false) = false)",
        engine_raised = engine_raised_card_sql(alias),
    )
}

pub fn open_posted_card_sql(alias: &str, id_expr: &str) -> String {
    open_posted_card_before_sql(alias, id_expr, "")
}

pub fn open_posted_card_before_sql(alias: &str, id_expr: &str, before_expr: &str) -> String {
    let (posted, fence) = if before_expr.is_empty() {
        (String::new(), String::new())
    } else {
        (
            format!("AND {alias}.sequence < {before_expr}"),
            format!("AND later.sequence < {before_expr}"),
        )
    };
    format!(
        "({alias}.aggregate_id = {id_expr} AND {alias}.event_type = 'UserQuestionAsked' {posted} \
          AND NOT EXISTS (SELECT 1 FROM events later \
            WHERE later.aggregate_id = {id_expr} AND later.sequence > {alias}.sequence {fence} \
              AND (CASE WHEN {engine_raised} \
                     THEN (later.event_type = 'UserQuestionAnswered' \
                           AND later.payload->>'tool_use_id' = {alias}.payload->>'tool_use_id') \
                     ELSE {closes} END)))",
        engine_raised = engine_raised_card_sql(alias),
        closes = closes_question_sql(alias, "later"),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCardKind {
    AgentQuestion,
    PreparedRequest,
    EngineRequest,
    PermissionPrompt,
}

impl OpenCardKind {
    pub fn holds_typed_messages(self) -> bool {
        self != Self::AgentQuestion
    }

    pub fn asked_by_the_agent(self) -> bool {
        matches!(self, Self::AgentQuestion | Self::PreparedRequest)
    }
}

pub const OPEN_QUESTION_TOOL_REFUSAL: &str = "Your question is still open, so this call did \
     not run. The answer comes back as the result of your question tool. Make this call again \
     after that.";

pub async fn agent_question_open(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let mut conn = pool.acquire().await?;
    Ok(open_cards(&mut conn, thread_id)
        .await?
        .iter()
        .any(|card| card.kind.asked_by_the_agent()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCard {
    pub id: String,
    pub sequence: i64,
    pub kind: OpenCardKind,
}

fn open_card_rows_sql(fence: &str) -> String {
    format!(
        "SELECT c.payload->>'tool_use_id' AS id, c.sequence, \
           CASE WHEN {engine_raised} THEN 'engine_request' \
                WHEN c.payload ? 'permission_request' THEN 'prepared_request' \
                ELSE 'agent_question' END AS kind \
         FROM events c WHERE {posted} {fence} \
         UNION ALL \
         SELECT c.payload->>'request_id', c.sequence, 'permission_prompt' \
         FROM events c WHERE c.aggregate_id = $1 {fence} AND {prompts}",
        engine_raised = engine_raised_card_sql("c"),
        posted = open_posted_card_sql("c", "$1"),
        prompts = unresolved_permission_prompt_sql("c"),
    )
}

fn open_card_kind(wire: &str) -> OpenCardKind {
    match wire {
        "engine_request" => OpenCardKind::EngineRequest,
        "prepared_request" => OpenCardKind::PreparedRequest,
        "permission_prompt" => OpenCardKind::PermissionPrompt,
        _ => OpenCardKind::AgentQuestion,
    }
}

pub async fn open_cards(
    conn: &mut sqlx::PgConnection,
    thread_id: Uuid,
) -> Result<Vec<OpenCard>, sqlx::Error> {
    let rows: Vec<(Option<String>, i64, String)> = sqlx::query_as(&format!(
        "SELECT id, sequence, kind FROM ({}) o ORDER BY sequence",
        open_card_rows_sql("")
    ))
    .bind(thread_id.to_string())
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, sequence, kind)| OpenCard {
            id: id.unwrap_or_default(),
            sequence,
            kind: open_card_kind(&kind),
        })
        .collect())
}

pub async fn open_card(
    conn: &mut sqlx::PgConnection,
    thread_id: Uuid,
) -> Result<Option<OpenCard>, sqlx::Error> {
    Ok(open_cards(conn, thread_id).await?.into_iter().next())
}

pub async fn open_permission_card_exists(
    conn: &mut sqlx::PgConnection,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    open_permission_card_before(conn, thread_id, None).await
}

pub async fn open_permission_card_before(
    conn: &mut sqlx::PgConnection,
    thread_id: Uuid,
    before: Option<i64>,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM ({}) o WHERE o.kind <> 'agent_question')",
        open_card_rows_sql("AND ($2::bigint IS NULL OR c.sequence < $2)")
    ))
    .bind(thread_id.to_string())
    .bind(before)
    .fetch_one(conn)
    .await
}

pub fn open_card_exists_sql(id_expr: &str) -> String {
    format!(
        "(EXISTS (SELECT 1 FROM events oc WHERE {}) OR {})",
        open_posted_card_sql("oc", id_expr),
        unresolved_permission_prompt_exists_sql(id_expr)
    )
}

/// INVARIANT: Whether `thread_id` has an *open card*, read on `conn`. Pass the append
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

pub async fn card_waits_behind_older(
    conn: &mut sqlx::PgConnection,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<bool, sqlx::Error> {
    let cards = open_cards(conn, thread_id).await?;
    Ok(cards.iter().skip(1).any(|card| card.id == tool_use_id))
}

fn unanswered_question_predicate_sql(id_expr: &str) -> String {
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

const TURN_ENDED_EVENT_TYPES: &[&str] = &["ResponseGenerated", "SessionEnded"];

fn turn_moved_on_event_types() -> impl Iterator<Item = &'static str> {
    crate::engine::thread_events::ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES
        .iter()
        .chain(TURN_ENDED_EVENT_TYPES.iter())
        .copied()
}

fn park_ending_event_types() -> impl Iterator<Item = &'static str> {
    turn_moved_on_event_types().chain(std::iter::once("UserQuestionAnswered"))
}

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

pub const CARD_EVENT_TYPES_SQL: &str = "'UserQuestionAsked','UserQuestionQueued'";

pub fn waiting_queued_card_sql(alias: &str) -> String {
    format!(
        "({alias}.event_type = 'UserQuestionQueued' \
          AND NOT EXISTS (SELECT 1 FROM events settled \
            WHERE settled.aggregate_id = {alias}.aggregate_id \
              AND settled.event_type IN ('UserQuestionAsked', 'UserQuestionAnswered') \
              AND settled.payload->>'tool_use_id' = {alias}.payload->>'tool_use_id'))"
    )
}

/// INVARIANT: Whether an event of this type ends the thread's question park whatever
/// question is open. A `CodingAgentToolResult` is absent: it ends the park only
/// when it pairs with the question's own `tool_use_id`. For a consumer that
/// follows events one by one.
pub fn ends_question_park(event_type: &str) -> bool {
    park_ending_event_types().any(|t| t == event_type)
}

/// INVARIANT: True when the thread's most recent `UserQuestionAsked` has no later answer,
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

/// INVARIANT: [`thread_has_unanswered_question`] for a caller that must not read a failed
/// lookup as "not parked". Apply Now refuses on an `Err`, because applying over
/// a live question card is the direction that loses the user's answer.
pub async fn thread_parked_on_question(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    let sql = format!("SELECT {}", unanswered_question_exists_sql("$1"));
    sqlx::query_scalar::<_, bool>(&sql)
        .bind(thread_id.to_string())
        .fetch_one(pool)
        .await
}

/// INVARIANT: True when an engine teardown must leave this thread exactly as it is because
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
