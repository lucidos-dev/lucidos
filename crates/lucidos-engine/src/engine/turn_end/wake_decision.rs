//! The *wake decision* turn-end requirement: a child thread says whether its
//! parent must act on this turn's report.
//!
//! The child decides with the `wake_parent` tool or `lucidos wake-parent`. A
//! turn that ends owing its parent a card and holding no decision is decided
//! here, by one call to the `WakeDecision` judgment site. The decision lands
//! on the child as `ParentWakeRequested` or `ParentWakeNotRequested`, before
//! its terminal event, so the fan-in reads it.
//!
//! It defaults to OpenAI Decisions while the chat provider serves OpenAI,
//! unless the user picked a backend or pinned `model_wake_decision` (ADRs 0435,
//! 0472). A failed OpenAI call falls back to chat.
//! Chat runs forced, and unpinned only on a model `lucidos-eval wake-decision`
//! cleared (ADR 0448).
//!
//! A child whose spawn declared a *wake mode* makes no decision: the mode
//! decides at the fan-in, with no model call (ADR 0451). The engine still
//! records a wake for its cut-short turn or empty reply.
//!
//! **Every doubt wakes.** A failed call, a missing answer, an empty reply, a
//! turn the engine cut short, an unreadable row and no measured model all wake
//! the parent. Only a confident no files the report, and only a clean finish
//! ever consults it.
//! The call's one output is that yes or no: it has no tool and no side effect.

use uuid::Uuid;

use super::read_decision::TurnRead;
use crate::engine::aux_purpose::asks_no_model;
use crate::engine::chat::agent_archive::agent_thread_actor;
use crate::engine::event_bus::{BusEvent, Delivery, EventBus};
use crate::engine::model_call::AuxCapture;
use crate::engine::thread_events::{
    ChildCompletionStatus, EventMeta, MessageOrigin, ThreadEvent, WakeMode,
};
use crate::engine::ContextPurpose;
use crate::llm::judgment::{
    for_site, Answers, ChatJudgmentProvider, JudgmentProvider, JudgmentSite, NoulCriteria, Question,
};
use crate::llm::tools::WAKE_ARG;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The lowest probability of "the parent must act" that still wakes it. Below
/// it the report is filed, so the model has to be fairly sure of a no.
const WAKE_THRESHOLD: f64 = 0.3;

/// The id of the one question the gate asks.
const WAKE_QUESTION_ID: &str = "parent_must_act";

/// The most bytes of a message the call reads.
const MESSAGE_BYTES: usize = 4_000;
/// The most bytes of the child's reply the call reads.
const REPLY_BYTES: usize = 8_000;

/// A child's yes or no about waking its parent for this turn's report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WakeDecision {
    /// The parent must act now, so the report wakes it.
    Requested,
    /// The parent need not act now. A clean finish files the report.
    NotRequested,
}

impl WakeDecision {
    pub(crate) fn from_wake(wake: bool) -> Self {
        if wake {
            Self::Requested
        } else {
            Self::NotRequested
        }
    }

    /// The `parent_wake_requested` value this decision projects to.
    fn wake(self) -> bool {
        self == Self::Requested
    }

    /// Read `wake_parent`'s arguments. A missing or non-boolean `wake` is an
    /// error the model reads back, never a default.
    pub(crate) fn from_tool_args(args: &serde_json::Value) -> Result<Self, String> {
        args[WAKE_ARG]
            .as_bool()
            .map(Self::from_wake)
            .ok_or_else(|| {
                format!(
                "Error: wake_parent needs `{WAKE_ARG}`: true when your parent must act on this \
                 turn's report now, false when it can read it on its next turn."
            )
            })
    }

    fn event(self) -> ThreadEvent {
        match self {
            Self::Requested => ThreadEvent::ParentWakeRequested,
            Self::NotRequested => ThreadEvent::ParentWakeNotRequested,
        }
    }

