//! Socket tickets: how a browser socket names its device.
//!
//! A browser `WebSocket` sets no header and sends no `Referer`. So a `fetch`
//! mints a ticket under the strict reveal rule. The handshake redeems it once,
//! for the one socket it names.

use crate::core::subprocess_origin::QUERY_SOCKET_TICKET;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::http::Uri;
use uuid::Uuid;

pub(super) const TICKET_TTL: Duration = Duration::from_secs(30);

/// A minted ticket as a client sees it.
#[derive(serde::Serialize)]
pub(super) struct SocketTicketView {
    ticket: String,
    expires_in_secs: u64,
}

impl SocketTicketView {
    pub(super) fn new(ticket: String) -> SocketTicketView {
        SocketTicketView {
            ticket,
            expires_in_secs: TICKET_TTL.as_secs(),
        }
    }
}

/// The one socket a ticket opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SocketTarget {
    Call { thread_id: Uuid },
    Terminal { thread_id: Uuid, terminal_id: Uuid },
}

struct SocketTicket {
    device_id: String,
    target: SocketTarget,
    expires_at: Instant,
    generation: Option<Uuid>,
}

#[derive(Clone, Default)]
pub struct SocketTickets {
    inner: Arc<Mutex<HashMap<String, SocketTicket>>>,
}

impl SocketTickets {
    pub fn new() -> Self {
        Self::default()
    }

    pub(super) fn mint(
        &self,
        device_id: &str,
        target: SocketTarget,
        generation: Option<Uuid>,
    ) -> Option<String> {
        let ticket = super::secret_reveal::fresh_token();
        let mut guard = self.inner.lock().ok()?;
        let now = Instant::now();
        guard.retain(|_, t| t.expires_at > now);
        guard.insert(
            ticket.clone(),
            SocketTicket {
                device_id: device_id.to_string(),
                target,
                generation,
                expires_at: now + TICKET_TTL,
            },
        );
        Some(ticket)
    }

    pub(super) fn peek_device(&self, ticket: &str, generation: Option<Uuid>) -> Option<String> {
        let guard = self.inner.lock().ok()?;
        guard
            .get(ticket)
            .filter(|t| t.expires_at > Instant::now() && t.generation == generation)
            .map(|t| t.device_id.clone())
    }

    /// Spend the ticket. It names its device only for the socket it was
    /// minted for, and a wrong target spends it all the same.
    pub(super) fn redeem(
        &self,
        ticket: &str,
        target: SocketTarget,
        generation: Option<Uuid>,
    ) -> Option<String> {
        let entry = self.inner.lock().ok()?.remove(ticket)?;
        (entry.expires_at > Instant::now()
            && entry.target == target
            && entry.generation == generation)
            .then_some(entry.device_id)
    }
}

pub(super) fn ticket_in(uri: &Uri) -> Option<String> {
    let axum::extract::Query(mut query) =
        axum::extract::Query::<HashMap<String, String>>::try_from_uri(uri).ok()?;
    query.remove(QUERY_SOCKET_TICKET)
}

