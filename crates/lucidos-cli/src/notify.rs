use clap::ValueEnum;
use serde_json::{json, Value};

use crate::http::{client as http_client, send_and_print};
use crate::workspace::{BoxError, Workspace};

/// Tap kinds accepted by `--tap`. Wire shape matches the server's `Tap`
/// discriminated union — see `crates/lucidos-engine/src/scheduler/notifications.rs`.
/// `Modal` (default), `None` (passive), and `Navigate` (deep-link via the
/// same router `navigate_ui` uses).
///
/// The CLI flag only picks the discriminant; the target/sub-field shape for
/// `Navigate` is implied by the other flags (`--app-id` → navigate-to-app;
/// `--thread-id` → navigate-to-thread).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliTap {
    Modal,
    None,
    Navigate,
}

/// All optional deep-link fields. Each one mirrors the same-named field on the
/// `POST /api/v1/notifications` body and on the `send_notification` LLM tool —
/// adding a field here is a contract change in all three places.
#[derive(Default)]
pub(crate) struct NotifyExtras<'a> {
    pub app_id: Option<&'a str>,
    pub tap: Option<CliTap>,
    pub thread_id: Option<&'a str>,
    pub event_id: Option<&'a str>,
}

/// Build the JSON body the CLI POSTs to `/api/v1/notifications`.
///
/// Mirrors the `send_notification` LLM tool's `app_id` rule for every
/// `Option<&str>`: an empty / whitespace-only string is treated as absent so
/// the deep-link is not stamped (the popover would otherwise try to navigate
/// to an app id or thread id that doesn't exist). `tap` defaults to the
/// server's default (`{"kind":"modal"}`) when omitted — explicit pass-through
/// only.
pub(crate) fn build_request_body(title: &str, message: &str, extras: &NotifyExtras) -> Value {
    let mut obj = serde_json::Map::new();
    obj.insert("title".into(), Value::String(title.to_string()));
    obj.insert("message".into(), Value::String(message.to_string()));
    insert_trimmed(&mut obj, "app_id", extras.app_id);
    if let Some(tap) = extras.tap {
        obj.insert("tap".into(), build_tap_value(tap, extras));
    }
    insert_trimmed(&mut obj, "thread_id", extras.thread_id);
    insert_trimmed(&mut obj, "event_id", extras.event_id);
    Value::Object(obj)
}

/// Build the structured `Tap` JSON the server's `Tap` enum decodes from.
/// `Modal` / `None` are kind-only objects. `Navigate` infers the target
/// from the other extras: presence of `--app-id` → `target=app`, presence
/// of `--thread-id` → `target=thread` (carrying `event_id` if set). The
/// caller has already validated that at least one of those is present
/// when `--tap navigate` was passed (see `cmd_notify`).
fn build_tap_value(tap: CliTap, extras: &NotifyExtras) -> Value {
    match tap {
        CliTap::Modal => json!({"kind": "modal"}),
        CliTap::None => json!({"kind": "none"}),
        CliTap::Navigate => {
            // Order: thread-id wins when both are present (it's the more
            // common CTA shape — "answer this question"). If both are wanted
            // the LLM tool gives full structured access; the CLI is the
            // simple-script surface and picks one.
            if let Some(tid) = trim_nonempty(extras.thread_id) {
                let mut to = serde_json::Map::new();
                to.insert("target".into(), Value::String("thread".into()));
                to.insert("id".into(), Value::String(tid.to_string()));
                if let Some(eid) = trim_nonempty(extras.event_id) {
                    to.insert("event_id".into(), Value::String(eid.to_string()));
                }
                json!({"kind": "navigate", "to": Value::Object(to)})
            } else if let Some(aid) = trim_nonempty(extras.app_id) {
                json!({
                    "kind": "navigate",
                    "to": {"target": "app", "app_id": aid}
                })
            } else {
                // cmd_notify rejects this combo before we get here; fall back
                // to modal so a future caller can't accidentally produce an
                // invalid Navigate (missing `to`) — the server would 400 it.
                json!({"kind": "modal"})
            }
        }
    }
}

/// Trim and return None for absent / empty / whitespace-only inputs — the
/// uniform "treat empty as absent" rule for every optional string field.
fn trim_nonempty(raw: Option<&str>) -> Option<&str> {
    raw.map(str::trim).filter(|s| !s.is_empty())
}

fn insert_trimmed(obj: &mut serde_json::Map<String, Value>, key: &str, raw: Option<&str>) {
    if let Some(s) = trim_nonempty(raw) {
        obj.insert(key.into(), Value::String(s.to_string()));
    }
}

