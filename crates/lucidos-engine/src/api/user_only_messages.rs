//! User-only messages and the agent pause at the API (ADR 0649).
//!
//! A chat send on a team gateway may carry *user mentions* and an *agent
//! mention*. The engine reads the pause, flips it as the route says, and either
//! takes the ordinary path or records a `UserOnlyMessagePosted` that starts no
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
#[cfg(test)]
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{ActorMode, MessageOrigin};
#[cfg(test)]
use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
use crate::gateway_users::UserSnapshot;

/// The 400 for mentions on a gateway with fewer than two users.
const NO_TEAM: &str = "Mentioning a user needs a gateway with two or more users.";

/// The 503 while the gateway has not named its users yet.
fn users_unknown() -> ApiError {
    ApiError::new(
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
        "Lucidos is still reading who is on this gateway. Send again in a moment.",
    )
    .with_reason("users_unknown")
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route(
        "/threads/:thread_id/agent-pause",
        get(read_agent_pause).post(set_agent_pause),
    )
}

pub(super) use crate::engine::thread_users::UserAddressing;

/// Check a send's mentions. `None` takes the ordinary path untouched: a
/// gateway with one user, and every input that is not a user typing.
pub(super) fn user_addressing(
    mentions: Vec<String>,
    agent_mentioned: bool,
    mode: ActorMode,
    origin: Option<&MessageOrigin>,
    snapshot: &UserSnapshot,
) -> Result<Option<UserAddressing>, ApiError> {
    if snapshot.user_count() < 2 {
        return match mentions.is_empty() {
            true => Ok(None),
            false => Err(ApiError::bad_request(NO_TEAM).with_reason("no_team")),
        };
    }
    if mode != ActorMode::Human || !matches!(origin, Some(MessageOrigin::Device { .. })) {
        return Ok(None);
    }
    let sender = origin.and_then(|actor| snapshot.user_of_origin(actor));
    let users = snapshot.user_ids();
    let mut unique: Vec<String> = Vec::with_capacity(mentions.len());
    for id in mentions {
        if !users.contains(id.as_str()) {
            return Err(ApiError::bad_request(format!(
                "No user {id} on this gateway."
            )));
        }
        if sender.as_ref().is_none_or(|user| user.id != id) && !unique.contains(&id) {
            unique.push(id);
        }
    }
    Ok(Some(UserAddressing {
        mentions: unique,
        agent_mentioned,
    }))
}

/// [`user_addressing`] for a chat send, with the gateway's current list.
///
/// While the gateway has not answered, a team would read as one user. A
/// user's send that a team would route differently is then refused with a
/// 503: one with mentions, or one to a paused thread. Every other send takes
/// the ordinary path, as a team's would.
pub(super) async fn addressing_of_send(
    pool: &PgPool,
    thread_id: Option<Uuid>,
    mentions: Vec<String>,
    agent_mentioned: bool,
    mode: ActorMode,
    origin: Option<&MessageOrigin>,
) -> Result<Option<UserAddressing>, ApiError> {
    if mode != ActorMode::Human || !matches!(origin, Some(MessageOrigin::Device { .. })) {
        return Ok(None);
    }
    if let Some(snapshot) = crate::gateway_users::known() {
        return user_addressing(mentions, agent_mentioned, mode, origin, &snapshot);
    }
    if !mentions.is_empty() {
        return Err(users_unknown());
    }
    let paused = match thread_id {
        Some(tid) => agent_pause::agent_paused(pool, tid)
            .await
            .map_err(ApiError::db)?,
        None => false,
    };
    match paused {
        true => Err(users_unknown()),
        false => Ok(None),
    }
}

/// Refuse a user-only message that would create a coding-agent thread with
/// no session. A thread not started yet is never paused and has no open
/// question, so the mentions alone decide it, before anything records.
pub(super) fn refuse_user_only_start(
    addressing: Option<&UserAddressing>,
    starts_coding_agent: bool,
) -> Result<(), ApiError> {
    let Some(addressing) = addressing.filter(|_| starts_coding_agent) else {
        return Ok(());
    };
    let route = agent_pause::user_only_message_route(
        !addressing.mentions.is_empty(),
        addressing.agent_mentioned,
        false,
        false,
    );
    match route.audience {
        MessageAudience::UserOnly => Err(ApiError::bad_request(
            "A coding-agent thread starts with a message to its agent. Mention the agent, or start a chat thread.",
        )),
        MessageAudience::Agent => Ok(()),
    }
}

