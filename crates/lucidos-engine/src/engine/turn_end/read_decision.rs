//! The *read decision* turn-end requirement: a turn that ended without one is
//! decided by one judgment call (ADR 0417), at `JudgmentSite::ReadDecision`.
//!
//! Every thread kind takes the same path. One yes/no question reads the turn's
//! opening message, labelled by who sent it, and its reply. A question the
//! user answered inside the turn shows between the two. A turn that ends with
//! work still in flight says so, so a progress report reads as one (ADR 0424).
//! An agent-opened turn says when its report wakes the parent agent, so a
//! routine report to that agent reads as one (ADR 0446). A turn that already
//! notified the user names the notification, and the notification replaces
//! the read unless the reply holds a failure it does not mention.
//!
//! The site's chat model answers, unless the user picked a System One row
//! (ADR 0472). The reply is never changed. A failed call or a missing answer
//! records a read request, so a report never hides. A coding-agent turn whose
//! change already lists the thread is a no, with no call (ADR 0421).

use std::time::Duration;

use uuid::Uuid;

use super::wake_decision::{OwedReport, TurnFinish};
use crate::engine::model_call::AuxCapture;
use crate::engine::read_request::{record_read_decision, ReadDecision};
use crate::engine::thread_events::{ActorMode, AnswerKind, ChildCompletionStatus, QuestionOption};
use crate::engine::ContextPurpose;
use crate::llm::judgment::{
    for_site, Answers, ChatJudgmentProvider, JudgmentProvider, JudgmentSite, NoulCriteria, Question,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The id of the one question. A chat model sees it as its answer's key.
pub(crate) const READ_QUESTION_ID: &str = "read";

/// The probability of yes at and above which the turn asks to be read.
///
/// A missed must-read costs more than a reply read for nothing, so a tie reads.
pub(crate) const READ_THRESHOLD: f64 = 0.5;

/// What the question asks. The turn arrives as the state.
pub(crate) const READ_DECISION_INSTRUCTIONS: &str = "Should the user read the agent's reply? \
    The state is one turn.\n\n\
    A heading says what opened the turn: the user's message, a child thread's report, a trigger \
    run, or another agent's prompt. A report or prompt that opened the turn is not the user \
    asking for a report. When \"The user answered\" follows it, that answer resumed the turn, \
    and the reply speaks to it.\n\n\
    Ask one question: does the reply hand the user something new they would miss if they never \
    opened it?\n\n\
    A trigger run's reply is worth reading only when the run found something the user does not \
    know yet: a new change, finding, failure or decision. A new problem the user must fix \
    counts, even inside a reply that says nothing needs them. A run that found nothing new is not \
    worth reading, however much it lists what it checked. That holds even when it repeats items, \
    deadlines or standing problems from earlier runs, or a block only the user can clear that \
    has not changed. When the trigger's instruction says to stay quiet in that case, that \
    settles it. A run whose instruction asks for a report on every run, such as a briefing, a \
    summary or tonight's plan, is worth reading even when much of it repeats earlier runs: that \
    report is what the user asked to read.\n\n\
    When a closing note says the thread's work is still in flight, a progress report (which \
    step finished, what started, what it waits on) is not worth reading. It is worth reading \
    only when the reply holds something the user should read before the work finishes, such as \
    a failure, a finding or a decision.\n\n\
    When a closing note says the parent agent acts on this reply now, a routine result for it \
    is not worth the user's read. It is still worth reading when it holds a failure, a gap the \
    user must know about, or something only the user can do or decide, such as a payment, a \
    sign-in or an approval. The parent agent seeing it does not make it the parent's to settle.\n\n\
    When a closing note says the turn already sent the user a notification, the reply is not \
    worth reading. The notification is how the user hears, and they do not need both. The one \
    exception is a failure the notification does not mention.";

/// What a yes means.
const READ_YES: &str = "The reply holds findings, research, an answer to the user's question, \
    a draft they asked for, what a scheduled run found that the user does not have yet, a \
    failure, or a decision only the user can make. Also yes when the user said they would read \
    it later.";

/// What a no means.
const READ_NO: &str = "The reply only confirms or carries out what the user just said or \
    picked. Also no for small talk, an acknowledgement, a plain \"done\", a run that found \
    nothing, and a kickoff that only says work started and will report back. Also no for a \
    reply about a change that is ready, applied or merged, even when it explains what was \
    wrong, counts commits and files, or names a step such as a restart. Such a reply is a yes \
    only when it also holds a failure, a finding beyond the change, or a decision for the user. \
    Also no when a closing note says the turn already sent the user a notification, unless the \
    reply holds a failure the notification does not mention.";

/// The most bytes of the opening message the call reads.
const MESSAGE_BYTES: usize = 4_000;
/// The most bytes of the reply the call reads.
const REPLY_BYTES: usize = 8_000;
/// The most bytes of a push's title the call reads.
const PUSH_TITLE_BYTES: usize = 200;
/// The most bytes of a push's message the call reads.
pub(crate) const PUSH_MESSAGE_BYTES: usize = 1_500;

/// Who the text that opened a turn speaks for.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OpeningSource {
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

    pub(crate) fn heading(self) -> &'static str {
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
pub struct TurnRead {
    /// Whether the turn already recorded a read decision.
    pub decided: bool,
    /// The message that opened the turn.
    pub message: String,
    /// Who sent the opening message.
    pub source: OpeningSource,
    /// The user's latest answer inside the turn. It resumed the turn, so the
    /// reply speaks to it rather than to the opener.
    pub answered: Option<AnsweredQuestion>,
    /// The turn's final reply.
    pub reply: String,
    /// Whether the thread's change already lists it, in Review or Blocked, so
    /// a read request would only outlive the change. The database function
    /// `change_replaces_read_request` defines it (ADR 0421).
    pub change_replaces_read_request: bool,
    /// What the thread still waits on as the turn ends.
    pub in_flight: InFlight,
    /// Whether this turn's report wakes the parent agent, which then acts on
    /// it. The prompt says so only for a turn the user did not open or resume.
    pub report_wakes_parent: bool,
    /// The notifications the turn delivered before its reply. A notification
    /// replaces the read, unless the reply holds a failure it does not mention.
    pub pushed: Vec<PushedNotification>,
}

impl TurnRead {
    /// Whether the user opened this turn, or resumed it with an answer, so the
    /// reply speaks to them whoever else reads it.
    pub(crate) fn opened_by_the_user(&self) -> bool {
        self.source == OpeningSource::UserMessage || self.answered.is_some()
    }

    /// The closing note for a report the parent agent acts on now, or `None`.
    fn parent_note(&self) -> Option<&'static str> {
        (self.report_wakes_parent && !self.opened_by_the_user()).then_some(
            "Closing note: this reply is the thread's report to the parent agent that started \
             it, and the parent agent acts on it now.",
        )
    }
}

