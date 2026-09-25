//! `navigate_ui`: which device a navigate reaches, and what the agent is told.
//!
//! `GET /api/v1/events` broadcasts `NavigationRequested` to every connected
//! page, and a page drops one whose device actor is not its own
//! (`thread-sync.ts`). So the actor stamped here IS the routing decision. The
//! engine never learns whether a page acted, which is why every result says
//! "sent" and names the device, never "opened".

use super::ToolOutcome;
use crate::core::devices::SeenDevice;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};
use sqlx::PgPool;
use uuid::Uuid;

/// A device's name, the way Settings → Devices shows it.
async fn device_label(pool: &PgPool, device_id: &str) -> String {
    crate::core::DeviceStore::display_name(pool, device_id)
        .await
        .unwrap_or_else(|| crate::core::devices::resolve_device_name(None, device_id))
}

/// A device as an event actor.
pub(crate) async fn device_actor(pool: &PgPool, device_id: &str) -> MessageOrigin {
    MessageOrigin::Device {
        device_id: device_id.to_string(),
        label: device_label(pool, device_id).await,
    }
}

/// Where a navigate went, and why there. The result text is built from this.
enum Recipient {
    /// The agent named the device.
    Chosen(String),
    /// The last used device, the default.
    LastUsed(String),
    /// No device in this turn: every page showing the thread handles it.
    EveryDevice,
}

/// Settle the `device` argument into who receives the navigate.
///
/// An explicit id must be a registered device. An unknown one would be
/// dropped by every page with nobody told, so it is refused here instead.
async fn recipient(
    pool: &PgPool,
    args: &serde_json::Value,
    last_used: Option<&str>,
) -> Result<(Recipient, Option<MessageOrigin>), String> {
    // A blank string is a model filling every optional field, so it reads as
    // omitted rather than costing a retry.
    let chosen = match args.get("device") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(id)) => Some(id.trim()).filter(|id| !id.is_empty()),
        Some(other) => return Err(format!("Error: device must be a device id, got {other}")),
    };
    if let Some(id) = chosen {
        match crate::core::DeviceStore::is_registered(pool, id).await {
            Ok(true) => {}
            Ok(false) => {
                return Err(format!(
                    "Error: device '{id}' is not a known device. Pass an id from the \
                     Known devices list, or omit device to use the last used device."
                ))
            }
            Err(e) => return Err(format!("Error: could not look up device '{id}': {e}")),
        }
    }
    let Some(id) = chosen.or(last_used) else {
        return Ok((Recipient::EveryDevice, None));
    };
    let label = device_label(pool, id).await;
    let actor = MessageOrigin::Device {
        device_id: id.to_string(),
        label: label.clone(),
    };
    let who = if chosen.is_some() {
        Recipient::Chosen(label)
    } else {
        Recipient::LastUsed(label)
    };
    Ok((who, Some(actor)))
}

/// Emit one `NavigationRequested` scoped to `actor`. The one emitter of that
/// event from a tool call. The OAuth authorization page is a persisted
/// `OAuthAuthorizationRequested` instead, since a flow waits on it.
pub(crate) async fn emit_navigation(
    bus: &EventBus,
    payload: &serde_json::Value,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
) -> Result<(), String> {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::NavigationRequested {
            payload: payload.to_string(),
        },
        meta: EventMeta::with_actor(actor),
    })
    .await
    .map(|_| ())
    .map_err(|e| format!("failed to emit NavigationRequested: {e}"))
}

/// The `navigate_ui` tool. `last_used` is the turn's last used device.
pub(crate) async fn navigate_ui_impl(
    bus: &EventBus,
    pool: &PgPool,
    args: &serde_json::Value,
    thread_id: Uuid,
    last_used: Option<&str>,
) -> ToolOutcome {
    let target = match args.get("target").and_then(|v| v.as_str()) {
        Some(t) if !t.is_empty() => t,
        _ => return Err("Error: target is required".to_string()),
    };
    if target == "url"
        && args
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .is_empty()
    {
        return Err("Error: url is required when target is 'url'".to_string());
    }
    // Same guard the notification tap takes, and for the same reason: the
    // page dereferences this id with no way to ask what was meant. `current`
    // resolves to the calling thread, and the resolved value rides the event.
    let mut payload = args.clone();
    crate::api::resolve_thread_id_in_nav_payload(&mut payload, Some(thread_id))
        .map_err(|e| format!("Error: {e}"))?;

    let (who, actor) = recipient(pool, args, last_used).await?;
    let device_id = match &actor {
        Some(MessageOrigin::Device { device_id, .. }) => Some(device_id.clone()),
        _ => None,
    };
    log!("[Navigate] navigate_ui thread={thread_id} target={target} device={device_id:?}");
    emit_navigation(bus, &payload, thread_id, actor)
        .await
        .map_err(|e| format!("Error: {e}"))?;

    let mut out = sent_line(&describe_target(target, &payload), &who);
    if let Some(id) = &device_id {
        out.push_str(&presence_note(
            device_presence(pool, id).await.as_ref(),
            &payload,
        ));
    }
    if target == "url" {
        out.push_str(URL_NOTE);
    }
    if let Some(guide) = page_guide(target, args) {
        out.push_str("\n\n");
        out.push_str(guide);
    }
    Ok(out)
}