    /// What the agent reads back after deciding, from the tool and the CLI.
    pub(crate) fn ack(self) -> &'static str {
        match self {
            Self::Requested => {
                "Recorded. When this turn ends, your report wakes your parent thread. A \
                 follow-up that reaches you first reopens the decision."
            }
            Self::NotRequested => {
                "Recorded. When this turn ends cleanly, your report is filed on your parent \
                 thread, which reads it on its next turn. A failure still wakes it, and a \
                 follow-up that reaches you first reopens the decision."
            }
        }
    }

    /// What a child whose parent declared its wake mode reads back from the
    /// tool or the CLI. It records nothing.
    pub(crate) fn declared_ack(mode: WakeMode) -> String {
        let effect = match mode {
            WakeMode::Wake => "Your clean report wakes it.",
            WakeMode::WakeWhenAllDone => {
                "Your clean report wakes it once no other child of its batch is still working."
            }
            WakeMode::NoWake => {
                "Every report you send is filed, a failure too, and it reads them on its next \
                 turn. If you need the user, ask on your own card."
            }
        };
        let failure = match mode {
            WakeMode::NoWake => "",
            WakeMode::Wake | WakeMode::WakeWhenAllDone => " A failure still wakes it.",
        };
        format!(
            "Nothing recorded. Your parent declared your wake mode, `{}`, when it spawned you, \
             so this turn's report follows it and this call changes nothing. {effect}{failure}",
            mode.as_str()
        )
    }

    /// The decision a probability of "the parent must act" stands for. An
    /// absent answer wakes.
    fn from_probability(must_act: Option<f64>) -> Self {
        match must_act {
            Some(p) if p < WAKE_THRESHOLD => Self::NotRequested,
            _ => Self::Requested,
        }
    }
}

/// Record a wake decision on `thread_id`, by `actor`. `None` is the engine
/// deciding at the turn end.
pub(crate) async fn record_wake_decision(
    bus: &EventBus,
    thread_id: Uuid,
    decision: WakeDecision,
    actor: Option<MessageOrigin>,
) -> Result<(), BoxError> {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: decision.event(),
        meta: EventMeta::with_actor(actor),
    })
    .await?;
    Ok(())
}

/// Record the calling thread's own agent's wake decision.
pub(crate) async fn record_agent_wake_decision(
    bus: &EventBus,
    thread_id: Uuid,
    decision: WakeDecision,
) -> Result<(), BoxError> {
    record_wake_decision(
        bus,
        thread_id,
        decision,
        Some(agent_thread_actor(thread_id)),
    )
    .await
}

/// What a thread owes its parent about waking it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WakeDuty {
    /// No parent, or an unreadable row. The gate then decides for the turn.
    NoParent,
    /// The spawn declared a wake mode, which decides at the fan-in.
    Declared(WakeMode),
    /// A parent and no mode: each turn makes a wake decision.
    Decide,
}

impl WakeDuty {
    pub(crate) async fn of_thread(pool: &sqlx::PgPool, thread_id: Uuid) -> Self {
        // A thread whose handoff is pending reports at its successor
        // session's end. One that handed off to a legacy successor thread reports
        // through it. Neither owes a decision of its own now.
        let row = sqlx::query_as::<_, (Option<Uuid>, Option<String>)>(&format!(
            "SELECT CASE WHEN successor_thread_id IS NULL AND NOT {} THEN parent_thread_id END, \
                    wake_mode \
             FROM thread_summaries WHERE thread_id = $1",
            crate::engine::successor::pending_handoff_sql("$1")
        ))
        .bind(thread_id)
        .fetch_optional(pool)
        .await
        .unwrap_or_else(|e| {
            crate::log!("[TurnEnd] Parent of {} unreadable: {}", thread_id, e);
            None
        });
        match row {
            Some((Some(_), Some(mode))) => {
                WakeMode::parse(&mode).map_or(Self::Decide, Self::Declared)
            }
            Some((Some(_), None)) => Self::Decide,
            _ => Self::NoParent,
        }
    }

    /// Whether the thread is offered `wake_parent` and told the wake rule.
    pub(crate) async fn owes_a_decision(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
        Self::of_thread(pool, thread_id).await == Self::Decide
    }
}

/// How a turn ended, as the wake decision needs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnFinish {
    /// The agent finished with a reply. Carries the status the fan-in reports.
    Finished(ChildCompletionStatus),
    /// The engine ended the turn short: a cap or a broken loop. It wakes under
    /// every mode but `no-wake`.
    CutShort,
}

/// What the gate reads about a child whose turn still owes its parent a card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwedReport {
    parent_id: Uuid,
    /// The wake decision this turn already recorded, if any.
    decision: Option<WakeDecision>,
    /// The wake mode the spawn declared, which then decides instead.
    wake_mode: Option<WakeMode>,
    /// Whether another child of its `wake-when-all-done` batch still works.
    batch_still_working: bool,
    /// The child ends its turn holding an event wait, so the card waits too.
    /// That held card wakes under every mode but `no-wake`.
    holds_live_wait: bool,
}

