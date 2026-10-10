//! The *read decision* turn-end requirement: a turn that ended without one is
//! decided by one forced auxiliary call (ADR 0417).
//!
//! Every thread kind takes the same path. An auxiliary model reads the turn's
//! opening message, labelled by who sent it, and its reply, with `request_read`
//! as its only tool, forced.
//! The reply is never changed. A call that fails records a read request, so
//! the error stays visible in Review rather than hiding a report. A coding-agent
//! turn whose change already lists the thread is a no, with no call (ADR 0421).

use uuid::Uuid;

use crate::engine::aux_purpose::AuxCall;
use crate::engine::model_call::AuxCapture;
use crate::engine::read_request::{record_read_decision, ReadDecision};
use crate::engine::thread_events::ActorMode;
use crate::engine::ContextPurpose;
use crate::llm::provider::{Message, MessageContent};
use crate::llm::tool_names::REQUEST_READ;
use crate::llm::ModelSelection;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// What the forced call is told about its one job.
const READ_DECISION_SYSTEM_PROMPT: &str = "You decide whether the user should \
    read an agent's reply. Call request_read once. A heading says what opened the turn: the \
    user's message, a child thread's report, a trigger run, or another agent's prompt. A report \
    or prompt that opened the turn is not the user asking for a report. read: true when the \
    reply hands the user something to read (findings, research, an answer, what a scheduled run \
    found), or when the user said they would read it later. read: false for small talk, an \
    acknowledgement, a plain \"done\", a reply that only says a change is ready or applied \
    (even with a step to pick it up, such as switching to the new version), or a run that found \
    nothing.";

/// The most bytes of the opening message the call reads.
const MESSAGE_BYTES: usize = 4_000;
/// The most bytes of the reply the call reads.
const REPLY_BYTES: usize = 8_000;

/// Who the text that opened a turn speaks for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum OpeningSource {
    #[default]
    UserMessage,
    ChildThreadReport,
    TriggerRun,
    AgentOrEngine,
}

impl OpeningSource {
    /// The source of an opening event, from its type and its sender's mode.
    pub(crate) fn from_event(event_type: &str, mode: Option<ActorMode>) -> Self {
        match (event_type, mode) {
            ("ChildThreadCompleted", _) => Self::ChildThreadReport,
            ("TriggerStarted", _) => Self::TriggerRun,
            (_, Some(ActorMode::Agent | ActorMode::Engine)) => Self::AgentOrEngine,
            _ => Self::UserMessage,
        }
    }

    /// Read the columns an opener query selects. An unknown mode reads as a
    /// human, and says so in the log.
    fn from_columns(event_type: Option<&str>, mode: Option<String>) -> Self {
        let mode = mode.and_then(|m| {
            serde_json::from_value(serde_json::Value::String(m.clone()))
                .inspect_err(|e| crate::log!("[TurnEnd] Opener mode {:?} unknown: {}", m, e))
                .ok()
        });
        Self::from_event(event_type.unwrap_or_default(), mode)
    }

    fn heading(self) -> &'static str {
        match self {
            Self::UserMessage => "The user's message",
            Self::ChildThreadReport => {
                "A finished child thread's report to the agent, not from the user"
            }
            Self::TriggerRun => "The trigger's instruction for this run, not from the user",
            Self::AgentOrEngine => "A prompt from another agent or the engine, not from the user",
        }
    }
}

/// One turn, as the read decision sees it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TurnRead {
    /// Whether the turn already recorded a read decision.
    pub(crate) decided: bool,
    /// The message that opened the turn.
    pub(crate) message: String,
    /// Who sent the opening message.
    pub(crate) source: OpeningSource,
    /// The turn's final reply.
    pub(crate) reply: String,
    /// Whether the thread's change already lists it, in Review or Blocked, so
    /// a read request would only outlive the change. The database function
    /// `change_replaces_read_request` defines it (ADR 0421).
    pub(crate) change_replaces_read_request: bool,
}

