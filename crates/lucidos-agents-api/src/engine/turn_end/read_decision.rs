use crate::engine::turn_end::expected_wake_decision::ExpectedWakeDecisionExt;
use crate::engine::turn_end::wake_decision::WakeDecisionExt;
use std::time::Duration;

use uuid::Uuid;

use super::expected_wake_decision::TurnStart;
use super::wake_decision::{OwedReport, TurnFinish, WakeDuty};
use crate::engine::model_call::AuxCapture;
use crate::engine::read_request::{record_agent_read_decision, record_read_decision, ReadDecision};
use crate::engine::thread_events::{
    ActorMode, AnswerKind, ChildCompletionStatus, QuestionOption, SessionStart,
    EVENT_WAIT_REASON_KIND,
};
use crate::engine::ContextPurpose;
use crate::llm::judgment::{
    for_site, Answers, ChatJudgmentProvider, JudgmentProvider, JudgmentSite, NoulCriteria, Question,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub(crate) const READ_QUESTION_ID: &str = "read";

pub(crate) const READ_THRESHOLD: f64 = 0.5;

macro_rules! must_see_criteria {
    () => {
        "a failure, a gap the user must know about, or something only the user can do or \
         decide, such as a payment, a sign-in or an approval"
    };
}

macro_rules! parent_acts_read_guidance {
    () => {
        concat!(
            "Your report goes to your parent agent, which acts on it, and the user reads your \
             parent. A routine result (a pass, nothing found, already fixed, a ready change, a status, \
             a review verdict) is a no, because your parent carries the read. That replaces the yes \
             for an answer and no change. Your parent asks the user for any approval. Say yes only for ",
            must_see_criteria!(),
            ". A turn the user opens in this thread is answered as usual."
        )
    };
}

pub(crate) const PARENT_ACTS_READ_RULE: &str = concat!(
    "SUB-THREAD READ DECISION: ",
    parent_acts_read_guidance!(),
    " Your no also clears an earlier yes the user has not read yet, so say yes again while \
     that earlier item still needs the user."
);

pub(crate) const PARENT_MAY_ACT_READ_RULE: &str = concat!(
    "SUB-THREAD READ DECISION, only when you wake your parent: ",
    parent_acts_read_guidance!()
);

pub(crate) const READ_DECISION_INSTRUCTIONS: &str = concat!(
    "Should the user read the agent's reply? \
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
    settles it. A scheduled report with no material change is a no, even when the trigger \
    asks for one each run. A material change, such as a sharp increase in spending or a new \
    charge, is a finding even inside a scheduled report.\n\n\
    When a closing note says the thread's work is still in flight, a progress report (which \
    step finished, what started, what it waits on) is not worth reading. It is worth reading \
    only when the reply holds something the user should read before the work finishes, such as \
    a failure, a finding or a decision.\n\n\
    When a closing note says the parent agent acts on this reply now, a routine result for it \
    is not worth the user's read. It is still worth reading when it holds ",
    must_see_criteria!(),
    ". The parent agent seeing it does not make it the parent's to settle.\n\n\
    When a closing note says the turn already sent the user a notification, the reply is not \
    worth reading. The notification is how the user hears, and they do not need both. The one \
    exception is a failure the notification does not mention."
);

const READ_YES: &str = "The reply holds findings, research, an answer to the user's question, \
    a draft they asked for, a new finding from a trigger run that the user would otherwise \
    miss, a failure, or a decision only the user can make. Also yes when the user said they would read \
    it later.";

const READ_NO: &str = "The reply only confirms or carries out what the user just said or \
    picked. Also no for small talk, an acknowledgement, a plain \"done\", a run that found \
    nothing, and a kickoff that only says work started and will report back. Also no for a \
    reply about a change that is ready, applied or merged, even when it explains what was \
    wrong, counts commits and files, or names a step such as a restart. Such a reply is a yes \
    only when it also holds a failure, a finding beyond the change, or a decision for the user. \
    Also no when a closing note says the turn already sent the user a notification, unless the \
    reply holds a failure the notification does not mention.";

const MESSAGE_BYTES: usize = 4_000;
const REPLY_BYTES: usize = 8_000;
const PUSH_TITLE_BYTES: usize = 200;
pub(crate) const PUSH_MESSAGE_BYTES: usize = 1_500;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OpeningSource {
    #[default]
    UserMessage,
    ChildThreadReport,
    TriggerRun,
    AgentOrEngine,
}

impl OpeningSource {
    pub(crate) fn from_event(event_type: &str, mode: Option<ActorMode>) -> Self {
        match (event_type, mode) {
            ("ChildThreadCompleted", _) => Self::ChildThreadReport,
            ("TriggerStarted", _) => Self::TriggerRun,
            (_, Some(ActorMode::Agent | ActorMode::Engine)) => Self::AgentOrEngine,
            _ => Self::UserMessage,
        }
    }

    fn from_columns(event_type: Option<&str>, mode: Option<String>) -> Self {
        let mode = mode.and_then(|m| {
            serde_json::from_value(serde_json::Value::String(m.clone()))
                .inspect_err(|e| crate::log_legacy!("[TurnEnd] Opener mode {:?} unknown: {}", m, e))
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnRead {
    pub decided: bool,
    pub message: String,
    pub source: OpeningSource,
    pub continues_trigger_run: bool,
    pub answered: Option<AnsweredQuestion>,
    pub reply: String,
    pub change_replaces_read_request: bool,
    pub in_flight: InFlight,
    pub report_wakes_parent: bool,
    pub pushed: Vec<PushedNotification>,
    pub user_wrote_in_turn: bool,
}

impl TurnRead {
    pub(crate) fn opened_by_the_user(&self) -> bool {
        self.source == OpeningSource::UserMessage || self.answered.is_some()
    }

    pub(crate) fn reply_is_the_users(&self) -> bool {
        self.opened_by_the_user() || self.user_wrote_in_turn
    }

    fn parent_note(&self) -> Option<&'static str> {
        (self.report_wakes_parent && !self.opened_by_the_user()).then_some(
            "Closing note: this reply is the thread's report to the parent agent that started \
             it. The parent agent acts on a waking report now and reads a filed repeat on its \
             next turn.",
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnsweredQuestion {
    pub question: String,
    pub answer: String,
}

impl AnsweredQuestion {
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
            crate::log_legacy!("[TurnEnd] Answers on {} unreadable: {}", thread_id, e);
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct PushedNotification {
    pub title: String,
    pub message: String,
}

impl PushedNotification {
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
            crate::log_legacy!("[TurnEnd] Pushes on {} unreadable: {}", thread_id, e);
            Vec::new()
        })
        .into_iter()
        .map(|(title, message)| Self { title, message })
        .collect()
    }

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct InFlight {
    pub sub_threads: i64,
    pub event_waits: i64,
    pub successor_session: bool,
}

const IN_FLIGHT_COLUMNS_SQL: &str = "t.awaited_children_count::bigint, \
     t.live_event_wait_count::bigint";

impl InFlight {
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
        if waits.is_empty() && !self.successor_session {
            return None;
        }
        let mut note = "Closing note: the thread's work is still in flight.".to_owned();
        if !waits.is_empty() {
            note.push_str(&format!(" It waits on {}.", waits.join(" and ")));
        }
        if self.successor_session {
            note.push_str(" A successor session carries it on.");
        }
        Some(note)
    }

    async fn of_thread(pool: &sqlx::PgPool, thread_id: Uuid) -> Self {
        sqlx::query_as::<_, (i64, i64)>(&format!(
            "SELECT {IN_FLIGHT_COLUMNS_SQL} FROM thread_summaries t WHERE t.thread_id = $1"
        ))
        .bind(thread_id)
        .fetch_optional(pool)
        .await
        .unwrap_or_else(|e| {
            crate::log_legacy!(
                "[TurnEnd] In-flight state of {} unreadable: {}",
                thread_id,
                e
            );
            None
        })
        .map(|(sub_threads, event_waits)| Self {
            sub_threads,
            event_waits,
            successor_session: false,
        })
        .unwrap_or_default()
    }
}

const OPENING_COLUMNS_SQL: &str = "COALESCE(payload->>'text', payload->>'prompt', \
     payload->>'summary', '') AS text, event_type, \
     COALESCE(payload->>'mode', \
              CASE WHEN payload->'origin'->>'kind' = 'engine' THEN 'engine' END) AS mode";

async fn human_input_after(pool: &sqlx::PgPool, thread_id: Uuid, after_sequence: i64) -> bool {
    sqlx::query_scalar(&format!(
        "SELECT EXISTS (SELECT 1 FROM ( \
           SELECT {OPENING_COLUMNS_SQL} FROM events \
            WHERE thread_id = $1 AND sequence > $2 AND event_type = ANY($3)) input \
          WHERE input.mode IS NULL OR input.mode NOT IN ('agent', 'engine'))"
    ))
    .bind(thread_id)
    .bind(after_sequence)
    .bind(HUMAN_INPUT_EVENT_TYPES)
    .fetch_one(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log_legacy!("[TurnEnd] Input to thread {} unreadable: {}", thread_id, e);
        true
    })
}

const HUMAN_INPUT_EVENT_TYPES: &[&str] = &["MessageReceived", "PromptInjected"];

fn coding_agent_turn_openers() -> Vec<&'static str> {
    crate::engine::thread_events::CC_ORIGINATING_EVENT_TYPES
        .iter()
        .copied()
        .chain(["PromptInjected", "CodingAgentPromptSent"])
        .collect()
}

pub(crate) async fn coding_agent_turn_read(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    session_start: Option<i64>,
) -> Result<TurnRead, sqlx::Error> {
    #[derive(sqlx::FromRow)]
    struct TurnRow {
        turn_start: i64,
        message: String,
        event_type: Option<String>,
        mode: Option<String>,
        decided: bool,
        reply: Option<String>,
        change_replaces_read_request: bool,
        handoff_start: Option<sqlx::types::Json<SessionStart>>,
    }
    let TurnRow {
        turn_start,
        message,
        event_type,
        mode,
        decided,
        reply,
        change_replaces_read_request,
        handoff_start,
    } =
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
             SELECT turn_start.seq AS turn_start, COALESCE(opener.text, '') AS message, \
                    opener.event_type, opener.mode, \
                    EXISTS (SELECT 1 FROM events e \
                             WHERE e.aggregate = 'thread' AND e.aggregate_id = $1 \
                               AND e.event_type IN ('ThreadReadRequested', 'ThreadReadNotRequested') \
                               AND e.sequence > turn_start.seq) AS decided, \
                    (SELECT e.payload->>'text' FROM events e \
                      WHERE e.aggregate = 'thread' AND e.aggregate_id = $1 \
                        AND e.event_type = 'ResponseGenerated' AND e.sequence > turn_start.seq \
                      ORDER BY e.sequence DESC LIMIT 1) AS reply, \
                    COALESCE((SELECT change_replaces_read_request( \
                                       t.coding_agent_change_state, t.coding_agent_unproposed_reason) \
                                FROM thread_summaries t WHERE t.thread_id = $1::uuid), FALSE) \
                        AS change_replaces_read_request, \
                    (SELECT CASE WHEN e.event_type = 'SessionHandedOff' \
                                 THEN COALESCE(NULLIF(e.payload->'starts', 'null'::jsonb), $4::jsonb) END \
                               FROM events e \
                              WHERE e.aggregate = 'thread' AND e.aggregate_id = $1 \
                                AND e.event_type IN ('SessionHandedOff', 'SessionHandoffWithdrawn') \
                                AND e.sequence > GREATEST(turn_start.seq, $3::bigint) \
                              ORDER BY e.sequence DESC LIMIT 1) AS handoff_start \
               FROM turn_start LEFT JOIN opener ON TRUE"
        ))
        .bind(thread_id.to_string())
        .bind(coding_agent_turn_openers())
        .bind(session_start)
        .bind(sqlx::types::Json(SessionStart::default()))
        .fetch_one(pool)
        .await?;
    Ok(TurnRead {
        decided,
        message,
        source: OpeningSource::from_columns(event_type.as_deref(), mode),
        answered: AnsweredQuestion::latest(pool, thread_id, turn_start).await,
        reply: reply.unwrap_or_default(),
        change_replaces_read_request,
        in_flight: InFlight {
            successor_session: handoff_start.is_some_and(|start| start.0.is_unprompted()),
            ..InFlight::of_thread(pool, thread_id).await
        },
        user_wrote_in_turn: human_input_after(pool, thread_id, turn_start).await,
        ..TurnRead::default()
    })
}

pub(crate) async fn coding_agent_reply_is_the_users(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    let coding_agent: bool =
        sqlx::query_scalar("SELECT is_coding_agent FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await
            .unwrap_or_else(|e| {
                crate::log_legacy!("[TurnEnd] Kind of thread {} unreadable: {}", thread_id, e);
                None
            })
            .unwrap_or(false);
    !coding_agent
        || coding_agent_turn_read(pool, thread_id, None)
            .await
            .map_or(true, |turn| turn.reply_is_the_users())
}

pub(crate) async fn chat_turn_read(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    origin_id: Uuid,
    reply: &str,
) -> TurnRead {
    let opening = Opening::read(pool, thread_id, origin_id).await;
    let (answered, pushed, user_wrote_in_turn) = match opening.sequence {
        Some(sequence) => tokio::join!(
            AnsweredQuestion::latest(pool, thread_id, sequence),
            PushedNotification::delivered(pool, thread_id, sequence),
            human_input_after(pool, thread_id, sequence - 1),
        ),
        None => (None, Vec::new(), true),
    };
    TurnRead {
        message: opening.text,
        source: opening.source,
        continues_trigger_run: opening.continues_trigger_run,
        answered,
        reply: reply.to_string(),
        in_flight: InFlight::of_thread(pool, thread_id).await,
        pushed,
        user_wrote_in_turn,
        ..TurnRead::default()
    }
}

pub(crate) struct ForcedReadDecision<'a> {
    pub(crate) provider: &'a dyn JudgmentProvider,
    pub(crate) deadline: Duration,
    pub(crate) capture: &'a AuxCapture,
}

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
        turn.continues_trigger_run
            .then(|| "This turn continues a trigger run.".to_string()),
        turn.in_flight.note(),
        turn.parent_note().map(str::to_string),
        PushedNotification::note(&turn.pushed),
    ]
    .into_iter()
    .flatten()
    .fold(prompt, |prompt, note| format!("{prompt}\n\n{note}"))
}

