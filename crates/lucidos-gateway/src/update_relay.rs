//! The update relay (ADR 0338): how a session that cannot install gets the
//! desktop client to install for it.
//!
//! The gateway only carries messages. The client's Rust side posts a heartbeat
//! here, and the reply hands it a pending request. The install itself runs in
//! the client and never here: ADR 0108 records why a gateway-spawned installer
//! kills itself part way through.
//!
//! Memory only. A successful install restarts the service and this state with
//! it, so the requesting session settles on the running version instead.

use crate::release_check::version_is_newer;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A client whose last heartbeat is older than this is not attached. Three
/// missed beats at the client's idle cadence.
pub const ATTACHED_WITHIN: Duration = Duration::from_secs(15);

/// An unclaimed request older than this is dropped, so a client that attaches
/// hours after the tap never installs out of the blue.
pub const REQUEST_EXPIRES_AFTER: Duration = Duration::from_secs(120);

/// A claimed request with no progress this long after the claim is dropped.
/// The client sends its first frame the moment a run starts. So silence means
/// the claim never reached it, such as a heartbeat reply it timed out on.
pub const CLAIM_UNCONFIRMED_AFTER: Duration = Duration::from_secs(30);

/// The progress phases after which a run is over without a restart. Mirrors
/// the terminal arms of `AppUpdatePhase` in the client's `updater.rs`.
const TERMINAL_PHASES: &[&str] = &["failed", "bundle-swap-failed", "cancelled"];

/// One heartbeat from the desktop client.
#[derive(Debug, Deserialize)]
pub struct Heartbeat {
    /// The client's own version.
    pub version: String,
    /// Why an unattended install could not run here, or absent when it could.
    #[serde(default)]
    pub blocker: Option<String>,
    /// The latest progress frame of a relayed run.
    #[serde(default)]
    pub progress: Option<RelayedProgress>,
}

/// A progress frame, tagged with the request it belongs to.
#[derive(Debug, Deserialize)]
pub struct RelayedProgress {
    pub request: String,
    /// An `AppUpdateProgress` frame exactly as the client emits it to its own
    /// webviews. Carried opaque, so the requesting session reads the same shape
    /// the local dialog does.
    pub frame: Value,
}

/// A request, as handed to the client and returned to the requester.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ticket {
    pub id: String,
    /// The release the requester saw. The client installs whatever its updater
    /// resolves, and its frames name that version.
    pub version: String,
}

/// Why a request was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    NoClient,
    Blocked(String),
    NoNewerRelease,
    InFlight,
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Refusal::NoClient => "the desktop app is not running on this machine".to_string(),
            Refusal::Blocked(reason) => reason.clone(),
            Refusal::NoNewerRelease => "the desktop app is already up to date".to_string(),
            Refusal::InFlight => "an update is already in progress".to_string(),
        }
    }
}

struct Client {
    version: String,
    blocker: Option<String>,
    seen: Instant,
}

enum Relay {
    Idle,
    /// Waiting for the client's next heartbeat to claim it.
    Requested {
        ticket: Ticket,
        at: Instant,
    },
    /// Claimed at `claimed`. `frame` is the latest progress, absent until the
    /// first arrives.
    Running {
        ticket: Ticket,
        claimed: Instant,
        frame: Option<Value>,
    },
    /// Over without a restart: failed, refused or cancelled.
    Ended {
        ticket: Ticket,
        frame: Value,
    },
}

struct State {
    client: Option<Client>,
    relay: Relay,
}

/// The gateway's one relay. Held on `GatewayInner`.
pub struct UpdateRelay {
    state: Mutex<State>,
    /// Ticket ids are `<boot>-<n>`, so a frame from a run before a gateway
    /// restart can never match a ticket issued after it.
    boot: u128,
    next: AtomicU64,
}

impl Default for UpdateRelay {
    fn default() -> Self {
        let boot = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        UpdateRelay {
            state: Mutex::new(State {
                client: None,
                relay: Relay::Idle,
            }),
            boot,
            next: AtomicU64::new(1),
        }
    }
}

impl State {
    fn attached(&self, now: Instant) -> Option<&Client> {
        self.client
            .as_ref()
            .filter(|c| now.saturating_duration_since(c.seen) < ATTACHED_WITHIN)
    }

