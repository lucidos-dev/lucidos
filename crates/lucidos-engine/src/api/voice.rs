//! `GET /api/v1/voice?thread_id=<uuid>`: the socket a *voice session* runs on.
//!
//! Authenticated like every other `/api/v1` route, and proxied by the gateway
//! as an upgrade (ADR 0151). `/api/v1/ws-echo` is the diagnostic that tells a
//! broken hop apart from a broken call.
//!
//! **The client stays dumb.** Binary frames are audio and text frames are the
//! small control vocabulary in `voice::wire`. Nothing here names a provider.
//!
//! Five refusals happen before the upgrade, because each is a thing the caller
//! asked for and can stop asking for: voice switched off, a coding-agent
//! thread, an unknown thread, a thread that is not the home thread (or no home
//! thread at all), and a thread already on a call. A thread the
//! engine cannot read answers 500 there too, which is nobody's fault. A
//! failure to reach the talker happens after, as an `error` frame, because the
//! caller cannot have known and a person has to read it.

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
    let actor = super::actor::user_actor(&headers, None);

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
    //
    // **No browser reads this message.** A `WebSocket` hides the handshake's
    // status and body alike, so our own client shows `CALL_REFUSED` whatever
    // is written here (`voice/refusals.ts`). Keeping the sentence honest is
    // still worth it: it reaches the log, the API tests, and any client that
    // is not a browser. What keeps a person from meeting this refusal at all
    // is the control being absent, which is the layer above.
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

    /// The workspace's home thread, the one thread a call runs on.
    async fn the_home_thread(pool: &sqlx::PgPool) -> Uuid {
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        crate::engine::home_thread::ensure_home_thread(&bus, pool)
            .await
            .expect("create the home thread")
    }

    /// A thread a coding agent holds.
    ///
    /// `source` reads `claude_code` for a started one and for a DRAFT alike.
    /// The compose write mirrors the picked mode into it, so a draft says who
    /// holds it before it carries a single message.
    async fn a_coding_agent_thread(pool: &sqlx::PgPool, state: &str) -> Uuid {
        let thread_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO thread_summaries (thread_id, source, state) VALUES ($1, 'claude_code', $2)",
        )
        .bind(thread_id)
        .bind(state)
        .execute(pool)
        .await
        .expect("create the thread");
        thread_id
    }

    /// Opt this workspace into voice. Every other test here is about a rule
    /// that only applies once somebody has.
    async fn voice_is_on(pool: &sqlx::PgPool) {
        let key = crate::core::prefs::VOICE_ENABLED.key();
        crate::core::PreferenceStore::set_row_for_test(pool, key, "true")
            .await
            .expect("turn voice on");
    }

    /// Voice ships off. A workspace that never opted in has no call to place,
    /// and the refusal lands before the thread is even looked up. Every
    /// workspace has Home, so the voice switch is the only one it names.
    #[tokio::test]
    async fn a_call_is_refused_while_voice_is_off() {
        let (pool, db_name) = setup_test_db().await;
        let sessions = LiveVoiceSessions::new();
        let thread_id = the_home_thread(&pool).await;

        let error = admit(&pool, &sessions, thread_id)
            .await
            .err()
            .expect("should refuse");
        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert!(error.message.contains("under Voice"), "{}", error.message);
        assert_eq!(sessions.count(), 0, "a refusal must claim no slot");

        // And it is the switch that decides, not the thread.
        voice_is_on(&pool).await;
        admit(&pool, &sessions, thread_id)
            .await
            .expect("a call once voice is on");

        teardown_test_db(&db_name).await;
    }

    /// A call reaches the Lucidos Agent and nothing else (ADR 0165).
    ///
    /// Both states, because the draft is the one the user actually hits: they
    /// pick Claude Code in the compose view and press the control before the
    /// thread has a single message in it.
    ///
    /// The message names the way back rather than saying only "no". The
    /// destination picker sits in the same row as the control they pressed.
    #[tokio::test]
    async fn a_call_on_a_coding_agent_thread_is_refused() {
        let (pool, db_name) = setup_test_db().await;
        let sessions = LiveVoiceSessions::new();
        voice_is_on(&pool).await;

        for state in ["composing", "active"] {
            let thread_id = a_coding_agent_thread(&pool, state).await;
            let error = admit(&pool, &sessions, thread_id)
                .await
                .err()
                .unwrap_or_else(|| panic!("should refuse a {} coding-agent thread", state));
            assert_eq!(error.status, StatusCode::FORBIDDEN);
            assert!(
                error.message.contains("Switch the destination"),
                "the refusal must name the way back: {}",
                error.message
            );
            assert_eq!(sessions.count(), 0, "a refusal must claim no slot");
        }

        // The home thread is a Lucidos Agent thread, and it takes the call.
        let home = the_home_thread(&pool).await;
        admit(&pool, &sessions, home)
            .await
            .expect("a call on the home thread");

        teardown_test_db(&db_name).await;
    }

    /// Voice sessions live in the home thread alone (ADR 0362, invariant I12).
    /// Any other Lucidos Agent thread is refused, and the message names where
    /// to go instead.
    #[tokio::test]
    async fn a_call_on_any_thread_but_home_is_refused() {
        let (pool, db_name) = setup_test_db().await;
        let sessions = LiveVoiceSessions::new();
        voice_is_on(&pool).await;
        let home = the_home_thread(&pool).await;
        let other = a_chat_thread(&pool).await;

        let error = admit(&pool, &sessions, other)
            .await
            .err()
            .expect("should refuse a chat thread that is not home");
        assert_eq!(error.status, StatusCode::FORBIDDEN);
        assert!(error.message.contains("home thread"), "{}", error.message);
        assert_eq!(sessions.count(), 0, "a refusal must claim no slot");

        admit(&pool, &sessions, home)
            .await
            .expect("a call on the home thread");

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
        let thread_id = the_home_thread(&pool).await;

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
