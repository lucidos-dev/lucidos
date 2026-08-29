//! `GET /api/v1/voice?thread_id=<uuid>`: the socket a *voice session* runs on.
//!
//! Authenticated like every other `/api/v1` route, and proxied by the gateway
//! as an upgrade (ADR 0151). `/api/v1/ws-echo` is the diagnostic that tells a
//! broken hop apart from a broken call.
//!
//! **The client stays dumb.** Binary frames are audio and text frames are the
//! small control vocabulary in `voice::wire`. Nothing here names a provider.
//!
//! Two refusals happen before the upgrade, because both are things the caller
//! asked for and can stop asking for: an unknown thread, and a thread already
//! on a call. A failure to reach the talker happens after, as an `error` frame,
//! because the caller cannot have known and a person has to read it.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use uuid::Uuid;

use super::error::ApiError;
use super::AppState;
use crate::voice::call::{CallTransport, CallerFrame};
use crate::voice::wire::{ClientControl, ServerFrame};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// The largest frame the socket accepts.
///
/// Caller audio arrives in chunks of a few kilobytes. The cap is generous
/// against that, and it stops an authenticated caller sizing the engine's read
/// buffer for free. Enforced on the SOCKET, so an oversized frame is refused as
/// it arrives rather than buffered first.
const MAX_FRAME_BYTES: usize = 256 * 1024;

#[derive(Debug, Deserialize)]
struct VoiceQuery {
    thread_id: Uuid,
}

async fn voice(
    State(state): State<AppState>,
    Query(query): Query<VoiceQuery>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let thread_id = query.thread_id;
    let (session_id, slot) = admit(&state.pool, &state.engine.voice_sessions, thread_id).await?;

    let provider = crate::voice::build::provider_for(&state.engine).await;
    let actor = super::actor::user_actor_resolved(&headers, &state.pool, None).await;

    Ok(upgrade
        .max_message_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| async move {
            // Held for the whole call, and freed on drop even if this task
            // panics, so a crash cannot leave a thread permanently busy.
            let _slot = slot;
            let mut transport = SocketTransport { socket };

            let provider = match provider {
                Ok(provider) => provider,
                Err(message) => {
                    log!("[Voice] No talker to call: {}", message);
                    let _ = transport
                        .send_frame(ServerFrame::Error {
                            message: "No voice model is configured. Set one in Settings."
                                .to_string(),
                        })
                        .await;
                    return;
                }
            };

            let opening = crate::voice::call::opening_for(&state.engine, thread_id).await;
            let reasoner = crate::voice::reasoner::ThreadTurn::new(state.engine.clone());
            crate::voice::call::run_call(
                &state.engine.event_bus,
                provider.as_ref(),
                &mut transport,
                &reasoner,
                opening,
                crate::voice::call::CallSubject {
                    thread_id,
                    session_id,
                    actor,
                },
            )
            .await;
        }))
}

/// Decide whether this thread may take a call, and claim its slot if so.
///
/// Separate from the handler because axum runs `WebSocketUpgrade` before the
/// handler body: a plain GET is refused with a 400 by the extractor, so
/// neither refusal here is reachable from an ordinary HTTP request. Testing
/// them means testing this.
async fn admit(
    pool: &sqlx::PgPool,
    sessions: &crate::voice::registry::LiveVoiceSessions,
    thread_id: Uuid,
) -> Result<(Uuid, crate::voice::registry::VoiceSessionSlot), ApiError> {
    // The master switch, ahead of everything else. Voice is experimental and
    // off unless a workspace opted in. A call on one that did not is refused
    // before any of it runs.
    if !crate::core::PreferenceStore::voice_enabled(pool).await {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Voice is off. Turn it on in Settings, under Voice.",
        ));
    }

    let exists: Option<(Uuid,)> =
        sqlx::query_as("SELECT thread_id FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await
            .map_err(ApiError::db)?;
    if exists.is_none() {
        return Err(ApiError::not_found(
            "That thread does not exist, so there is nothing to talk about.",
        ));
    }

    let session_id = Uuid::new_v4();
    match sessions.claim(thread_id, session_id) {
        Some(slot) => Ok((session_id, slot)),
        None => Err(ApiError::new(
            StatusCode::CONFLICT,
            "This thread is already on a call. End that one first.",
        )),
    }
}

