use super::super::LucidosEngine;
use crate::core::preference_catalog::{self, PrefScope};
use crate::core::PreferenceStore;
use crate::llm::tool_names as tn;

/// Platform-aware copy the LLM relays after enabling push for a device. The
/// `[PUSH_NOTIFICATION_REQUEST]` marker MUST stay first — the SSE processing loop
/// (agentic_loop/run.rs) keys off it to emit the thread event that drives the
/// frontend `initPushSubscription()` handshake.
const PUSH_ENABLED_REPLY: &str = "[PUSH_NOTIFICATION_REQUEST][ACTION COMPLETED] Push notifications enabled for this device. Tell the user what to expect, keyed off the current request device in [USER DEVICE & PREFERENCES]: if its details say \"Lucidos desktop app\" they are in the native desktop app — notifications arrive as native macOS notifications governed by System Settings → Notifications (macOS asks for permission on first launch; if no banner appears, allow Lucidos there); do NOT mention browser permission or site settings. Otherwise they are in a web browser or installed PWA — the browser will now ask for notification permission and they should click Allow. Note: in a development build (tauri-dev) native desktop banners don't appear at all — run Lucidos in a browser/PWA to receive notifications while developing. Either way, they'll get notifications for triggered tasks and alerts.";

/// Format a duration suffix like " (3m 12s)" from an optional start + an end
/// time, or "" when the start time is unknown (legacy run records). Used by the
/// `get_backup_status` surface.
fn backup_duration_suffix(
    started_at: Option<chrono::DateTime<chrono::Utc>>,
    finished_at: chrono::DateTime<chrono::Utc>,
) -> String {
    match started_at {
        Some(s) => {
            let secs = (finished_at - s).num_seconds().max(0);
            format!(" ({}m {}s)", secs / 60, secs % 60)
        }
        None => String::new(),
    }
}

/// Human-readable byte size (GB / MB / B) for the `get_backup_status` surface.
fn human_size(bytes: Option<u64>) -> String {
    match bytes {
        Some(b) if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        Some(b) if b >= 1 << 20 => format!("{:.0} MB", b as f64 / (1u64 << 20) as f64),
        Some(b) => format!("{b} B"),
        None => "?".to_string(),
    }
}

