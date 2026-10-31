//! `POST /api/v1/voice/ticket`: the one-shot *call ticket* a voice socket
//! presents.
//!
//! A browser sends no `Referer` on a socket handshake, so the upgrade alone
//! cannot tell the shell from an app page on the same origin. The mint is a
//! plain `fetch`, which carries one. So the mint runs the strict origin rule
//! and every refusal a caller can act on. The upgrade only redeems what the
//! mint issued (ADR 0598).
//!
//! A ticket binds one device to one thread, lives [`TICKET_TTL`], and spends
//! once. It is kept in memory like a reveal token: a restart costs one retry.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::routing::post;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::error::ApiError;
use super::secret_reveal::{reveal_request_allowed, RefererRule};
use super::AppState;
use crate::engine::thread_events::MessageOrigin;

/// Query parameter the voice upgrade carries its ticket in.
pub const QUERY_CALL_TICKET: &str = "ticket";

/// How long a minted ticket stays usable. Long enough for the client to dial
/// once the microphone is open, short enough that a leaked one is worthless.
const TICKET_TTL: Duration = Duration::from_secs(30);

/// What a mint refused on origin grounds is told. The caller may be the owner
/// on a paired device whose browser withholds the page address.
pub(super) const NOT_FROM_THE_SHELL: &str =
    "A call starts from the Lucidos page itself, never from an app \
     page. This request named an app page, or no page at all. If this is Lucidos, the browser \
     is withholding the page address (the Referer header): allow it for this site.";

struct CallTicket {
    device_id: String,
    thread_id: Uuid,
    expires_at: Instant,
}

/// The live call tickets, keyed by ticket.
#[derive(Clone, Default)]
pub struct CallTickets {
    inner: Arc<Mutex<HashMap<String, CallTicket>>>,
}

impl CallTickets {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint a ticket that lets `device_id` call `thread_id`, once.
    ///
    /// Expired entries go on the way through, so unused tickets cannot
    /// accumulate. A poisoned mutex answers `None` rather than panicking.
    pub(super) fn mint(&self, device_id: &str, thread_id: Uuid) -> Option<String> {
        let ticket = super::secret_reveal::fresh_token();
        let mut guard = self.inner.lock().ok()?;
        let now = Instant::now();
        guard.retain(|_, t| t.expires_at > now);
        guard.insert(
            ticket.clone(),
            CallTicket {
                device_id: device_id.to_string(),
                thread_id,
                expires_at: now + TICKET_TTL,
            },
        );
        Some(ticket)
    }

    /// The device a live ticket was minted by, without spending it.
    ///
    /// `api::mutating_gate` asks this to name an upgrade that carries no
    /// device header. The handler then redeems.
    pub(super) fn peek_device(&self, ticket: &str) -> Option<String> {
        let guard = self.inner.lock().ok()?;
        guard
            .get(ticket)
            .filter(|t| t.expires_at > Instant::now())
            .map(|t| t.device_id.clone())
    }

    /// Spend `ticket` on `thread_id`, answering the device it binds.
    ///
    /// The entry goes whichever way this lands. A ticket presented on the
    /// wrong thread has been mishandled, and a second try is not a case to
    /// support.
    pub(super) fn redeem(&self, ticket: &str, thread_id: Uuid) -> Option<String> {
        let entry = self.inner.lock().ok()?.remove(ticket)?;
        (entry.expires_at > Instant::now() && entry.thread_id == thread_id)
            .then_some(entry.device_id)
    }
}

/// The ticket an upgrade URI carries, if any.
pub(super) fn ticket_in(uri: &Uri) -> Option<String> {
    let axum::extract::Query(mut query) =
        axum::extract::Query::<HashMap<String, String>>::try_from_uri(uri).ok()?;
    query.remove(QUERY_CALL_TICKET)
}

#[derive(Deserialize)]
struct CallTicketRequest {
    thread_id: Uuid,
}

#[derive(Serialize)]
struct CallTicketResponse {
    ticket: String,
    expires_in_secs: u64,
}

