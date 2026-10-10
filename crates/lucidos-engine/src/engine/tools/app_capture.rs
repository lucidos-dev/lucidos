//! `capture_app` / `refresh_app`: which device answers, and what the agent is told.
//!
//! Every connected page receives a thread event, so the device actor stamped
//! here IS the routing decision, as for `navigate_ui` (`navigate.rs`). A page
//! acts only on a request scoped to it or to no device (`isForThisDevice` in
//! `device-scope.ts`). The engine also refuses an answer from any other device. So
//! a page with no app frame cannot win the race with a quick error.

use super::navigate::{device_actor, device_label, device_presence};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, ThreadEvent};
use crate::engine::CaptureResult;
use sqlx::PgPool;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::oneshot;
use uuid::Uuid;

/// How long the agent waits for an answer. The page gives up after 8 s
/// (`captureAppUI`), so its own error arrives before this.
pub(crate) const CAPTURE_TIMEOUT: Duration = Duration::from_secs(10);

/// A capture waiting for its answer.
pub struct PendingCapture {
    /// The device that must answer. `None` when the turn has no last used
    /// device, and then any page may.
    target: Option<String>,
    tx: oneshot::Sender<CaptureResult>,
}

/// The captures waiting for an answer, by request id.
pub type PendingCaptures = Mutex<HashMap<String, PendingCapture>>;

/// Why an answer was not taken.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum CaptureRefusal {
    /// No capture waits under this id: it timed out, or was already answered.
    Unknown,
    /// The capture waits for another device. It stays open for that one.
    OtherDevice,
}

/// Hand `result` to the capture `request_id` waits on, if `answering` is the
/// device it waits for.
pub(crate) fn resolve_capture(
    pending: &PendingCaptures,
    request_id: &str,
    answering: Option<&str>,
    result: CaptureResult,
) -> Result<(), CaptureRefusal> {
    let mut captures = pending.lock().unwrap();
    let Entry::Occupied(waiting) = captures.entry(request_id.to_string()) else {
        return Err(CaptureRefusal::Unknown);
    };
    let target = waiting.get().target.as_deref();
    if target.is_some_and(|target| answering != Some(target)) {
        return Err(CaptureRefusal::OtherDevice);
    }
    // A closed receiver means the tool call itself is gone (a canceled turn),
    // so there is nobody left to tell.
    let _ = waiting.remove().tx.send(result);
    Ok(())
}

/// The `capture_app` and `refresh_app` tools. `last_used` is the turn's last
/// used device, the one `navigate_ui` also sends to.
// One argument per collaborator; a bundle struct would only rename them.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn app_ui_tool_impl(
    bus: &EventBus,
    pool: &PgPool,
    pending: &PendingCaptures,
    args: &serde_json::Value,
    thread_id: Uuid,
    refresh: bool,
    last_used: Option<&str>,
    timeout: Duration,
) -> String {
    let app_id = args
        .get("app_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let meta = || EventMeta::with_actor(last_used.map(device_actor));

    if refresh {
        bus.emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: ThreadEvent::AppUiRefreshRequested {
                    app_id: app_id.clone(),
                },
                meta: meta(),
            },
            "[AppCapture] AppUiRefreshRequested (refresh_app tool)",
        )
        .await;
    }
    if args
        .get("skip_capture")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return "App UI refreshed.".to_string();
    }
    if refresh {
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let request_id = Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    pending.lock().unwrap().insert(
        request_id.clone(),
        PendingCapture {
            target: last_used.map(str::to_string),
            tx,
        },
    );
    log!("[AppCapture] capture thread={thread_id} app={app_id} device={last_used:?}");
    let emitted = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::AppUiCaptureRequested {
                app_id,
                request_id: request_id.clone(),
            },
            meta: meta(),
        })
        .await;
    if let Err(e) = emitted {
        pending.lock().unwrap().remove(&request_id);
        return format!("Error: Capture failed: could not send the request: {e}");
    }

    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(capture)) => match last_used {
            Some(device) if capture.screenshot.is_empty() => {
                answered_error(&device_label(pool, device).await, capture.dom)
            }
            _ => crate::engine::agentic_loop::format_capture_result(
                &capture.screenshot,
                &capture.dom,
            ),
        },
        Ok(Err(_)) => "Error: Capture failed: the frontend channel was dropped.".to_string(),
        Err(_) => {
            pending.lock().unwrap().remove(&request_id);
            timed_out(pool, last_used, timeout).await
        }
    }
}

/// An error the target device sent back, prefixed with that device's name, so
/// the agent knows which screen it is about. A DOM-only capture is not an
/// error and passes through unchanged.
fn answered_error(label: &str, dom: String) -> String {
    match dom.strip_prefix("Error: ") {
        Some(reason) => {
            format!("Error from \"{label}\", the user's last used device in this turn: {reason}")
        }
        None => dom,
    }
}

/// Nobody answered. Name who was asked, and whether Lucidos looks open there.
async fn timed_out(pool: &PgPool, last_used: Option<&str>, timeout: Duration) -> String {
    let secs = timeout.as_secs();
    let Some(device) = last_used else {
        return format!(
            "Error: Capture timed out ({secs}s). No device answered. This turn has no last \
             used device, so the request went to every connected device and none could \
             capture the app UI."
        );
    };
    let label = device_label(pool, device).await;
    let mut out = format!(
        "Error: Capture timed out ({secs}s). \"{label}\", the user's last used device in this \
         turn, did not answer."
    );
    if let Some(seen) = device_presence(pool, device)
        .await
        .filter(|seen| !seen.visible_now)
    {
        out.push_str(&format!(
            " Lucidos is not seen visible on it now (last seen {}), so it is likely closed \
             or suspended there. Ask the user to open Lucidos on that device.",
            crate::engine::agent_context::format_age(seen.seen_secs_ago)
        ));
    }
    out
}

#[cfg(test)]
#[path = "app_capture_tests.rs"]
mod tests;
