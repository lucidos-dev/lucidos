use super::super::CognosEngine;
use crate::core::PreferenceStore;
use crate::engine::event_bus::{BusEvent, SystemEvent};

impl CognosEngine {
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
                        log!("Warning: failed to set devices.push_enabled: {}", e);
                    }
                }

                if enabled {
                    // Return marker — the SSE processing loop will detect this and send the event
                    Ok("[PUSH_NOTIFICATION_REQUEST][ACTION COMPLETED] Push notifications enabled. The browser will now ask for permission to show notifications.".to_string())
                } else {
                    Ok("[ACTION COMPLETED] Push notifications declined. The user won't be asked again.".to_string())
                }
            }
            _ => Ok(format!("Unknown preferences tool: {}", name)),
        }
    }
}