/// Mint a ticket for the owner's device to call `thread_id`.
///
/// Every refusal the upgrade would make, made here first. A browser hides a
/// handshake's body, and it does not hide a `fetch`'s, so this is where the
/// person reads why there is no call.
async fn mint_call_ticket(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<CallTicketRequest>,
) -> Result<Json<CallTicketResponse>, ApiError> {
    let ticket = mint(
        &state.pool,
        &state.engine.voice_sessions,
        &state.call_tickets,
        &headers,
        request.thread_id,
    )
    .await?;
    Ok(Json(CallTicketResponse {
        ticket,
        expires_in_secs: TICKET_TTL.as_secs(),
    }))
}

/// The mint, apart from its extractors so the tests can drive it.
pub(super) async fn mint(
    pool: &sqlx::PgPool,
    sessions: &crate::voice::registry::LiveVoiceSessions,
    tickets: &CallTickets,
    headers: &HeaderMap,
    thread_id: Uuid,
) -> Result<String, ApiError> {
    // The strict rule: a browser-shaped mint must name the page it came from,
    // and that page must not be an app. A `POST` passes the service worker
    // untouched, so the shell's own `Referer` always arrives.
    if !reveal_request_allowed(headers, RefererRule::Required) {
        crate::log!("[Voice] Refusing a call ticket for an app document or an unnamed page");
        return Err(ApiError::new(StatusCode::FORBIDDEN, NOT_FROM_THE_SHELL));
    }
    let actor = super::actor::require_owner_device_for(
        headers,
        pool,
        None,
        super::voice::NOT_A_PERSONS_DEVICE,
    )
    .await?;
    let MessageOrigin::Device { device_id, .. } = &actor else {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            super::voice::NOT_A_PERSONS_DEVICE,
        ));
    };
    super::voice::refuse_call_on(pool, sessions, thread_id).await?;
    tickets
        .mint(device_id, thread_id)
        .ok_or_else(|| ApiError::internal("The call ticket store is poisoned."))
}

pub(super) fn router() -> Router<AppState> {
    Router::new().route("/voice/ticket", post(mint_call_ticket))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: &str = "owners-phone";

    #[test]
    fn a_ticket_spends_once_on_its_own_thread() {
        let tickets = CallTickets::new();
        let home = Uuid::new_v4();
        let ticket = tickets.mint(PHONE, home).expect("a ticket");
        assert_eq!(tickets.redeem(&ticket, home).as_deref(), Some(PHONE));
        assert_eq!(tickets.redeem(&ticket, home), None, "a ticket spends once");
    }

    /// A ticket presented on another thread is spent by the attempt, so it
    /// cannot then open the thread it was minted for.
    #[test]
    fn a_ticket_on_another_thread_is_refused_and_spent() {
        let tickets = CallTickets::new();
        let home = Uuid::new_v4();
        let ticket = tickets.mint(PHONE, home).expect("a ticket");
        assert_eq!(tickets.redeem(&ticket, Uuid::new_v4()), None);
        assert_eq!(tickets.redeem(&ticket, home), None);
    }

    #[test]
    fn a_peek_names_the_device_without_spending() {
        let tickets = CallTickets::new();
        let home = Uuid::new_v4();
        let ticket = tickets.mint(PHONE, home).expect("a ticket");
        assert_eq!(tickets.peek_device(&ticket).as_deref(), Some(PHONE));
        assert_eq!(tickets.peek_device("not-a-ticket"), None);
        assert_eq!(tickets.redeem(&ticket, home).as_deref(), Some(PHONE));
        assert_eq!(
            tickets.peek_device(&ticket),
            None,
            "a spent ticket names nobody"
        );
    }

    #[test]
    fn an_expired_ticket_is_refused() {
        let tickets = CallTickets::new();
        let home = Uuid::new_v4();
        let ticket = tickets.mint(PHONE, home).expect("a ticket");
        tickets
            .inner
            .lock()
            .expect("the store")
            .get_mut(&ticket)
            .expect("the entry")
            .expires_at = Instant::now() - Duration::from_secs(1);
        assert_eq!(tickets.peek_device(&ticket), None);
        assert_eq!(tickets.redeem(&ticket, home), None);
    }

    #[test]
    fn the_ticket_is_read_from_its_query_parameter() {
        let uri: Uri = format!("/api/v1/voice?thread_id=x&{QUERY_CALL_TICKET}=abc")
            .parse()
            .expect("a valid URI");
        assert_eq!(ticket_in(&uri).as_deref(), Some("abc"));
        let bare: Uri = "/api/v1/voice?thread_id=x".parse().expect("a valid URI");
        assert_eq!(ticket_in(&bare), None);
    }
}