impl OwedReport {
    /// The report `thread_id` owes its parent for this turn, or `None` when it
    /// owes none. An unreadable row reads as none: the decision stays unset,
    /// which wakes.
    pub(crate) async fn of_thread(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<Self> {
        let sql = format!(
            "SELECT c.parent_thread_id, c.parent_wake_requested, c.wake_mode, \
                    c.live_event_wait_count > 0, {} \
               FROM thread_summaries c \
              WHERE c.thread_id = $1 AND c.parent_thread_id IS NOT NULL \
                AND c.parent_callback_pending",
            crate::engine::event_bus::batch_still_working_sql("c", "$2", "$3")
        );
        sqlx::query_as::<_, (Uuid, Option<bool>, Option<String>, bool, bool)>(&sql)
            .bind(thread_id)
            .bind(crate::core::store::active_thread_statuses())
            .bind(WakeMode::WakeWhenAllDone.as_str())
            .fetch_optional(pool)
            .await
            .unwrap_or_else(|e| {
                crate::log!("[TurnEnd] Owed report of {} unreadable: {}", thread_id, e);
                None
            })
            .map(
                |(parent_id, wake, wake_mode, holds_live_wait, batch_still_working)| Self {
                    parent_id,
                    decision: wake.map(WakeDecision::from_wake),
                    wake_mode: wake_mode.as_deref().and_then(WakeMode::parse),
                    batch_still_working,
                    holds_live_wait,
                },
            )
    }

    /// Whether the report wakes the parent when `finish` ends the turn and
    /// `decision` is recorded, as the fan-in delivers it.
    pub(crate) fn wakes_parent(&self, finish: TurnFinish, decision: Option<WakeDecision>) -> bool {
        match finish {
            // The fan-in holds the card until the wait ends, so this turn end
            // delivers nothing.
            _ if self.holds_live_wait => false,
            TurnFinish::CutShort => self.wake_mode != Some(WakeMode::NoWake),
            TurnFinish::Finished(status) => {
                Delivery::for_report(
                    status,
                    self.wake_mode,
                    decision.map(WakeDecision::wake),
                    self.batch_still_working,
                ) == Delivery::Wake
            }
        }
    }
}

/// What the call reads besides the child's turn: what the parent last said,
/// and how many of its other children are still working.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParentView {
    pub latest_reply: String,
    pub siblings_running: i64,
}

impl ParentView {
    /// Read `parent_id` around its child `child_id`. Unreadable parts read as
    /// empty, and the log records the error.
    async fn read(pool: &sqlx::PgPool, parent_id: Uuid, child_id: Uuid) -> Self {
        let sql = format!(
            "SELECT (SELECT e.payload->>'text' FROM events e \
                      WHERE e.aggregate = 'thread' AND e.aggregate_id = $1::text \
                        AND e.event_type = 'ResponseGenerated' \
                      ORDER BY e.sequence DESC LIMIT 1), \
                    (SELECT COUNT(*) FROM thread_summaries s \
                      WHERE s.parent_thread_id = $1 AND s.thread_id <> $2 AND {})",
            crate::core::store::still_working_sql("s", "$3")
        );
        let row: Option<(Option<String>, i64)> = sqlx::query_as(&sql)
            .bind(parent_id)
            .bind(child_id)
            .bind(crate::core::store::active_thread_statuses())
            .fetch_optional(pool)
            .await
            .unwrap_or_else(|e| {
                crate::log!("[TurnEnd] Parent {} unreadable: {}", parent_id, e);
                None
            });
        row.map(|(latest_reply, siblings_running)| Self {
            latest_reply: latest_reply.unwrap_or_default(),
            siblings_running,
        })
        .unwrap_or_default()
    }
}

/// The state the call judges.
pub(crate) fn wake_state(
    turn: &TurnRead,
    status: ChildCompletionStatus,
    parent: &ParentView,
) -> serde_json::Value {
    serde_json::json!({
        "what_opened_the_childs_turn": turn.source.heading(),
        "message_that_opened_the_childs_turn":
            crate::core::middle_truncate(&turn.message, MESSAGE_BYTES),
        "parents_latest_reply": crate::core::middle_truncate(&parent.latest_reply, MESSAGE_BYTES),
        "childs_reply": crate::core::middle_truncate(&turn.reply, REPLY_BYTES),
        "childs_finish": status,
        "parents_other_children": siblings_note(parent.siblings_running),
    })
}