/// A question the agent asked inside the turn, and the user's answer as text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnsweredQuestion {
    pub question: String,
    pub answer: String,
}

impl AnsweredQuestion {
    /// The latest answered question on `thread_id` after event `after_sequence`.
    /// A cancelled or superseded card is no answer. An unreadable row reads as
    /// none, and the log records the error.
    async fn latest(pool: &sqlx::PgPool, thread_id: Uuid, after_sequence: i64) -> Option<Self> {
        type Row = (
            Option<String>,
            Option<sqlx::types::Json<Vec<QuestionOption>>>,
            sqlx::types::Json<AnswerKind>,
        );
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT q.payload->>'question', q.payload->'options', a.payload->'answer' \
               FROM events a \
               LEFT JOIN LATERAL ( \
                 SELECT payload FROM events q \
                  WHERE q.thread_id = a.thread_id AND q.event_type = 'UserQuestionAsked' \
                    AND q.payload->>'tool_use_id' = a.payload->>'tool_use_id' \
                  ORDER BY q.sequence DESC LIMIT 1) q ON TRUE \
              WHERE a.thread_id = $1 AND a.event_type = 'UserQuestionAnswered' \
                AND a.sequence > $2 \
              ORDER BY a.sequence DESC",
        )
        .bind(thread_id)
        .bind(after_sequence)
        .fetch_all(pool)
        .await
        .unwrap_or_else(|e| {
            crate::log!("[TurnEnd] Answers on {} unreadable: {}", thread_id, e);
            Vec::new()
        });
        rows.into_iter().find_map(|(question, options, answer)| {
            let options = options.map(|o| o.0).unwrap_or_default();
            Some(Self {
                answer: answer.0.text(&options)?,
                question: question.unwrap_or_default(),
            })
        })
    }
}

