//! The single chokepoint for a preference write's SIDE EFFECTS.
//!
//! Both entry paths — the `set_preference` LLM tool and the HTTP
//! `PUT /api/v1/preferences` handler (which the Settings UI uses) — funnel
//! through [`LucidosEngine::apply_preference_write`], so a preference's
//! side-effects can never diverge by who made the write:
//! - `language` / `timezone` refresh the engine's in-memory `user_language` /
//!   `user_timezone` and emit `LanguageSet` / `TimezoneSet` (the frontend
//!   live-applies on those), so the Settings → Locale controls take
//!   effect without an engine restart.
//! - `push_notifications` syncs `devices.push_enabled`.
//!
//! The persisted `PreferencesChanged` is NOT emitted here. It belongs to
//! `PreferenceStore`'s write path, so a writer that bypasses this chokepoint
//! (the scheduler's backup schedule and the HTTP retention handler both do,
//! deliberately) still announces. That is the fix for the bug where each of
//! them hand-rolled the emit at its own call site.
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
        // Cloned up front: the store write below consumes the actor for the
        // `PreferencesChanged` it emits, and the push side-effect still needs
        // it for the paired `DevicePushChanged`.
        let side_effect_actor = actor.clone();
        // Side-effect declared in the catalog (plain/unknown key → None).
        let side_effect = preference_catalog::lookup(key)
            .map(|s| s.side_effect)
            .unwrap_or(PrefSideEffect::None);

        refuse_bad_value(key, value, side_effect)?;

        // Persist. Scope follows the caller's device_id, matching the historical
        // HTTP behavior (frontend sends device_id for device-scoped keys, omits it
        // for global ones).
        // The store announces `PreferencesChanged` from inside the write, so
        // this chokepoint no longer emits it: its remaining job is the
        // catalog-declared SIDE EFFECTS below (in-memory caches, the legacy
        // per-key events, the devices.push_enabled mirror).
        let write = if let Some(did) = device_id {
            PreferenceStore::set_for_device(&self.pool, &self.event_bus, key, value, did, actor)
                .await
        } else {
            PreferenceStore::set(&self.pool, &self.event_bus, key, value, actor).await
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
                    if let Err(e) = DeviceStore::set_push_enabled(
                        &self.pool,
                        &self.event_bus,
                        did,
                        enabled,
                        side_effect_actor,
                    )
                    .await
                    {
                        log!("[Preferences] Failed to set devices.push_enabled: {}", e);
                    }
                }
            }
            PrefSideEffect::None => {}
        }

        Ok(PreferenceWriteOutcome { push_enabled })
    }
}

/// The value checks every writer must pass, run BEFORE the store write so a
/// refusal leaves the saved value exactly as it was.
fn refuse_bad_value(key: &str, value: &str, side_effect: PrefSideEffect) -> Result<(), String> {
    // A timezone write updates in-memory `user_timezone` and is loaded back at
    // startup. A malformed IANA name would silently degrade trigger scheduling
    // to UTC (`scheduler::task_runner` falls back). The tool pre-validates too,
    // but a raw HTTP/SDK caller reaches only this check.
    if side_effect == PrefSideEffect::Timezone && value.parse::<chrono_tz::Tz>().is_err() {
        return Err(format!(
            "Invalid timezone '{}'. Use an IANA name like 'Europe/Oslo' or 'America/New_York'.",
            value
        ));
    }

    // The *style library* is one JSON document, and a bad one would cost
    // the user every style they wrote. So it is checked here rather than in
    // the tool handler: the Settings UI reaches only this path, and it is
    // the writer that edits the document. Refused whole, never trimmed.
    if key == crate::core::PREF_RESPONSE_STYLES {
        crate::core::response_style::validate_document(value)?;
    }

    // Any app can write the style remote, so part tokens in it pass the
    // part grammar here, where every writer arrives (ADR 0307).
    if key == crate::core::themes::STYLE_OVERRIDES_KEY {
        crate::core::themes::validate_style_overrides(value)?;
    }

    // Keys whose catalog check every writer must pass, here where
    // `PUT /api/v1/preferences` also arrives, not only in the tool:
    // - the proxy reads its timeout on every call and fails loudly on a bad
    //   value, so a bad write would break every proxied call;
    // - `theme` named the light/dark mode before the rename, so an older
    //   app still sends `theme=dark`, refused with the key it meant.
    if key == crate::core::PREF_PROXY_TIMEOUT_SECS || key == crate::core::themes::THEME_KEY {
        if let Some(spec) = preference_catalog::lookup(key) {
            preference_catalog::validate(spec, value)?;
        }
    }

    // Settings writes the local model host through this path, and so does any
    // caller that can present itself as the shell (ADR 0156 decision 1). An
    // empty value clears it, and every reader then falls back to env or default.
    if key == crate::core::PREF_LOCAL_BASE_URL && !value.trim().is_empty() {
        if let Some(reason) = crate::core::preferences::local_base_url_rejection(value) {
            return Err(reason);
        }
    }

    // A `backup_schedule` write re-registers the backup cron via the
    // scheduler's `PreferencesChanged` subscriber. A bad expression is refused
    // here, on the path every writer shares, rather than failing to register
    // later. `"off"` and other inactive values skip cron parsing.
    if key == crate::core::backup::PREF_BACKUP_SCHEDULE
        && crate::core::backup::is_schedule_active(value)
    {
        crate::engine::tools::scheduler::parse_standard_cron(value)
            .map_err(|e| format!("Invalid backup schedule cron '{}': {}", value, e))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The write path Settings uses refuses a public model host, whoever sends
    /// it, so the stored value stays as it was.
    #[test]
    fn a_public_local_base_url_is_refused_on_the_shared_write_path() {
        let err = refuse_bad_value(
            crate::core::PREF_LOCAL_BASE_URL,
            "https://attacker.example/v1",
            PrefSideEffect::None,
        )
        .expect_err("a public host must not become the local model host");
        assert!(err.contains("attacker.example"), "{err}");
    }

    /// Settings saving a real local setup still works, and so does clearing the
    /// field to fall back to `LUCIDOS_LOCAL_BASE_URL` or the default.
    #[test]
    fn a_loopback_or_lan_local_base_url_passes_the_shared_write_path() {
        for url in [
            crate::core::DEFAULT_LOCAL_BASE_URL,
            "http://127.0.0.1:1234/v1",
            "http://192.168.1.20:11434/v1",
            "",
            "  ",
        ] {
            refuse_bad_value(crate::core::PREF_LOCAL_BASE_URL, url, PrefSideEffect::None)
                .unwrap_or_else(|e| panic!("{url}: {e}"));
        }
    }
}