/// The parent's other children still working, in words.
fn siblings_note(running: i64) -> String {
    match running {
        0 => "This child is the last of the parent's children to report. No other child is \
              still working."
            .to_string(),
        1 => "1 other child of the parent is still working.".to_string(),
        n => format!("{n} other children of the parent are still working."),
    }
}

/// The one question the call answers.
pub(crate) fn wake_questions() -> Vec<(String, Question)> {
    vec![(
        WAKE_QUESTION_ID.to_string(),
        Question::Noul {
            instructions: "A child thread finished a turn and reports to the parent thread that \
                started it. Does the parent have to act on this report now? Read what the parent \
                asked for in the message that opened the child's turn, and what the parent said \
                it would do next in its latest reply. A parent that combines its children's \
                results acts on the last report, never on an earlier one. A result the parent \
                needs for its next step wakes it, even while other children still work. A child \
                that stopped short of its task, or says it will finish later, has stopped \
                working, so its parent must act."
                .to_string(),
            criteria: Some(NoulCriteria {
                yes: "The parent must act now. The parent said it will use, check, relay, apply \
                    or schedule this result itself, or it combines results and this report is the \
                    last. Or the report raises what only the parent can handle: a problem, a \
                    blocker for the other children, a choice, or unfinished work."
                    .to_string(),
                no: "The report only informs, and the parent can read it on its next turn. The \
                    parent waits to combine this result with reports still to come from other \
                    children. Or the parent handed the user to the child, saying the child shows, \
                    presents or confirms its work to the user directly, and the child did. Or \
                    nothing the parent planned waits on it."
                    .to_string(),
            }),
        },
    )]
}

/// The decision one judgment answers. A missing answer wakes.
pub(crate) fn read_wake_answer(answers: &Answers) -> WakeDecision {
    WakeDecision::from_probability(answers.noul(WAKE_QUESTION_ID))
}

/// The wake decision's chat backend: the site's chat model, forced to answer.
///
/// Forced, so every reply carries the answer and runs without thinking. A
/// model that refuses a forced call fails it, and a failed call wakes.
pub fn wake_decision_chat_judge(
    provider: std::sync::Arc<dyn crate::llm::LlmProvider>,
    reasoning: Option<String>,
) -> ChatJudgmentProvider {
    ChatJudgmentProvider::new(provider, reasoning).forcing_the_answer()
}

/// One wake decision, as `lucidos-eval wake-decision` scores it.
pub struct EvalWakeDecision {
    pub wake: bool,
    /// The probability of yes, or `None` when the backend gave no answer.
    pub probability: Option<f64>,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// One wake decision for `lucidos-eval`'s wake-decision check: the gate's own
/// state, question and threshold over `provider`, recording nothing. The eval
/// hands in its own token, since no engine source may mint one.
#[doc(hidden)]
pub async fn decide_wake_for_eval(
    provider: &dyn JudgmentProvider,
    turn: &TurnRead,
    status: ChildCompletionStatus,
    parent: &ParentView,
    call: crate::llm::metered::CallToken,
) -> Result<EvalWakeDecision, BoxError> {
    let judgment = provider
        .ask(wake_state(turn, status, parent), wake_questions(), call)
        .await?;
    Ok(EvalWakeDecision {
        wake: read_wake_answer(&judgment.answers) == WakeDecision::Requested,
        probability: judgment.answers.noul(WAKE_QUESTION_ID),
        input_tokens: judgment.usage.input_tokens,
        output_tokens: judgment.usage.output_tokens,
    })
}

/// The provider the gate asks, or `None` when nothing measured can answer.
///
/// `chat` is absent when no model the eval cleared is reachable and none is
/// pinned. A System One pick then answers alone, and without one the parent
/// wakes with no call (ADR 0448).
pub(crate) fn wake_judge(
    system_one: Option<Box<dyn JudgmentProvider>>,
    chat: Option<ChatJudgmentProvider>,
) -> Option<Box<dyn JudgmentProvider>> {
    match (system_one, chat) {
        (system_one, Some(chat)) => Some(for_site(system_one, chat)),
        (Some(primary), None) => Some(primary),
        (None, None) => None,
    }
}

/// Ask `provider` once, within `deadline`. A failed or timed-out call wakes.
pub(crate) async fn decide_with(
    provider: &dyn JudgmentProvider,
    capture: &AuxCapture,
    deadline: std::time::Duration,
    state: serde_json::Value,
) -> WakeDecision {
    match capture
        .until(tokio::time::Instant::now() + deadline)
        .judge(provider, state, wake_questions())
        .await
    {
        Ok(judgment) => read_wake_answer(&judgment.answers),
        Err(e) => {
            crate::log!("[TurnEnd] Wake decision failed, so the parent wakes: {}", e);
            WakeDecision::Requested
        }
    }
}

/// What the gate does for one owed report, before any call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WakeStep {
    /// Record this decision in the engine's name, with no call.
    Record(WakeDecision),
    /// Record nothing: the turn decided, its card waits on an event wait, or
    /// its declared wake mode decides at the fan-in.
    Skip,
    /// Ask the `WakeDecision` judgment site.
    Ask(ChildCompletionStatus),
}

