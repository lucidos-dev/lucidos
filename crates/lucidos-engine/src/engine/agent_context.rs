use crate::core::devices::SeenDevice;
use crate::core::{DeviceStore, EventRow, EventStore, PreferenceStore};
use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

const SAFE_PREFERENCE_KEYS: &[&str] = &[
    "timezone",
    "language",
    "theme",
    "font-family",
    "ui-scale",
    "motion",
    "text-size",
    "font-size",
    "push_notifications",
    "chat_model",
    "chat_reasoning_effort",
    "image_model",
];

/// Build the user environment block shared by the chat agent and coding-agent
/// sessions, for an already resolved device. It is intentionally compact and
/// allowlisted: `preferences` also stores operational values such as VAPID
/// keys. Callers go through [`build_user_device_preferences_context_for_turn`].
async fn build_user_device_preferences_context(
    pool: &PgPool,
    device_id: Option<&str>,
    event_device: Option<&str>,
) -> String {
    let device_section = build_device_lines(pool, device_id, event_device).await;
    let known_device_lines = build_known_device_lines(pool).await;
    let preference_lines = build_preference_lines(pool, device_id).await;

    render_user_device_preferences_context(
        device_section,
        known_device_lines,
        preference_lines,
        device_id.is_some(),
    )
}

/// The events a user acts through inside a turn. Each carries the device it
/// came from, as an `actor`, an `origin`, or a bare `device_id`.
const USER_ACTION_EVENT_TYPES: &[&str] = &[
    "MessageReceived",
    "UserPromptInjected",
    "UserQuestionAnswered",
];

