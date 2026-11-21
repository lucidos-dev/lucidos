//! The durable automation pause read by step admission.

use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::permission::request::{
    raise_request, RequestCard, RequestOption,
};
use crate::engine::thread_events::permission::{Action, Holder, Lifetime, PermissionChoice};
use crate::engine::thread_events::EventChannel;

/// An automation pause standing on a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationPause {
    pub reason: String,
    pub paused_event_id: Uuid,
}

/// The automation pause standing on `thread_id`, if its last pause event
/// paused it.
pub async fn automation_pause<'e>(
    pool: impl sqlx::PgExecutor<'e>,
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
            crate::log_legacy!(
                "[AutomationPause] pause for thread {} unreadable, treating as paused: {}",
                thread_id,
                e
            );
            true
        }
    }
}

/// An owner action holding an automatic wake.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepHold {
    Stop,
    AutomationPause(AutomationPause),
}

#[derive(Debug)]
pub enum StepHoldReadError {
    Stop(sqlx::Error),
    AutomationPause(sqlx::Error),
}

/// The hold that takes precedence for a wake that can run now.
pub async fn step_hold(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Option<StepHold>, StepHoldReadError> {
    let mut connection = pool.acquire().await.map_err(StepHoldReadError::Stop)?;
    if super::thread_stop::stopped_before(&mut connection, thread_id, i64::MAX)
        .await
        .map_err(StepHoldReadError::Stop)?
    {
        return Ok(Some(StepHold::Stop));
    }
    Ok(automation_pause(&mut *connection, thread_id)
        .await
        .map_err(StepHoldReadError::AutomationPause)?
        .map(StepHold::AutomationPause))
}

/// What an automatic step that came due may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepVerdict {
    /// No pause stands, so the step runs as it always did.
    Run,
    /// A pause stands. The owner has a request for the step, and the step
    /// waits for the answer.
    Asked,
    /// A human Stop holds the step without raising a permission request.
    Held,
}

/// One raise at a time, so two steps coming due together raise one request.
static RAISING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Ask before `step` runs on `thread_id`. A paused thread gets one open
/// request per step: a step that comes due again joins the open one.
pub async fn ask_before_step(bus: &EventBus, thread_id: Uuid, step: Action) -> StepVerdict {
    match step_hold(bus.pool(), thread_id).await {
        Ok(None) => return StepVerdict::Run,
        Ok(Some(StepHold::Stop)) => return StepVerdict::Held,
        Ok(Some(StepHold::AutomationPause(_))) => {}
        Err(StepHoldReadError::Stop(e)) => {
            crate::log_legacy!("[AutomationPause] State of {thread_id} unreadable, holding: {e}");
            return StepVerdict::Held;
        }
        Err(StepHoldReadError::AutomationPause(e)) => {
            crate::log_legacy!("[AutomationPause] State of {thread_id} unreadable, asking: {e}");
        }
    }
    let _raising = RAISING.lock().await;
    match open_step_request(bus.pool(), thread_id, step).await {
        Ok(Some(_)) => return StepVerdict::Asked,
        Ok(None) => {}
        Err(e) => crate::log_legacy!(
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
        crate::log_legacy!(
            "[AutomationPause] Could not ask for {step:?} on thread {thread_id}: {e}"
        );
    }
    StepVerdict::Asked
}

/// The open request for `step` on `thread_id`, by card id. A *queued card*
/// counts: the owner sees it once the open card closes.
pub async fn open_step_request(
    pool: &PgPool,
    thread_id: Uuid,
    step: Action,
) -> Result<Option<String>, sqlx::Error> {
    use crate::engine::question_park_sql::{
        unanswered_question_predicate_sql, waiting_queued_card_sql,
    };
    sqlx::query_scalar(&format!(
        "SELECT uqa.payload->>'tool_use_id' FROM events uqa \
         WHERE (({open}) OR (uqa.aggregate_id = $1 AND {queued})) \
           AND uqa.payload->'permission_request'->>'action' = $2 \
         ORDER BY uqa.sequence DESC LIMIT 1",
        open = unanswered_question_predicate_sql("$1"),
        queued = waiting_queued_card_sql("uqa"),
    ))
    .bind(thread_id.to_string())
    .bind(step_wire(step))
    .fetch_optional(pool)
    .await
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
        Action::BuildApprovedPlan => (
            "Send the agent back to build the plan? You approved building it in this \
             session, and the turn ended before any code changed.",
            "Build the plan",
            "Tell the agent to start the first phase, this once.",
            "Nothing runs. The plan stays approved and unbuilt.",
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
                choice: PermissionChoice::grant(Lifetime::Once, Holder::Thread { thread_id }),
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