pub(crate) fn cmd_notify(
    ws: &Workspace,
    title: &str,
    message: &str,
    extras: NotifyExtras,
) -> Result<(), BoxError> {
    if title.trim().is_empty() {
        return Err("--title must not be empty".into());
    }
    if message.trim().is_empty() {
        return Err("--message must not be empty".into());
    }
    // Fail fast on the same shape the server rejects with 400 — saner error
    // surface for script authors than waiting for the HTTP round-trip.
    if matches!(extras.tap, Some(CliTap::Navigate))
        && trim_nonempty(extras.thread_id).is_none()
        && trim_nonempty(extras.app_id).is_none()
    {
        return Err(
            "--tap navigate requires --thread-id (deep-link to a thread) or --app-id (deep-link to an app)"
                .into(),
        );
    }

    let url = format!("{}/api/v1/notifications", ws.base_url());
    let body = build_request_body(title, message, &extras);
    send_and_print("POST", &url, http_client()?.post(&url).json(&body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn body_includes_title_and_message() {
        let body = build_request_body("hello", "world", &NotifyExtras::default());
        assert_eq!(body["title"], json!("hello"));
        assert_eq!(body["message"], json!("world"));
    }

    #[test]
    fn body_omits_all_extras_when_absent() {
        let body = build_request_body("t", "m", &NotifyExtras::default());
        for key in ["app_id", "tap", "thread_id", "event_id"] {
            assert!(body.get(key).is_none(), "expected {key} absent: {body}");
        }
    }

    #[test]
    fn body_includes_app_id_when_present() {
        let body = build_request_body(
            "t",
            "m",
            &NotifyExtras {
                app_id: Some("habit-tracker"),
                ..Default::default()
            },
        );
        assert_eq!(body["app_id"], json!("habit-tracker"));
    }

    #[test]
    fn body_includes_navigate_thread_with_event_id() {
        // The full deep-link shape used by `When agent needs me` — engine
        // looks at `tap.to.target = thread` + `to.id` + `to.event_id` to drive
        // scroll-and-pulse.
        let body = build_request_body(
            "Claude is asking",
            "Ship it?",
            &NotifyExtras {
                tap: Some(CliTap::Navigate),
                thread_id: Some("00000000-0000-0000-0000-000000000001"),
                event_id: Some("00000000-0000-0000-0000-000000000002"),
                ..Default::default()
            },
        );
        assert_eq!(body["tap"]["kind"], json!("navigate"));
        assert_eq!(body["tap"]["to"]["target"], json!("thread"));
        assert_eq!(
            body["tap"]["to"]["id"],
            json!("00000000-0000-0000-0000-000000000001")
        );
        assert_eq!(
            body["tap"]["to"]["event_id"],
            json!("00000000-0000-0000-0000-000000000002")
        );
        // thread_id / event_id also flow through as top-level fields — the
        // engine uses them to stamp the persisted notification row and the
        // modal's "Open thread" button irrespective of tap kind.
        assert_eq!(
            body["thread_id"],
            json!("00000000-0000-0000-0000-000000000001")
        );
        assert_eq!(
            body["event_id"],
            json!("00000000-0000-0000-0000-000000000002")
        );
    }

    #[test]
    fn body_includes_navigate_app_when_only_app_id() {
        let body = build_request_body(
            "Time to log",
            "Daily check-in",
            &NotifyExtras {
                tap: Some(CliTap::Navigate),
                app_id: Some("habit-tracker"),
                ..Default::default()
            },
        );
        assert_eq!(body["tap"]["kind"], json!("navigate"));
        assert_eq!(body["tap"]["to"]["target"], json!("app"));
        assert_eq!(body["tap"]["to"]["app_id"], json!("habit-tracker"));
    }

    #[test]
    fn body_navigate_prefers_thread_when_both_present() {
        // The CLI picks one when both are passed — thread wins (more common
        // CTA shape). Callers needing full control should use the LLM tool.
        let body = build_request_body(
            "x",
            "y",
            &NotifyExtras {
                tap: Some(CliTap::Navigate),
                thread_id: Some("00000000-0000-0000-0000-000000000010"),
                app_id: Some("habit-tracker"),
                ..Default::default()
            },
        );
        assert_eq!(body["tap"]["to"]["target"], json!("thread"));
    }

    /// Wire shapes each `CliTap` produces are the contract the server's
    /// `Tap` enum receives — pin them so a rename can't drift silently.
    #[test]
    fn cli_tap_wire_shape_pins_to_server_contract() {
        let modal = build_tap_value(CliTap::Modal, &NotifyExtras::default());
        assert_eq!(modal, json!({"kind": "modal"}));
        let none = build_tap_value(CliTap::None, &NotifyExtras::default());
        assert_eq!(none, json!({"kind": "none"}));
    }

    /// `--tap none` needs neither --app-id nor --thread-id (passive variant).
    /// The body just carries `"tap":{"kind":"none"}` through to the server.
    #[test]
    fn body_includes_tap_none_without_app_or_thread() {
        let body = build_request_body(
            "FYI",
            "Backup complete",
            &NotifyExtras {
                tap: Some(CliTap::None),
                ..Default::default()
            },
        );
        assert_eq!(body["tap"], json!({"kind": "none"}));
        assert!(body.get("app_id").is_none());
        assert!(body.get("thread_id").is_none());
    }

    /// Match `execute_send_notification`'s rule: empty string = absent.
    /// Otherwise the popover deep-links to an app id that doesn't exist.
    /// Applies uniformly across every optional string field.
    #[test]
    fn body_treats_empty_and_whitespace_as_absent_for_every_extra() {
        let body = build_request_body(
            "t",
            "m",
            &NotifyExtras {
                app_id: Some(""),
                tap: None,
                thread_id: Some(""),
                event_id: Some("\t  "),
            },
        );
        for key in ["app_id", "tap", "thread_id", "event_id"] {
            assert!(body.get(key).is_none(), "expected {key} absent: {body}");
        }
    }
}
