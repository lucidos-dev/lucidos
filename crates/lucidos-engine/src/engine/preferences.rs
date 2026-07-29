//! The single chokepoint for writing a user preference.
//!
//! Both entry paths — the `set_preference` LLM tool and the HTTP
//! `PUT /api/v1/preferences` handler (which the Settings UI uses) — funnel
//! through [`LucidosEngine::apply_preference_write`], so a preference's
//! side-effects can never diverge by who made the write:
//! - `language` / `timezone` refresh the engine's in-memory `user_language` /
//!   `user_timezone` and emit `LanguageSet` / `TimezoneSet` (the frontend
//!   live-applies on those), so the new Settings → System "Locale" controls take
//!   effect without an engine restart.
//! - `push_notifications` syncs `devices.push_enabled`.
//! - everything else emits the persisted `PreferencesChanged` the frontend
//!   already refetches on.
//!
//! This chokepoint is intentionally **permissive about the key**: the HTTP path
//! legitimately writes internal keys (`command_guard`, `keybindings`,
//! `capture_context`, …) that the human edits in Settings. The catalog *gate*
//! that rejects internal/unknown keys lives in the `set_preference` tool handler
//! (`engine/tools/preferences.rs`), not here — the chokepoint only consults the
//! catalog to decide a key's side-effect.

use super::LucidosEngine;
use crate::core::preference_catalog::{self, PrefSideEffect};
use crate::core::{DeviceStore, PreferenceStore};
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

/// What a preference write resolved to, for callers that need to react.
pub(crate) struct PreferenceWriteOutcome {
    /// `Some(enabled)` when the key was `push_notifications` — the tool layer
    /// uses it to drive the `[PUSH_NOTIFICATION_REQUEST]` permission handshake.
    pub push_enabled: Option<bool>,
}

impl LucidosEngine {
    /// Write a preference, applying any catalog-declared side-effect and emitting
    /// the right event. `device_id = Some` writes a per-device override; `None`
    /// writes the global value (the caller decides scope — the tool resolves it
    /// from the catalog, the HTTP handler from the request body). Does NOT
    /// validate the key against the catalog (see module docs); callers that need
    /// the gate apply it first.
    pub(crate) async fn apply_preference_write(
        &self,
        key: &str,
        value: &str,
        device_id: Option<&str>,
        actor: Option<MessageOrigin>,
    ) -> Result<PreferenceWriteOutcome, String> {
        // Side-effect declared in the catalog (plain/unknown key → None).
        let side_effect = preference_catalog::lookup(key)
            .map(|s| s.side_effect)
            .unwrap_or(PrefSideEffect::None);

        // A timezone write updates in-memory `user_timezone` AND is loaded back
        // at startup, so a malformed IANA name would silently degrade trigger
        // scheduling to UTC (`scheduler::task_runner` falls back). The retired
        // set_timezone tool validated; preserve that guarantee on the shared path
        // (the tool also pre-validates, but a raw HTTP/SDK caller hits only here)
        // — and BEFORE the store write, so a reject can't leave the DB holding a
        // value the in-memory state never adopted.
        if side_effect == PrefSideEffect::Timezone && value.parse::<chrono_tz::Tz>().is_err() {
            return Err(format!(
                "Invalid timezone '{}'. Use an IANA name like 'Europe/Oslo' or 'America/New_York'.",
                value
            ));
        }

        // A `backup_schedule` write re-registers the backup cron via the
        // scheduler's `PreferencesChanged` subscriber. Validate the cron up front
        // on this shared path (so a raw HTTP/SDK caller is covered, not just the
        // catalog-gated `set_preference` tool) and BEFORE the store write — so a
        // bad expression is rejected to the caller instead of silently failing to
        // register later. `"off"` and other inactive values disable the schedule
        // and skip cron parsing.
        if key == crate::core::backup::PREF_BACKUP_SCHEDULE
            && crate::core::backup::is_schedule_active(value)
        {
            crate::engine::tools::scheduler::parse_standard_cron(value)
                .map_err(|e| format!("Invalid backup schedule cron '{}': {}", value, e))?;
        }

        // Persist. Scope follows the caller's device_id, matching the historical
        // HTTP behavior (frontend sends device_id for device-scoped keys, omits it
        // for global ones).
        let write = if let Some(did) = device_id {
            PreferenceStore::set_for_device(&self.pool, key, value, did).await
        } else {
            PreferenceStore::set(&self.pool, key, value).await
        };
        write.map_err(|e| format!("Failed to save preference '{}': {}", key, e))?;

        let mut push_enabled = None;
        match side_effect {
            PrefSideEffect::Language => {
                *self.user_language.write().await = value.to_string();
                self.event_bus
                    .emit(BusEvent::System(SystemEvent::LanguageSet {
                        language: value.to_string(),
                    }))
                    .await
                    .map_err(|e| format!("Failed to emit LanguageSet: {}", e))?;
            }
            PrefSideEffect::Timezone => {
                *self.user_timezone.write().await = value.to_string();
                self.event_bus
                    .emit(BusEvent::System(SystemEvent::TimezoneSet {
                        timezone: value.to_string(),
                    }))
                    .await
                    .map_err(|e| format!("Failed to emit TimezoneSet: {}", e))?;
            }
            PrefSideEffect::Push => {
                let enabled = value == "enabled";
                push_enabled = Some(enabled);
                // Keep the push-filtering query's source-of-truth column in sync.
                // Best-effort: a failure here shouldn't fail the whole write —
                // the preference (the user's intent) is already persisted.
                if let Some(did) = device_id {
                    if let Err(e) = DeviceStore::set_push_enabled(&self.pool, did, enabled).await {
                        log!("[Preferences] Failed to set devices.push_enabled: {}", e);
                    }
                }
                self.emit_preferences_changed(key, value, actor).await?;
            }
            PrefSideEffect::None => {
                self.emit_preferences_changed(key, value, actor).await?;
            }
        }

        Ok(PreferenceWriteOutcome { push_enabled })
    }

    async fn emit_preferences_changed(
        &self,
        key: &str,
        value: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<(), String> {
        self.event_bus
            .emit(BusEvent::System(SystemEvent::PreferencesChanged {
                key: key.to_string(),
                value: Some(value.to_string()),
                actor,
            }))
            .await
            .map(|_| ())
            .map_err(|e| format!("Failed to emit PreferencesChanged: {}", e))
    }
}
