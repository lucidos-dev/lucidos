//! The client half of the update relay (ADR 0338): a session that cannot
//! install asks the gateway, and this process runs the install for it.
//!
//! Its own thread, in Rust rather than in a webview. At login the window is
//! hidden and shows the picker. A hidden WKWebView can also be suspended. So a
//! relay living in a page would work only while a window happened to be awake.
//!
//! Each heartbeat carries this client's version, its remote install blocker,
//! and the latest progress frame of a relayed run. The reply may hand it a
//! request, which it runs through the same `run_app_update` a click runs.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager};

const HEARTBEAT_PATH: &str = "/~/api/v1/control/desktop-client/heartbeat";

/// Idle cadence. The gateway counts a client attached for three of these.
const IDLE_INTERVAL: Duration = Duration::from_secs(5);

/// Cadence while a relayed run is under way, so the requester's progress bar
/// moves rather than jumping every five seconds.
const RUN_INTERVAL: Duration = Duration::from_secs(1);

/// A progress frame waiting for the next heartbeat.
#[derive(Debug, Clone, PartialEq)]
struct Pending {
    request: String,
    frame: Value,
}

/// What the heartbeat thread and a relayed run share.
#[derive(Default)]
struct Feed {
    /// The newest frame not yet delivered. Older ones are dropped on purpose:
    /// the requester shows where the run is, not every step it took.
    pending: Mutex<Option<Pending>>,
    running: AtomicBool,
}

impl Feed {
    fn publish(&self, pending: Pending) {
        *lock(&self.pending) = Some(pending);
    }

    fn take(&self) -> Option<Pending> {
        lock(&self.pending).take()
    }

    /// Put an undelivered frame back, unless a newer one has landed since.
    fn restore(&self, pending: Pending) {
        lock(&self.pending).get_or_insert(pending);
    }

    fn set_running(&self, running: bool) {
        self.running.store(running, Ordering::Relaxed);
    }

    fn interval(&self) -> Duration {
        if self.running.load(Ordering::Relaxed) || lock(&self.pending).is_some() {
            RUN_INTERVAL
        } else {
            IDLE_INTERVAL
        }
    }
}

/// A poisoned lock means a panic while holding it. Every write here is a
/// whole-value replace, so no half-written state can be left behind.
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Marks a run as relayed, and carries its frames back to the requester.
#[derive(Clone)]
pub struct RelayTag {
    request: String,
    feed: Arc<Feed>,
}

impl RelayTag {
    /// Queue a progress frame for the next heartbeat.
    pub fn publish(&self, frame: &impl Serialize) {
        if let Ok(frame) = serde_json::to_value(frame) {
            self.feed.publish(Pending {
                request: self.request.clone(),
                frame,
            });
        }
    }
}

/// A request the gateway handed this client.
#[derive(Debug, Deserialize, PartialEq)]
struct Ticket {
    id: String,
    version: String,
}

#[derive(Deserialize)]
struct HeartbeatReply {
    #[serde(default)]
    request: Option<Ticket>,
}

fn heartbeat_body(version: &str, blocker: Option<&str>, progress: Option<&Pending>) -> String {
    json!({
        "version": version,
        "blocker": blocker,
        "progress": progress.map(|p| json!({ "request": p.request, "frame": p.frame })),
    })
    .to_string()
}

/// The request in a heartbeat reply, if any. An unreadable reply is no request.
fn requested(reply: &str) -> Option<Ticket> {
    serde_json::from_str::<HeartbeatReply>(reply).ok()?.request
}

/// Start the heartbeat thread. Packaged macOS only; `desktop::launch` calls it.
pub fn start(app: AppHandle, port: u16) {
    let feed = Arc::new(Feed::default());
    let (nudge, nudged) = std::sync::mpsc::channel();
    std::thread::spawn(move || beat_forever(app, port, feed, nudge, nudged));
}

fn beat_forever(
    app: AppHandle,
    port: u16,
    feed: Arc<Feed>,
    nudge: Sender<()>,
    nudged: Receiver<()>,
) {
    let version = env!("LUCIDOS_APP_VERSION");
    loop {
        let progress = feed.take();
        let blocker = crate::updater::remote_install_blocker();
        let body = heartbeat_body(version, blocker.as_deref(), progress.as_ref());
        match crate::desktop::gateway_body(port, "POST", HEARTBEAT_PATH, Some(&body)) {
            Some(reply) => {
                if let Some(ticket) = requested(&reply) {
                    eprintln!(
                        "[update-relay] running request {} for {}",
                        ticket.id, ticket.version
                    );
                    run_relayed(&app, &feed, &nudge, ticket);
                }
            }
            // The gateway is restarting or not up yet. Keep the frame for the
            // next beat.
            None => {
                if let Some(progress) = progress {
                    feed.restore(progress);
                }
            }
        }
        match nudged.recv_timeout(feed.interval()) {
            Ok(()) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => std::thread::sleep(feed.interval()),
        }
    }
}

