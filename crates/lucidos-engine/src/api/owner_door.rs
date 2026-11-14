//! Owner-only doors and the *unattested caller* (ADR 0192, ADR 0641).
//!
//! A door asks [`owner_attestation`] who knocks. The owner passes, and an
//! agent hears the door's refusal. An unattested caller is asked about, or
//! refused, as the door's [`OnUnattested`] says. An ask runs in four steps:
//!
//! 1. [`park_unattested_acts`] buffers an unattested mutating request and
//!    opens a task-local slot around its handler.
//! 2. The door's gate fills the slot through [`ask`], before it writes.
//! 3. The middleware raises the card and answers 202, whatever error type the
//!    handler built from the refusal.
//! 4. A stamped device's Allow once spends the card, and [`act_on_card_answer`]
//!    replays the request. A task-local replay scope passes every gate.
//!
//! Outside the middleware, on a `GET` or in a spawned task, the slot is
//! closed, so an ask fails closed with [`NOT_ATTESTED_AS_OWNER`].
//!
//! **A layer, not a boundary.** A process running as the owner can still
//! rewrite Lucidos's own files. Only an OS sandbox closes that.

use std::cell::RefCell;
use std::sync::{Arc, OnceLock};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Json, Router};
use uuid::Uuid;

use super::actor::{owner_attestation, OwnerAttestation, NOT_ATTESTED_AS_OWNER};
use super::error::ApiError;
use super::AppState;
use crate::engine::thread_events::{
    AnswerKind, EventChannel, MessageOrigin, UnattestedAct, UnattestedRequest,
};
use crate::engine::unattested_act;

/// An owner-only act, in the engine's words, and the thread it acts on.
#[derive(Debug, Clone)]
pub(crate) struct OwnerAct {
    /// What the card says the caller asks to do. The engine wrote it, and
    /// any title in it went through `owner_approval::one_line`.
    pub(crate) act: String,
    /// The card lands here. `None` lands it in the home thread.
    pub(crate) thread: Option<Uuid>,
}

impl OwnerAct {
    pub(crate) fn new(act: impl Into<String>) -> Self {
        Self {
            act: act.into(),
            thread: None,
        }
    }

    pub(crate) fn on_thread(mut self, thread: Option<Uuid>) -> Self {
        self.thread = thread;
        self
    }
}

/// What a door does with an unattested caller.
#[derive(Debug, Clone)]
pub(crate) enum OnUnattested {
    /// Park the request on an owner approval card, and run it on Allow once.
    Ask(OwnerAct),
    /// Refuse outright. For a door whose replay would answer nobody (a reveal,
    /// a ticket), whose body holds a secret, or that answers a card.
    Refuse,
}

tokio::task_local! {
    static PARKED: RefCell<Option<OwnerAct>>;
    static REPLAYING: ();
}

/// Is this request the replay of an act the owner allowed once?
pub(crate) fn replaying() -> bool {
    REPLAYING.try_with(|_| ()).is_ok()
}

/// What an asking door answers. The middleware replaces it with the card's
/// 202. With no slot open, nothing can park, so the door refuses.
pub(crate) fn ask(act: OwnerAct) -> ApiError {
    match PARKED.try_with(|slot| slot.replace(Some(act))) {
        Ok(_) => ApiError::new(StatusCode::ACCEPTED, WAITING_FOR_THE_OWNER),
        Err(_) => ApiError::new(StatusCode::FORBIDDEN, NOT_ATTESTED_AS_OWNER),
    }
}

/// [`ask`] or refuse, as the door said.
pub(crate) fn unattested(on_unattested: OnUnattested) -> ApiError {
    match on_unattested {
        OnUnattested::Ask(act) => ask(act),
        OnUnattested::Refuse => ApiError::new(StatusCode::FORBIDDEN, NOT_ATTESTED_AS_OWNER),
    }
}

/// What a parked caller hears.
pub const WAITING_FOR_THE_OWNER: &str =
    "Only the workspace owner may do this, and this request does not prove it came from \
     them. It waits for the owner: Allow once on the card in Lucidos runs it one time.";

/// Card answers stay the owner's own, so an unattested caller never answers.
pub const UNATTESTED_CARD_ANSWER: &str =
    "A card in Lucidos is answered by the workspace owner, and this request does not prove \
     it came from them. A device id or the machine-local token is a claim any local process \
     can make. Answer the card in the Lucidos app instead.";

/// The `/api/v1` router, set once it is built, so a replay runs through every
/// layer a live request does.
#[derive(Clone, Default)]
pub struct DoorRouter(Arc<OnceLock<Router>>);

