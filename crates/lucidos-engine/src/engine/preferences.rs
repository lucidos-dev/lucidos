use super::LucidosEngine;
use crate::core::prefs::{self, PrefSideEffect};
use crate::core::{DeviceStore, PreferenceStore};
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

pub struct PreferenceWriteOutcome {
    pub push_enabled: Option<bool>,
}

impl LucidosEngine {
    pub async fn apply_preference_write(
        &self,
        key: &str,
        value: &str,
        device_id: Option<&str>,
        actor: Option<MessageOrigin>,
    ) -> Result<PreferenceWriteOutcome, String> {
        let side_effect_actor = actor.clone();
        let side_effect = prefs::lookup(key)
            .map(|s| s.side_effect)
            .unwrap_or(PrefSideEffect::None);

        refuse_bad_value(key, value, side_effect)?;
        if key == prefs::WEBHOOK_PUBLIC_URL.key() && device_id.is_some() {
            return Err("The public webhook URL applies to the whole workspace.".into());
        }
        let _landing_gate = if key == prefs::LANDING_SETTING.key() {
            prefs::validate(&prefs::LANDING_SETTING.spec, value)?;
            if device_id.is_some() {
                return Err("The landing setting applies to the whole workspace.".into());
            }
            if value == "another-host" {
                crate::engine::git_ops::probe_origin_landing_queue(self.repo_root()).await?;
            }
            Some(self.landing_setting_gate.lock().await)
        } else {
            None
        };

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
                        log_legacy!("[Preferences] Failed to set devices.push_enabled: {}", e);
                    }
                }
            }
            PrefSideEffect::None => {}
        }

        Ok(PreferenceWriteOutcome { push_enabled })
    }

    pub async fn apply_preference_reset(
        &self,
        key: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, String> {
        let _landing_gate = if key == prefs::LANDING_SETTING.key() {
            Some(self.landing_setting_gate.lock().await)
        } else {
            None
        };
        let removed = PreferenceStore::delete(&self.pool, &self.event_bus, key, actor)
            .await
            .map_err(|e| format!("Failed to delete preference '{key}': {e}"))?;
        if !removed {
            return Ok(false);
        }
        match prefs::lookup(key).map(|spec| spec.side_effect) {
            Some(PrefSideEffect::Language) => {
                self.user_language.write().await.clear();
                self.event_bus
                    .emit(BusEvent::System(SystemEvent::LanguageSet {
                        language: String::new(),
                    }))
                    .await
                    .map_err(|e| format!("Failed to emit LanguageSet: {e}"))?;
            }
            Some(PrefSideEffect::Timezone) => {
                self.user_timezone.write().await.clear();
                self.event_bus
                    .emit(BusEvent::System(SystemEvent::TimezoneSet {
                        timezone: String::new(),
                    }))
                    .await
                    .map_err(|e| format!("Failed to emit TimezoneSet: {e}"))?;
            }
            Some(PrefSideEffect::Push | PrefSideEffect::None) | None => {}
        }
        Ok(true)
    }
}