/// A `send_notification` the turn delivered: its title and message.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct PushedNotification {
    pub title: String,
    pub message: String,
}

impl PushedNotification {
    /// The notifications `thread_id` delivered after event `after_sequence`,
    /// oldest first. A call whose result failed reached nobody, so it is not
    /// one. An unreadable row reads as none, and the log records the error.
    async fn delivered(pool: &sqlx::PgPool, thread_id: Uuid, after_sequence: i64) -> Vec<Self> {
        sqlx::query_as::<_, (String, String)>(
            "SELECT COALESCE(c.payload->'args'->>'title', ''), \
                    COALESCE(c.payload->'args'->>'message', '') \
               FROM events c \
               JOIN events r ON r.thread_id = c.thread_id AND r.event_type = 'ToolResult' \
                AND r.payload->>'tool_called_event_id' = c.id::text \
                AND COALESCE((r.payload->>'success')::boolean, TRUE) \
              WHERE c.thread_id = $1 AND c.event_type = 'ToolCalled' \
                AND c.payload->>'name' = $3 AND c.sequence > $2 \
              ORDER BY c.sequence",
        )
        .bind(thread_id)
        .bind(after_sequence)
        .bind(crate::llm::tool_names::SEND_NOTIFICATION)
        .fetch_all(pool)
        .await
        .unwrap_or_else(|e| {
            crate::log!("[TurnEnd] Pushes on {} unreadable: {}", thread_id, e);
            Vec::new()
        })
        .into_iter()
        .map(|(title, message)| Self { title, message })
        .collect()
    }

    /// The closing note for what the user already got, or `None` without one.
    fn note(pushed: &[Self]) -> Option<String> {
        let what = match pushed.len() {
            0 => return None,
            1 => "this notification".to_string(),
            n => format!("these {n} notifications"),
        };
        let pushes: Vec<String> = pushed
            .iter()
            .map(|p| {
                format!(
                    "Title: {}\nMessage: {}",
                    crate::core::middle_truncate(&p.title, PUSH_TITLE_BYTES),
                    crate::core::middle_truncate(&p.message, PUSH_MESSAGE_BYTES),
                )
            })
            .collect();
        Some(format!(
            "Closing note: before this reply, the turn sent the user {what}, so they have \
             already seen it.\n{}",
            pushes.join("\n\n")
        ))
    }
}

/// What a thread still waits on when its turn ends: the facts behind the
/// drawer's Waiting dot. A turn that ends in flight is a step, not the end.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InFlight {
    /// Sub-threads still mid-turn or idling on their own event wait.
    pub sub_threads: i64,
    /// The thread's own live event waits.
    pub event_waits: i64,
}

/// The `InFlight` columns of a thread summary aliased `t`.
const IN_FLIGHT_COLUMNS_SQL: &str =
    "(t.active_children_count + t.waiting_children_count)::bigint, \
     t.live_event_wait_count::bigint";

impl InFlight {
    /// The closing note the decision reads, or `None` once nothing waits.
    fn note(self) -> Option<String> {
        let counted = |n: i64, noun: &str| match n {
            0 => None,
            1 => Some(format!("1 {noun}")),
            n => Some(format!("{n} {noun}s")),
        };
        let waits: Vec<String> = [
            counted(self.sub_threads, "sub-thread"),
            counted(self.event_waits, "event wait"),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!waits.is_empty()).then(|| {
            format!(
                "Closing note: the thread's work is still in flight. It waits on {}.",
                waits.join(" and ")
            )
        })
    }