/// INVARIANT: A ticket redeems only beside the device it names.
///
/// The gateway's authenticated header wins over the ticket: a header naming
/// another device refuses the socket.
pub(super) fn header_agrees(headers: &axum::http::HeaderMap, device_id: &str) -> bool {
    headers
        .get(crate::core::subprocess_origin::HEADER_DEVICE_ID)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|header| header == device_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE: &str = "owners-phone";

    fn call(thread_id: Uuid) -> SocketTarget {
        SocketTarget::Call { thread_id }
    }

    #[test]
    fn a_ticket_spends_once_on_its_own_socket() {
        let tickets = SocketTickets::new();
        let home = Uuid::new_v4();
        let ticket = tickets.mint(PHONE, call(home), None).expect("a ticket");
        assert_eq!(
            tickets.redeem(&ticket, call(home), None).as_deref(),
            Some(PHONE)
        );
        assert_eq!(
            tickets.redeem(&ticket, call(home), None),
            None,
            "a ticket spends once"
        );
    }

    #[test]
    fn a_ticket_for_another_socket_is_refused_and_spent() {
        let tickets = SocketTickets::new();
        let home = Uuid::new_v4();
        let ticket = tickets.mint(PHONE, call(home), None).expect("a ticket");
        assert_eq!(tickets.redeem(&ticket, call(Uuid::new_v4()), None), None);
        assert_eq!(tickets.redeem(&ticket, call(home), None), None);
    }

    #[test]
    fn a_ticket_for_a_call_never_opens_a_terminal_on_the_same_thread() {
        let tickets = SocketTickets::new();
        let thread_id = Uuid::new_v4();
        let ticket = tickets
            .mint(PHONE, call(thread_id), None)
            .expect("a ticket");
        let terminal = SocketTarget::Terminal {
            thread_id,
            terminal_id: Uuid::new_v4(),
        };
        assert_eq!(tickets.redeem(&ticket, terminal, None), None);
    }

    #[test]
    fn a_terminal_ticket_names_its_terminal() {
        let tickets = SocketTickets::new();
        let thread_id = Uuid::new_v4();
        let terminal_id = Uuid::new_v4();
        let target = SocketTarget::Terminal {
            thread_id,
            terminal_id,
        };
        let stale = tickets.mint(PHONE, target, None).expect("a ticket");
        let other = SocketTarget::Terminal {
            thread_id,
            terminal_id: Uuid::new_v4(),
        };
        assert_eq!(tickets.redeem(&stale, other, None), None);
        let ticket = tickets.mint(PHONE, target, None).expect("a ticket");
        assert_eq!(
            tickets.redeem(&ticket, target, None).as_deref(),
            Some(PHONE)
        );
    }

    #[test]
    fn a_peek_names_the_device_without_spending() {
        let tickets = SocketTickets::new();
        let home = Uuid::new_v4();
        let ticket = tickets.mint(PHONE, call(home), None).expect("a ticket");
        assert_eq!(tickets.peek_device(&ticket, None).as_deref(), Some(PHONE));
        assert_eq!(tickets.peek_device("not-a-ticket", None), None);
        assert_eq!(
            tickets.redeem(&ticket, call(home), None).as_deref(),
            Some(PHONE)
        );
        assert_eq!(
            tickets.peek_device(&ticket, None),
            None,
            "a spent ticket names nobody"
        );
    }

    #[test]
    fn an_expired_ticket_is_refused() {
        let tickets = SocketTickets::new();
        let home = Uuid::new_v4();
        let ticket = tickets.mint(PHONE, call(home), None).expect("a ticket");
        tickets
            .inner
            .lock()
            .expect("the store")
            .get_mut(&ticket)
            .expect("the entry")
            .expires_at = Instant::now() - Duration::from_secs(1);
        assert_eq!(tickets.peek_device(&ticket, None), None);
        assert_eq!(tickets.redeem(&ticket, call(home), None), None);
    }

    #[test]
    fn generation_changes_refuse_old_tickets_even_with_the_same_device() {
        let tickets = SocketTickets::new();
        let home = Uuid::new_v4();
        let restored = Some(Uuid::new_v4());
        let old = tickets.mint(PHONE, call(home), None).unwrap();
        assert_eq!(tickets.peek_device(&old, restored), None);
        assert_eq!(tickets.redeem(&old, call(home), restored), None);
        assert_eq!(tickets.redeem(&old, call(home), None), None);
        let current = tickets.mint(PHONE, call(home), restored).unwrap();
        assert_eq!(
            tickets.peek_device(&current, restored).as_deref(),
            Some(PHONE)
        );
        assert_eq!(tickets.peek_device(&current, None), None);
        assert_eq!(tickets.peek_device(&current, Some(Uuid::new_v4())), None);
        assert_eq!(
            tickets.redeem(&current, call(home), restored).as_deref(),
            Some(PHONE)
        );
    }

    #[test]
    fn the_ticket_is_read_from_its_query_parameter() {
        let uri: Uri = format!("/api/v1/voice?thread_id=x&{QUERY_SOCKET_TICKET}=abc")
            .parse()
            .expect("a valid URI");
        assert_eq!(ticket_in(&uri).as_deref(), Some("abc"));
        let bare: Uri = "/api/v1/voice?thread_id=x".parse().expect("a valid URI");
        assert_eq!(ticket_in(&bare), None);
    }

    #[test]
    fn a_header_naming_another_device_disagrees() {
        let mut headers = axum::http::HeaderMap::new();
        assert!(
            header_agrees(&headers, PHONE),
            "no header defers to the ticket"
        );
        headers.insert(
            crate::core::subprocess_origin::HEADER_DEVICE_ID,
            PHONE.parse().unwrap(),
        );
        assert!(header_agrees(&headers, PHONE));
        assert!(!header_agrees(&headers, "owners-laptop"));
    }
}