impl DoorRouter {
    pub(crate) fn set(&self, router: Router) {
        if self.0.set(router).is_err() {
            crate::log!("[OwnerDoor] The door router was already set; keeping the first");
        }
    }
}

/// A thread as a card names it: its title in quotes, escaped, or its id.
pub(crate) async fn thread_named(pool: &sqlx::PgPool, thread_id: Uuid) -> String {
    let title: Option<String> =
        sqlx::query_scalar("SELECT title FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .flatten();
    let title = title.unwrap_or_else(|| thread_id.to_string());
    format!(
        "\u{201c}{}\u{201d}",
        crate::api::owner_approval::one_line(&title)
    )
}

/// Park an unattested caller's request on an owner approval card when its
/// door asks. Every other request passes untouched.
pub(crate) async fn park_unattested_acts(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    if !super::mutating_gate::method_mutates(req.method())
        || owner_attestation(req.headers()) != OwnerAttestation::Unattested
    {
        return next.run(req).await;
    }
    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, super::API_BODY_LIMIT).await {
        Ok(bytes) => bytes,
        Err(e) => {
            return ApiError::new(StatusCode::PAYLOAD_TOO_LARGE, e.to_string()).into_response();
        }
    };
    let method = parts.method.to_string();
    let path = parts
        .uri
        .path_and_query()
        .map_or_else(|| parts.uri.path().to_string(), |p| p.as_str().to_string());
    let caller = super::actor::user_actor(&parts.headers, None);
    let restore_generation = parts
        .headers
        .get(super::restore_generation::GENERATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let req = Request::from_parts(parts, Body::from(bytes.clone()));
    let (response, parked) = with_parking_slot(next.run(req)).await;
    let (Some(act), Some(caller)) = (parked, caller) else {
        return response;
    };
    let Ok(body) = String::from_utf8(bytes.to_vec()) else {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "Only the workspace owner may do this, and a binary body cannot wait for them. \
             Do it in the Lucidos app instead.",
        )
        .into_response();
    };
    let body = (!body.is_empty()).then_some(body);
    let request = UnattestedRequest::Http {
        method,
        path,
        body,
        restore_generation,
    };
    match park(&state, act, caller, request).await {
        Ok(parked) => parked.into_response(),
        Err(e) => {
            crate::log!("[OwnerDoor] Could not park an unattested request: {e}");
            ApiError::internal(format!("Could not ask the owner: {e}")).into_response()
        }
    }
}

/// Run `handler` with a parking slot open, and return what it parked.
async fn with_parking_slot<F: std::future::Future<Output = Response>>(
    handler: F,
) -> (Response, Option<OwnerAct>) {
    PARKED
        .scope(RefCell::new(None), async move {
            let response = handler.await;
            (response, PARKED.with(RefCell::take))
        })
        .await
}

/// Raise the card for `request` and return the 202 that says so.
async fn park(
    state: &AppState,
    act: OwnerAct,
    caller: MessageOrigin,
    request: UnattestedRequest,
) -> Result<(StatusCode, Json<serde_json::Value>), Box<dyn std::error::Error + Send + Sync>> {
    let bus = &state.engine.event_bus;
    let thread_id = match act.thread {
        Some(thread) => thread,
        None => crate::engine::home_thread::ensure_home_thread(bus, &state.pool).await?,
    };
    let unattested = UnattestedAct { caller, request };
    let question = unattested_act::card_question(&act.act, &unattested);
    let card = unattested_act::raise_card(bus, thread_id, question, unattested, EventChannel::Chat)
        .await?;
    crate::log!(
        "[OwnerDoor] Parked an unattested request to {} on card {card} in thread {thread_id}",
        act.act
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::json!({
            "error": WAITING_FOR_THE_OWNER,
            "reason": "waiting_for_owner",
            "thread_id": thread_id,
        })),
    ))
}