/// Run a relayed request on the async runtime. A successful run never returns,
/// since it relaunches this process. Any other ending wakes the heartbeat at
/// once, so the requester hears the outcome without waiting a full beat.
fn run_relayed(app: &AppHandle, feed: &Arc<Feed>, nudge: &Sender<()>, ticket: Ticket) {
    let tag = RelayTag {
        request: ticket.id,
        feed: Arc::clone(feed),
    };
    let app = app.clone();
    let feed = Arc::clone(feed);
    let nudge = nudge.clone();
    feed.set_running(true);
    tauri::async_runtime::spawn(async move {
        let run = app.state::<crate::updater::AppUpdateRun>();
        if let Err(e) = crate::updater::run_app_update(app.clone(), &run, Some(tag)).await {
            eprintln!("[update-relay] relayed update ended: {e}");
        }
        feed.set_running(false);
        let _ = nudge.send(());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(request: &str, phase: &str) -> Pending {
        Pending {
            request: request.to_string(),
            frame: json!({ "version": "1.1.0", "phase": phase }),
        }
    }

    #[test]
    fn a_heartbeat_carries_version_blocker_and_progress() {
        let progress = pending("r-1", "downloading");
        let body: Value =
            serde_json::from_str(&heartbeat_body("1.0.0", Some("blocked"), Some(&progress)))
                .unwrap();
        assert_eq!(
            body,
            json!({
                "version": "1.0.0",
                "blocker": "blocked",
                "progress": { "request": "r-1", "frame": progress.frame },
            })
        );
    }

    #[test]
    fn an_idle_heartbeat_carries_no_progress() {
        let body: Value = serde_json::from_str(&heartbeat_body("1.0.0", None, None)).unwrap();
        assert_eq!(body["blocker"], Value::Null);
        assert_eq!(body["progress"], Value::Null);
    }

    #[test]
    fn a_reply_naming_a_request_hands_it_over() {
        assert_eq!(
            requested(r#"{"request":{"id":"r-1","version":"1.1.0"}}"#),
            Some(Ticket {
                id: "r-1".to_string(),
                version: "1.1.0".to_string()
            })
        );
    }

    #[test]
    fn a_reply_with_no_request_or_no_sense_hands_nothing_over() {
        for reply in [r#"{"request":null}"#, "{}", "not json", ""] {
            assert_eq!(requested(reply), None, "{reply:?}");
        }
    }

    #[test]
    fn only_the_newest_frame_waits_for_the_next_beat() {
        let feed = Feed::default();
        feed.publish(pending("r-1", "checking"));
        feed.publish(pending("r-1", "downloading"));
        assert_eq!(feed.take(), Some(pending("r-1", "downloading")));
        assert_eq!(feed.take(), None);
    }

    // A failed delivery must not overwrite a frame the run produced since.
    #[test]
    fn a_frame_that_failed_to_send_never_overwrites_a_newer_one() {
        let feed = Feed::default();
        feed.restore(pending("r-1", "checking"));
        assert_eq!(feed.take(), Some(pending("r-1", "checking")));

        feed.publish(pending("r-1", "failed"));
        feed.restore(pending("r-1", "downloading"));
        assert_eq!(feed.take(), Some(pending("r-1", "failed")));
    }

    #[test]
    fn the_heartbeat_speeds_up_while_a_relayed_run_has_news() {
        let feed = Feed::default();
        assert_eq!(feed.interval(), IDLE_INTERVAL);
        feed.set_running(true);
        assert_eq!(feed.interval(), RUN_INTERVAL);
        feed.set_running(false);
        feed.publish(pending("r-1", "failed"));
        assert_eq!(feed.interval(), RUN_INTERVAL, "a final frame is still owed");
    }

    #[test]
    fn a_tag_publishes_the_frame_under_its_request() {
        let feed = Arc::new(Feed::default());
        let tag = RelayTag {
            request: "r-1".to_string(),
            feed: Arc::clone(&feed),
        };
        tag.publish(&json!({ "phase": "installing" }));
        assert_eq!(
            feed.take(),
            Some(Pending {
                request: "r-1".to_string(),
                frame: json!({ "phase": "installing" }),
            })
        );
    }
}