/// The *last used device*: the device of the user's newest action in the
/// current turn, from `turn_anchor` (the event that started it) on.
///
/// A question answered from the laptop after the turn started on the phone
/// makes the laptop the last used device. Nothing before the anchor counts.
/// With no anchor, or a failed lookup, the turn's own device stands.
/// `navigate_ui` and the context block both read it here, so they agree.
pub(crate) async fn last_used_device(
    pool: &PgPool,
    thread_id: Uuid,
    turn_anchor: Option<Uuid>,
    turn_device: Option<&str>,
) -> Option<String> {
    let fallback = turn_device.map(str::to_string);
    let Some(anchor) = turn_anchor else {
        return fallback;
    };
    let newest = sqlx::query_scalar::<_, String>(
        "SELECT device FROM ( \
             SELECT sequence, COALESCE( \
                 CASE WHEN payload->'actor'->>'kind' = 'device' \
                      THEN payload->'actor'->>'device_id' END, \
                 CASE WHEN payload->'origin'->>'kind' = 'device' \
                      THEN payload->'origin'->>'device_id' END, \
                 NULLIF(payload->>'device_id', '')) AS device \
             FROM events \
             WHERE thread_id = $1 \
               AND event_type = ANY($3) \
               AND sequence >= (SELECT sequence FROM events WHERE id = $2 AND thread_id = $1) \
         ) turn WHERE device IS NOT NULL \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .bind(anchor)
    .bind(USER_ACTION_EVENT_TYPES)
    .fetch_optional(pool)
    .await;
    match newest {
        Ok(found) => found.or(fallback),
        Err(e) => {
            log!("[AgentContext] last used device lookup failed for thread {thread_id}: {e}");
            fallback
        }
    }
}

/// The block for one turn, about its last used device.
///
/// `turn_device` and `turn_device_label` are what the turn's own event says.
/// The label only describes that device, so it is dropped when a later action
/// moved the last used device elsewhere.
pub(crate) async fn build_user_device_preferences_context_for_turn(
    pool: &PgPool,
    thread_id: Uuid,
    turn_anchor: Option<Uuid>,
    turn_device: Option<&str>,
    turn_device_label: Option<&str>,
) -> String {
    let device = last_used_device(pool, thread_id, turn_anchor, turn_device).await;
    let label = turn_device_label.filter(|_| device.as_deref() == turn_device);
    build_user_device_preferences_context(pool, device.as_deref(), label).await
}

/// At most this many devices are listed, most recently seen first.
const KNOWN_DEVICES_LIMIT: i64 = 5;
/// A device unseen for longer than this is left out of the list.
const KNOWN_DEVICES_WITHIN_DAYS: i32 = 30;

/// `device_section` is the last used device when the block is built. A later
/// answer in the same live turn can move it, so the header says when.
/// `navigate_ui` resolves it again at call time.
fn render_user_device_preferences_context(
    device_section: Vec<String>,
    known_device_lines: Vec<String>,
    preference_lines: Vec<String>,
    has_device_id: bool,
) -> String {
    if device_section.is_empty() && known_device_lines.is_empty() && preference_lines.is_empty() {
        return String::new();
    }

    let mut out = String::from("[USER DEVICE & PREFERENCES]\n");
    if !device_section.is_empty() {
        out.push_str("Last used device (when this context was built):\n");
        out.push_str(&device_section.join("\n"));
        out.push('\n');
    } else {
        out.push_str("Last used device: unavailable for this turn.\n");
    }
    if !known_device_lines.is_empty() {
        out.push_str("Known devices, most recently seen first:\n");
        out.push_str(&known_device_lines.join("\n"));
        out.push('\n');
    }

    if !preference_lines.is_empty() {
        if has_device_id {
            out.push_str(
                "Effective preferences for this device (global plus device-specific overrides):\n",
            );
        } else {
            out.push_str("Global user-facing preferences:\n");
        }
        out.push_str(&preference_lines.join("\n"));
        out.push('\n');
    }

    out.push_str(
        "Use these facts when interpreting user-facing UI/UX requests. \
         Apps should respect theme, font, UI scale and motion (key animations on \
         `data-motion`), and use rem/em-sized layout where user scale should apply.\n",
    );
    out.push_str("[END USER DEVICE & PREFERENCES]");
    out
}

/// Build context for a coding-agent input from the persisted event that caused
/// it. That event anchors the turn.
pub(crate) async fn build_user_device_preferences_context_for_origin(
    pool: &PgPool,
    event_store: &EventStore,
    thread_id: Uuid,
    origin_id: Uuid,
) -> String {
    let (device_id, event_device) = match event_store.get_event_by_id(origin_id).await {
        Ok(Some(row)) => device_from_event_row(&row),
        Ok(None) => (None, None),
        Err(e) => {
            log!(
                "[AgentContext] Failed to load origin event {} for device/preferences context: {}",
                origin_id,
                e
            );
            (None, None)
        }
    };
    build_user_device_preferences_context_for_turn(
        pool,
        thread_id,
        Some(origin_id),
        device_id.as_deref(),
        event_device.as_deref(),
    )
    .await
}

pub(crate) fn prepend_user_device_preferences_context(context: &str, text: &str) -> String {
    if context.trim().is_empty() {
        return text.to_string();
    }
    if text.trim().is_empty() {
        return context.to_string();
    }
    format!("{context}\n\n{text}")
}

fn device_from_event_row(row: &EventRow) -> (Option<String>, Option<String>) {
    if row.event_type != "MessageReceived" && row.event_type != "UserPromptInjected" {
        return (None, None);
    }
    let device_id = row
        .payload
        .get("device_id")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            row.payload
                .get("origin")
                .and_then(|origin| origin.get("device_id"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
    let event_device = row
        .payload
        .get("device")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            row.payload
                .get("origin")
                .and_then(|origin| origin.get("label"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
    (device_id, event_device)
}

async fn build_device_lines(
    pool: &PgPool,
    device_id: Option<&str>,
    event_device: Option<&str>,
) -> Vec<String> {
    let Some(device_id) = device_id else {
        return event_device
            .map(split_device_label_details)
            .map(|(label, details)| {
                let mut lines = vec![format!("- label: {label}")];
                if let Some(details) = details {
                    lines.push(format!("- details: {details}"));
                }
                lines
            })
            .unwrap_or_default();
    };

    let tooltip = DeviceStore::tooltip_info(pool, device_id).await;
    let raw = tooltip.as_deref().or(event_device).unwrap_or(device_id);
    let (label, details) = split_device_label_details(raw);
    let mut lines = vec![format!("- id: {device_id}"), format!("- label: {label}")];
    if let Some(details) = details {
        lines.push(format!("- details: {details}"));
    }
    lines
}

/// A failed read leaves the list out rather than failing the turn. The last
/// used device above still stands, and `navigate_ui` defaults to it.
async fn build_known_device_lines(pool: &PgPool) -> Vec<String> {
    match DeviceStore::recently_seen(pool, KNOWN_DEVICES_LIMIT, KNOWN_DEVICES_WITHIN_DAYS).await {
        Ok(devices) => devices.iter().map(format_known_device).collect(),
        Err(e) => {
            log!(
                "[AgentContext] Failed to list known devices for agent context: {}",
                e
            );
            Vec::new()
        }
    }
}

fn format_known_device(device: &SeenDevice) -> String {
    let mut facts: Vec<String> = device.details.iter().cloned().collect();
    facts.push(format!("seen {}", format_age(device.seen_secs_ago)));
    if device.visible_now {
        facts.push("Lucidos visible now".to_string());
    }
    format!(
        "- {} (id {}): {}",
        device.label,
        device.id,
        facts.join("; ")
    )
}

pub(crate) fn format_age(secs: i64) -> String {
    match secs {
        s if s < 60 => "just now".to_string(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86_400 => format!("{} h ago", s / 3600),
        s if s < 2 * 86_400 => "1 day ago".to_string(),
        s => format!("{} days ago", s / 86_400),
    }
}

fn split_device_label_details(raw: &str) -> (String, Option<String>) {
    let mut lines = raw.lines().map(str::trim).filter(|line| !line.is_empty());
    let label = lines.next().unwrap_or("unknown").to_string();
    let details = lines.collect::<Vec<_>>().join("; ");
    let details = if details.is_empty() {
        None
    } else {
        Some(details)
    };
    (label, details)
}

async fn build_preference_lines(pool: &PgPool, device_id: Option<&str>) -> Vec<String> {
    let prefs = match device_id {
        Some(id) => PreferenceStore::get_all_for_device(pool, id).await,
        None => PreferenceStore::get_all(pool).await,
    };
    let prefs = match prefs {
        Ok(prefs) => prefs,
        Err(e) => {
            log!(
                "[AgentContext] Failed to load user-facing preferences for agent context: {}",
                e
            );
            HashMap::new()
        }
    };

    format_preference_lines(&prefs)
}

fn format_preference_lines(prefs: &HashMap<String, String>) -> Vec<String> {
    SAFE_PREFERENCE_KEYS
        .iter()
        .filter_map(|key| {
            prefs
                .get(*key)
                .filter(|value| !value.trim().is_empty())
                .map(|value| format!("- {key}: {value}"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    #[tokio::test]
    async fn context_loads_registered_device_and_effective_user_preferences() {
        let (pool, db_name) = setup_test_db().await;
        crate::test_support::seed_device(
            &pool,
            "device-ios",
            Some("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) Version/18.0 Mobile/15E148 Safari/604.1"),
            Some("Ios pwa"),
        )
        .await;
        crate::test_support::seed_preference(&pool, "theme", "dark")
            .await
            .unwrap();
        crate::test_support::seed_preference_for_device(&pool, "theme", "light", "device-ios")
            .await
            .unwrap();
        crate::test_support::seed_preference_for_device(&pool, "ui-scale", "125", "device-ios")
            .await
            .unwrap();
        // Written through the guarded silent door, matching production: a
        // transport secret is not a setting, so it must not announce.
        PreferenceStore::set_silent(&pool, "vapid_keys", r#"{"private":"secret"}"#)
            .await
            .unwrap();

        let context = build_user_device_preferences_context(&pool, Some("device-ios"), None).await;

        assert!(context.contains("- id: device-ios"));
        assert!(context.contains("- label: Ios pwa"));
        assert!(context.contains("- theme: light"));
        assert!(context.contains("- ui-scale: 125"));
        assert!(context.contains("Safari") || context.contains("iOS"));
        assert!(!context.contains("vapid"));
        assert!(!context.contains("secret"));
        assert!(
            context.contains("- Ios pwa (id device-ios): Safari/604.1 on iOS; seen just now"),
            "the known devices list names the device: {context}"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// A coding agent lives across many user messages. Each one the user sent
    /// gets a block built when it is forwarded, so the device and the ages are
    /// current. Engine-made inputs carry none: no user acted.
    #[test]
    fn only_a_user_input_with_an_origin_gets_a_fresh_block() {
        use crate::engine::{AgentInputKind, AgentUserInput};
        let origin = Uuid::new_v4();
        let input = |origin_event_id, kind| AgentUserInput {
            text: "go on".into(),
            images: None,
            origin_event_id,
            kind,
        };
        assert_eq!(
            input(Some(origin), AgentInputKind::User).user_origin(),
            Some(origin)
        );
        assert_eq!(
            input(None, AgentInputKind::User).user_origin(),
            None,
            "auto-harden and apply prompts have no user behind them"
        );
        assert_eq!(
            input(Some(origin), AgentInputKind::ReentryFromEngine).user_origin(),
            None,
            "a finished child thread is not a user action"
        );
    }

    /// The agent picks a device on purpose from this list. So each row carries
    /// the id to pass, the label to tell the user, and how fresh it is.
    #[test]
    fn a_known_device_line_carries_id_label_details_and_freshness() {
        let laptop = SeenDevice {
            id: "laptop".into(),
            label: "My MacBook".into(),
            details: Some("Chrome/149.0.0.0 on macOS".into()),
            seen_secs_ago: 5,
            visible_now: true,
        };
        assert_eq!(
            format_known_device(&laptop),
            "- My MacBook (id laptop): Chrome/149.0.0.0 on macOS; seen just now; Lucidos visible now"
        );
        let phone = SeenDevice {
            id: "phone".into(),
            label: "device-phone".into(),
            details: None,
            seen_secs_ago: 2 * 3600 + 5,
            visible_now: false,
        };
        assert_eq!(
            format_known_device(&phone),
            "- device-phone (id phone): seen 2 h ago"
        );
    }

    #[test]
    fn ages_read_in_the_largest_whole_unit() {
        assert_eq!(format_age(59), "just now");
        assert_eq!(format_age(60), "1 min ago");
        assert_eq!(format_age(3599), "59 min ago");
        assert_eq!(format_age(86_399), "23 h ago");
        assert_eq!(format_age(86_400), "1 day ago");
        assert_eq!(format_age(3 * 86_400), "3 days ago");
    }

    /// "Request device" hid the incident: the user answered from the laptop,
    /// and the turn still counted the phone. The block names the concept the
    /// navigate actually uses.
    #[test]
    fn the_block_names_the_last_used_device_and_lists_known_devices() {
        let context = render_user_device_preferences_context(
            vec!["- id: device-ios".to_string()],
            vec!["- My MacBook (id laptop): seen just now".to_string()],
            vec![],
            true,
        );
        assert!(
            context.contains("Last used device (when this context was built):\n- id: device-ios"),
            "{context}"
        );
        assert!(context.contains("Known devices"), "{context}");
        assert!(context.contains("- My MacBook (id laptop)"), "{context}");
        assert!(!context.contains("request device"), "{context}");
    }

    /// A trigger turn has no device and may have no preferences. It still needs
    /// the list, or it cannot aim `navigate_ui` at any device on purpose.
    #[test]
    fn a_turn_with_no_device_still_lists_known_devices() {
        let context = render_user_device_preferences_context(
            vec![],
            vec!["- My MacBook (id laptop): seen just now".to_string()],
            vec![],
            false,
        );
        assert!(
            context.contains("Last used device: unavailable"),
            "{context}"
        );
        assert!(context.contains("- My MacBook (id laptop)"), "{context}");
    }

    #[test]
    fn context_block_includes_device_and_effective_user_preferences() {
        let context = render_user_device_preferences_context(
            vec![
                "- id: device-ios".to_string(),
                "- label: Ios pwa".to_string(),
                "- details: Safari/604.1 on iOS".to_string(),
            ],
            vec![],
            vec!["- theme: light".to_string(), "- ui-scale: 125".to_string()],
            true,
        );

        assert!(context.contains("[USER DEVICE & PREFERENCES]"));
        assert!(context.contains("- id: device-ios"));
        assert!(context.contains("- label: Ios pwa"));
        assert!(context.contains("- theme: light"));
        assert!(context.contains("- ui-scale: 125"));
        assert!(context.contains("Safari"));
        assert!(context.contains("Effective preferences for this device"));
    }

    #[test]
    fn preference_lines_filter_non_allowlisted_preferences() {
        let prefs = HashMap::from([
            ("theme".to_string(), "light".to_string()),
            ("ui-scale".to_string(), "125".to_string()),
            ("language".to_string(), "".to_string()),
            (
                "vapid_keys".to_string(),
                r#"{"private":"secret"}"#.to_string(),
            ),
        ]);

        let lines = format_preference_lines(&prefs);
        let context = lines.join("\n");

        assert!(context.contains("- theme: light"));
        assert!(context.contains("- ui-scale: 125"));
        assert!(!context.contains("language"));
        assert!(!context.contains("vapid"));
        assert!(!context.contains("secret"));
    }

    #[test]
    fn context_is_empty_when_no_device_or_safe_preferences_exist() {
        let context = render_user_device_preferences_context(vec![], vec![], vec![], false);

        assert_eq!(context, "");
    }

    #[test]
    fn split_device_label_details_uses_first_line_as_label() {
        let (label, details) = split_device_label_details("Ios pwa\nSafari/604.1 on iOS");

        assert_eq!(label, "Ios pwa");
        assert_eq!(details.as_deref(), Some("Safari/604.1 on iOS"));
    }

    #[test]
    fn device_from_event_row_reads_injected_prompt_origin() {
        let row = EventRow::new(
            "UserPromptInjected",
            serde_json::json!({
                "origin": {
                    "device_id": "device-ios",
                    "label": "Ios pwa"
                }
            }),
        );

        let (device_id, event_device) = device_from_event_row(&row);

        assert_eq!(device_id.as_deref(), Some("device-ios"));
        assert_eq!(event_device.as_deref(), Some("Ios pwa"));
    }

    #[test]
    fn prefix_helper_adds_context_before_user_text() {
        let text = prepend_user_device_preferences_context(
            "[USER DEVICE & PREFERENCES]\n- theme: light\n[END USER DEVICE & PREFERENCES]",
            "Build the app.",
        );

        assert!(text.starts_with("[USER DEVICE & PREFERENCES]"));
        assert!(text.ends_with("Build the app."));
    }
}
