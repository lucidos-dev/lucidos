//! The *automation pause*: a coding-agent thread whose automatic engine steps
//! ask the owner first, until the owner unpauses it.
//!
//! A checkpoint-and-pause order used to live only in chat text, so the engine
//! could not see it and its own turn-end nudge restarted the work. The pause is
//! that order as durable state. Like the *agent pause*, it lives only in
//! events: the last `AutomationPaused` or `AutomationUnpaused` on the thread wins.
//!
//! Pausing only restricts, so the caller's reach decides it. Unpausing is an
//! owner act, never an agent's relay. See
//! `docs/plans/2026-11-12-permissions-automation-pause-and-silent-drafts.md`.

use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::permission::{
    raise_request, Action, Holder, Lifetime, PermissionChoice, PermissionRequest, RequestCard,
    RequestOption, Resolution,
};
use crate::engine::thread_events::{
    AutomationUnpauseCause, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::engine::LucidosEngine;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// An automation pause standing on a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationPause {
    pub reason: String,
    pub paused_event_id: Uuid,
}

/// The automation pause standing on `thread_id`, if its last pause event
/// paused it.
pub async fn automation_pause(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<AutomationPause>, sqlx::Error> {
    let last: Option<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT id, event_type, payload->>'reason' FROM events \
         WHERE thread_id = $1 AND event_type IN ('AutomationPaused', 'AutomationUnpaused') \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await?;
    Ok(match last {
        Some((id, event_type, reason)) if event_type == "AutomationPaused" => {
            Some(AutomationPause {
                reason: reason.unwrap_or_default(),
                paused_event_id: id,
            })
        }
        _ => None,
    })
}

/// Whether an automatic step on `thread_id` must ask the owner first.
///
/// An unreadable pause counts as paused. An unasked step cannot be taken
/// back, while an asked one only waits for the owner's answer.
pub async fn automation_is_paused(pool: &PgPool, thread_id: Uuid) -> bool {
    match automation_pause(pool, thread_id).await {
        Ok(pause) => pause.is_some(),
        Err(e) => {
            crate::log!(
                "[AutomationPause] pause for thread {} unreadable, treating as paused: {}",
                thread_id,
                e
            );
            true
        }
    }
}

/// What a pause or unpause call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseChange {
    /// The call recorded a new pause event.
    Recorded,
    /// The thread was already in the asked state. Nothing was recorded.
    Unchanged,
}

/// Why a pause or unpause did not happen. The route answers `NotFound` with
/// 404, `Refused` with 400 and `Internal` with 500.
#[derive(Debug)]
pub enum PauseError {
    NotFound,
    /// The caller asked for something a pause does not allow.
    Refused(String),
    /// The database or the event bus failed.
    Internal(String),
}

impl PauseError {
    fn internal(error: impl std::fmt::Display) -> Self {
        Self::Internal(error.to_string())
    }
}

impl std::fmt::Display for PauseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => f.write_str("thread not found"),
            Self::Refused(why) | Self::Internal(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for PauseError {}

/// Pause automation on a coding-agent thread, as `actor`. A repeat on a
/// paused thread records nothing, so the first reason stands.
pub async fn pause_automation(
    pool: &PgPool,
    bus: &EventBus,
    thread_id: Uuid,
    reason: &str,
    actor: Option<MessageOrigin>,
) -> Result<PauseChange, PauseError> {
    require_pausable_thread(pool, thread_id).await?;
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(PauseError::Refused(
            "an automation pause needs a reason".into(),
        ));
    }
    if automation_pause(pool, thread_id)
        .await
        .map_err(PauseError::internal)?
        .is_some()
    {
        return Ok(PauseChange::Unchanged);
    }
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::AutomationPaused {
            reason: reason.to_string(),
        },
        meta: EventMeta::with_actor(actor),
    })
    .await
    .map_err(PauseError::internal)?;
    Ok(PauseChange::Recorded)
}

