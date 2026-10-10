//! The *read decision* turn-end requirement: a turn that ended without one is
//! decided by one forced auxiliary call (ADR 0417).
//!
//! Every thread kind takes the same path. An auxiliary model reads the turn's
//! opening message and its reply, with `request_read` as its only tool, forced.
//! The reply is never changed. A call that fails records a read request, so
//! the error stays visible in Review rather than hiding a report.

use uuid::Uuid;

use crate::engine::aux_purpose::AuxCall;
use crate::engine::model_call::AuxCapture;
use crate::engine::read_request::{record_read_decision, ReadDecision};
use crate::engine::ContextPurpose;
use crate::llm::provider::{Message, MessageContent};
use crate::llm::tool_names::REQUEST_READ;
use crate::llm::ModelSelection;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// What the forced call is told about its one job.
const READ_DECISION_SYSTEM_PROMPT: &str = "You decide whether the user should \
    read an agent's reply. Call request_read once. read: true when the reply holds what the \
    user asked for (a report, findings, research, an answer), when they said they would read \
    it later, or when a scheduled run found something. read: false for small talk, an \
    acknowledgement, a plain \"done\", a reply that only says a change is ready, or a run \
    that found nothing.";

/// The most bytes of the opening message the call reads.
const MESSAGE_BYTES: usize = 4_000;
/// The most bytes of the reply the call reads.
const REPLY_BYTES: usize = 8_000;

/// One turn, as the read decision sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TurnRead {
    /// Whether the turn already recorded a read decision.
    pub(crate) decided: bool,
    /// The message that opened the turn.
    pub(crate) message: String,
    /// The turn's final reply.
    pub(crate) reply: String,
}

/// The text of a turn's opening event: a message, a prompt, or a finished
/// child's summary.
const OPENING_TEXT_SQL: &str =
    "COALESCE(payload->>'text', payload->>'prompt', payload->>'summary', '')";

/// The events that can open a coding-agent turn: every originating event, and
/// an engine prompt such as a re-entry, an event-wait delivery or a resume.
fn coding_agent_turn_openers() -> Vec<&'static str> {
    crate::engine::agent_session::resume::CC_ORIGINATING_EVENT_TYPES
        .iter()
        .copied()
        .chain(["PromptInjected", "CodingAgentPromptSent"])
        .collect()
}

/// Read a coding-agent turn from its events. The turn is everything since the
/// previous `CodingAgentIdled`, so a follow-up that lands mid-turn moves
/// nothing: its decision, its first opener, and its latest reply.
pub(crate) async fn coding_agent_turn_read(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<TurnRead, sqlx::Error> {
    let (message, decided, reply): (String, bool, Option<String>) =
        sqlx::query_as(&format!(
            "WITH turn_start AS ( \
               SELECT COALESCE(MAX(sequence), -1) AS seq FROM events \
                WHERE aggregate = 'thread' AND aggregate_id = $1 \
                  AND event_type = 'CodingAgentIdled') \
             SELECT COALESCE((SELECT {OPENING_TEXT_SQL} FROM events e \
                      WHERE e.aggregate = 'thread' AND e.aggregate_id = $1 \
                        AND e.event_type = ANY($2) AND e.sequence > turn_start.seq \
                      ORDER BY e.sequence ASC LIMIT 1), ''), \
                    EXISTS (SELECT 1 FROM events e \
                             WHERE e.aggregate = 'thread' AND e.aggregate_id = $1 \
                               AND e.event_type IN ('ThreadReadRequested', 'ThreadReadNotRequested') \
                               AND e.sequence > turn_start.seq), \
                    (SELECT e.payload->>'text' FROM events e \
                      WHERE e.aggregate = 'thread' AND e.aggregate_id = $1 \
                        AND e.event_type = 'ResponseGenerated' AND e.sequence > turn_start.seq \
                      ORDER BY e.sequence DESC LIMIT 1) \
               FROM turn_start"
        ))
        .bind(thread_id.to_string())
        .bind(coding_agent_turn_openers())
        .fetch_one(pool)
        .await?;
    Ok(TurnRead {
        decided,
        message,
        reply: reply.unwrap_or_default(),
    })
}

/// The forced decision for one undecided turn, on `call`'s model.
pub(crate) struct ForcedReadDecision<'a> {
    pub(crate) call: &'a AuxCall,
    pub(crate) capture: &'a AuxCapture,
}