    /// Drop a relay nobody will finish: a request left unclaimed too long, a
    /// claim the client never confirmed, or a run whose client stopped beating.
    /// Either way the requester hears "did not run" rather than waiting on a
    /// dead run, and the next request is not refused as in flight.
    fn settle(&mut self, now: Instant) {
        let dead = match &self.relay {
            Relay::Requested { at, .. } => {
                now.saturating_duration_since(*at) >= REQUEST_EXPIRES_AFTER
            }
            Relay::Running {
                claimed,
                frame: None,
                ..
            } if now.saturating_duration_since(*claimed) >= CLAIM_UNCONFIRMED_AFTER => true,
            Relay::Running { .. } => self.attached(now).is_none(),
            Relay::Idle | Relay::Ended { .. } => false,
        };
        if dead {
            self.relay = Relay::Idle;
        }
    }
}

fn is_terminal(frame: &Value) -> bool {
    frame
        .get("phase")
        .and_then(Value::as_str)
        .is_some_and(|phase| TERMINAL_PHASES.contains(&phase))
}

impl UpdateRelay {
    /// Record a heartbeat, and hand back the pending request if there is one.
    /// A request is handed out exactly once.
    pub fn heartbeat(&self, beat: Heartbeat, now: Instant) -> Option<Ticket> {
        let mut st = self.lock();
        st.client = Some(Client {
            version: beat.version,
            blocker: beat.blocker,
            seen: now,
        });
        if let Some(progress) = beat.progress {
            let current = match &st.relay {
                Relay::Running {
                    ticket, claimed, ..
                } if ticket.id == progress.request => Some((ticket.clone(), *claimed)),
                _ => None,
            };
            if let Some((ticket, claimed)) = current {
                st.relay = if is_terminal(&progress.frame) {
                    Relay::Ended {
                        ticket,
                        frame: progress.frame,
                    }
                } else {
                    Relay::Running {
                        ticket,
                        claimed,
                        frame: Some(progress.frame),
                    }
                };
            }
        }
        st.settle(now);
        match &st.relay {
            Relay::Requested { ticket, .. } => {
                let ticket = ticket.clone();
                st.relay = Relay::Running {
                    ticket: ticket.clone(),
                    claimed: now,
                    frame: None,
                };
                Some(ticket)
            }
            _ => None,
        }
    }

    /// Queue a request for the client, or say why not. `latest` is the release
    /// check's newest published version.
    pub fn request(&self, latest: Option<&str>, now: Instant) -> Result<Ticket, Refusal> {
        let mut st = self.lock();
        st.settle(now);
        let client = st.attached(now).ok_or(Refusal::NoClient)?;
        if let Some(reason) = &client.blocker {
            return Err(Refusal::Blocked(reason.clone()));
        }
        let version = latest
            .filter(|latest| version_is_newer(latest, &client.version))
            .ok_or(Refusal::NoNewerRelease)?
            .to_string();
        if matches!(st.relay, Relay::Requested { .. } | Relay::Running { .. }) {
            return Err(Refusal::InFlight);
        }
        let n = self.next.fetch_add(1, Ordering::Relaxed);
        let ticket = Ticket {
            id: format!("{}-{n}", self.boot),
            version,
        };
        st.relay = Relay::Requested {
            ticket: ticket.clone(),
            at: now,
        };
        Ok(ticket)
    }

    /// The wire shape on `gateway/status`. `client` is null when no client is
    /// attached, and `relay` is null when nothing is pending, running or
    /// recently ended.
    pub fn snapshot(&self, now: Instant) -> Value {
        let mut st = self.lock();
        st.settle(now);
        let client = st.attached(now).map(|c| {
            json!({
                "version": c.version,
                "blocker": c.blocker,
            })
        });
        let request = match &st.relay {
            Relay::Idle => Value::Null,
            Relay::Requested { ticket, .. } => {
                json!({ "id": ticket.id, "version": ticket.version, "state": "requested", "progress": null })
            }
            Relay::Running { ticket, frame, .. } => {
                json!({ "id": ticket.id, "version": ticket.version, "state": "running", "progress": frame })
            }
            Relay::Ended { ticket, frame } => {
                json!({ "id": ticket.id, "version": ticket.version, "state": "ended", "progress": frame })
            }
        };
        json!({ "client": client, "request": request })
    }

    /// A poisoned lock means a panic while holding it. Every write here is a
    /// whole-value replace, so no half-written state can be left behind.
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
#[path = "update_relay_tests.rs"]
mod tests;