/// The caller's end of a call, over a WebSocket.
struct SocketTransport {
    socket: WebSocket,
}

#[axum::async_trait]
impl CallTransport for SocketTransport {
    async fn recv(&mut self) -> CallerFrame {
        loop {
            return match self.socket.recv().await {
                Some(Ok(Message::Binary(pcm))) => CallerFrame::Audio(pcm),
                Some(Ok(Message::Text(text))) => match serde_json::from_str::<ClientControl>(&text)
                {
                    Ok(control) => CallerFrame::Control(control),
                    Err(_) => CallerFrame::Undecodable,
                },
                // axum answers a ping itself, and a pong is nothing to act on.
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                Some(Ok(Message::Close(_))) | None => CallerFrame::Closed,
                Some(Err(e)) => {
                    log!("[Voice] The caller's socket failed: {}", e);
                    CallerFrame::Closed
                }
            };
        }
    }

    async fn send_audio(&mut self, pcm: Vec<u8>) -> Result<(), BoxError> {
        self.socket.send(Message::Binary(pcm)).await?;
        Ok(())
    }

    async fn send_frame(&mut self, frame: ServerFrame) -> Result<(), BoxError> {
        let text = serde_json::to_string(&frame)?;
        self.socket.send(Message::Text(text)).await?;
        Ok(())
    }
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/voice", get(voice))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};
    use crate::voice::registry::LiveVoiceSessions;

    async fn a_chat_thread(pool: &sqlx::PgPool) -> Uuid {
        let thread_id = Uuid::new_v4();
        sqlx::query("INSERT INTO thread_summaries (thread_id, source) VALUES ($1, 'chat')")
            .bind(thread_id)
            .execute(pool)
            .await
            .expect("create the thread");
        thread_id
    }

    /// Opt this workspace into voice. Every other test here is about a rule
    /// that only applies once somebody has.
    async fn voice_is_on(pool: &sqlx::PgPool) {
        crate::core::PreferenceStore::set_row_for_test(pool, "voice_enabled", "true")
            .await
            .expect("turn voice on");
    }

    /// Voice ships off. A workspace that never opted in has no call to place,
    /// and the refusal lands before the thread is even looked up.
    #[tokio::test]
    async fn a_call_is_refused_while_voice_is_off() {
        let (pool, db_name) = setup_test_db().await;
        let sessions = LiveVoiceSessions::new();
        let thread_id = a_chat_thread(&pool).await;

        let error = admit(&pool, &sessions, thread_id)
            .await
            .err()
            .expect("should refuse");
        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert_eq!(sessions.count(), 0, "a refusal must claim no slot");

        // And it is the switch that decides, not the thread.
        voice_is_on(&pool).await;
        admit(&pool, &sessions, thread_id)
            .await
            .expect("a call once voice is on");

        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_call_on_a_thread_that_does_not_exist_is_refused() {
        let (pool, db_name) = setup_test_db().await;
        let sessions = LiveVoiceSessions::new();
        voice_is_on(&pool).await;

        let error = admit(&pool, &sessions, Uuid::new_v4())
            .await
            .err()
            .expect("should refuse");
        assert_eq!(error.status, StatusCode::NOT_FOUND);
        assert_eq!(sessions.count(), 0, "a refusal must claim no slot");

        teardown_test_db(&db_name).await;
    }

    /// One live session per thread. It is what keeps every start paired with
    /// exactly one end, so the second caller is refused rather than queued.
    #[tokio::test]
    async fn a_second_call_on_a_busy_thread_is_refused() {
        let (pool, db_name) = setup_test_db().await;
        let sessions = LiveVoiceSessions::new();
        voice_is_on(&pool).await;
        let thread_id = a_chat_thread(&pool).await;

        let (first_id, held) = admit(&pool, &sessions, thread_id)
            .await
            .expect("first call");
        let error = admit(&pool, &sessions, thread_id)
            .await
            .err()
            .expect("should refuse");
        assert_eq!(error.status, StatusCode::CONFLICT);
        assert_eq!(sessions.count(), 1, "the refusal must not evict the first");

        // Ringing off frees the thread, and the next call gets a fresh id.
        drop(held);
        let (second_id, _next) = admit(&pool, &sessions, thread_id).await.expect("next call");
        assert_ne!(first_id, second_id);

        teardown_test_db(&db_name).await;
    }
}