/// The gate's choice for `owed`, given the turn's reply and how it ended. Only
/// an undeclared child that finished with a reply is ever asked.
pub(crate) fn wake_step(owed: &OwedReport, reply: &str, finish: TurnFinish) -> WakeStep {
    match finish {
        // A short-cut turn records a yes even over the agent's own no. The
        // fan-in honours it under every mode but `no-wake`.
        TurnFinish::CutShort => WakeStep::Record(WakeDecision::Requested),
        _ if owed.decision.is_some() || owed.holds_live_wait => WakeStep::Skip,
        TurnFinish::Finished(_) if reply.trim().is_empty() => {
            WakeStep::Record(WakeDecision::Requested)
        }
        TurnFinish::Finished(_) if owed.wake_mode.is_some() => WakeStep::Skip,
        TurnFinish::Finished(status) => WakeStep::Ask(status),
    }
}

impl crate::engine::LucidosEngine {
    /// Make the wake decision `owed` still lacks, and record it in the
    /// engine's name. Run before the turn's terminal event. Returns the
    /// decision the fan-in reads, which is `None` while it stays unset.
    pub(crate) async fn force_wake_decision(
        &self,
        thread_id: Uuid,
        owed: &OwedReport,
        turn: &TurnRead,
        finish: TurnFinish,
    ) -> Option<WakeDecision> {
        let decision = match wake_step(owed, &turn.reply, finish) {
            WakeStep::Record(decision) => decision,
            WakeStep::Skip => return owed.decision,
            WakeStep::Ask(status) => self.judge_wake(thread_id, owed, turn, status).await,
        };
        match record_wake_decision(&self.event_bus, thread_id, decision, None).await {
            Ok(()) => Some(decision),
            Err(e) => {
                crate::log!(
                    "[TurnEnd] Recording the wake decision for thread {} failed, so it wakes: {}",
                    thread_id,
                    e
                );
                None
            }
        }
    }

    /// One call to the `WakeDecision` judgment site.
    async fn judge_wake(
        &self,
        thread_id: Uuid,
        owed: &OwedReport,
        turn: &TurnRead,
        status: ChildCompletionStatus,
    ) -> WakeDecision {
        let call = self.aux_call(ContextPurpose::WakeDecision).await;
        let chat =
            (!asks_no_model(ContextPurpose::WakeDecision, call.selection().source)).then(|| {
                wake_decision_chat_judge(call.provider(), call.reasoning().map(str::to_string))
            });
        let system_one = self
            .turn_end_system_one(JudgmentSite::WakeDecision, call.attempt_timeout())
            .await;
        let Some(provider) = wake_judge(system_one, chat) else {
            crate::log!(
                "[TurnEnd] No measured wake-decision model is reachable for thread {}, so the \
                 parent wakes",
                thread_id
            );
            return WakeDecision::Requested;
        };
        let parent = ParentView::read(&self.pool, owed.parent_id, thread_id).await;
        let capture = AuxCapture::new(&self.event_bus, thread_id, ContextPurpose::WakeDecision);
        decide_with(
            provider.as_ref(),
            &capture,
            call.deadline(),
            wake_state(turn, status, &parent),
        )
        .await
    }
}

#[cfg(test)]
#[path = "wake_decision_tests.rs"]
mod tests;
