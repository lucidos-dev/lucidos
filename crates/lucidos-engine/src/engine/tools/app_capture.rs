//! `capture_app` / `refresh_app`: which device answers, what the agent is told,
//! and where a saved capture goes.
//!
//! Every connected page receives a thread event, so the device actor stamped
//! here IS the routing decision, as for `navigate_ui` (`navigate.rs`). A page
//! acts only on a request scoped to it or to no device (`isForThisDevice` in
//! `device-scope.ts`). The engine also refuses an answer from any other device. So
//! a page with no app frame cannot win the race with a quick error.

use super::navigate::{device_actor, device_label, device_presence};
use crate::core::{ArtifactManager, WriteAnnouncement};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{CaptureFormat, EventMeta, ThreadEvent};
use crate::engine::CaptureResult;
use crate::llm::tool_names as tn;
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

/// Where a capture is saved, and the format its extension names.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SaveTarget {
    path: String,
    format: CaptureFormat,
}

/// Read `save_as_artifact`, refusing a path that leaves `data/artifacts/` or
/// whose extension names no format a frame can encode.
pub(crate) fn parse_save_target(args: &serde_json::Value) -> Result<Option<SaveTarget>, String> {
    let Some(path) = args.get("save_as_artifact").and_then(|v| v.as_str()) else {
        return Ok(None);
    };
    if crate::core::is_path_traversal(path) {
        return Err(format!(
            "Invalid save_as_artifact path \"{path}\": it must not contain '..' or start with '/' or '\\'."
        ));
    }
    let extension = std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let format = CaptureFormat::from_extension(&extension).ok_or_else(|| {
        format!(
            "Invalid save_as_artifact path \"{path}\": end it in {}, which picks the format.",
            CaptureFormat::extensions()
        )
    })?;
    Ok(Some(SaveTarget {
        path: path.to_string(),
        format,
    }))
}

/// Write a capture's picture to its target, after checking that the device
/// encoded the format the extension names. Returns the line the agent reads,
/// or the error to report after `Error: `.
async fn save_capture(
    artifacts: &ArtifactManager,
    bus: &EventBus,
    target: &SaveTarget,
    screenshot_b64: &str,
    thread_id: Uuid,
) -> Result<String, String> {
    use base64::Engine;
    let path = &target.path;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(screenshot_b64)
        .map_err(|e| format!("Nothing saved to {path}: the picture is not valid base64: {e}"))?;
    let sent = crate::core::blobs::sniff_image_mime(&bytes).unwrap_or("an unknown format");
    if sent != target.format.mime() {
        return Err(format!(
            "Nothing saved to {path}: the device sent {sent}, not {}. The app may run an older \
             SDK, or the browser cannot encode that format. Try a .png path.",
            target.format.mime()
        ));
    }
    // A failure here may come after the file is written, at the commit, so
    // the error never claims nothing was saved.
    artifacts
        .write_and_commit(
            bus,
            path,
            &bytes,
            &format!("feat: app capture saved to {path}"),
            WriteAnnouncement::Entity {
                source: Some(tn::CAPTURE_APP.to_string()),
                writer_thread_id: Some(thread_id),
            },
        )
        .await
        .map_err(|e| format!("Saving {path} failed: {e}"))?;
    let size = match image::ImageReader::new(std::io::Cursor::new(&bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())
        .and_then(|reader| reader.into_dimensions().map_err(|e| e.to_string()))
    {
        Ok((w, h)) => format!("{w}x{h} px, "),
        Err(e) => {
            log!("[AppCapture] saved {path}, but its pixel size is unreadable: {e}");
            String::new()
        }
    };
    Ok(format!(
        "Saved the screenshot to {path} ({}, {size}{}).",
        target.format.mime(),
        crate::core::format_byte_size(bytes.len())
    ))
}

/// The `capture_app` and `refresh_app` tools. `last_used` is the turn's last
/// used device, the one `navigate_ui` also sends to.
// One argument per collaborator; a bundle struct would only rename them.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn app_ui_tool_impl(
    bus: &EventBus,
    pool: &PgPool,
    pending: &PendingCaptures,
    artifacts: &ArtifactManager,
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
    let save = match parse_save_target(args) {
        Ok(save) => save,
        Err(e) => return format!("Error: {e}"),
    };
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
                save_format: save.as_ref().map(|target| target.format),
            },
            meta: meta(),
        })
        .await;
    if let Err(e) = emitted {
        pending.lock().unwrap().remove(&request_id);
        return format!("Error: Capture failed: could not send the request: {e}");
    }

    match tokio::time::timeout(timeout, rx).await {
        Ok(Ok(capture)) if capture.screenshot.is_empty() => {
            let out = match last_used {
                Some(device) => answered_error(&device_label(pool, device).await, capture.dom),
                None => capture.dom,
            };
            match &save {
                Some(target) if !out.starts_with("Error") => format!(
                    "Error: Nothing saved to {}: the capture carried no picture.\n{out}",
                    target.path
                ),
                _ => out,
            }
        }
        Ok(Ok(capture)) => {
            let lead = match &save {
                Some(target) => {
                    match save_capture(artifacts, bus, target, &capture.screenshot, thread_id).await
                    {
                        Ok(line) => line,
                        Err(e) => return format!("Error: {e}"),
                    }
                }
                None => String::new(),
            };
            crate::engine::agentic_loop::format_capture_result(
                &capture.screenshot,
                &lead,
                &capture.dom,
            )
        }
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