pub(crate) fn wake_settles_first(read_decided: bool, turn: &TurnRead) -> bool {
    !read_decided && !turn.opened_by_the_user()
}

pub fn read_decision_chat_judge(
    provider: std::sync::Arc<dyn crate::llm::LlmProvider>,
    reasoning: Option<String>,
) -> ChatJudgmentProvider {
    ChatJudgmentProvider::new(provider, reasoning).forcing_the_answer()
}

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

pub(crate) fn decision_from(answers: &Answers) -> ReadDecision {
    match answers.noul(READ_QUESTION_ID) {
        Some(p) if p < READ_THRESHOLD => ReadDecision::NotRequested,
        _ => ReadDecision::Requested,
    }
}

impl ForcedReadDecision<'_> {
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

pub struct EvalReadDecision {
    pub read: bool,
    pub probability: Option<f64>,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

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
        input_tokens: judgment.usage.unwrap_or_default().input_tokens,
        output_tokens: judgment.usage.unwrap_or_default().output_tokens,
    })
}

#[easy_ext::ext(ReadDecisionExt)]
pub(crate) impl crate::engine::LucidosEngine {
    async fn force_read_decision(&self, thread_id: Uuid, turn: &TurnRead) -> ReadDecision {
        let call = self.aux_call(ContextPurpose::ReadDecision).await;
        let system_one = self
            .judgment_system_one(JudgmentSite::ReadDecision, call.attempt_timeout())
            .await;
        let chat = read_decision_chat_judge(call.provider(), call.reasoning().map(str::to_string));
        let provider = for_site(system_one, chat);
        let capture = AuxCapture::new(&self.event_bus, thread_id, ContextPurpose::ReadDecision);
        let forced = ForcedReadDecision {
            provider: provider.as_ref(),
            deadline: call.deadline(),
            capture: &capture,
        };
        record_forced_decision(
            &self.pool,
            &self.event_bus,
            thread_id,
            Some(turn),
            forced.decide(turn).await,
        )
        .await
    }
}

