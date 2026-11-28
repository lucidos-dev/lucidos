use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{AppHandle, Manager};

const HEARTBEAT_PATH: &str = "/~/api/v1/control/desktop-client/heartbeat";

const HEARTBEAT_VERSION: &str = crate::LUCIDOS_RELEASE;

const IDLE_INTERVAL: Duration = Duration::from_secs(5);

const RUN_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, PartialEq)]
struct Pending {
    request: String,
    frame: Value,
}

#[derive(Default)]
struct Feed {
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

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Clone)]
pub struct RelayTag {
    request: String,
    feed: Arc<Feed>,
}

impl RelayTag {
    pub fn publish(&self, frame: &impl Serialize) {
        if let Ok(frame) = serde_json::to_value(frame) {
            self.feed.publish(Pending {
                request: self.request.clone(),
                frame,
            });
        }
    }
}

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

fn requested(reply: &str) -> Option<Ticket> {
    serde_json::from_str::<HeartbeatReply>(reply).ok()?.request
}

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
    loop {
        let progress = feed.take();
        let blocker = crate::updater::remote_install_blocker();
        let body = heartbeat_body(HEARTBEAT_VERSION, blocker.as_deref(), progress.as_ref());
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
    fn a_heartbeat_reports_the_release_not_the_build_id() {
        let release = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../RELEASE"),
        )
        .unwrap();
        assert_eq!(HEARTBEAT_VERSION, release.trim());
        assert_ne!(HEARTBEAT_VERSION, env!("LUCIDOS_APP_VERSION"));
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
