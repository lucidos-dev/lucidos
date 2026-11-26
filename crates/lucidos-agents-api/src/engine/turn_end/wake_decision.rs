use uuid::Uuid;

use super::read_decision::{TurnRead, PARENT_ACTS_READ_RULE, PARENT_MAY_ACT_READ_RULE};
use crate::engine::aux_purpose::asks_no_model;
use crate::engine::event_bus::{BusEvent, Delivery, EventBus};
use crate::engine::model_call::AuxCapture;
use crate::engine::thread_events::agent_thread_actor;
use crate::engine::thread_events::{
    ChildCompletionStatus, EventMeta, MessageOrigin, ThreadEvent, WakeMode,
};
use crate::engine::ContextPurpose;
use crate::llm::judgment::{
    for_site, Answers, ChatJudgmentProvider, JudgmentProvider, JudgmentSite, NoulCriteria, Question,
};
use crate::llm::tools::WAKE_ARG;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const WAKE_THRESHOLD: f64 = 0.3;

const WAKE_QUESTION_ID: &str = "parent_must_act";

const MESSAGE_BYTES: usize = 4_000;
const REPLY_BYTES: usize = 8_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WakeDecision {
    Requested,
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

    fn wake(self) -> bool {
        self == Self::Requested
    }

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

    fn from_probability(must_act: Option<f64>) -> Self {
        match must_act {
            Some(p) if p < WAKE_THRESHOLD => Self::NotRequested,
            _ => Self::Requested,
        }
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WakeDuty {
    NoParent,
    Declared(WakeMode),
    Decide,
}

impl WakeDuty {
    pub(crate) async fn of_thread(pool: &sqlx::PgPool, thread_id: Uuid) -> Self {
        let row = sqlx::query_as::<_, (Option<Uuid>, Option<String>)>(&format!(
            "SELECT CASE WHEN successor_thread_id IS NULL AND NOT {} THEN parent_thread_id END, \
                    wake_mode \
             FROM thread_summaries WHERE thread_id = $1",
            crate::engine::handoff_sql::pending_handoff_sql("$1")
        ))
        .bind(thread_id)
        .fetch_optional(pool)
        .await
        .unwrap_or_else(|e| {
            crate::log_legacy!("[TurnEnd] Parent of {} unreadable: {}", thread_id, e);
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

    pub(crate) fn read_rule(self) -> &'static str {
        match self {
            Self::Decide => PARENT_MAY_ACT_READ_RULE,
            duty if duty.parent_acts() => PARENT_ACTS_READ_RULE,
            _ => "",
        }
    }

    pub(crate) fn parent_acts(self) -> bool {
        matches!(
            self,
            Self::Declared(WakeMode::Wake | WakeMode::WakeWhenAllDone)
        )
    }

    pub(crate) async fn owes_a_decision(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
        Self::of_thread(pool, thread_id).await == Self::Decide
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnFinish {
    Finished(ChildCompletionStatus),
    CutShort,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OwedReport {
    parent_id: Uuid,
    decision: Option<WakeDecision>,
    wake_mode: Option<WakeMode>,
    batch_still_working: bool,
    holds_live_wait: bool,
}

impl OwedReport {
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
                crate::log_legacy!("[TurnEnd] Owed report of {} unreadable: {}", thread_id, e);
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

    pub(crate) fn wakes_parent(&self, finish: TurnFinish, decision: Option<WakeDecision>) -> bool {
        match finish {
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

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParentView {
    pub latest_reply: String,
    pub siblings_running: i64,
}

impl ParentView {
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
                crate::log_legacy!("[TurnEnd] Parent {} unreadable: {}", parent_id, e);
                None
            });
        row.map(|(latest_reply, siblings_running)| Self {
            latest_reply: latest_reply.unwrap_or_default(),
            siblings_running,
        })
        .unwrap_or_default()
    }
}

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

fn siblings_note(running: i64) -> String {
    match running {
        0 => "This child is the last of the parent's children to report. No other child is \
              still working."
            .to_string(),
        1 => "1 other child of the parent is still working.".to_string(),
        n => format!("{n} other children of the parent are still working."),
    }
}

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

pub(crate) fn read_wake_answer(answers: &Answers) -> WakeDecision {
    WakeDecision::from_probability(answers.noul(WAKE_QUESTION_ID))
}

pub fn wake_decision_chat_judge(
    provider: std::sync::Arc<dyn crate::llm::LlmProvider>,
    reasoning: Option<String>,
) -> ChatJudgmentProvider {
    ChatJudgmentProvider::new(provider, reasoning).forcing_the_answer()
}

pub struct EvalWakeDecision {
    pub wake: bool,
    pub probability: Option<f64>,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

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
        input_tokens: judgment.usage.unwrap_or_default().input_tokens,
        output_tokens: judgment.usage.unwrap_or_default().output_tokens,
    })
}

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
            crate::log_legacy!("[TurnEnd] Wake decision failed, so the parent wakes: {}", e);
            WakeDecision::Requested
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WakeStep {
    Record(WakeDecision),
    Skip,
    Ask(ChildCompletionStatus),
}

pub(crate) fn wake_step(owed: &OwedReport, reply: &str, finish: TurnFinish) -> WakeStep {
    match finish {
        TurnFinish::CutShort => WakeStep::Record(WakeDecision::Requested),
        _ if owed.decision.is_some() || owed.holds_live_wait => WakeStep::Skip,
        TurnFinish::Finished(_) if reply.trim().is_empty() => {
            WakeStep::Record(WakeDecision::Requested)
        }
        TurnFinish::Finished(_) if owed.wake_mode.is_some() => WakeStep::Skip,
        TurnFinish::Finished(status) => WakeStep::Ask(status),
    }
}

#[easy_ext::ext(WakeDecisionExt)]
pub(crate) impl crate::engine::LucidosEngine {
    /// INVARIANT: Make the wake decision `owed` still lacks, and record it in the
    /// engine's name. Run before the turn's terminal event. Returns the
    /// decision the fan-in reads, which is `None` while it stays unset.
    async fn force_wake_decision(
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
                crate::log_legacy!(
                    "[TurnEnd] Recording the wake decision for thread {} failed, so it wakes: {}",
                    thread_id,
                    e
                );
                None
            }
        }
    }

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
            .judgment_system_one(JudgmentSite::WakeDecision, call.attempt_timeout())
            .await;
        let Some(provider) = wake_judge(system_one, chat) else {
            crate::log_legacy!(
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
