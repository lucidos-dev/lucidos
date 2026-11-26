use std::sync::Arc;

use uuid::Uuid;

use crate::engine::chat::process::ChatProcessExt;
use crate::engine::event_bus::EventBus;
use crate::engine::permission::menu::{
    ALLOW_ONCE_LABEL, ALWAYS_ALLOW_LABEL, ALWAYS_DENY_LABEL, DONT_ALLOW_LABEL,
};
use crate::engine::permission::remote_start::{thread_start, ThreadStart};
use crate::engine::permission::{
    raise_turn_holding_request, Action, Holder, Lifetime, PermissionChoice, PermissionRequest,
    RequestCard, RequestOption,
};
use crate::engine::start_failure::StartAwaitsOwner;
use crate::engine::thread_events::{
    ActorMode, AnswerKind, CancelCause, EventChannel, EventMeta, MessageOrigin, RemoteStartAsk,
};
use crate::engine::LucidosEngine;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const START_CARD_HEADER: &str = "Remote start";

#[derive(Debug, PartialEq)]
enum StartState {
    NotAsked,
    Unasked(RemoteStartAsk),
    Open,
    Allowed,
    Closed,
}

async fn start_card(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<Option<(PermissionRequest, Option<AnswerKind>)>, BoxError> {
    let row: Option<(serde_json::Value, Option<serde_json::Value>)> = sqlx::query_as(&format!(
        "SELECT q.payload->'permission_request', a.payload->'answer' FROM events q \
         LEFT JOIN events a ON a.aggregate_id = q.aggregate_id \
              AND a.event_type = 'UserQuestionAnswered' \
              AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id' \
         WHERE q.aggregate_id = $1 AND q.event_type IN ({}) \
           AND q.payload->'permission_request'->>'action' = 'start-remote-thread' \
         ORDER BY q.sequence ASC LIMIT 1",
        crate::engine::question_park_sql::CARD_EVENT_TYPES_SQL,
    ))
    .bind(thread_id.to_string())
    .fetch_optional(pool)
    .await?;
    let Some((request, answer)) = row else {
        return Ok(None);
    };
    Ok(Some((
        serde_json::from_value(request)?,
        answer.map(serde_json::from_value).transpose()?,
    )))
}

fn chosen(request: &PermissionRequest, answer: &AnswerKind) -> Option<PermissionChoice> {
    match answer {
        AnswerKind::Selected { option_id } => request.choices.get(option_id).cloned(),
        _ => None,
    }
}

async fn start_state(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<StartState, BoxError> {
    let Some(pairing) = thread_start(pool, thread_id)
        .await?
        .and_then(|start| start.pairing())
    else {
        return Ok(StartState::NotAsked);
    };
    Ok(match start_card(pool, thread_id).await? {
        None => StartState::Unasked(pairing),
        Some((_, None)) => StartState::Open,
        Some((request, Some(answer))) => match chosen(&request, &answer) {
            Some(PermissionChoice::Grant { .. }) => StartState::Allowed,
            _ => StartState::Closed,
        },
    })
}

fn card(thread_id: Uuid, pairing: &RemoteStartAsk) -> RequestCard {
    let label = &pairing.pairing_label;
    let setting = "under Settings → Access → Pair a gateway";
    RequestCard {
        question: crate::engine::permission::PermissionCardText::new(&format!(
            "Let an agent on \u{201c}{}\u{201d} start this thread here?",
            crate::engine::permission::one_line(label)
        ))
        .paragraph("The thread runs once you allow it.")
        .finish(None),
        header: START_CARD_HEADER,
        action: Action::StartRemoteThread,
        breadth: Some(label.clone()),
        options: vec![
            RequestOption {
                label: ALLOW_ONCE_LABEL.into(),
                description: format!(
                    "Run this thread now. Later starts from \"{label}\" still ask."
                ),
                choice: PermissionChoice::grant(Lifetime::Once, Holder::Thread { thread_id }),
            },
            RequestOption {
                label: ALWAYS_ALLOW_LABEL.into(),
                description: format!(
                    "Run this thread now, and let agents on \"{label}\" start threads here \
                     without asking. You can change this {setting}."
                ),
                choice: PermissionChoice::grant(Lifetime::Standing, Holder::Workspace),
            },
            RequestOption {
                label: ALWAYS_DENY_LABEL.into(),
                description: format!(
                    "End this thread unstarted, and refuse later starts from \"{label}\". You \
                     can change this {setting}."
                ),
                choice: PermissionChoice::RefuseStanding,
            },
            RequestOption {
                label: DONT_ALLOW_LABEL.into(),
                description: format!(
                    "End this thread unstarted. Later starts from \"{label}\" still ask."
                ),
                choice: PermissionChoice::Refuse,
            },
        ],
        batch: None,
    }
}

pub(crate) const START_REFUSED_ERROR: &str =
    "This workspace's owner did not allow this remote start, so the thread does not run.";

pub(crate) async fn gate_remote_start(
    bus: &EventBus,
    thread_id: Uuid,
    channel: EventChannel,
) -> Result<(), BoxError> {
    match start_state(bus.pool(), thread_id).await? {
        StartState::NotAsked | StartState::Allowed => Ok(()),
        StartState::Unasked(pairing) => {
            raise_turn_holding_request(bus, thread_id, card(thread_id, &pairing), channel).await?;
            crate::log_legacy!(
                "[SpawnGate] Thread {thread_id} waits for the owner to allow its remote start"
            );
            Err(Box::new(StartAwaitsOwner))
        }
        StartState::Open => Err(Box::new(StartAwaitsOwner)),
        StartState::Closed => Err(START_REFUSED_ERROR.into()),
    }
}

pub(crate) async fn spend_is_held(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    origin: Option<&MessageOrigin>,
) -> bool {
    let asks = |origin: &MessageOrigin| {
        matches!(
            origin,
            MessageOrigin::Workspace {
                remote_start: Some(_),
                ..
            }
        )
    };
    match start_state(pool, thread_id).await {
        Ok(StartState::Allowed) => false,
        Ok(StartState::NotAsked) => {
            origin.is_some_and(asks) && !matches!(thread_start(pool, thread_id).await, Ok(Some(_)))
        }
        Ok(_) => true,
        Err(e) => {
            crate::log_legacy!(
                "[SpawnGate] Could not read the start of thread {thread_id}, so it spends nothing: {e}"
            );
            true
        }
    }
}

async fn start_lane(pool: &sqlx::PgPool, thread_id: Uuid) -> Result<EventChannel, BoxError> {
    let channel: Option<String> = sqlx::query_scalar(&format!(
        "SELECT payload->>'channel' FROM events \
         WHERE aggregate_id = $1 AND event_type IN ({}) \
           AND payload->'permission_request'->>'action' = 'start-remote-thread' \
         ORDER BY sequence ASC LIMIT 1",
        crate::engine::question_park_sql::CARD_EVENT_TYPES_SQL,
    ))
    .bind(thread_id.to_string())
    .fetch_optional(pool)
    .await?
    .flatten();
    channel
        .as_deref()
        .and_then(EventChannel::from_wire)
        .ok_or_else(|| format!("the start card of thread {thread_id} names no lane").into())
}

const START_BEGAN_EVENT_TYPES_SQL: &str = "'ContinuationRequested','ContinuationStarted',\
    'ContextCaptured','ResponseGenerated','ResponseFailed','ResponseCanceled',\
    'CodingAgentInputRead','CodingAgentTextStreamed','CodingAgentToolCalled'";

async fn answered_unbegun_starts(
    pool: &sqlx::PgPool,
) -> Result<Vec<(Uuid, Option<serde_json::Value>)>, BoxError> {
    Ok(sqlx::query_as(&format!(
        "SELECT ts.thread_id, a.payload->'actor' FROM events q \
         JOIN events a ON a.aggregate_id = q.aggregate_id \
              AND a.event_type = 'UserQuestionAnswered' \
              AND a.payload->>'tool_use_id' = q.payload->>'tool_use_id' \
         JOIN thread_summaries ts ON ts.thread_id::text = q.aggregate_id \
         WHERE q.event_type IN ({}) \
           AND q.payload->'permission_request'->>'action' = 'start-remote-thread' \
           AND ts.archive_state IS DISTINCT FROM 'archived' \
           AND NOT EXISTS (SELECT 1 FROM events b \
                WHERE b.aggregate_id = q.aggregate_id AND b.sequence > a.sequence \
                  AND b.event_type IN ({START_BEGAN_EVENT_TYPES_SQL}))",
        crate::engine::question_park_sql::CARD_EVENT_TYPES_SQL,
    ))
    .fetch_all(pool)
    .await?)
}

pub async fn begin_allowed_starts_on_startup(engine: &Arc<LucidosEngine>) {
    let rows = match answered_unbegun_starts(engine.pool()).await {
        Ok(rows) => rows,
        Err(e) => {
            crate::log_legacy!("[SpawnGate] Could not look for allowed starts to begin: {e}");
            return;
        }
    };
    for (thread_id, actor) in rows {
        match start_state(engine.pool(), thread_id).await {
            Ok(StartState::Allowed) => {}
            Ok(_) => continue,
            Err(e) => {
                crate::log_legacy!(
                    "[SpawnGate] Could not read the start of thread {thread_id}: {e}"
                );
                continue;
            }
        }
        let start = match thread_start(engine.pool(), thread_id).await {
            Ok(Some(start)) => start,
            Ok(None) => continue,
            Err(e) => {
                crate::log_legacy!(
                    "[SpawnGate] Could not read the start of thread {thread_id}: {e}"
                );
                continue;
            }
        };
        let actor = actor.and_then(|v| serde_json::from_value(v).ok());
        crate::log_legacy!(
            "[SpawnGate] Thread {thread_id} was allowed to start and never began, so it begins now"
        );
        let engine = engine.clone();
        tokio::spawn(async move {
            if let Err(e) = engine.begin_allowed_start(thread_id, start, actor).await {
                crate::log_legacy!(
                    "[SpawnGate] Could not begin the allowed start of thread {thread_id}: {e}"
                );
            }
        });
    }
}

#[easy_ext::ext(RemoteStartGateExt)]
pub(crate) impl LucidosEngine {
    fn spawn_remote_start_answer(
        self: &Arc<Self>,
        thread_id: Uuid,
        choice: Option<PermissionChoice>,
        actor: Option<MessageOrigin>,
    ) {
        let engine = self.clone();
        tokio::spawn(async move {
            if let Err(e) = engine.act_on_start_answer(thread_id, choice, actor).await {
                crate::log_legacy!(
                    "[SpawnGate] Could not act on the start answer for thread {thread_id}: {e}"
                );
            }
        });
    }

    async fn act_on_start_answer(
        self: &Arc<Self>,
        thread_id: Uuid,
        choice: Option<PermissionChoice>,
        actor: Option<MessageOrigin>,
    ) -> Result<(), BoxError> {
        let start = thread_start(self.pool(), thread_id)
            .await?
            .ok_or("the thread has no parked start")?;
        match choice {
            Some(PermissionChoice::Grant { .. }) => {
                self.begin_allowed_start(thread_id, start, actor).await
            }
            Some(PermissionChoice::Refuse | PermissionChoice::RefuseStanding) => {
                self.refuse_start(thread_id, &start, actor).await;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    async fn begin_allowed_start(
        self: &Arc<Self>,
        thread_id: Uuid,
        start: ThreadStart,
        actor: Option<MessageOrigin>,
    ) -> Result<(), BoxError> {
        if start_lane(self.pool(), thread_id).await? == EventChannel::ClaudeCode {
            crate::engine::thread_events::emit_continuation_requested_or_log(
                &self.event_bus,
                thread_id,
                crate::engine::continuation_reason::OWNER_ALLOWED_START_REASON,
                actor,
                "[SpawnGate] ContinuationRequested",
            )
            .await;
            return Ok(());
        }
        let origin = start.origin();
        let mode = origin
            .as_ref()
            .map_or(ActorMode::Agent, MessageOrigin::mode);
        let images = crate::engine::chat::queued_recovery::images_for_row(
            self.workspace_path(),
            &start.payload,
        );
        let (text, model, effort, provider) = (
            start.text_field("text").unwrap_or_default(),
            start.text_field("model"),
            start.text_field("reasoning_effort"),
            start.text_field("provider"),
        );
        self.process_message_with_steps(
            &text,
            model.as_deref(),
            None,
            None,
            effort.as_deref(),
            provider.as_deref(),
            images.as_deref(),
            None,
            None,
            None,
            Some(thread_id),
            None,
            None,
            None,
            None,
            None,
            None,
            mode,
            None,
            None,
            Some(crate::engine::PreEmittedOrigin::Message(start.origin_id)),
            None,
            origin,
            None,
            crate::engine::FollowUpUrgency::Normal,
            None,
        )
        .await
        .map(|_| ())
    }

    async fn refuse_start(
        &self,
        thread_id: Uuid,
        start: &ThreadStart,
        actor: Option<MessageOrigin>,
    ) {
        crate::engine::thread_events::emit_response_canceled(
            &self.event_bus,
            self.pool(),
            thread_id,
            CancelCause::Refused,
            String::new(),
            Vec::new(),
            None,
            None,
            EventMeta {
                request_event_id: Some(start.origin_id),
                actor,
                ..EventMeta::NONE
            },
            "[SpawnGate] ResponseCanceled",
        )
        .await;
    }
}

#[cfg(test)]
#[path = "spawn_gate_remote_start_tests.rs"]
mod tests;
