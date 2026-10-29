//! `GET /api/v1/voice?thread_id=<uuid>`: the socket a *voice session* runs on.
//!
//! Authenticated like every other `/api/v1` route, and proxied by the gateway
//! as an upgrade (ADR 0151). `/api/v1/ws-echo` is the diagnostic that tells a
//! broken hop apart from a broken call.
//!
//! **The client stays dumb.** Binary frames are audio and text frames are the
//! small control vocabulary in `voice::wire`. Nothing here names a provider.
//!
//! **A call is a write behind a GET.** It records what the caller says as the
//! owner's own words, starts turns, and answers cards. So the upgrade presents
//! a *call ticket* the owner's device minted (`api::voice_ticket`), and
//! [`admit`] redeems it.
//!
//! Seven refusals happen before the upgrade, because each is a thing the caller
//! asked for and can stop asking for. The mint makes all but the first, so a
//! person reads the reason there:
//!
//! - no live ticket for this thread;
//! - a caller that is not a person's device;
//! - voice switched off;
//! - a coding-agent thread;
//! - an unknown thread;
//! - a thread that is not the home thread (or no home thread at all);
//! - a thread already on a call.
//!
//! A thread the engine cannot read answers 500 there too, which is nobody's
//! fault. A failure to reach the talker happens after, as an `error` frame,
//! because the caller cannot have known and a person has to read it.

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

/// What a caller that is not one of the owner's devices is told.
pub(super) const NOT_A_PERSONS_DEVICE: &str = "A call records what is said as the workspace \
     owner's own words, so only one of their registered devices can place one. No agent or \
     script may place a call, on any authority. Open Home on a paired device to talk.";

/// What an upgrade with no live ticket for its thread is told.
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
        &state.engine.voice_sessions,
        &state.call_tickets,
        &headers,
        &uri,
        thread_id,
    )
    .await?;

    let provider = crate::voice::build::provider_for(&state.engine).await;

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

/// A call [`admit`] let through: its id, the thread's slot, and who is calling.
struct Admitted {
    session_id: Uuid,
    slot: crate::voice::registry::VoiceSessionSlot,
    actor: crate::engine::thread_events::MessageOrigin,
}

/// Redeem the upgrade's call ticket, re-check, and claim the thread's slot.
///
/// Separate from the handler because axum runs `WebSocketUpgrade` before the
/// handler body: a plain GET is refused with a 400 by the extractor, so
/// neither refusal here is reachable from an ordinary HTTP request. Testing
/// them means testing this.
async fn admit(
    pool: &sqlx::PgPool,
    sessions: &crate::voice::registry::LiveVoiceSessions,
    tickets: &super::voice_ticket::CallTickets,
    headers: &HeaderMap,
    uri: &Uri,
    thread_id: Uuid,
) -> Result<Admitted, ApiError> {
    // The ticket names the caller. The mint ran the origin check a handshake
    // cannot, because a browser sends no `Referer` on one. Spent here whatever
    // happens next, so a refused upgrade cannot be retried on the same ticket.
    let device_id = super::voice_ticket::ticket_in(uri)
        .and_then(|ticket| tickets.redeem(&ticket, thread_id))
        .ok_or_else(|| ApiError::new(StatusCode::FORBIDDEN, NO_LIVE_TICKET))?;

    // Through the gateway the header is the device it authenticated. A ticket
    // another device minted must not stand in for it. The check is explicit
    // because `require_user_actor` lets the override win over the header.
    let header_device = headers
        .get(super::actor::HEADER_DEVICE_ID)
        .and_then(|v| v.to_str().ok());
    if header_device.is_some_and(|header| header != device_id) {
        crate::log!("[Voice] Refusing a call: the ticket names another device than the header");
        return Err(ApiError::new(StatusCode::FORBIDDEN, NO_LIVE_TICKET));
    }

    // A ticket lives long enough for state to move, so every check the mint
    // made runs again. This is the floor under the mint.
    let actor = super::actor::require_owner_device_for(
        headers,
        pool,
        Some(&device_id),
        NOT_A_PERSONS_DEVICE,
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

/// Refuse a call on `thread_id` for anything about the thread or the
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
    // The master switch. Voice is experimental and
    // off unless a workspace opted in. A call on one that did not is refused
    // before any of it runs. An unreadable row reads as the default, off: on
    // would open a paid rented talker on a transient database error.
    if !crate::core::prefs::VOICE_ENABLED.read(pool).await {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "Voice is off. Turn it on in Settings, under Voice.",
        ));
    }

    // Who holds this thread decides whether there is a call to place at all.
    // A call reaches the Lucidos Agent and nothing else (ADR 0165). The doer
    // asks the same question again, and is the floor under this one.
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

    // Voice sessions live in the home thread alone (ADR 0362). The control is
    // drawn there and nowhere else, so this is the floor under it.
    match crate::engine::home_thread::is_home_thread(pool, thread_id).await {
        Ok(true) => {}
        Ok(false) => {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "A call runs on the home thread only. Open Home to talk.",
            ));
        }
        Err(e) => {
            log!(
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
#[path = "voice_tests.rs"]
mod tests;