    /// Read a thread's summary. An unreadable row reads as settled: the prompt
    /// gets no closing note, and the log records the error.
    async fn of_thread(pool: &sqlx::PgPool, thread_id: Uuid) -> Self {
        sqlx::query_as::<_, (i64, i64)>(&format!(
            "SELECT {IN_FLIGHT_COLUMNS_SQL} FROM thread_summaries t WHERE t.thread_id = $1"
        ))
        .bind(thread_id)
        .fetch_optional(pool)
        .await
        .unwrap_or_else(|e| {
            crate::log!(
                "[TurnEnd] In-flight state of {} unreadable: {}",
                thread_id,
                e
            );
            None
        })
        .map(|(sub_threads, event_waits)| Self {
            sub_threads,
            event_waits,
        })
        .unwrap_or_default()
    }
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
    let (turn_start, message, event_type, mode, decided, reply, change_replaces_read_request): (
        i64,
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
             SELECT turn_start.seq, COALESCE(opener.text, ''), opener.event_type, opener.mode, \
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
        answered: AnsweredQuestion::latest(pool, thread_id, turn_start).await,
        reply: reply.unwrap_or_default(),
        change_replaces_read_request,
        in_flight: InFlight::of_thread(pool, thread_id).await,
        ..TurnRead::default()
    })
}

/// Read a chat or trigger turn: the event that opened it, the reply it ends
/// on, the notifications it delivered, and what the thread still waits on.
pub(crate) async fn chat_turn_read(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    origin_id: Uuid,
    reply: &str,
) -> TurnRead {
    let opening = Opening::read(pool, origin_id).await;
    let (answered, pushed) = match opening.sequence {
        Some(sequence) => tokio::join!(
            AnsweredQuestion::latest(pool, thread_id, sequence),
            PushedNotification::delivered(pool, thread_id, sequence),
        ),
        None => (None, Vec::new()),
    };
    TurnRead {
        message: opening.text,
        source: opening.source,
        answered,
        reply: reply.to_string(),
        in_flight: InFlight::of_thread(pool, thread_id).await,
        pushed,
        ..TurnRead::default()
    }
}

/// The decision for one undecided turn, on the site's provider.
pub(crate) struct ForcedReadDecision<'a> {
    pub(crate) provider: &'a dyn JudgmentProvider,
    /// Bounds the call and its chat fallback together.
    pub(crate) deadline: Duration,
    pub(crate) capture: &'a AuxCapture,
}

/// The turn as the call reads it: the opening message under a heading naming
/// its sender, the user's answer when one resumed the turn, and the reply.
/// Closing notes follow when work is still in flight, when the parent agent
/// acts on the reply, and when the turn already notified the user.
pub(crate) fn read_decision_prompt(turn: &TurnRead) -> String {
    let answered = turn
        .answered
        .as_ref()
        .map(|a| {
            format!(
                "The agent then asked the user:\n{}\n\nThe user answered:\n{}\n\n",
                crate::core::middle_truncate(&a.question, MESSAGE_BYTES),
                crate::core::middle_truncate(&a.answer, MESSAGE_BYTES),
            )
        })
        .unwrap_or_default();
    let prompt = format!(
        "{}:\n{}\n\n{answered}The agent's reply:\n{}",
        turn.source.heading(),
        crate::core::middle_truncate(&turn.message, MESSAGE_BYTES),
        crate::core::middle_truncate(&turn.reply, REPLY_BYTES),
    );
    [
        turn.in_flight.note(),
        turn.parent_note().map(str::to_string),
        PushedNotification::note(&turn.pushed),
    ]
    .into_iter()
    .flatten()
    .fold(prompt, |prompt, note| format!("{prompt}\n\n{note}"))
}