/// Unpause for `cause`. The caller has already checked that `cause` is the
/// owner's act. A thread with no pause records nothing.
pub async fn unpause_automation(
    pool: &PgPool,
    bus: &EventBus,
    thread_id: Uuid,
    cause: AutomationUnpauseCause,
    actor: Option<MessageOrigin>,
) -> Result<PauseChange, BoxError> {
    if automation_pause(pool, thread_id).await?.is_none() {
        return Ok(PauseChange::Unchanged);
    }
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::AutomationUnpaused { cause },
        meta: EventMeta::with_actor(actor),
    })
    .await?;
    Ok(PauseChange::Recorded)
}

/// Only a coding-agent thread has automatic engine steps to pause. The
/// unpause route checks this too, so both verbs refuse the same threads.
pub async fn require_pausable_thread(pool: &PgPool, thread_id: Uuid) -> Result<(), PauseError> {
    let source: Option<String> =
        sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await
            .map_err(PauseError::internal)?;
    match source.as_deref() {
        Some("claude_code") => Ok(()),
        Some(_) => Err(PauseError::Refused(
            "an automation pause applies to coding-agent threads only".into(),
        )),
        None => Err(PauseError::NotFound),
    }
}

/// What an automatic step that came due may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StepVerdict {
    /// No pause stands, so the step runs as it always did.
    Run,
    /// A pause stands. The owner has a request for the step, and the step
    /// waits for the answer.
    Asked,
}

/// One raise at a time, so two steps coming due together raise one request.
static RAISING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Ask before `step` runs on `thread_id`. A paused thread gets one open
/// request per step: a step that comes due again joins the open one.
pub(crate) async fn ask_before_step(bus: &EventBus, thread_id: Uuid, step: Action) -> StepVerdict {
    if !automation_is_paused(bus.pool(), thread_id).await {
        return StepVerdict::Run;
    }
    let _raising = RAISING.lock().await;
    match open_step_request(bus.pool(), thread_id, step).await {
        Ok(Some(_)) => return StepVerdict::Asked,
        Ok(None) => {}
        Err(e) => crate::log!(
            "[AutomationPause] Open requests on thread {thread_id} unreadable, raising one: {e}"
        ),
    }
    if let Err(e) = raise_request(
        bus,
        thread_id,
        step_card(thread_id, step),
        EventChannel::ClaudeCode,
    )
    .await
    {
        crate::log!("[AutomationPause] Could not ask for {step:?} on thread {thread_id}: {e}");
    }
    StepVerdict::Asked
}

/// The open request for `step` on `thread_id`, by card id.
pub(crate) async fn open_step_request(
    pool: &PgPool,
    thread_id: Uuid,
    step: Action,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(&format!(
        "SELECT uqa.payload->>'tool_use_id' FROM events uqa \
         WHERE {} AND uqa.payload->'permission_request'->>'action' = $2 \
         ORDER BY uqa.sequence DESC LIMIT 1",
        crate::engine::agent_recovery::unanswered_question_predicate_sql("$1")
    ))
    .bind(thread_id.to_string())
    .bind(step_wire(step))
    .fetch_optional(pool)
    .await
}

/// Every step request on `thread_id` the owner can still tap, oldest first,
/// by card id: unanswered, and no later turn overtook it. Another card's
/// answer leaves it live, as the card renders.
async fn open_step_requests(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Vec<(String, Action)>, sqlx::Error> {
    let rows: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT q.payload->>'tool_use_id', q.payload->'permission_request'->'action' \
         FROM events q \
         WHERE q.aggregate_id = $1 AND q.event_type = 'UserQuestionAsked' \
           AND q.payload ? 'permission_request' \
           AND NOT EXISTS (SELECT 1 FROM events a \
                            WHERE a.aggregate_id = $1 AND a.event_type = 'UserQuestionAnswered' \
                              AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id') \
           AND NOT EXISTS (SELECT 1 FROM events t \
                            WHERE t.aggregate_id = $1 AND t.sequence > q.sequence \
                              AND t.event_type = ANY($2)) \
         ORDER BY q.sequence",
    )
    .bind(thread_id.to_string())
    .bind(ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(tool_use_id, action)| {
            let step = serde_json::from_value::<Action>(action).ok()?;
            step.is_engine_step().then_some((tool_use_id, step))
        })
        .collect())
}