impl LucidosEngine {
    /// Handler for the read-only `get_backup_status` tool. Reports the backup
    /// schedule (in the user's timezone) + computed next run, provider, retention,
    /// the last run with duration, recent run history, and staleness — so the
    /// agent can answer "when's my next/last backup?" without dropping to raw
    /// HTTP. Read-only: it makes no changes. The schedule/provider/retention are
    /// CHANGED via `set_preference`.
    pub(crate) async fn execute_get_backup_status(
        &self,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        use crate::core::backup::{
            self, is_schedule_active, BackupRunStatus, PREF_BACKUP_PROVIDER, PREF_BACKUP_SCHEDULE,
        };

        let pool = &self.pool;
        // A read that FAILED is not "no schedule set". `.ok().flatten()` collapsed
        // both into `None`, and the `_` arm below then reports
        // "Schedule: off (automatic backups disabled)" to the agent, which relays
        // it to the user as fact. Surface the unknown instead of a plausible
        // default, per the no-silent-plausible-defaults rule.
        let cron = match PreferenceStore::get(pool, PREF_BACKUP_SCHEDULE).await {
            Ok(v) => v,
            Err(e) => {
                return Ok(format!(
                    "Error: could not read the backup schedule preference: {}. Backup status is unknown; do not tell the user backups are off.",
                    e
                ))
            }
        };
        let provider = match PreferenceStore::get(pool, PREF_BACKUP_PROVIDER).await {
            Ok(v) => v,
            Err(e) => {
                return Ok(format!(
                    "Error: could not read the backup provider preference: {}. Backup status is unknown; do not tell the user backups are off.",
                    e
                ))
            }
        };
        let retention = backup::get_retention_count(pool).await;
        let tz: chrono_tz::Tz = self.user_timezone().await.parse().unwrap_or(chrono_tz::UTC);

        let mut out = String::new();

        match cron.as_deref() {
            Some(c) if is_schedule_active(c) => {
                out.push_str(&format!("Schedule: {} (timezone {})\n", c, tz));
                if let Ok(schedule) = crate::engine::tools::scheduler::parse_standard_cron(c) {
                    let now = chrono::Utc::now().with_timezone(&tz);
                    if let Some(next) = schedule.after(&now).next() {
                        out.push_str(&format!("Next run: {}\n", next.format("%Y-%m-%d %H:%M %Z")));
                    }
                }
                match provider.as_deref() {
                    Some(p) if !p.is_empty() => out.push_str(&format!("Provider: {}\n", p)),
                    _ => out.push_str(
                        "Provider: (none set — scheduled backups cannot upload until one is set and connected)\n",
                    ),
                }
            }
            _ => out.push_str("Schedule: off (automatic backups disabled)\n"),
        }
        out.push_str(&format!("Retention: keep {} most recent\n", retention));

        match backup::load_last_run(pool).await {
            Some(run) => {
                let when = run.at.with_timezone(&tz).format("%Y-%m-%d %H:%M %Z");
                let dur = backup_duration_suffix(run.started_at, run.at);
                match run.status {
                    BackupRunStatus::Success => out.push_str(&format!(
                        "Last run: success at {}{} — {} ({})\n",
                        when,
                        dur,
                        run.filename.as_deref().unwrap_or("?"),
                        human_size(run.size_bytes),
                    )),
                    BackupRunStatus::Failure => out.push_str(&format!(
                        "Last run: FAILED at {}{} — {}\n",
                        when,
                        dur,
                        run.error.as_deref().unwrap_or("unknown error"),
                    )),
                }
            }
            None => out.push_str("Last run: none recorded yet\n"),
        }

        let history = backup::load_recent_runs(pool, 10).await;
        if !history.is_empty() {
            out.push_str("\nRecent runs (newest first):\n");
            for r in &history {
                let when = r.finished_at.with_timezone(&tz).format("%Y-%m-%d %H:%M");
                let dur = backup_duration_suffix(r.started_at, r.finished_at);
                match r.status {
                    BackupRunStatus::Success => out.push_str(&format!(
                        "  ok   {}{} {}\n",
                        when,
                        dur,
                        human_size(r.size_bytes),
                    )),
                    BackupRunStatus::Failure => out.push_str(&format!(
                        "  fail {}{} — {}\n",
                        when,
                        dur,
                        r.error.as_deref().unwrap_or("error"),
                    )),
                }
            }
        }

        out.push_str(
            "\nChange the schedule/provider/retention with set_preference \
             (keys: backup_schedule, backup_provider, backup_retention). Restore is done \
             from the workspace picker, not from here.",
        );
        Ok(out)
    }