impl ForcedReadDecision<'_> {
    /// Ask the model, forced to call `request_read`. An empty reply has
    /// nothing to read, so it is a no without a call.
    pub(crate) async fn decide(
        &self,
        message: &str,
        reply: &str,
    ) -> Result<ReadDecision, BoxError> {
        if reply.trim().is_empty() {
            return Ok(ReadDecision::NotRequested);
        }
        let prompt = format!(
            "The user's message:\n{}\n\nThe agent's reply:\n{}",
            crate::core::middle_truncate(message, MESSAGE_BYTES),
            crate::core::middle_truncate(reply, REPLY_BYTES),
        );
        let messages = vec![Message {
            role: "user".into(),
            content: MessageContent::Text(prompt),
        }];
        let selection = ModelSelection::default()
            .with_effort(self.call.reasoning())
            .with_forced_tool(Some(REQUEST_READ));
        let deadline = tokio::time::Instant::now() + self.call.deadline();
        let response = self
            .capture
            .until(deadline)
            .chat(
                self.call.provider().as_ref(),
                messages,
                vec![crate::llm::tools::request_read_tool()],
                selection,
                Some(READ_DECISION_SYSTEM_PROMPT),
                None,
            )
            .await?;
        let call = response
            .tool_calls
            .iter()
            .find(|c| c.name == REQUEST_READ)
            .ok_or("the forced call answered without calling request_read")?;
        Ok(ReadDecision::from_tool_args(&call.arguments)?)
    }
}

impl crate::engine::LucidosEngine {
    /// Make the read decision a turn ended without, and record it in the
    /// engine's name. A failed call records a read request, never a no.
    pub(crate) async fn force_read_decision(
        &self,
        thread_id: Uuid,
        message: &str,
        reply: &str,
    ) -> ReadDecision {
        let call = self.aux_call(ContextPurpose::ReadDecision).await;
        let capture = AuxCapture::new(&self.event_bus, thread_id, ContextPurpose::ReadDecision);
        let forced = ForcedReadDecision {
            call: &call,
            capture: &capture,
        };
        record_forced_decision(
            &self.event_bus,
            thread_id,
            forced.decide(message, reply).await,
        )
        .await
    }
}

/// Record a forced decision, or a read request in its place when the call
/// failed. Returns what was recorded.
pub(crate) async fn record_forced_decision(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    outcome: Result<ReadDecision, BoxError>,
) -> ReadDecision {
    let decision = outcome.unwrap_or_else(|e| {
        crate::log!(
            "[TurnEnd] Read decision for thread {} failed, so it asks to be read: {}",
            thread_id,
            e
        );
        ReadDecision::Requested
    });
    if let Err(e) = record_read_decision(bus, thread_id, decision, None).await {
        crate::log!(
            "[TurnEnd] Recording the read decision for thread {} failed: {}",
            thread_id,
            e
        );
    }
    decision
}

/// The text of the event that opened a chat or trigger turn: a message, or a
/// trigger's prompt.
async fn opening_text(pool: &sqlx::PgPool, origin_id: Uuid) -> String {
    sqlx::query_scalar::<_, String>(&format!(
        "SELECT {OPENING_TEXT_SQL} FROM events WHERE id = $1"
    ))
    .bind(origin_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log!("[TurnEnd] Opening event {} unreadable: {}", origin_id, e);
        None
    })
    .unwrap_or_default()
}

impl crate::engine::LucidosEngine {
    /// The chat turn end's gate, just before `ResponseGenerated`: force the
    /// read decision when the turn made none. `reply` is the draft the turn
    /// ends on, and it stays the reply.
    pub(crate) async fn gate_chat_turn_end(
        &self,
        thread_id: Uuid,
        origin_id: Uuid,
        read_decided: bool,
        reply: &str,
    ) {
        if !read_decided {
            let message = opening_text(&self.pool, origin_id).await;
            self.force_read_decision(thread_id, &message, reply).await;
        }
    }

    /// The coding-agent turn end's gate, before `CodingAgentIdled`: force the
    /// read decision when the turn made none. A coding agent cannot be forced
    /// inside its own session, so the decision reads the turn's events.
    pub(crate) async fn gate_coding_agent_read_decision(&self, thread_id: Uuid) {
        let turn = match coding_agent_turn_read(&self.pool, thread_id).await {
            Ok(turn) => turn,
            Err(e) => {
                let unread = format!("the turn's events were unreadable: {e}");
                record_forced_decision(&self.event_bus, thread_id, Err(unread.into())).await;
                return;
            }
        };
        if !turn.decided {
            self.force_read_decision(thread_id, &turn.message, &turn.reply)
                .await;
        }
    }
}