fn step_wire(step: Action) -> String {
    serde_json::to_value(step)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The engine's words for a step request: the step once, Unpause, or Keep
/// paused.
fn step_card(thread_id: Uuid, step: Action) -> RequestCard {
    let (question, once, once_detail, keep_detail) = match step {
        Action::Harden => (
            "Continue hardening? Hardening is incomplete, and nothing is applied.",
            "Continue hardening",
            "Send the agent back to run /harden, this once.",
            "Nothing runs. The work stays unproposed until you press Harden or unpause.",
        ),
        Action::SetPlanMarker => (
            "Send the agent back to its plan? The work needs a plan marker before \
             Lucidos proposes it.",
            "Return to the plan",
            "Send the agent back to settle its plan marker, this once.",
            "Nothing runs. The work stays unproposed.",
        ),
        Action::HandOff => (
            "Hand off the session? It passed its context ceiling.",
            "Hand off now",
            "Ask the agent to write its handoff, this once.",
            "Nothing runs. Lucidos asks again when the next turn ends.",
        ),
        _ => (
            "Wake the agent to read what arrived? Reports are waiting.",
            "Wake it once",
            "The agent reads every report waiting now, this once.",
            "The reports wait. Your next message hands child reports over.",
        ),
    };
    RequestCard {
        question: format!("{question} Automatic steps on this thread are paused."),
        header: "Paused",
        action: step,
        breadth: None,
        options: vec![
            RequestOption {
                label: once.to_string(),
                description: format!("{once_detail} The thread stays paused."),
                choice: PermissionChoice::Grant {
                    lifetime: Lifetime::Once,
                    holder: Holder::Thread { thread_id },
                },
            },
            RequestOption {
                label: "Unpause".to_string(),
                description: "End the pause. This step runs now, and later steps run \
                              without asking."
                    .to_string(),
                choice: PermissionChoice::Unpause,
            },
            RequestOption {
                label: "Keep paused".to_string(),
                description: keep_detail.to_string(),
                choice: PermissionChoice::Refuse,
            },
        ],
        batch: None,
    }
}

/// The step a queued engine nudge takes, when a pause placed after it was
/// queued must hold it back (I2). Only the turn-end re-entries count: each
/// one's anchor names its engine reason.
pub(crate) async fn withheld_queued_step(
    pool: &PgPool,
    thread_id: Uuid,
    input: &crate::engine::AgentUserInput,
) -> Option<Action> {
    use crate::engine::AgentInputKind;
    if !matches!(
        input.kind,
        AgentInputKind::ReentryFromEngine | AgentInputKind::CeilingReentry
    ) {
        return None;
    }
    let anchor = input.origin_event_id?;
    let pause = match automation_pause(pool, thread_id).await {
        Ok(pause) => pause?,
        Err(e) => {
            crate::log!("[AutomationPause] Pause of {thread_id} unreadable at the forward: {e}");
            return None;
        }
    };
    let kind: Option<String> = sqlx::query_scalar(
        "SELECT a.payload->'origin'->'reason'->>'kind' FROM events a \
         WHERE a.id = $1 AND a.event_type = 'PromptInjected' \
           AND a.sequence < (SELECT p.sequence FROM events p WHERE p.id = $2)",
    )
    .bind(anchor)
    .bind(pause.paused_event_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        crate::log!("[AutomationPause] Anchor {anchor} unreadable at the forward: {e}");
        None
    })
    .flatten();
    let kind = kind?;
    use crate::engine::thread_events::EngineReason;
    [
        (EngineReason::MissingHardening, Action::Harden),
        (EngineReason::MissingPlan, Action::SetPlanMarker),
        (EngineReason::ContextCeiling, Action::HandOff),
    ]
    .into_iter()
    .find(|(reason, _)| {
        serde_json::to_value(reason)
            .ok()
            .is_some_and(|v| v.get("kind").and_then(|k| k.as_str()) == Some(kind.as_str()))
    })
    .map(|(_, step)| step)
}