/// INVARIANT: The value checks every writer must pass, run BEFORE the store write so a
/// refusal leaves the saved value exactly as it was.
fn refuse_bad_value(key: &str, value: &str, side_effect: PrefSideEffect) -> Result<(), String> {
    if let Some(reason) = prefs::retired_key_reason(key) {
        return Err(reason.to_string());
    }

    if let Some(refusal) = prefs::retired_value_refusal(key, value) {
        return Err(refusal);
    }

    // INVARIANT: A timezone write updates in-memory `user_timezone` and is loaded back at
    // startup. A malformed IANA name would silently degrade trigger scheduling
    // to UTC (`scheduler::task_runner` falls back). The tool pre-validates too,
    // but a raw HTTP/SDK caller reaches only this check.
    if side_effect == PrefSideEffect::Timezone && value.parse::<chrono_tz::Tz>().is_err() {
        return Err(format!(
            "Invalid timezone '{}'. Use an IANA name like 'Europe/Oslo' or 'America/New_York'.",
            value
        ));
    }

    // INVARIANT: The *style library* is one JSON document, and a bad one would cost
    // the user every style they wrote. So it is checked here rather than in
    // the tool handler: the Settings UI reaches only this path, and it is
    // the writer that edits the document. Refused whole, never trimmed.
    if key == prefs::RESPONSE_STYLES.key() {
        crate::core::response_style::validate_document(value)?;
    }
    if key == prefs::THREAD_FOLDERS.key() {
        crate::core::thread_folders::validate_document(value)?;
    }

    if key == crate::core::themes::STYLE_OVERRIDES_KEY {
        crate::core::themes::validate_style_overrides(value)?;
    }

    // INVARIANT: Keys whose catalog check every writer must pass, here where
    // `PUT /api/v1/preferences` also arrives, not only in the tool:
    // - the proxy reads its timeout on every call and fails loudly on a bad
    //   value, so a bad write would break every proxied call;
    // - `theme` named the light/dark mode before the rename, so an older
    //   app still sends `theme=dark`, refused with the key it meant;
    // - the ingress probe needs a public webhook URL it can parse.
    if [
        prefs::PROXY_TIMEOUT_SECS.key(),
        prefs::THEME.key(),
        prefs::WEBHOOK_PUBLIC_URL.key(),
    ]
    .contains(&key)
    {
        if let Some(spec) = prefs::lookup(key) {
            prefs::validate(spec, value)?;
        }
    }

    if key == prefs::LOCAL_BASE_URL.key() && !value.trim().is_empty() {
        if let Some(reason) = crate::core::preferences::local_base_url_rejection(value) {
            return Err(reason);
        }
    }

    if key == prefs::CLAUDE_GATEWAY_URL.key() && !value.trim().is_empty() {
        crate::llm::claude_gateway::GatewayOrigin::parse(value)?;
    }

    if key == prefs::BACKUP_SCHEDULE.key() && crate::core::backup::is_schedule_active(value) {
        crate::scheduler::cron_rules::parse_standard_cron(value)
            .map_err(|e| format!("Invalid backup schedule cron '{}': {}", value, e))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{boot_mock_engine, setup_test_db, teardown_test_db};

    #[tokio::test]
    async fn a_reset_clears_the_cached_locale_and_announces_it() {
        let (pool, db_name) = setup_test_db().await;
        pool.close().await;
        let (engine, _workspace) = boot_mock_engine(&db_name).await;
        let mut events = engine.event_bus.subscribe();
        for (key, value) in [
            (prefs::TIMEZONE.key(), "Europe/Oslo"),
            (prefs::LANGUAGE.key(), "Norwegian"),
        ] {
            engine
                .apply_preference_write(key, value, None, None)
                .await
                .unwrap();
        }
        assert_eq!(engine.user_timezone().await, "Europe/Oslo");

        assert!(engine
            .apply_preference_reset(prefs::TIMEZONE.key(), None)
            .await
            .unwrap());
        assert!(engine
            .apply_preference_reset(prefs::LANGUAGE.key(), None)
            .await
            .unwrap());

        assert_eq!(engine.user_timezone().await, "");
        assert_eq!(*engine.user_language.read().await, "");
        let mut cleared = Vec::new();
        while let Ok(event) = events.try_recv() {
            match event.typed {
                BusEvent::System(SystemEvent::TimezoneSet { timezone }) => cleared.push(timezone),
                BusEvent::System(SystemEvent::LanguageSet { language }) => cleared.push(language),
                _ => {}
            }
        }
        assert_eq!(cleared, ["Europe/Oslo", "Norwegian", "", ""]);

        assert!(!engine
            .apply_preference_reset(prefs::TIMEZONE.key(), None)
            .await
            .unwrap());
        drop(engine);
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_landing_setting_reset_waits_for_the_landing_gate() {
        let (pool, db_name) = setup_test_db().await;
        pool.close().await;
        let (engine, _workspace) = boot_mock_engine(&db_name).await;
        let key = prefs::LANDING_SETTING.key();
        PreferenceStore::set(&engine.pool, &engine.event_bus, key, "another-host", None)
            .await
            .unwrap();

        let gate = engine.landing_setting_gate.lock().await;
        let reset = tokio::spawn({
            let engine = engine.clone();
            async move { engine.apply_preference_reset(key, None).await }
        });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(
            !reset.is_finished(),
            "the reset ran past a held landing gate"
        );
        let stored = PreferenceStore::get_all(&engine.pool).await.unwrap();
        assert_eq!(stored.get(key).map(String::as_str), Some("another-host"));
        drop(gate);
        assert!(reset.await.unwrap().unwrap());
        drop(engine);
        teardown_test_db(&db_name).await;
    }

    #[test]
    fn a_retired_model_id_is_refused_on_the_shared_write_path() {
        for (key, retired, replacement) in prefs::RETIRED_VALUES {
            let err = refuse_bad_value(key, retired, PrefSideEffect::None)
                .expect_err("a retired model id must not be stored");
            assert!(err.contains(replacement), "{key}: {err}");
        }
    }

    #[test]
    fn a_retired_key_is_refused_on_the_shared_write_path() {
        for (key, reason) in prefs::RETIRED_KEYS {
            for value in ["true", "false"] {
                let err = refuse_bad_value(key, value, PrefSideEffect::None)
                    .expect_err("a retired key must not be stored");
                assert_eq!(err, *reason);
            }
        }
    }

    #[test]
    fn a_public_local_base_url_is_refused_on_the_shared_write_path() {
        let err = refuse_bad_value(
            prefs::LOCAL_BASE_URL.key(),
            "https://attacker.example/v1",
            PrefSideEffect::None,
        )
        .expect_err("a public host must not become the local model host");
        assert!(err.contains("attacker.example"), "{err}");
    }

    #[test]
    fn the_claude_gateway_url_must_be_an_https_origin() {
        let key = prefs::CLAUDE_GATEWAY_URL.key();
        for url in [
            "http://gateway.example",
            "https://gateway.example/v1",
            "https://api.anthropic.com",
            "gateway.example",
        ] {
            let err =
                refuse_bad_value(key, url, PrefSideEffect::None).expect_err("not a gateway origin");
            assert!(err.contains("Claude gateway URL"), "{url}: {err}");
        }
        for url in ["https://gateway.example", "http://127.0.0.1:4100", ""] {
            refuse_bad_value(key, url, PrefSideEffect::None)
                .unwrap_or_else(|e| panic!("{url}: {e}"));
        }
    }

    #[test]
    fn a_loopback_or_lan_local_base_url_passes_the_shared_write_path() {
        for url in [
            prefs::LOCAL_BASE_URL.default_text(),
            "http://127.0.0.1:1234/v1",
            "http://192.168.1.20:11434/v1",
            "",
            "  ",
        ] {
            refuse_bad_value(prefs::LOCAL_BASE_URL.key(), url, PrefSideEffect::None)
                .unwrap_or_else(|e| panic!("{url}: {e}"));
        }
    }
}
