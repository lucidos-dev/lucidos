use super::super::LucidosEngine;
use crate::core::environment_variables::validate_name;
use crate::core::{EnvironmentVariableStore, PreferenceStore};
use crate::engine::event_bus::{BusEvent, SystemEvent};

impl LucidosEngine {
    /// Handler for the `set_environment_variable` tool — the LLM-suggestable path
    /// to define a user environment variable (Settings → System → Environment
    /// variables). Validates the name (shape + not engine-reserved), upserts, and
    /// emits `EnvironmentVariableSet` (value carried — these are non-secret).
    /// Takes effect on the next spawned subprocess; no restart.
    pub(crate) async fn execute_environment_variable_tool(
        &self,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let name = args["name"].as_str().unwrap_or("").trim();
        let value = args["value"].as_str().unwrap_or("");

        if name.is_empty() {
            return Ok("Error: name is required".to_string());
        }
        if let Err(rejection) = validate_name(name) {
            return Ok(format!("Error: {}", rejection.message(name)));
        }

        if let Err(e) = EnvironmentVariableStore::upsert(&self.pool, name, value).await {
            return Ok(format!("Error: Failed to save environment variable: {}", e));
        }

        self.event_bus
            .emit(BusEvent::System(SystemEvent::EnvironmentVariableSet {
                name: name.to_string(),
                value: value.to_string(),
                actor: None,
            }))
            .await?;

        Ok(format!(
            "[ACTION COMPLETED] Environment variable '{}' set. It is injected into newly \
             spawned subprocesses (run_bash, run_python, scheduled scripts, coding agents) — \
             no restart needed. Note these are NOT secret; for API keys/tokens use a credential.",
            name
        ))
    }

    pub(crate) async fn execute_preferences_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
        device_id: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match name {
            "set_language" => {
                let language = args["language"].as_str().unwrap_or("");

                if language.is_empty() {
                    return Ok("Error: language is required".to_string());
                }

                // Save to database
                if let Err(e) = PreferenceStore::set(&self.pool, "language", language).await {
                    return Ok(format!("Error: Failed to save language: {}", e));
                }

                // Update the engine's language
                *self.user_language.write().await = language.to_string();

                // Emit event
                self.event_bus
                    .emit(BusEvent::System(SystemEvent::LanguageSet {
                        language: language.to_string(),
                    }))
                    .await?;

                Ok(format!(
                    "[ACTION COMPLETED] Language set to {}. Responses and session summaries will now be in {}.",
                    language, language
                ))
            }
            "set_timezone" => {
                let timezone = args["timezone"].as_str().unwrap_or("");

                if timezone.is_empty() {
                    return Ok("Error: timezone is required".to_string());
                }

                // Validate the timezone
                let tz_result: Result<chrono_tz::Tz, _> = timezone.parse();
                if tz_result.is_err() {
                    return Ok(format!(
                        "Error: Invalid timezone '{}'. Use IANA timezone names like 'America/New_York', 'Europe/London', 'Asia/Tokyo'.",
                        timezone
                    ));
                }

                // Save to database
                if let Err(e) = PreferenceStore::set(&self.pool, "timezone", timezone).await {
                    return Ok(format!("Error: Failed to save timezone: {}", e));
                }

                // Update the engine's timezone
                *self.user_timezone.write().await = timezone.to_string();

                // Emit event
                self.event_bus
                    .emit(BusEvent::System(SystemEvent::TimezoneSet {
                        timezone: timezone.to_string(),
                    }))
                    .await?;

                Ok(format!(
                    "[ACTION COMPLETED] Timezone set to {}. All triggers will now use this timezone for time conversions.",
                    timezone
                ))
            }
            "enable_push_notifications" => {
                let enabled = args["enabled"].as_bool().unwrap_or(true);
                let value = if enabled { "enabled" } else { "declined" };

                // Store per-device if device_id is available, otherwise global
                let pref_result = if let Some(did) = device_id {
                    PreferenceStore::set_for_device(&self.pool, "push_notifications", value, did)
                        .await
                } else {
                    PreferenceStore::set(&self.pool, "push_notifications", value).await
                };
                if let Err(e) = pref_result {
                    return Ok(format!("Error: Failed to save preference: {}", e));
                }

                // Also set devices.push_enabled to keep push filtering query working
                if let Some(did) = device_id {
                    if let Err(e) =
                        crate::core::DeviceStore::set_push_enabled(&self.pool, did, enabled).await
                    {
                        log!("[Preferences] Failed to set devices.push_enabled: {}", e);
                    }
                }

                if enabled {
                    // Return marker — the SSE processing loop (agentic_loop/run.rs) keys off
                    // the `[PUSH_NOTIFICATION_REQUEST]` prefix to emit the thread event that
                    // drives the frontend `initPushSubscription()` handshake, so it MUST stay
                    // first. The rest is platform-aware copy for the LLM to relay: the browser
                    // permission prompt only exists on web / PWA. The desktop (Tauri) app has no
                    // such prompt — its WKWebView can't subscribe to Web Push, so it renders
                    // native macOS notifications (UNUserNotificationCenter) governed by System
                    // Settings, which only work in a packaged build; native banners are inert in
                    // a tauri-dev build, so dev uses the browser/PWA web-push path instead. See
                    // system-knowhow/notifications.md §4 "Enabling".
                    Ok("[PUSH_NOTIFICATION_REQUEST][ACTION COMPLETED] Push notifications enabled for this device. Tell the user what to expect based on how they run Lucidos: in a web browser or installed PWA, the browser will now ask for notification permission — they should click Allow; in the packaged desktop app there is no browser or in-app permission prompt — notifications arrive as native macOS notifications governed by System Settings → Notifications (macOS asks for permission on first launch; if no banner appears, allow Lucidos there). Note: in a development build (tauri-dev) native desktop banners don't appear at all — run Lucidos in a browser/PWA to receive notifications while developing. Either way, they'll get notifications for triggered tasks and alerts.".to_string())
                } else {
                    Ok("[ACTION COMPLETED] Push notifications declined. The user won't be asked again.".to_string())
                }
            }
            _ => Ok(format!("Unknown preferences tool: {}", name)),
        }
    }
}
