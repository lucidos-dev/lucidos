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
    "text-size",
    "font-size",
    "push_notifications",
    "chat_model",
    "chat_reasoning_effort",
    "image_model",
];

/// Build the per-turn user environment block shared by the chat agent and
/// coding-agent sessions. It is intentionally compact and allowlisted:
/// `preferences` also stores operational values such as VAPID keys.
pub(crate) async fn build_user_device_preferences_context(
    pool: &PgPool,
    device_id: Option<&str>,
    event_device: Option<&str>,
) -> String {
    let device_section = build_device_lines(pool, device_id, event_device).await;
    let preference_lines = build_preference_lines(pool, device_id).await;

    render_user_device_preferences_context(device_section, preference_lines, device_id.is_some())
}

fn render_user_device_preferences_context(
    device_section: Vec<String>,
    preference_lines: Vec<String>,
    has_device_id: bool,
) -> String {
    if device_section.is_empty() && preference_lines.is_empty() {
        return String::new();
    }

    let mut out = String::from("[USER DEVICE & PREFERENCES]\n");
    if !device_section.is_empty() {
        out.push_str("Current request device:\n");
        out.push_str(&device_section.join("\n"));
        out.push('\n');
    } else {
        out.push_str("Current request device: unavailable for this turn.\n");
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
         Apps should respect theme, font, and UI scale, and use rem/em-sized \
         layout where user scale should apply.\n",
    );
    out.push_str("[END USER DEVICE & PREFERENCES]");
    out
}

/// Build context for a coding-agent turn from the persisted event that caused
/// the spawn or follow-up.
pub(crate) async fn build_user_device_preferences_context_for_origin(
    pool: &PgPool,
    event_store: &EventStore,
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
    build_user_device_preferences_context(pool, device_id.as_deref(), event_device.as_deref()).await
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
        DeviceStore::register(
            &pool,
            "device-ios",
            Some("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) Version/18.0 Mobile/15E148 Safari/604.1"),
        )
        .await
        .unwrap();
        DeviceStore::rename(&pool, "device-ios", Some("Ios pwa"))
            .await
            .unwrap();
        PreferenceStore::set(&pool, "theme", "dark").await.unwrap();
        PreferenceStore::set_for_device(&pool, "theme", "light", "device-ios")
            .await
            .unwrap();
        PreferenceStore::set_for_device(&pool, "ui-scale", "125", "device-ios")
            .await
            .unwrap();
        PreferenceStore::set(&pool, "vapid_keys", r#"{"private":"secret"}"#)
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

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[test]
    fn context_block_includes_device_and_effective_user_preferences() {
        let context = render_user_device_preferences_context(
            vec![
                "- id: device-ios".to_string(),
                "- label: Ios pwa".to_string(),
                "- details: Safari/604.1 on iOS".to_string(),
            ],
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
        let context = render_user_device_preferences_context(vec![], vec![], false);

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
