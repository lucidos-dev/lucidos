//! People-only messages and the agent pause at the API (ADR 0649).
//!
//! A chat send on a team gateway may carry *person mentions* and an *agent
//! mention*. The engine reads the pause, flips it as the route says, and either
//! takes the ordinary path or records a `PeopleMessagePosted` that starts no
//! turn. `/api/v1/threads/:thread_id/agent-pause` is the chip: GET reads the
//! pause, POST flips it.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::error::ApiError;
use super::AppState;
use crate::engine::agent_pause::{self, MessageAudience};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, AgentPauseCause, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::engine::thread_people;
use crate::gateway_operators::OperatorSnapshot;

/// The 400 for mentions on a gateway with fewer than two people.
const NO_TEAM: &str = "Mentioning a person needs a gateway with two or more people.";

/// The 503 while the gateway has not named its people yet.
fn operators_unknown() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "Lucidos is still reading who is on this gateway. Send again in a moment.",
    )
    .with_reason("operators_unknown")
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route(
        "/threads/:thread_id/agent-pause",
        get(read_agent_pause).post(set_agent_pause),
    )
}

/// The mentions a chat send carries, checked against the gateway's people.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PeopleAddressing {
    /// Operator ids, each once, in the order sent.
    pub mentions: Vec<String>,
    pub agent_mentioned: bool,
}

/// Check a send's mentions. `None` takes the ordinary path untouched: a
/// gateway with one person, and every input that is not a person typing.
pub(super) fn people_addressing(
    mentions: Vec<String>,
    agent_mentioned: bool,
    mode: ActorMode,
    snapshot: &OperatorSnapshot,
) -> Result<Option<PeopleAddressing>, ApiError> {
    if snapshot.person_count() < 2 {
        return match mentions.is_empty() {
            true => Ok(None),
            false => Err(ApiError::bad_request(NO_TEAM).with_reason("no_team")),
        };
    }
    if mode != ActorMode::Human {
        return match mentions.is_empty() && !agent_mentioned {
            true => Ok(None),
            false => Err(ApiError::bad_request(
                "Only a person's message can carry mentions.",
            )),
        };
    }
    let people = snapshot.person_ids();
    let mut unique: Vec<String> = Vec::with_capacity(mentions.len());
    for id in mentions {
        if !people.contains(id.as_str()) {
            return Err(ApiError::bad_request(format!(
                "No person {id} on this gateway."
            )));
        }
        if !unique.contains(&id) {
            unique.push(id);
        }
    }
    Ok(Some(PeopleAddressing {
        mentions: unique,
        agent_mentioned,
    }))
}

/// [`people_addressing`] for a chat send, with the gateway's current list.
///
/// While the gateway has not answered, a team would read as one person. A
/// person's send that a team would route differently is then refused with a
/// 503: one with mentions, or one to a paused thread. Every other send takes
/// the ordinary path, as a team's would.
pub(super) async fn addressing_of_send(
    pool: &PgPool,
    thread_id: Option<Uuid>,
    mentions: Vec<String>,
    agent_mentioned: bool,
    mode: ActorMode,
) -> Result<Option<PeopleAddressing>, ApiError> {
    if let Some(snapshot) = crate::gateway_operators::known() {
        return people_addressing(mentions, agent_mentioned, mode, &snapshot);
    }
    if mode != ActorMode::Human {
        return Ok(None);
    }
    if !mentions.is_empty() {
        return Err(operators_unknown());
    }
    let paused = match thread_id {
        Some(tid) => agent_pause::agent_paused(pool, tid)
            .await
            .map_err(ApiError::db)?,
        None => false,
    };
    match paused {
        true => Err(operators_unknown()),
        false => Ok(None),
    }
}

/// Refuse a people-only message that would create a coding-agent thread with
/// no session. A thread not started yet is never paused and has no open
/// question, so the mentions alone decide it, before anything records.
pub(super) fn refuse_people_only_start(
    addressing: Option<&PeopleAddressing>,
    starts_coding_agent: bool,
) -> Result<(), ApiError> {
    let Some(addressing) = addressing.filter(|_| starts_coding_agent) else {
        return Ok(());
    };
    let route = agent_pause::people_message_route(
        !addressing.mentions.is_empty(),
        addressing.agent_mentioned,
        false,
        false,
    );
    match route.audience {
        MessageAudience::PeopleOnly => Err(ApiError::bad_request(
            "A coding-agent thread starts with a message to its agent. Mention the agent, or start a chat thread.",
        )),
        MessageAudience::Agent => Ok(()),
    }
}