/// Whether a turn owing its parent a report settles the wake decision before
/// the read decision. Only a forced read decision on a turn the user did not
/// open reads where the report goes. Every other turn runs both at once.
pub(crate) fn wake_settles_first(read_decided: bool, turn: &TurnRead) -> bool {
    !read_decided && !turn.opened_by_the_user()
}

/// The read decision's chat backend: the site's chat model, forced to answer.
///
/// Forced, so every reply carries the answer and runs without thinking, as
/// ADR 0426 measured.
pub fn read_decision_chat_judge(
    provider: std::sync::Arc<dyn crate::llm::LlmProvider>,
    reasoning: Option<String>,
) -> ChatJudgmentProvider {
    ChatJudgmentProvider::new(provider, reasoning).forcing_the_answer()
}

/// The one question every backend answers.
fn read_question() -> Vec<(String, Question)> {
    vec![(
        READ_QUESTION_ID.to_string(),
        Question::Noul {
            instructions: READ_DECISION_INSTRUCTIONS.to_string(),
            criteria: Some(NoulCriteria {
                yes: READ_YES.to_string(),
                no: READ_NO.to_string(),
            }),
        },
    )]
}

/// The decision the answers make. A missing answer asks to be read.
pub(crate) fn decision_from(answers: &Answers) -> ReadDecision {
    match answers.noul(READ_QUESTION_ID) {
        Some(p) if p < READ_THRESHOLD => ReadDecision::NotRequested,
        _ => ReadDecision::Requested,
    }
}

impl ForcedReadDecision<'_> {
    /// Decide one turn. A thread whose change already lists it, or an empty
    /// reply, is a no without a call.
    pub(crate) async fn decide(&self, turn: &TurnRead) -> Result<ReadDecision, BoxError> {
        if turn.change_replaces_read_request || turn.reply.trim().is_empty() {
            return Ok(ReadDecision::NotRequested);
        }
        let judgment = self
            .capture
            .until(tokio::time::Instant::now() + self.deadline)
            .judge(
                self.provider,
                serde_json::Value::String(read_decision_prompt(turn)),
                read_question(),
            )
            .await?;
        Ok(decision_from(&judgment.answers))
    }
}

/// One read decision, as `lucidos-eval read-decision` scores it.
pub struct EvalReadDecision {
    pub read: bool,
    /// The probability of yes, or `None` when the backend gave no answer.
    pub probability: Option<f64>,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// One read decision for `lucidos-eval`'s read-decision check: the gate's own
/// state, question and threshold over `provider`, recording nothing. The eval
/// hands in its own token, since no engine source may mint one.
#[doc(hidden)]
pub async fn decide_read_for_eval(
    provider: &dyn JudgmentProvider,
    turn: &TurnRead,
    call: crate::llm::metered::CallToken,
) -> Result<EvalReadDecision, BoxError> {
    let judgment = provider
        .ask(
            serde_json::Value::String(read_decision_prompt(turn)),
            read_question(),
            call,
        )
        .await?;
    Ok(EvalReadDecision {
        read: decision_from(&judgment.answers) == ReadDecision::Requested,
        probability: judgment.answers.noul(READ_QUESTION_ID),
        input_tokens: judgment.usage.input_tokens,
        output_tokens: judgment.usage.output_tokens,
    })
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
        let system_one = self
            .turn_end_system_one(JudgmentSite::ReadDecision, call.attempt_timeout())
            .await;
        let chat = read_decision_chat_judge(call.provider(), call.reasoning().map(str::to_string));
        let provider = for_site(system_one, chat);
        let capture = AuxCapture::new(&self.event_bus, thread_id, ContextPurpose::ReadDecision);
        let forced = ForcedReadDecision {
            provider: provider.as_ref(),
            deadline: call.deadline(),
            capture: &capture,
        };
        record_forced_decision(&self.event_bus, thread_id, forced.decide(turn).await).await
    }
}

/// Record a decision, or a read request in its place when the call failed.
/// Returns what was recorded.
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