/// The event-wait re-entries a pause withheld on `thread_id`: their anchors
/// landed after the last pause, and the agent never read them.
async fn withheld_wait_reentries(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Vec<crate::engine::event_wait::WaitReentry>, sqlx::Error> {
    let rows: Vec<(Uuid, Option<String>)> = sqlx::query_as(
        "SELECT p.id, p.payload->>'text' FROM events p \
         WHERE p.thread_id = $1 AND p.event_type = 'PromptInjected' \
           AND p.payload->'origin'->'reason'->>'kind' = $2 \
           AND p.sequence > COALESCE(( \
             SELECT max(z.sequence) FROM events z \
             WHERE z.thread_id = $1 AND z.event_type = 'AutomationPaused'), 0) \
           AND NOT EXISTS ( \
             SELECT 1 FROM events r \
             WHERE r.thread_id = $1 AND r.event_type = 'CodingAgentInputRead' \
               AND r.payload->>'input_event_id' = p.id::text) \
         ORDER BY p.sequence",
    )
    .bind(thread_id)
    .bind(crate::engine::thread_events::EVENT_WAIT_REASON_KIND)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(anchor_event_id, text)| crate::engine::event_wait::WaitReentry {
                anchor_event_id,
                text: text.unwrap_or_default(),
            },
        )
        .collect())
}

impl LucidosEngine {
    /// Act on the owner's answer to a step request. A grant runs the step
    /// once and spends the grant. An unpause runs it too, since the owner
    /// ended the pause it waited behind.
    pub(crate) async fn act_on_step_answer(
        &self,
        thread_id: Uuid,
        request: &PermissionRequest,
        resolution: &Resolution,
        actor: Option<MessageOrigin>,
    ) {
        let step = request.action;
        if !step.is_engine_step() {
            return;
        }
        match resolution {
            Resolution::Granted(permission_id) => {
                let permission =
                    match crate::engine::permission::permission_row(self.pool(), *permission_id)
                        .await
                    {
                        Ok(Some(permission)) => permission,
                        Ok(None) => return,
                        Err(e) => {
                            crate::log!(
                            "[AutomationPause] Grant for {step:?} on {thread_id} unreadable: {e}"
                        );
                            return;
                        }
                    };
                if !crate::engine::permission::spend(&self.event_bus, &permission).await {
                    return;
                }
            }
            Resolution::Unpaused => {}
            Resolution::Refused | Resolution::Ignored => return,
        }
        let unpaused = *resolution == Resolution::Unpaused;
        // A step can run a whole turn, which the answer must not wait for.
        match self.try_clone_arc() {
            Some(engine) => {
                tokio::spawn(async move {
                    engine.run_step_or_log(thread_id, step, actor.clone()).await;
                    if unpaused {
                        engine
                            .release_paused_steps(thread_id, Some(step), actor)
                            .await;
                    }
                });
            }
            None => self.run_step_or_log(thread_id, step, actor).await,
        }
    }