/// Route one person's message: flip the pause as the route says, and answer
/// who the message is for.
pub(super) async fn route_people_message(
    pool: &PgPool,
    bus: &EventBus,
    thread_id: Uuid,
    addressing: &PeopleAddressing,
    actor: Option<MessageOrigin>,
) -> Result<MessageAudience, ApiError> {
    let paused = agent_pause::agent_paused(pool, thread_id)
        .await
        .map_err(ApiError::db)?;
    let question_open = paused
        && crate::engine::agent_question::lookup_active_question_tool_use_id(pool, thread_id)
            .await
            .is_some();
    let route = agent_pause::people_message_route(
        !addressing.mentions.is_empty(),
        addressing.agent_mentioned,
        paused,
        question_open,
    );
    if let Some(change) = route.pause_change {
        agent_pause::record_pause_change(bus, thread_id, change, AgentPauseCause::Mention, actor)
            .await
            .map_err(|e| ApiError::internal(e.to_string()))?;
    }
    Ok(route.audience)
}

/// One people-only message, as the send recorded it.
pub(super) struct PeopleMessage {
    pub text: String,
    pub image_hashes: Vec<String>,
    pub device_id: Option<String>,
    pub mentions: Vec<String>,
    pub event_id: Option<Uuid>,
}

/// Record a people-only message. It starts no turn.
pub(super) async fn post_people_message(
    bus: &EventBus,
    thread_id: Uuid,
    message: PeopleMessage,
    actor: Option<MessageOrigin>,
) -> Result<(), ApiError> {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::PeopleMessagePosted {
            text: message.text,
            user_image_hashes: message.image_hashes,
            device_id: message.device_id,
            mentioned_operator_ids: message.mentions,
        },
        meta: EventMeta {
            event_id: message.event_id,
            channel: Some(EventChannel::Chat),
            actor,
            ..EventMeta::NONE
        },
    })
    .await
    .map(|_| ())
    .map_err(|e| ApiError::internal(e.to_string()))
}

/// Tell the people a sent message is for, once the message is recorded. It
/// runs after the send acks, so the sender never waits on it.
pub(super) fn spawn_people_notice(
    engine: &crate::api::SharedEngine,
    thread_id: Uuid,
    event_id: Option<Uuid>,
    text: &str,
    addressing: &PeopleAddressing,
    people_only: bool,
    sender: &Option<MessageOrigin>,
) {
    let engine = engine.clone();
    let notice = thread_people::PeopleMessageNotice {
        thread_id,
        event_id,
        text: text.to_string(),
        mentions: addressing.mentions.clone(),
        people_only,
        sender: sender.clone(),
    };
    tokio::spawn(async move {
        let recorded = match event_id {
            Some(id) => await_recorded(&engine, id).await,
            None => Ok(true),
        };
        let told = match recorded {
            Ok(true) => {
                let snapshot = crate::gateway_operators::current();
                thread_people::on_people_message(&engine, notice, &snapshot).await
            }
            Ok(false) => Err("the message was never recorded".into()),
            Err(e) => Err(e),
        };
        if let Err(e) = told {
            log!(
                "[PeopleMessages] Could not tell the people of {}: {}",
                thread_id,
                e
            );
        }
    });
}

/// How long a notice waits for its message to record. A message to the agent
/// records when its turn starts, which may wait for a free capacity slot.
const RECORD_WAIT: std::time::Duration = std::time::Duration::from_secs(3600);

/// Wait until the event `event_id` is in the events table. `false` when it
/// never records within [`RECORD_WAIT`]. Subscribes before it looks, so the
/// event cannot land between the look and the wait.
async fn await_recorded(
    engine: &crate::api::SharedEngine,
    event_id: Uuid,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let mut emitted = engine.event_bus.subscribe();
    let is_recorded = || async {
        sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM events WHERE id = $1)")
            .bind(event_id)
            .fetch_one(engine.pool())
            .await
    };
    if is_recorded().await? {
        return Ok(true);
    }
    let wait = async {
        loop {
            match emitted.recv().await {
                Ok(event) if event.event_id == event_id => return Ok(true),
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    if is_recorded().await? {
                        return Ok(true);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(false),
            }
        }
    };
    tokio::time::timeout(RECORD_WAIT, wait)
        .await
        .unwrap_or(Ok(false))
}

