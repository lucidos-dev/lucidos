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
/// [`crate::engine::thread_events::ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES`].
///
/// A park is about the turn, not the card. A card Lucidos raised stays open
/// after its turn ends ([`open_posted_card_sql`]), but no turn is parked on it.
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
    format!(
        "EXISTS ( SELECT 1 FROM events p WHERE p.aggregate_id = {id_expr} AND {} )",
        unresolved_permission_prompt_sql("p")
    )
}

/// SQL over one `events {alias}` row: an agent permission prompt nobody
/// resolved.
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

/// SQL over one `events {alias}` card row: Lucidos raised the card itself, as
/// a *permission request* no agent prepared. No agent call waits on it, so no
/// turn's end can close it.
pub fn engine_raised_card_sql(alias: &str) -> String {
    format!(
        "({alias}.payload ? 'permission_request' \
          AND NOT {alias}.payload->'permission_request' ? 'prepared')"
    )
}

/// SQL over one `events {alias}` card row: the card may not post while a turn
/// runs. Lucidos raised it, and no turn holds for its answer. The SQL twin of
/// `PermissionRequest::waits_for_turn_end`.
pub fn card_waits_for_turn_end_sql(alias: &str) -> String {
    format!(
        "({engine_raised} AND COALESCE(({alias}.payload->'permission_request'->>'holds_a_turn')::boolean, \
          false) = false)",
        engine_raised = engine_raised_card_sql(alias),
    )
}

/// SQL over one `events {alias}` row: a posted card that is still *open*.
///
/// This is the single definition of an open posted card (ADR 0803):
///
/// - A card Lucidos raised stays open until its own answer. A retirement is
///   an answer too, of kind `Superseded`.
/// - An agent's card stays open until its answer, its own tool result, or
///   its turn's end.
pub fn open_posted_card_sql(alias: &str, id_expr: &str) -> String {
    open_posted_card_before_sql(alias, id_expr, "")
}

/// [`open_posted_card_sql`] as it stood just before sequence `before_expr`.
/// The card hold reads it to find the card an event closed first.
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

/// What kind of card a thread's *open card* is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenCardKind {
    /// The agent's own question. Its call waits for the answer.
    AgentQuestion,
    /// A *permission request* the agent prepared. Its call waits too.
    PreparedRequest,
    /// A *permission request* Lucidos raised itself.
    EngineRequest,
    /// An agent permission prompt, such as a command approval.
    PermissionPrompt,
}

impl OpenCardKind {
    /// Whether a typed message waits behind this card.
    pub fn holds_typed_messages(self) -> bool {
        self != Self::AgentQuestion
    }

    /// Whether the agent asked this card itself, so its other tool calls wait
    /// for the answer.
    pub fn asked_by_the_agent(self) -> bool {
        matches!(self, Self::AgentQuestion | Self::PreparedRequest)
    }
}

/// What an agent reads when a tool call is refused while its own question is
/// open (ADR 0803). The Claude Code hook carries it in a single-quoted shell
/// word, so it holds no single quote.
pub const OPEN_QUESTION_TOOL_REFUSAL: &str = "Your question is still open, so this call did \
     not run. The answer comes back as the result of your question tool. Make this call again \
     after that.";

/// Whether the agent's own question is open on `thread_id`, so its other tool
/// calls are refused. Any open card counts, not only the first.
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

/// A thread's *open card*: the oldest card nobody answered yet. Every other
/// open card waits behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenCard {
    /// The card's `tool_use_id`, or a prompt's `request_id`.
    pub id: String,
    pub sequence: i64,
    pub kind: OpenCardKind,
}

/// Every open card on `$1` as `(id, sequence, kind)` rows, with `fence`
/// narrowing the rows of both arms by `c.sequence`.
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

/// Every open card on `thread_id`, oldest first. The first is the thread's
/// *open card*. More than one exists only on a thread from before ADR 0803.
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

/// The thread's *open card*, if any.
pub async fn open_card(
    conn: &mut sqlx::PgConnection,
    thread_id: Uuid,
) -> Result<Option<OpenCard>, sqlx::Error> {
    Ok(open_cards(conn, thread_id).await?.into_iter().next())
}

/// Whether a permission card prevents dispatch of typed inputs.
///
/// A card is open by the same rule as the card slot and the step dedup.
/// A stricter rule here would hold input behind a card that the slot,
/// the dedup and the app all read as closed. Nothing could then free it.
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
    sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM ({}) o WHERE o.kind <> 'agent_question')",
        open_card_rows_sql("AND ($2::bigint IS NULL OR c.sequence < $2)")
    ))
    .bind(thread_id.to_string())
    .bind(before)
    .fetch_one(conn)
    .await
}

/// SQL `EXISTS(...)`: the thread has an *open card*. That is an open posted
/// card, or an unresolved agent permission prompt. The single definition
/// behind "one open card per thread": the EventBus gate and the card queue
/// both read it.
pub fn open_card_exists_sql(id_expr: &str) -> String {
    format!(
        "(EXISTS (SELECT 1 FROM events oc WHERE {}) OR {})",
        open_posted_card_sql("oc", id_expr),
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

/// Whether the posted card `tool_use_id` is open but waits behind an older
/// open card. The owner answers cards one at a time, oldest first.
pub async fn card_waits_behind_older(
    conn: &mut sqlx::PgConnection,
    thread_id: Uuid,
    tool_use_id: &str,
) -> Result<bool, sqlx::Error> {
    let cards = open_cards(conn, thread_id).await?;
    Ok(cards.iter().skip(1).any(|card| card.id == tool_use_id))
}

/// The park predicate itself, over one `events uqa` row.
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