#[cfg(test)]
use crate::engine::thread_users::{
    post_user_only_message, route_user_only_message, UserOnlyMessage,
};

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
    let snapshot = crate::gateway_users::known().ok_or_else(users_unknown)?;
    if snapshot.user_count() < 2 {
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
    use crate::gateway_users::User;

    fn user(id: &str) -> User {
        User {
            id: id.into(),
            name: id.to_uppercase(),
            emails: vec![],
            device_ids: vec![format!("{id}-phone")],
        }
    }

    fn snapshot(ids: &[&str]) -> UserSnapshot {
        UserSnapshot {
            users: ids.iter().map(|id| user(id)).collect(),
            relayed: vec![],
        }
    }

    #[test]
    fn a_one_user_gateway_refuses_mentions_and_ignores_the_rest() {
        let alone = snapshot(&["sam"]);
        assert_eq!(
            user_addressing(vec![], true, ActorMode::Human, sam().as_ref(), &alone).unwrap(),
            None
        );
        let refused = user_addressing(
            vec!["sam".into()],
            false,
            ActorMode::Human,
            sam().as_ref(),
            &alone,
        )
        .unwrap_err();
        assert_eq!(refused.status, axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn a_team_gateway_checks_and_dedupes_mentions() {
        let team = snapshot(&["sam", "kim"]);
        assert_eq!(
            user_addressing(
                vec!["kim".into(), "kim".into()],
                false,
                ActorMode::Human,
                sam().as_ref(),
                &team
            )
            .unwrap(),
            Some(UserAddressing {
                mentions: vec!["kim".into()],
                agent_mentioned: false,
            })
        );
        assert!(user_addressing(
            vec!["lee".into()],
            false,
            ActorMode::Human,
            sam().as_ref(),
            &team
        )
        .is_err());
    }

    #[test]
    fn input_that_is_not_a_user_typing_takes_the_ordinary_path() {
        let team = snapshot(&["sam", "kim"]);
        for mode in [ActorMode::Agent, ActorMode::Engine] {
            assert_eq!(
                user_addressing(vec![], false, mode, sam().as_ref(), &team).unwrap(),
                None
            );
            assert_eq!(
                user_addressing(vec!["kim".into()], false, mode, sam().as_ref(), &team).unwrap(),
                None
            );
        }
    }

    #[test]
    fn self_mentions_are_not_addressing() {
        let team = snapshot(&["sam", "kim"]);
        for (ids, expected) in [
            (vec!["sam".into()], vec![]),
            (vec!["sam".into(), "kim".into()], vec![String::from("kim")]),
        ] {
            let addressing = user_addressing(ids, false, ActorMode::Human, sam().as_ref(), &team)
                .unwrap()
                .unwrap();
            assert_eq!(addressing.mentions, expected);
            let route = agent_pause::user_only_message_route(
                !addressing.mentions.is_empty(),
                false,
                false,
                false,
            );
            assert_eq!(
                route.audience,
                if expected.is_empty() {
                    MessageAudience::Agent
                } else {
                    MessageAudience::UserOnly
                }
            );
        }
    }

    #[tokio::test]
    async fn non_device_input_bypasses_a_paused_thread() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        agent_pause::set_agent_paused(&pool, &bus, thread, true, sam())
            .await
            .unwrap();
        for origin in [
            None,
            Some(MessageOrigin::Api {
                user_agent: None,
                mode: ActorMode::Human,
                source_thread_id: None,
                emitting_trigger_id: None,
            }),
            Some(MessageOrigin::Workspace {
                workspace: "other-workspace".into(),
                thread_id: None,
                event_id: None,
                user_agent: None,
                mode: ActorMode::Human,
                verdict: None,
            }),
        ] {
            let addressing = addressing_of_send(
                &pool,
                Some(thread),
                vec![],
                false,
                ActorMode::Human,
                origin.as_ref(),
            )
            .await
            .unwrap();
            assert_eq!(
                addressing, None,
                "non-device input must take the ordinary agent path"
            );
        }
        assert!(agent_pause::agent_paused(&pool, thread).await.unwrap());
        teardown_test_db(&db_name).await;
    }

    use crate::test_support::{setup_test_db, teardown_test_db};

    fn sam() -> Option<MessageOrigin> {
        Some(MessageOrigin::Device {
            device_id: "sam-phone".into(),
        })
    }

    fn mention(ids: &[&str], agent: bool) -> UserAddressing {
        UserAddressing {
            mentions: ids.iter().map(|id| id.to_string()).collect(),
            agent_mentioned: agent,
        }
    }

    async fn post(bus: &EventBus, thread: Uuid, text: &str, mentions: &[&str]) {
        let message = UserOnlyMessage {
            text: text.into(),
            image_hashes: vec![],
            device_id: Some("sam-phone".into()),
            mentions: mentions.iter().map(|id| id.to_string()).collect(),
            event_id: None,
        };
        post_user_only_message(bus, thread, message, sam())
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
                agent_draft: None,
                mode: "claude_code".into(),
                actor: sam(),
            },
            meta: EventMeta::with_actor(sam()),
        })
        .await
        .unwrap();
        let (_, _, draft_status, _) = summary(&pool, thread).await;

        let first = route_user_only_message(&pool, &bus, thread, &mention(&["kim"], false), sam())
            .await
            .unwrap();
        assert_eq!(first, MessageAudience::UserOnly);
        post(&bus, thread, "Kim, can you look?", &["kim"]).await;
        let (state, source, status, listed) = summary(&pool, thread).await;
        assert_eq!((state.as_str(), source.as_str()), ("active", "chat"));
        assert_eq!(status, draft_status, "no turn moved the status");
        assert!(listed);

        let untagged = route_user_only_message(&pool, &bus, thread, &mention(&[], false), sam())
            .await
            .unwrap();
        assert_eq!(
            untagged,
            MessageAudience::UserOnly,
            "paused, so untagged stays user-only"
        );

        let woken = route_user_only_message(&pool, &bus, thread, &mention(&[], true), sam())
            .await
            .unwrap();
        assert_eq!(woken, MessageAudience::Agent);
        assert_eq!(
            event_types(&pool, thread).await,
            [
                "ThreadStarted",
                "AgentPaused",
                "UserOnlyMessagePosted",
                "AgentResumed"
            ]
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// A coding-agent thread's first message must reach its agent. The check
    /// is pure, so it runs before the send consumes a draft.
    #[test]
    fn a_user_only_message_cannot_start_a_coding_agent_thread() {
        let kim = mention(&["kim"], false);
        assert!(refuse_user_only_start(Some(&kim), true).is_err());
        assert!(refuse_user_only_start(Some(&kim), false).is_ok());
        assert!(refuse_user_only_start(Some(&mention(&["kim"], true)), true).is_ok());
        assert!(refuse_user_only_start(None, true).is_ok());
    }

    /// An untagged reply on a paused thread with an open question card is
    /// the card's answer, so it reaches the agent.
    #[tokio::test]
    async fn an_untagged_reply_to_an_open_card_wakes_a_paused_agent() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        route_user_only_message(&pool, &bus, thread, &mention(&["kim"], false), sam())
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
                unattested_act: None,
                permission_request: None,
            },
            meta: EventMeta {
                channel: Some(EventChannel::Chat),
                ..EventMeta::NONE
            },
        })
        .await
        .unwrap();

        let reply = route_user_only_message(&pool, &bus, thread, &mention(&[], false), sam())
            .await
            .unwrap();
        assert_eq!(reply, MessageAudience::Agent);
        let aside = route_user_only_message(&pool, &bus, thread, &mention(&["kim"], false), sam())
            .await
            .unwrap();
        assert_eq!(aside, MessageAudience::UserOnly, "a mention stays an aside");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// A user-only message can be a thread's first event, and it lists the
    /// thread.
    #[tokio::test]
    async fn a_user_only_message_starts_a_chat_thread() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());

        let thread = Uuid::new_v4();
        route_user_only_message(&pool, &bus, thread, &mention(&["kim"], false), sam())
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