/// A known thread's id, or a 400 or 404.
async fn known_thread(state: &AppState, raw: &str) -> Result<Uuid, ApiError> {
    let thread_id = Uuid::parse_str(raw)
        .map_err(|e| ApiError::bad_request(format!("Invalid thread_id: {e}")))?;
    let known: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM thread_summaries WHERE thread_id = $1)")
            .bind(thread_id)
            .fetch_one(&state.pool)
            .await
            .map_err(ApiError::db)?;
    if !known {
        return Err(ApiError::not_found(format!(
            "No thread {thread_id} in this workspace."
        )));
    }
    Ok(thread_id)
}

/// GET /api/v1/threads/:thread_id/agent-pause: whether the agent is paused,
/// as `{ "paused" }`. A client cannot fold it from a transcript page, since
/// the last flip may sit on an older page.
async fn read_agent_pause(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let thread_id = known_thread(&state, &thread_id).await?;
    let paused = agent_pause::agent_paused(&state.pool, thread_id)
        .await
        .map_err(ApiError::db)?;
    Ok(Json(serde_json::json!({ "paused": paused })))
}

#[derive(Deserialize)]
struct AgentPauseBody {
    paused: bool,
}

/// POST /api/v1/threads/:thread_id/agent-pause: the chip. Any paired device
/// may flip it, since the pause is never authority. Answers `{ "paused" }`.
async fn set_agent_pause(
    State(state): State<AppState>,
    Path(thread_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AgentPauseBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let actor = crate::api::actor::require_user_actor(&headers, &state.pool, None).await?;
    let snapshot = crate::gateway_operators::known().ok_or_else(operators_unknown)?;
    if snapshot.person_count() < 2 {
        return Err(ApiError::bad_request(NO_TEAM).with_reason("no_team"));
    }
    let thread_id = known_thread(&state, &thread_id).await?;
    let paused = agent_pause::set_agent_paused(
        &state.pool,
        &state.engine.event_bus,
        thread_id,
        body.paused,
        Some(actor),
    )
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(serde_json::json!({ "paused": paused })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gateway_operators::Operator;

    fn person(id: &str) -> Operator {
        Operator {
            id: id.into(),
            name: id.to_uppercase(),
            emails: vec![],
            device_ids: vec![format!("{id}-phone")],
        }
    }

    fn snapshot(ids: &[&str]) -> OperatorSnapshot {
        OperatorSnapshot {
            operators: ids.iter().map(|id| person(id)).collect(),
            relayed: vec![],
        }
    }

    #[test]
    fn a_one_person_gateway_refuses_mentions_and_ignores_the_rest() {
        let alone = snapshot(&["sam"]);
        assert_eq!(
            people_addressing(vec![], true, ActorMode::Human, &alone).unwrap(),
            None
        );
        let refused =
            people_addressing(vec!["sam".into()], false, ActorMode::Human, &alone).unwrap_err();
        assert_eq!(refused.status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn a_team_gateway_checks_and_dedupes_mentions() {
        let team = snapshot(&["sam", "kim"]);
        assert_eq!(
            people_addressing(
                vec!["kim".into(), "kim".into()],
                false,
                ActorMode::Human,
                &team
            )
            .unwrap(),
            Some(PeopleAddressing {
                mentions: vec!["kim".into()],
                agent_mentioned: false,
            })
        );
        assert!(people_addressing(vec!["lee".into()], false, ActorMode::Human, &team).is_err());
    }

    #[test]
    fn input_that_is_not_a_person_typing_takes_the_ordinary_path() {
        let team = snapshot(&["sam", "kim"]);
        for mode in [ActorMode::Agent, ActorMode::Engine] {
            assert_eq!(people_addressing(vec![], false, mode, &team).unwrap(), None);
            assert!(people_addressing(vec!["kim".into()], false, mode, &team).is_err());
        }
    }

    use crate::test_support::{setup_test_db, teardown_test_db};

    fn sam() -> Option<MessageOrigin> {
        Some(MessageOrigin::Device {
            device_id: "sam-phone".into(),
        })
    }

    fn mention(ids: &[&str], agent: bool) -> PeopleAddressing {
        PeopleAddressing {
            mentions: ids.iter().map(|id| id.to_string()).collect(),
            agent_mentioned: agent,
        }
    }

    async fn post(bus: &EventBus, thread: Uuid, text: &str, mentions: &[&str]) {
        let message = PeopleMessage {
            text: text.into(),
            image_hashes: vec![],
            device_id: Some("sam-phone".into()),
            mentions: mentions.iter().map(|id| id.to_string()).collect(),
            event_id: None,
        };
        post_people_message(bus, thread, message, sam())
            .await
            .unwrap();
    }

    async fn event_types(pool: &PgPool, thread: Uuid) -> Vec<String> {
        sqlx::query_scalar("SELECT event_type FROM events WHERE thread_id = $1 ORDER BY sequence")
            .bind(thread)
            .fetch_all(pool)
            .await
            .unwrap()
    }

    async fn summary(pool: &PgPool, thread: Uuid) -> (String, String, String, bool) {
        sqlx::query_as(
            "SELECT state, source, status::text, has_response FROM thread_summaries \
             WHERE thread_id = $1",
        )
        .bind(thread)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    /// A draft started with a teammate pauses, becomes a real chat thread,
    /// and runs no turn. The agent mention resumes it.
    #[tokio::test]
    async fn a_mention_pauses_a_draft_and_the_agent_mention_resumes_it() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        bus.emit(BusEvent::Thread {
            thread_id: thread,
            event: ThreadEvent::ThreadStarted {
                recovered_compose: None,
                mode: "claude_code".into(),
                actor: sam(),
            },
            meta: EventMeta::with_actor(sam()),
        })
        .await
        .unwrap();
        let (_, _, draft_status, _) = summary(&pool, thread).await;

        let first = route_people_message(&pool, &bus, thread, &mention(&["kim"], false), sam())
            .await
            .unwrap();
        assert_eq!(first, MessageAudience::PeopleOnly);
        post(&bus, thread, "Kim, can you look?", &["kim"]).await;
        let (state, source, status, listed) = summary(&pool, thread).await;
        assert_eq!((state.as_str(), source.as_str()), ("active", "chat"));
        assert_eq!(status, draft_status, "no turn moved the status");
        assert!(listed);

        let untagged = route_people_message(&pool, &bus, thread, &mention(&[], false), sam())
            .await
            .unwrap();
        assert_eq!(
            untagged,
            MessageAudience::PeopleOnly,
            "paused, so untagged stays people-only"
        );

        let woken = route_people_message(&pool, &bus, thread, &mention(&[], true), sam())
            .await
            .unwrap();
        assert_eq!(woken, MessageAudience::Agent);
        assert_eq!(
            event_types(&pool, thread).await,
            [
                "ThreadStarted",
                "AgentPaused",
                "PeopleMessagePosted",
                "AgentResumed"
            ]
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// A coding-agent thread's first message must reach its agent. The check
    /// is pure, so it runs before the send consumes a draft.
    #[test]
    fn a_people_only_message_cannot_start_a_coding_agent_thread() {
        let kim = mention(&["kim"], false);
        assert!(refuse_people_only_start(Some(&kim), true).is_err());
        assert!(refuse_people_only_start(Some(&kim), false).is_ok());
        assert!(refuse_people_only_start(Some(&mention(&["kim"], true)), true).is_ok());
        assert!(refuse_people_only_start(None, true).is_ok());
    }

    /// An untagged reply on a paused thread with an open question card is
    /// the card's answer, so it reaches the agent.
    #[tokio::test]
    async fn an_untagged_reply_to_an_open_card_wakes_a_paused_agent() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        route_people_message(&pool, &bus, thread, &mention(&["kim"], false), sam())
            .await
            .unwrap();
        bus.emit(BusEvent::Thread {
            thread_id: thread,
            event: ThreadEvent::UserQuestionAsked {
                tool_use_id: "toolu_card".into(),
                cc_session_id: String::new(),
                question: "A or B?".into(),
                options: vec![],
                worktree_path: None,
                multi_select: false,
                owner_approval: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::Chat),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();

        let reply = route_people_message(&pool, &bus, thread, &mention(&[], false), sam())
            .await
            .unwrap();
        assert_eq!(reply, MessageAudience::Agent);
        let aside = route_people_message(&pool, &bus, thread, &mention(&["kim"], false), sam())
            .await
            .unwrap();
        assert_eq!(
            aside,
            MessageAudience::PeopleOnly,
            "a mention stays an aside"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// A people-only message can be a thread's first event, and it lists the
    /// thread.
    #[tokio::test]
    async fn a_people_only_message_starts_a_chat_thread() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let thread = Uuid::new_v4();
        route_people_message(&pool, &bus, thread, &mention(&["kim"], false), sam())
            .await
            .unwrap();
        post(&bus, thread, "Kim, a new thread for us", &["kim"]).await;
        let (state, source, _, listed) = summary(&pool, thread).await;
        assert_eq!(
            (state.as_str(), source.as_str(), listed),
            ("active", "chat", true)
        );
        let first: Option<String> =
            sqlx::query_scalar("SELECT first_message FROM thread_summaries WHERE thread_id = $1")
                .bind(thread)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(first.as_deref(), Some("Kim, a new thread for us"));

        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