    /// Dispatch for the unified preference tools. `set_preference` / `get_preferences`
    /// replace the former set_language / set_timezone / enable_push_notifications
    /// trio — the side-effects those carried (in-memory locale, LanguageSet /
    /// TimezoneSet, the push handshake + devices.push_enabled sync) now live in the
    /// shared write chokepoint (`engine/preferences.rs`) keyed off the preference
    /// catalog.
    pub(crate) async fn execute_preferences_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
        device_id: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match name {
            tn::SET_PREFERENCE => self.execute_set_preference(args, device_id).await,
            tn::GET_PREFERENCES => self.execute_get_preferences(device_id).await,
            _ => Ok(format!("Unknown preferences tool: {}", name)),
        }
    }

    async fn execute_set_preference(
        &self,
        args: &serde_json::Value,
        device_id: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let key = args["key"].as_str().unwrap_or("").trim();
        let value = args["value"].as_str().unwrap_or("");
        if key.is_empty() {
            return Ok(
                "Error: key is required. Call get_preferences to see settable keys.".to_string(),
            );
        }

        // Catalog gate: only agent-settable keys, with validated values. Internal
        // keys (command_guard, backup config, keybindings, …) are rejected here —
        // the agent must not disable its own command guard.
        let spec = match preference_catalog::lookup(key) {
            Some(s) => s,
            None => {
                return Ok(match preference_catalog::internal_hint(key) {
                    Some(hint) => format!("Error: '{}' can't be changed with set_preference — {}.", key, hint),
                    None => format!(
                        "Error: unknown preference '{}'. Call get_preferences to see the settable keys.",
                        key
                    ),
                });
            }
        };
        if let Err(e) = preference_catalog::validate(spec, value) {
            return Ok(format!("Error: {}", e));
        }

        // Device-scope is decided by the catalog — the agent never passes a device
        // id. A device-scoped key needs the caller's device; without one (e.g. a
        // trigger context) the write can't be attributed to a device.
        let scoped_device = match spec.scope {
            PrefScope::Device => match device_id {
                Some(did) => Some(did),
                None => {
                    return Ok(format!(
                        "Error: '{}' is a per-device setting, but this context has no device. Ask the user to change it from the device itself.",
                        key
                    ));
                }
            },
            PrefScope::Global => None,
        };

        let outcome = match self
            .apply_preference_write(key, value, scoped_device, None)
            .await
        {
            Ok(o) => o,
            Err(e) => return Ok(format!("Error: {}", e)),
        };

        // Push has bespoke, platform-aware copy + a frontend handshake.
        match outcome.push_enabled {
            Some(true) => return Ok(PUSH_ENABLED_REPLY.to_string()),
            Some(false) => {
                return Ok("[ACTION COMPLETED] Push notifications declined for this device. The user won't be asked again.".to_string());
            }
            None => {}
        }

        let scope_note = match spec.scope {
            PrefScope::Device => " for this device",
            PrefScope::Global => "",
        };
        // `chat_model` / `chat_reasoning_effort` are the DEFAULT for NEW Lucidos
        // Agent threads only — a thread that's already running reuses its own
        // last-used model/effort, independent of this preference (per-thread model
        // memory; see `PreferenceStore::resolve_chat_overrides_for_thread`). So the
        // generic "open views pick this up" note would be misleading here: it must
        // NOT imply this preference switches the current/running thread on its next
        // turn. (The in-thread picker CAN change a running thread — that path writes
        // a per-thread value, not this account default.)
        let effect_note = match key {
            "chat_model" => " This is the default for NEW Lucidos Agent threads. A thread that's already running — including this one — keeps its current model (whatever it last used), so this preference change does NOT switch the current thread's model on its next turn. To change a running thread's model, use its in-thread model picker.",
            "chat_reasoning_effort" => " This is the default for NEW Lucidos Agent threads. A thread that's already running — including this one — keeps its current reasoning effort (whatever it last used), so this preference change does NOT change the current thread's effort on its next turn. To change a running thread's effort, use its in-thread picker.",
            _ => " Open Lucidos views pick this up automatically.",
        };
        Ok(format!(
            "[ACTION COMPLETED] {} set to '{}'{}.{}",
            spec.label, value, scope_note, effect_note
        ))
    }

    async fn execute_get_preferences(
        &self,
        device_id: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        // Read both the global map and the device-effective map so the agent can
        // see when a per-device override is shadowing the global value (the
        // device-scope trap that cost ~30 tool calls in a real thread).
        let global = PreferenceStore::get_all(&self.pool).await?;
        let effective = match device_id {
            Some(did) => PreferenceStore::get_all_for_device(&self.pool, did).await?,
            None => global.clone(),
        };

        let mut out = String::from(
            "Settable preferences — set with set_preference(key, value). Device-scoped keys apply to the calling device only.\n",
        );
        for spec in preference_catalog::CATALOG {
            let current = effective
                .get(spec.key)
                .map(String::as_str)
                .unwrap_or("(unset)");
            let scope = match spec.scope {
                PrefScope::Device => "device",
                PrefScope::Global => "global",
            };
            out.push_str(&format!(
                "- {} [{}] = {} | allowed: {} | default: {}\n",
                spec.key,
                scope,
                current,
                preference_catalog::allowed_values_hint(spec),
                spec.default,
            ));
            // Make a shadowing device override explicit.
            if spec.scope == PrefScope::Device {
                let glob = global.get(spec.key).map(String::as_str);
                let eff = effective.get(spec.key).map(String::as_str);
                if let (Some(g), Some(e)) = (glob, eff) {
                    if g != e {
                        out.push_str(&format!(
                            "    ↳ this device overrides the global value '{}'\n",
                            g
                        ));
                    }
                }
            }
        }

        // Known internal / managed-elsewhere keys that exist in the store — shown
        // read-only so the agent can explain them but knows not to set them.
        let internal: Vec<String> = preference_catalog::INTERNAL_KEYS
            .iter()
            .filter_map(|(k, hint)| {
                effective
                    .get(*k)
                    .map(|v| format!("- {} = {} (read-only: {})", k, v, hint))
            })
            .collect();
        if !internal.is_empty() {
            out.push_str("\nRead-only / managed in Settings (not settable via set_preference):\n");
            for line in internal {
                out.push_str(&line);
                out.push('\n');
            }
        }

        Ok(out)
    }
}
