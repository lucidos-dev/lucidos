use crate::release_check::ReleaseCheck;
use lucidos_update_channel::is_upgrade;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const ATTACHED_WITHIN: Duration = Duration::from_secs(15);

pub const REQUEST_EXPIRES_AFTER: Duration = Duration::from_secs(120);

pub const CLAIM_UNCONFIRMED_AFTER: Duration = Duration::from_secs(30);

const TERMINAL_PHASES: &[&str] = &["failed", "bundle-swap-failed", "cancelled"];

#[derive(Debug, Deserialize)]
pub struct Heartbeat {
    pub version: String,
    #[serde(default)]
    pub blocker: Option<String>,
    #[serde(default)]
    pub progress: Option<RelayedProgress>,
}

#[derive(Debug, Deserialize)]
pub struct RelayedProgress {
    pub request: String,
    pub frame: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Ticket {
    pub id: String,
    pub version: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    NoClient,
    Blocked(String),
    NoNewerRelease,
    CheckFailed(String),
    InFlight,
}

impl Refusal {
    pub fn message(&self) -> String {
        match self {
            Refusal::NoClient => "the desktop app is not running on this machine".to_string(),
            Refusal::Blocked(reason) => reason.clone(),
            Refusal::NoNewerRelease => {
                "no release newer than the desktop app is published yet".to_string()
            }
            Refusal::CheckFailed(error) => {
                format!("could not check for a newer release: {error}")
            }
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
    Requested {
        ticket: Ticket,
        at: Instant,
    },
    Running {
        ticket: Ticket,
        claimed: Instant,
        frame: Option<Value>,
    },
    Ended {
        ticket: Ticket,
        frame: Value,
    },
}

struct State {
    client: Option<Client>,
    relay: Relay,
}

pub struct UpdateRelay {
    state: Mutex<State>,
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

    pub fn request(&self, latest: Option<&str>, now: Instant) -> Result<Ticket, Refusal> {
        let mut st = self.lock();
        st.settle(now);
        let client = st.attached(now).ok_or(Refusal::NoClient)?;
        if let Some(reason) = &client.blocker {
            return Err(Refusal::Blocked(reason.clone()));
        }
        if matches!(st.relay, Relay::Requested { .. } | Relay::Running { .. }) {
            return Err(Refusal::InFlight);
        }
        let version = latest
            .filter(|latest| is_upgrade(&client.version, latest))
            .ok_or(Refusal::NoNewerRelease)?
            .to_string();
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

    pub async fn request_refreshing(
        &self,
        check: &ReleaseCheck,
        now: Instant,
    ) -> Result<Ticket, Refusal> {
        let asked = self.request(check.latest_version().as_deref(), now);
        if !matches!(asked, Err(Refusal::NoNewerRelease)) {
            return asked;
        }
        let checked = check.refresh(true).await;
        let refused = self.request(check.latest_version().as_deref(), Instant::now());
        match (&refused, checked["last_error"].as_str()) {
            (Err(Refusal::NoNewerRelease), Some(error)) => {
                Err(Refusal::CheckFailed(error.to_string()))
            }
            _ => refused,
        }
    }

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

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
#[path = "update_relay_tests.rs"]
mod tests;