/// Act on an answered card. A device's Allow once runs the act behind it. A
/// Don't allow on a coding-agent start closes the turns it parked. Every other
/// card and answer is left alone.
pub(crate) async fn act_on_card_answer(
    state: &AppState,
    thread_id: Uuid,
    tool_use_id: &str,
    answer: &AnswerKind,
    actor: Option<MessageOrigin>,
) {
    let AnswerKind::Selected { option_id } = answer else {
        return;
    };
    if let Err(e) = unattested_act::close_refused_start(
        &state.engine.event_bus,
        thread_id,
        tool_use_id,
        option_id,
        actor,
    )
    .await
    {
        crate::log!("[OwnerDoor] Could not close the start refused on card {tool_use_id}: {e}");
    }
    let allowed = match unattested_act::allowed_card(
        &state.pool,
        thread_id,
        Some(tool_use_id),
        false,
    )
    .await
    {
        Ok(Some((_, act))) => act,
        Ok(None) => return,
        Err(e) => {
            crate::log!("[OwnerDoor] Could not read card {tool_use_id}: {e}");
            return;
        }
    };
    match allowed.request {
        // The spawn gate spends the Allow when the run reaches it.
        UnattestedRequest::StartCodingAgent => {
            let resumed = crate::engine::thread_events::emit_continuation_requested_or_log(
                &state.engine.event_bus,
                thread_id,
                crate::engine::agent_recovery::OWNER_ALLOWED_START_REASON,
                None,
                "[OwnerDoor] ContinuationRequested (owner allowed the start)",
            )
            .await;
            if !resumed {
                notify_failed(state, thread_id, "the thread could not be resumed").await;
            }
        }
        UnattestedRequest::Http { .. } => {
            if !unattested_act::spend(&state.engine.event_bus, thread_id, tool_use_id).await {
                return;
            }
            if let Err(e) = replay(&state.door_router, &allowed).await {
                crate::log!("[OwnerDoor] The allowed act on card {tool_use_id} failed: {e}");
                notify_failed(state, thread_id, &e).await;
            }
        }
    }
}

/// Replay an allowed request into the `/api/v1` router, as its caller.
async fn replay(door_router: &DoorRouter, act: &UnattestedAct) -> Result<(), String> {
    use tower::ServiceExt as _;
    let UnattestedRequest::Http {
        method,
        path,
        body,
        restore_generation,
    } = &act.request
    else {
        return Err("only a request replays".to_string());
    };
    let router = door_router
        .0
        .get()
        .cloned()
        .ok_or("the engine's router is not built yet")?;
    let mut request = Request::builder()
        .method(method.as_str())
        .uri(path.as_str())
        .body(Body::from(body.clone().unwrap_or_default()))
        .map_err(|e| e.to_string())?;
    *request.headers_mut() = caller_headers(&act.caller, body.is_some());
    if let Some(value) = restore_generation
        .as_deref()
        .and_then(|v| HeaderValue::from_str(v).ok())
    {
        request
            .headers_mut()
            .insert(super::restore_generation::GENERATION, value);
    }
    let response = REPLAYING
        .scope((), router.oneshot(request))
        .await
        .map_err(|e| e.to_string())?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap_or_default();
    Err(format!("{status}: {}", String::from_utf8_lossy(&bytes)))
}

/// The credentials the parked caller presented, rebuilt so the replay names
/// the same actor and passes the same identity check.
fn caller_headers(caller: &MessageOrigin, json: bool) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if json {
        headers.insert(
            axum::http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    let MessageOrigin::UnattestedCaller {
        user_agent,
        claimed_device_id,
        claimed_local_token,
        ..
    } = caller
    else {
        return headers;
    };
    let token = claimed_local_token
        .then(super::local_auth::machine_local_token)
        .flatten();
    for (name, value) in [
        (super::actor::HEADER_DEVICE_ID, claimed_device_id.as_deref()),
        ("user-agent", user_agent.as_deref()),
        (lucidos_local_token::HEADER_LOCAL_TOKEN, token),
    ] {
        if let Some(value) = value.and_then(|v| HeaderValue::from_str(v).ok()) {
            headers.insert(name, value);
        }
    }
    headers
}

/// Tell the owner an act they allowed did not land.
async fn notify_failed(state: &AppState, thread_id: Uuid, error: &str) {
    use crate::engine::event_bus::{BusEvent, SystemEvent};
    const TITLE: &str = "An act you allowed failed";
    let message = format!("The request you allowed once did not run: {error}");
    let id = Uuid::new_v4();
    if let Err(e) = state
        .engine
        .event_bus
        .emit(BusEvent::System(SystemEvent::NotificationCreated {
            id: id.to_string(),
            title: TITLE.to_string(),
            message: message.clone(),
            task_id: None,
            app_id: None,
            thread_id: Some(thread_id.to_string()),
            event_id: None,
            tap: crate::scheduler::notifications::Tap::Modal,
            actor: None,
            audience: None,
            push_group: None,
        }))
        .await
    {
        crate::log!("[OwnerDoor] Could not record the failure notice: {e}");
    }
    crate::scheduler::push::send_push_to_all(&state.engine, TITLE, &message, Some(id));
}

#[cfg(test)]
#[path = "owner_door_tests.rs"]
mod tests;