pub(crate) async fn record_forced_decision(
    pool: &sqlx::PgPool,
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    turn: Option<&TurnRead>,
    outcome: Result<ReadDecision, BoxError>,
) -> ReadDecision {
    let decision = outcome.unwrap_or_else(|e| {
        crate::log_legacy!(
            "[TurnEnd] Read decision for thread {} failed, so it asks to be read: {}",
            thread_id,
            e
        );
        ReadDecision::Requested
    });
    let reply_is_the_users = async { turn.is_none_or(TurnRead::reply_is_the_users) };
    let parent_carries = parent_carries_read(pool, thread_id, decision, reply_is_the_users).await;
    if let Err(e) = record_read_decision(bus, thread_id, decision, None, parent_carries).await {
        crate::log_legacy!(
            "[TurnEnd] Recording the read decision for thread {} failed: {}",
            thread_id,
            e
        );
    }
    decision
}

pub(crate) async fn parent_carries_read(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    decision: ReadDecision,
    reply_is_the_users: impl std::future::Future<Output = bool>,
) -> bool {
    match decision {
        ReadDecision::Requested => {
            WakeDuty::of_thread(pool, thread_id).await.parent_acts()
                && unseen_request(pool, thread_id).await != UnseenRequest::Users
                && !reply_is_the_users.await
        }
        ReadDecision::NotRequested => {
            unseen_request(pool, thread_id).await == UnseenRequest::ParentReport
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum UnseenRequest {
    None,
    ParentReport,
    Users,
}

async fn unseen_request(pool: &sqlx::PgPool, thread_id: Uuid) -> UnseenRequest {
    match sqlx::query_scalar::<_, bool>(UNSEEN_PARENT_REPORT_SQL)
        .bind(thread_id)
        .fetch_optional(pool)
        .await
    {
        Ok(None) => UnseenRequest::None,
        Ok(Some(true)) => UnseenRequest::ParentReport,
        Ok(Some(false)) => UnseenRequest::Users,
        Err(e) => {
            crate::log_legacy!(
                "[TurnEnd] Read request of thread {} unreadable: {}",
                thread_id,
                e
            );
            UnseenRequest::Users
        }
    }
}

const UNSEEN_PARENT_REPORT_SQL: &str = "\
    SELECT COALESCE(( \
             SELECT (e.payload->>'parent_report')::boolean FROM events e \
              WHERE e.thread_id = $1 AND e.event_type = 'ThreadReadRequested' \
              ORDER BY e.sequence DESC LIMIT 1), FALSE) \
      FROM thread_summaries WHERE thread_id = $1 AND read_requested";

#[derive(Default)]
struct Opening {
    text: String,
    source: OpeningSource,
    continues_trigger_run: bool,
    sequence: Option<i64>,
}

impl Opening {
    fn belongs_to_trigger_run(&self) -> bool {
        self.source == OpeningSource::TriggerRun || self.continues_trigger_run
    }

    async fn reply_is_the_users(&self, pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
        let Some(sequence) = self.sequence else {
            return true;
        };
        let (answered, user_wrote_in_turn) = tokio::join!(
            AnsweredQuestion::latest(pool, thread_id, sequence),
            human_input_after(pool, thread_id, sequence - 1),
        );
        TurnRead {
            source: self.source,
            answered,
            user_wrote_in_turn,
            ..TurnRead::default()
        }
        .reply_is_the_users()
    }

    async fn read(pool: &sqlx::PgPool, thread_id: Uuid, origin_id: Uuid) -> Self {
        sqlx::query_as::<_, (String, String, Option<String>, i64, bool)>(&format!(
            "SELECT {OPENING_COLUMNS_SQL}, sequence,
                CASE WHEN event_type IN ('ChildThreadCompleted', 'ContinuationStarted')
                    OR (event_type = 'PromptInjected'
                        AND payload->'origin'->>'kind' = 'engine'
                        AND payload->'origin'->'reason'->>'kind' = $3)
                THEN COALESCE((
                    SELECT prior.event_type = 'TriggerStarted'
                    FROM events prior
                    WHERE prior.thread_id = opener.thread_id
                      AND prior.sequence < opener.sequence
                      AND (prior.event_type = 'TriggerStarted'
                           OR (prior.event_type = 'MessageReceived'
                               AND (prior.payload->>'request_event_id' IS NULL
                                    OR EXISTS (
                                        SELECT 1 FROM events stamp
                                        WHERE stamp.thread_id = prior.thread_id
                                          AND stamp.sequence > prior.sequence
                                          AND stamp.sequence < opener.sequence
                                          AND stamp.event_type = 'ChatTurnSettingsResolved'
                                          AND stamp.payload->>'request_event_id' = prior.id::text
                                    ))))
                    ORDER BY prior.sequence DESC
                    LIMIT 1
                ), false)
                ELSE false END
             FROM events opener WHERE id = $1 AND thread_id = $2"
        ))
        .bind(origin_id)
        .bind(thread_id)
        .bind(EVENT_WAIT_REASON_KIND)
        .fetch_optional(pool)
        .await
        .unwrap_or_else(|e| {
            crate::log_legacy!("[TurnEnd] Opening event {} unreadable: {}", origin_id, e);
            None
        })
        .map(
            |(text, event_type, mode, sequence, continues_trigger_run)| Self {
                text,
                source: OpeningSource::from_columns(Some(&event_type), mode),
                sequence: Some(sequence),
                continues_trigger_run,
            },
        )
        .unwrap_or_default()
    }
}

#[easy_ext::ext(TurnEndDecisionExt)]
pub(crate) impl crate::engine::LucidosEngine {
    async fn decide_chat_agent_read(
        &self,
        thread_id: Uuid,
        origin_id: Uuid,
        args: &serde_json::Value,
    ) -> Result<(bool, &'static str), String> {
        let decision = ReadDecision::from_tool_args(args)?;
        let opening = Opening::read(&self.pool, thread_id, origin_id).await;
        if decision == ReadDecision::Requested && opening.belongs_to_trigger_run() {
            return Ok((false, "The engine will decide at the end of the turn."));
        }
        let reply_is_the_users = opening.reply_is_the_users(&self.pool, thread_id);
        let parent_carries =
            parent_carries_read(&self.pool, thread_id, decision, reply_is_the_users).await;
        record_agent_read_decision(&self.event_bus, thread_id, decision, parent_carries)
            .await
            .map_err(|e| format!("Error: recording the read decision failed: {e}"))?;
        Ok((true, decision.ack()))
    }

    /// INVARIANT: The chat turn end's gate, just before `ResponseGenerated`: force the
    /// read decision when the turn made none, and the wake decision when the
    /// thread owes its parent a report without one. Beside them runs the
    /// expected wake decision. `reply` is the draft the turn ends on, and it
    /// stays the reply.
    async fn gate_chat_turn_end(
        &self,
        thread_id: Uuid,
        origin_id: Uuid,
        read_decided: bool,
        reply: &str,
        finish: TurnFinish,
    ) {
        let decisions = async {
            let owed = OwedReport::of_thread(&self.pool, thread_id).await;
            if read_decided && owed.is_none() {
                return;
            }
            let turn = chat_turn_read(&self.pool, thread_id, origin_id, reply).await;
            self.force_turn_end_decisions(thread_id, &turn, read_decided, owed, finish)
                .await;
        };
        let expected_wake =
            self.force_expected_wake_decision(thread_id, TurnStart::Opened(origin_id), reply);
        tokio::join!(decisions, expected_wake);
    }

    async fn gate_coding_agent_turn_end(
        &self,
        thread_id: Uuid,
        has_changes: bool,
        session_start: Option<i64>,
    ) {
        let owed = OwedReport::of_thread(&self.pool, thread_id).await;
        let turn = coding_agent_turn_read(&self.pool, thread_id, session_start).await;
        let reply = turn.as_ref().map(|t| t.reply.clone()).unwrap_or_default();
        let decisions = async {
            let turn = match turn {
                Ok(turn) => turn,
                Err(e) => {
                    // INVARIANT: The wake decision stays unset, which wakes the parent.
                    let unread = format!("the turn's events were unreadable: {e}");
                    record_forced_decision(
                        &self.pool,
                        &self.event_bus,
                        thread_id,
                        None,
                        Err(unread.into()),
                    )
                    .await;
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
        };
        let expected_wake =
            self.force_expected_wake_decision(thread_id, TurnStart::SinceLastIdle, &reply);
        tokio::join!(decisions, expected_wake);
    }

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
