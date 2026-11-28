use clap::ValueEnum;
use serde_json::{json, Value};

use crate::http::{client as http_client, send_and_print};
use crate::workspace::{BoxError, Workspace};

/// Tap kinds accepted by `--tap`. Wire shape matches the server's `Tap`
/// discriminated union — see `crates/lucidos-engine/src/scheduler/notifications.rs`.
/// `Modal` (default) and `Navigate` (deep-link via the same router `navigate_ui`
/// uses). Every notification is openable — the passive `none` kind was retired
/// (`docs/plans/2026-07-02-remove-notification-tap-none.md`).
///
/// The CLI flag only picks the discriminant; the target/sub-field shape for
/// `Navigate` is implied by the other flags (`--app-id` → navigate-to-app,
/// refined by `--fragment`; `--thread-id` → navigate-to-thread).
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliTap {
    Modal,
    Navigate,
}

#[derive(Default)]
pub(crate) struct NotifyExtras<'a> {
    pub app_id: Option<&'a str>,
    pub tap: Option<CliTap>,
    pub thread_id: Option<&'a str>,
    pub event_id: Option<&'a str>,
    pub fragment: Option<&'a str>,
}

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

fn build_tap_value(tap: CliTap, extras: &NotifyExtras) -> Value {
    match tap {
        CliTap::Modal => json!({"kind": "modal"}),
        CliTap::Navigate => {
            if let Some(tid) = trim_nonempty(extras.thread_id) {
                let mut to = serde_json::Map::new();
                to.insert("target".into(), Value::String("thread".into()));
                to.insert("id".into(), Value::String(tid.to_string()));
                if let Some(eid) = trim_nonempty(extras.event_id) {
                    to.insert("event_id".into(), Value::String(eid.to_string()));
                }
                json!({"kind": "navigate", "to": Value::Object(to)})
            } else if let Some(aid) = trim_nonempty(extras.app_id) {
                let mut to = serde_json::Map::new();
                to.insert("target".into(), Value::String("app".into()));
                to.insert("app_id".into(), Value::String(aid.to_string()));
                if let Some(frag) = trim_nonempty(extras.fragment) {
                    to.insert("fragment".into(), Value::String(frag.to_string()));
                }
                json!({"kind": "navigate", "to": Value::Object(to)})
            } else {
                json!({"kind": "modal"})
            }
        }
    }
}

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
        assert!(body["tap"]["to"].get("fragment").is_none());
    }

    #[test]
    fn body_navigate_app_carries_the_fragment() {
        let body = build_request_body(
            "Time to log",
            "Daily check-in",
            &NotifyExtras {
                tap: Some(CliTap::Navigate),
                app_id: Some("habit-tracker"),
                fragment: Some("day-2026-08-28"),
                ..Default::default()
            },
        );
        assert_eq!(body["tap"]["to"]["target"], json!("app"));
        assert_eq!(body["tap"]["to"]["fragment"], json!("day-2026-08-28"));
    }

    #[test]
    fn body_navigate_thread_ignores_the_fragment() {
        let body = build_request_body(
            "x",
            "y",
            &NotifyExtras {
                tap: Some(CliTap::Navigate),
                thread_id: Some("00000000-0000-0000-0000-000000000010"),
                fragment: Some("day-2026-08-28"),
                ..Default::default()
            },
        );
        assert_eq!(body["tap"]["to"]["target"], json!("thread"));
        assert!(body["tap"]["to"].get("fragment").is_none());
    }

    #[test]
    fn body_navigate_prefers_thread_when_both_present() {
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

    #[test]
    fn cli_tap_wire_shape_pins_to_server_contract() {
        let modal = build_tap_value(CliTap::Modal, &NotifyExtras::default());
        assert_eq!(modal, json!({"kind": "modal"}));
    }

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
                fragment: Some("  "),
            },
        );
        for key in ["app_id", "tap", "thread_id", "event_id"] {
            assert!(body.get(key).is_none(), "expected {key} absent: {body}");
        }

        let navigated = build_request_body(
            "t",
            "m",
            &NotifyExtras {
                app_id: Some("habit-tracker"),
                tap: Some(CliTap::Navigate),
                fragment: Some("  "),
                ..Default::default()
            },
        );
        assert!(navigated["tap"]["to"].get("fragment").is_none());
    }
}