/// The first line of every result: what was sent, to whom, and that sent is
/// not opened. The agent relays this, so it must name a place the user can
/// look, and must not promise the screen changed.
fn sent_line(what: &str, who: &Recipient) -> String {
    let to = match who {
        Recipient::Chosen(label) => format!("to \"{label}\", the device you named"),
        Recipient::LastUsed(label) => {
            format!("to \"{label}\", the user's last used device in this turn")
        }
        Recipient::EveryDevice => "to every device showing this thread. This turn has no last \
                                   used device, so no single device was chosen"
            .to_string(),
    };
    format!(
        "Sent a request to open {what} {to}. Sent is not opened: no page confirms it acted. \
         A device showing this thread opens it; any other offers the user an Open button. \
         Tell the user which device you sent it to, never that it is already on their screen."
    )
}

/// The target device as the Known devices list sees it. A failed lookup is
/// unknown, so it yields `None` and the result says nothing about presence.
async fn device_presence(pool: &PgPool, device_id: &str) -> Option<SeenDevice> {
    match crate::core::DeviceStore::seen(pool, device_id).await {
        Ok(seen) => seen,
        Err(e) => {
            log!("[Navigate] presence lookup failed for device {device_id}: {e}");
            None
        }
    }
}

/// Whether anyone is likely there to receive the navigate. Only a live page
/// gets it, and the engine keeps no copy, so an absent device loses it.
/// Absence is a hint, not proof: an iOS PWA can pause its heartbeat.
pub(super) fn presence_note(presence: Option<&SeenDevice>, payload: &serde_json::Value) -> String {
    let Some(seen) = presence else {
        return String::new();
    };
    if seen.visible_now {
        return " Lucidos was visible on it within the last 2 minutes.".to_string();
    }
    let mut to = payload.clone();
    if let Some(args) = to.as_object_mut() {
        args.remove("device");
    }
    let tap = serde_json::json!({"kind": "navigate", "to": to});
    format!(
        " Lucidos is not seen visible on it now (last seen {}). \
         A navigate is not stored or retried: if Lucidos is closed or suspended there, \
         it never arrives. Tell the user so. To reach that device later, offer \
         send_notification with tap {tap}. A notification stays in the inbox on every \
         device, is pushed when no device is active, and opens this target when tapped.",
        crate::engine::agent_context::format_age(seen.seen_secs_ago)
    )
}

/// Where a URL lands is the client's decision (`openUrl`), so the agent must
/// not name one surface. A browser can also refuse it as a blocked popup.
const URL_NOTE: &str = " It opens wherever they configured links to open (the in-app \
     browser panel, their system browser, or a new tab), so call it their browser. A \
     browser can refuse it as a blocked popup, and then the client offers an Open button.";

/// A short name for the destination, for the sent line.
fn describe_target(target: &str, args: &serde_json::Value) -> String {
    let arg = |key: &str| args.get(key).and_then(|v| v.as_str()).unwrap_or("");
    match target {
        "file" => format!("the file {}", arg("file_path")),
        "app" => format!("the app {}", arg("app_id")),
        "thread" => format!("thread {}", arg("id")),
        "trigger" => format!("trigger {}", arg("id")),
        "url" => arg("url").to_string(),
        "settings" if !arg("settings_view").is_empty() => {
            format!("Settings → {}", arg("settings_view"))
        }
        other => format!("the {other} view"),
    }
}

/// What a page offers, for the few pages whose contents the agent keeps
/// getting wrong. Appended after the sent line.
fn page_guide(target: &str, args: &serde_json::Value) -> Option<&'static str> {
    if target != "settings" {
        return None;
    }
    match args.get("settings_view").and_then(|v| v.as_str()) {
        Some("models") => Some(MODELS_SETTINGS_GUIDE),
        Some("backup") => Some(BACKUP_SETTINGS_GUIDE),
        Some("environment-variables") => Some(ENV_VARS_SETTINGS_GUIDE),
        _ => None,
    }
}

const MODELS_SETTINGS_GUIDE: &str = "Settings → Models shows:\n\
     - The active Chat & triggers model (the model picker) and reasoning effort\n\
     - Image generation and background-task models (title, image description, memory)\n\
     - Providers (Anthropic, OpenAI, OpenRouter, xAI, local) and the model registry\n\
     Tell the user they can change the active model from the picker here. To switch \
     it for them instead, use set_preference(key='chat_model'); to add a model to the \
     picker, use manage_models.";

/// A guide describing a screen is a promise about that screen. This one once
/// advertised a cloud-backup list and an in-app Restore button (both gone) and
/// implied the page connects the provider account. Settings → Accounts owns
/// that. `backup_guide_names_the_accounts_page_and_not_a_restore_ui` pins it.
pub(super) const BACKUP_SETTINGS_GUIDE: &str = "Settings → System → Backup shows:\n\
     - A health card: last run outcome, last cloud backup + age, staleness warning\n\
     - The provider dropdown (Google Drive / Dropbox), and a red line linking to \
       Settings → Accounts when the selected provider has no connected account\n\
     - 'Back up now', the schedule dropdown, and the retention dropdown\n\
     - Show/generate the encryption key (required to restore, cannot be recovered)\n\
     This page does NOT connect the provider account, and has no account UI at all: \
     that is Settings → Accounts, and it is the only place to do it. Do not tell the \
     user to connect an account here. Restore is not in the app either, it happens \
     from the workspace picker.";

const ENV_VARS_SETTINGS_GUIDE: &str = "Settings → System → Environment variables shows:\n\
     - The user's environment variables as NAME = value rows\n\
     - Buttons to add, edit, or delete a variable\n\
     These are non-secret values injected into every subprocess Lucidos spawns \
     (run_bash, run_python, scheduled scripts, coding agents), which pick a change up \
     on the next spawn with no restart. The engine loads the same store into its own \
     process environment once at startup, so a variable the engine itself reads \
     changes only after an engine restart. For secrets like API keys, the user should \
     use credentials instead.";

#[cfg(test)]
#[path = "navigate_tests.rs"]
mod tests;