    /// Settle what a pause left waiting once the owner ends it. Each other
    /// open step request closes, and its step runs, since it waited behind the
    /// pause. Withheld event-wait deliveries re-enter even with no reports
    /// request open. `ran` is the step the owner's Unpause already ran.
    /// Boxed, because closing a request answers a card, whose answer path
    /// calls back into this.
    pub(crate) fn release_paused_steps(
        self: &std::sync::Arc<Self>,
        thread_id: Uuid,
        ran: Option<Action>,
        actor: Option<MessageOrigin>,
    ) -> futures::future::BoxFuture<'_, ()> {
        Box::pin(self.release_paused_steps_unboxed(thread_id, ran, actor))
    }

    async fn release_paused_steps_unboxed(
        self: &std::sync::Arc<Self>,
        thread_id: Uuid,
        ran: Option<Action>,
        actor: Option<MessageOrigin>,
    ) {
        use crate::engine::agent_question::{answer_pending_question, AnswerResult};
        let open = open_step_requests(self.pool(), thread_id)
            .await
            .unwrap_or_else(|e| {
                crate::log!("[AutomationPause] Open requests on {thread_id} unreadable: {e}");
                Vec::new()
            });
        let mut reports_read = ran == Some(Action::ReadWaitingReports);
        for (tool_use_id, step) in open {
            let closed = answer_pending_question(
                self,
                thread_id,
                tool_use_id,
                crate::engine::thread_events::AnswerKind::Superseded,
                actor.clone(),
            )
            .await;
            if !matches!(closed, AnswerResult::Resumed) {
                continue;
            }
            reports_read |= step == Action::ReadWaitingReports;
            self.run_step_or_log(thread_id, step, actor.clone()).await;
        }
        if reports_read {
            return;
        }
        match withheld_wait_reentries(self.pool(), thread_id).await {
            Ok(withheld) => {
                for reentry in withheld {
                    self.run_wait_reentry(thread_id, reentry, true).await;
                }
            }
            Err(e) => crate::log!("[AutomationPause] Withheld deliveries on {thread_id}: {e}"),
        }
    }

    /// Boxed, because a step's turn can answer a card, whose answer runs a
    /// step: the future type would otherwise contain itself.
    fn run_step_or_log(
        &self,
        thread_id: Uuid,
        step: Action,
        actor: Option<MessageOrigin>,
    ) -> futures::future::BoxFuture<'_, ()> {
        Box::pin(async move {
            if let Err(e) = self.run_engine_step(thread_id, step, actor).await {
                crate::log!("[AutomationPause] {step:?} on thread {thread_id} did not run: {e}");
            }
        })
    }

    /// Run one automatic step the owner allowed. A live session takes it in
    /// session. Otherwise the thread resumes with the step's prompt.
    pub(crate) async fn run_engine_step(
        &self,
        thread_id: Uuid,
        step: Action,
        actor: Option<MessageOrigin>,
    ) -> Result<(), BoxError> {
        use crate::engine::agent_recovery::{
            HANDOFF_REQUESTED_REASON, PLAN_REQUESTED_REASON, REPORTS_REQUESTED_MESSAGE,
            REPORTS_REQUESTED_REASON,
        };
        use crate::engine::change_ops::CodingAgentChangeOps;
        let live = CodingAgentChangeOps::live_session_info(self, thread_id)
            .await
            .map(|session| session.msg_tx);
        let reason = match step {
            Action::Harden => return self.harden_held_work(thread_id, actor).await,
            Action::SetPlanMarker => {
                let Some(hold) = crate::engine::change_ops::held_plan(self.pool(), thread_id).await
                else {
                    return Ok(());
                };
                if let Some(tx) = live {
                    return self
                        .nudge_held_proposal_in_session(
                            thread_id,
                            crate::engine::change_ops::ProposalHold::Plan(hold),
                            &tx,
                        )
                        .await;
                }
                PLAN_REQUESTED_REASON
            }
            Action::HandOff => {
                use crate::engine::context_ceiling as ceiling;
                let crossing = match ceiling::past_ceiling(self.pool(), thread_id).await? {
                    Some(crossing) => crossing,
                    None => {
                        let Some(crossing) =
                            ceiling::ceiling_crossing(self.pool(), thread_id, false).await
                        else {
                            return Ok(());
                        };
                        ceiling::record_ceiling_reached(&self.event_bus, thread_id, crossing)
                            .await?;
                        crossing
                    }
                };
                if let Some(tx) = live {
                    let input =
                        ceiling::ceiling_reentry_input(&self.event_bus, thread_id, crossing)
                            .await?;
                    return tx.send(input).map_err(|_| "Session channel closed".into());
                }
                HANDOFF_REQUESTED_REASON
            }
            Action::ReadWaitingReports => {
                let withheld = withheld_wait_reentries(self.pool(), thread_id).await?;
                if !withheld.is_empty() {
                    for reentry in withheld {
                        self.run_wait_reentry(thread_id, reentry, true).await;
                    }
                    return Ok(());
                }
                if let Some(tx) = live {
                    return tx
                        .send(crate::engine::AgentUserInput {
                            text: REPORTS_REQUESTED_MESSAGE.to_string(),
                            images: None,
                            origin_event_id: None,
                            kind: crate::engine::AgentInputKind::ReentryFromEngine,
                            completion: None,
                        })
                        .map_err(|_| "Session channel closed".into());
                }
                REPORTS_REQUESTED_REASON
            }
            other => return Err(format!("{other:?} is not an automatic step").into()),
        };
        if crate::engine::thread_events::emit_continuation_requested_or_log(
            &self.event_bus,
            thread_id,
            reason,
            actor,
            "[AutomationPause] ContinuationRequested",
        )
        .await
        {
            Ok(())
        } else {
            Err(format!("the {reason} resume could not be requested").into())
        }
    }
}

#[cfg(test)]
#[path = "automation_pause_tests.rs"]
mod tests;
