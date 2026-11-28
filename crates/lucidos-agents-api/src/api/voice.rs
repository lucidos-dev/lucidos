use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
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

/// INVARIANT: The largest frame the socket accepts.
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

const NO_LIVE_TICKET: &str = "A call needs a live ticket for this thread. Mint one at \
     POST /api/v1/voice/ticket and dial before it expires.";

async fn voice(
    State(state): State<AppState>,
    Query(query): Query<VoiceQuery>,
    headers: HeaderMap,
    uri: Uri,
    upgrade: WebSocketUpgrade,
) -> Result<Response, ApiError> {
    let thread_id = query.thread_id;
    let Admitted {
        session_id,
        slot,
        actor,
    } = admit(
        &state.pool,
        &state.engine.event_bus,
        &state.engine.voice_sessions,
        &state.socket_tickets,
        &headers,
        &uri,
        thread_id,
    )
    .await?;

    let provider = crate::voice::build::provider_for(&state.engine).await;

    Ok(upgrade
        .max_message_size(MAX_FRAME_BYTES)
        .on_upgrade(move |socket| async move {
            // INVARIANT: Held for the whole call, and freed on drop even if this task
            // panics, so a crash cannot leave a thread permanently busy.
            let _slot = slot;
            let mut transport = SocketTransport { socket };

            let provider = match provider {
                Ok(provider) => provider,
                Err(message) => {
                    log_legacy!("[Voice] No talker to call: {}", message);
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
            let doer = crate::voice::doer::ThreadTurn::new(state.engine.clone());
            let decisions = crate::voice::decision::ThreadDecisions::new(state.engine.clone());
            crate::voice::call::run_call(
                &state.engine.event_bus,
                provider.as_ref(),
                &mut transport,
                &doer,
                &decisions,
                opening,
                crate::voice::call::CallSubject {
                    thread_id,
                    session_id,
                    actor: Some(actor),
                },
            )
            .await;
        }))
}

struct Admitted {
    session_id: Uuid,
    slot: crate::voice::registry::VoiceSessionSlot,
    actor: crate::engine::thread_events::MessageOrigin,
}

async fn admit(
    pool: &sqlx::PgPool,
    event_bus: &crate::engine::event_bus::EventBus,
    sessions: &crate::voice::registry::LiveVoiceSessions,
    tickets: &super::socket_ticket::SocketTickets,
    headers: &HeaderMap,
    uri: &Uri,
    thread_id: Uuid,
) -> Result<Admitted, ApiError> {
    let generation = crate::core::snapshot::boundary::read(pool)
        .await
        .map_err(ApiError::db)?
        .map(|boundary| boundary.restore_id);
    let device_id = super::socket_ticket::ticket_in(uri)
        .and_then(|ticket| {
            tickets.redeem(
                &ticket,
                super::socket_ticket::SocketTarget::Call { thread_id },
                generation,
            )
        })
        .ok_or_else(|| ApiError::new(StatusCode::FORBIDDEN, NO_LIVE_TICKET))?;

    if !super::socket_ticket::header_agrees(headers, &device_id) {
        crate::log_legacy!(
            "[Voice] Refusing a call: the ticket names another device than the header"
        );
        return Err(ApiError::new(StatusCode::FORBIDDEN, NO_LIVE_TICKET));
    }

    let actor = super::actor::require_owner_device(
        headers,
        pool,
        event_bus,
        Some(&device_id),
        "start a voice call",
    )
    .await?;
    refuse_call_on(pool, sessions, thread_id).await?;

    let session_id = Uuid::new_v4();
    match sessions.claim(thread_id, session_id) {
        Some(slot) => Ok(Admitted {
            session_id,
            slot,
            actor,
        }),
        None => Err(thread_busy()),
    }
}

fn thread_busy() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "This thread is already on a call. End that one first.",
    )
}

/// INVARIANT: Refuse a call on `thread_id` for anything about the thread or the
/// workspace. The caller is already known to be the owner's device.
///
/// Both the ticket mint and the upgrade ask. Only the mint's answer reaches a
/// person: a browser hides a handshake's status and body alike, so our client
/// shows `CALL_REFUSED` for an upgrade refusal (`voice/refusals.ts`).
pub(super) async fn refuse_call_on(
    pool: &sqlx::PgPool,
    sessions: &crate::voice::registry::LiveVoiceSessions,
    thread_id: Uuid,
) -> Result<(), ApiError> {
    if !crate::core::prefs::VOICE_ENABLED.read(pool).await {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Voice is off. Turn it on in Settings, under Voice.",
        ));
    }

    match crate::voice::doer::doer_for(pool, thread_id).await {
        crate::voice::doer::ThreadDoer::LucidosAgent => {}
        crate::voice::doer::ThreadDoer::CodingAgent => {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "A call runs on a Lucidos Agent conversation. Switch the \
                 destination to Lucidos to talk.",
            ));
        }
        crate::voice::doer::ThreadDoer::NoSuchThread => {
            return Err(ApiError::not_found(
                "That thread does not exist, so there is nothing to talk about.",
            ));
        }
        crate::voice::doer::ThreadDoer::Unknown => {
            return Err(ApiError::internal(
                "Could not read that thread, so the call was not placed.",
            ));
        }
    }

    match crate::engine::home_thread::is_home_thread(pool, thread_id).await {
        Ok(true) => {}
        Ok(false) => {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "A call runs on the home thread only. Open Home to talk.",
            ));
        }
        Err(e) => {
            log_legacy!(
                "[Voice] Could not read whether {} is home: {}",
                thread_id,
                e
            );
            return Err(ApiError::internal(
                "Could not read that thread, so the call was not placed.",
            ));
        }
    }

    if sessions.is_busy(thread_id) {
        return Err(thread_busy());
    }
    Ok(())
}

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
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                Some(Ok(Message::Close(_))) | None => CallerFrame::Closed,
                Some(Err(e)) => {
                    log_legacy!("[Voice] The caller's socket failed: {}", e);
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
#[path = "voice_tests.rs"]
mod tests;