/// The event that opened a chat or trigger turn.
#[derive(Default)]
struct Opening {
    text: String,
    source: OpeningSource,
    /// Where the turn starts in the event log. `None` when the event is
    /// unreadable, and then no answer is read either.
    sequence: Option<i64>,
}

impl Opening {
    async fn read(pool: &sqlx::PgPool, origin_id: Uuid) -> Self {
        sqlx::query_as::<_, (String, String, Option<String>, i64)>(&format!(
            "SELECT {OPENING_COLUMNS_SQL}, sequence FROM events WHERE id = $1"
        ))
        .bind(origin_id)
        .fetch_optional(pool)
        .await
        .unwrap_or_else(|e| {
            crate::log!("[TurnEnd] Opening event {} unreadable: {}", origin_id, e);
            None
        })
        .map(|(text, event_type, mode, sequence)| Self {
            text,
            source: OpeningSource::from_columns(Some(&event_type), mode),
            sequence: Some(sequence),
        })
        .unwrap_or_default()
    }
}

impl crate::engine::LucidosEngine {
    /// The chat turn end's gate, just before `ResponseGenerated`: force the
    /// read decision when the turn made none, and the wake decision when the
    /// thread owes its parent a report without one. `reply` is the draft the
    /// turn ends on, and it stays the reply.
    pub(crate) async fn gate_chat_turn_end(
        &self,
        thread_id: Uuid,
        origin_id: Uuid,
        read_decided: bool,
        reply: &str,
        finish: TurnFinish,
    ) {
        let owed = OwedReport::of_thread(&self.pool, thread_id).await;
        if read_decided && owed.is_none() {
            return;
        }
        let turn = chat_turn_read(&self.pool, thread_id, origin_id, reply).await;
        self.force_turn_end_decisions(thread_id, &turn, read_decided, owed, finish)
            .await;
    }

    /// The coding-agent turn end's gate, before `CodingAgentIdled`: force the
    /// read decision and the wake decision the turn did not make. A coding
    /// agent cannot be forced inside its own session, so both read the turn's
    /// events. `has_changes` is the idle's own, which names the report's
    /// status.
    pub(crate) async fn gate_coding_agent_turn_end(&self, thread_id: Uuid, has_changes: bool) {
        let owed = OwedReport::of_thread(&self.pool, thread_id).await;
        let turn = match coding_agent_turn_read(&self.pool, thread_id).await {
            Ok(turn) => turn,
            Err(e) => {
                // The wake decision stays unset, which wakes the parent.
                let unread = format!("the turn's events were unreadable: {e}");
                record_forced_decision(&self.event_bus, thread_id, Err(unread.into())).await;
                return;
            }
        };
        let status = if has_changes {
            ChildCompletionStatus::Success
        } else {
            ChildCompletionStatus::NoChanges
        };
        let read_decided = turn.decided;
        self.force_turn_end_decisions(
            thread_id,
            &turn,
            read_decided,
            owed,
            TurnFinish::Finished(status),
        )
        .await;
    }

    /// Force whichever of the two decisions `turn` lacks. They run
    /// concurrently, unless the read decision needs the wake decision's result.
    async fn force_turn_end_decisions(
        &self,
        thread_id: Uuid,
        turn: &TurnRead,
        read_decided: bool,
        owed: Option<OwedReport>,
        finish: TurnFinish,
    ) {
        if let Some(owed) = owed
            .as_ref()
            .filter(|_| wake_settles_first(read_decided, turn))
        {
            let decision = self
                .force_wake_decision(thread_id, owed, turn, finish)
                .await;
            let turn = TurnRead {
                report_wakes_parent: owed.wakes_parent(finish, decision),
                ..turn.clone()
            };
            self.force_read_decision(thread_id, &turn).await;
            return;
        }
        let read = async {
            if !read_decided {
                self.force_read_decision(thread_id, turn).await;
            }
        };
        let wake = async {
            if let Some(owed) = &owed {
                self.force_wake_decision(thread_id, owed, turn, finish)
                    .await;
            }
        };
        tokio::join!(read, wake);
    }
}