/// A turn's opening event as three columns: its text (a message, a prompt, or
/// a finished child's summary), its type, and its sender's mode. An engine
/// prompt to a coding agent carries no mode, only an engine origin.
const OPENING_COLUMNS_SQL: &str = "COALESCE(payload->>'text', payload->>'prompt', \
     payload->>'summary', '') AS text, event_type, \
     COALESCE(payload->>'mode', \
              CASE WHEN payload->'origin'->>'kind' = 'engine' THEN 'engine' END) AS mode";

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
    let (message, event_type, mode, decided, reply, change_replaces_read_request): (
        String,
        Option<String>,
        Option<String>,
        bool,
        Option<String>,
        bool,
    ) =
        sqlx::query_as(&format!(
            "WITH turn_start AS ( \
               SELECT COALESCE(MAX(sequence), -1) AS seq FROM events \
                WHERE aggregate = 'thread' AND aggregate_id = $1 \
                  AND event_type = 'CodingAgentIdled'), \
                  opener AS ( \
               SELECT {OPENING_COLUMNS_SQL} FROM events e, turn_start \
                WHERE e.aggregate = 'thread' AND e.aggregate_id = $1 \
                  AND e.event_type = ANY($2) AND e.sequence > turn_start.seq \
                ORDER BY e.sequence ASC LIMIT 1) \
             SELECT COALESCE(opener.text, ''), opener.event_type, opener.mode, \
                    EXISTS (SELECT 1 FROM events e \
                             WHERE e.aggregate = 'thread' AND e.aggregate_id = $1 \
                               AND e.event_type IN ('ThreadReadRequested', 'ThreadReadNotRequested') \
                               AND e.sequence > turn_start.seq), \
                    (SELECT e.payload->>'text' FROM events e \
                      WHERE e.aggregate = 'thread' AND e.aggregate_id = $1 \
                        AND e.event_type = 'ResponseGenerated' AND e.sequence > turn_start.seq \
                      ORDER BY e.sequence DESC LIMIT 1), \
                    COALESCE((SELECT change_replaces_read_request( \
                                       t.coding_agent_change_state, t.coding_agent_unproposed_reason) \
                                FROM thread_summaries t WHERE t.thread_id = $1::uuid), FALSE) \
               FROM turn_start LEFT JOIN opener ON TRUE"
        ))
        .bind(thread_id.to_string())
        .bind(coding_agent_turn_openers())
        .fetch_one(pool)
        .await?;
    Ok(TurnRead {
        decided,
        message,
        source: OpeningSource::from_columns(event_type.as_deref(), mode),
        reply: reply.unwrap_or_default(),
        change_replaces_read_request,
    })
}

/// The forced decision for one undecided turn, on `call`'s model.
pub(crate) struct ForcedReadDecision<'a> {
    pub(crate) call: &'a AuxCall,
    pub(crate) capture: &'a AuxCapture,
}

/// What the forced call reads: the opening message under a heading naming its
/// sender, then the reply.
pub(crate) fn read_decision_prompt(turn: &TurnRead) -> String {
    format!(
        "{}:\n{}\n\nThe agent's reply:\n{}",
        turn.source.heading(),
        crate::core::middle_truncate(&turn.message, MESSAGE_BYTES),
        crate::core::middle_truncate(&turn.reply, REPLY_BYTES),
    )
}

impl ForcedReadDecision<'_> {
    /// Decide one turn, forced to call `request_read`. A thread whose change
    /// already lists it, or an empty reply, is a no without a call.
    pub(crate) async fn decide(&self, turn: &TurnRead) -> Result<ReadDecision, BoxError> {
        if turn.change_replaces_read_request || turn.reply.trim().is_empty() {
            return Ok(ReadDecision::NotRequested);
        }
        let messages = vec![Message {
            role: "user".into(),
            content: MessageContent::Text(read_decision_prompt(turn)),
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
        turn: &TurnRead,
    ) -> ReadDecision {
        let call = self.aux_call(ContextPurpose::ReadDecision).await;
        let capture = AuxCapture::new(&self.event_bus, thread_id, ContextPurpose::ReadDecision);
        let forced = ForcedReadDecision {
            call: &call,
            capture: &capture,
        };
        record_forced_decision(&self.event_bus, thread_id, forced.decide(turn).await).await
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

/// The event that opened a chat or trigger turn: its text, and who sent it.
async fn opening(pool: &sqlx::PgPool, origin_id: Uuid) -> (String, OpeningSource) {
    sqlx::query_as::<_, (String, String, Option<String>)>(&format!(
        "SELECT {OPENING_COLUMNS_SQL} FROM events WHERE id = $1"
    ))
    .bind(origin_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log!("[TurnEnd] Opening event {} unreadable: {}", origin_id, e);
        None
    })
    .map(|(text, event_type, mode)| (text, OpeningSource::from_columns(Some(&event_type), mode)))
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
            let (message, source) = opening(&self.pool, origin_id).await;
            let turn = TurnRead {
                message,
                source,
                reply: reply.to_string(),
                ..TurnRead::default()
            };
            self.force_read_decision(thread_id, &turn).await;
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
            self.force_read_decision(thread_id, &turn).await;
        }
    }
}
