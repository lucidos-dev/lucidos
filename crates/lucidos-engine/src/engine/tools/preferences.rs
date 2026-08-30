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

/// What `get_backup_status` knows about the selected provider's account.
///
/// Distinguishing all six states is the point: the `backup_provider` preference
/// only names a destination, it connects nothing, so a provider that is set is
/// not a provider that works. Collapsing "no account" into the same silence as
/// "connected" is what let an agent tell a user their Dropbox backups were
/// configured when every upload would have failed (2026-08-05).
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProviderAccount {
    /// No `backup_provider` preference set.
    Unset,
    /// The preference names a provider this engine doesn't have.
    UnknownProvider,
    /// The lookup itself failed. Genuinely unknown, never reported as a "no".
    LookupFailed(String),
    /// No OAuth account exists for this provider.
    NotConnected,
    /// Connected, but the grant is short one or more of the scopes the provider
    /// needs. Carries WHICH, so the agent can name them exactly as the Backup
    /// page does: the two read the same `provider_readiness` verdict and must
    /// not be able to describe it differently.
    MissingScope(Vec<&'static str>),
    /// Connected with everything the provider needs.
    Ready,
}

/// The `Provider:` line of `get_backup_status`. Pure so every branch is
/// unit-testable without a pool. Always names Settings → Accounts (never the
/// Backup page) as where an account is connected.
fn backup_provider_line(provider: &str, account: &ProviderAccount) -> String {
    match account {
        ProviderAccount::Unset => "Provider: (none set. Scheduled backups cannot upload \
             until one is set and its account is connected.)\n"
            .to_string(),
        ProviderAccount::UnknownProvider => format!(
            "Provider: {provider} (NOT a known provider id. Valid ids: {}.)\n",
            crate::core::backup::PROVIDER_IDS.join(", ")
        ),
        ProviderAccount::LookupFailed(e) => format!(
            "Provider: {provider} (could not check whether the account is connected: {e}. \
             Do not tell the user backups are working or broken; say the check failed.)\n"
        ),
        ProviderAccount::NotConnected => format!(
            "Provider: {provider} (NOT CONNECTED. Backups will run and the upload will FAIL. \
             Connect the account with connect_oauth_account, or send the user to \
             Settings → Accounts. It cannot be connected on the Backup page. Do not report \
             backup setup as complete until this says connected.)\n"
        ),
        ProviderAccount::MissingScope(missing) => format!(
            "Provider: {provider} (account connected but MISSING the access backups need: {}. \
             Re-run connect_oauth_account with the backup scopes, or use 'Grant access' in \
             Settings → System → Backup. Uploads will fail until then.)\n",
            missing.join(", ")
        ),
        ProviderAccount::Ready => format!("Provider: {provider} (account connected)\n"),
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
        // Display only, so the default is the right answer on an unreadable
        // row; the prune caller in `scheduler::backup` skips instead.
        let retention = backup::get_retention_count(pool)
            .await
            .unwrap_or(backup::DEFAULT_BACKUP_RETENTION);
        let tz: chrono_tz::Tz = self.user_timezone().await.parse().unwrap_or(chrono_tz::UTC);

        // Resolve the provider's account state before rendering. Reported
        // whatever the schedule says: a manual "Back up now" needs a connected
        // account too, so hiding this behind an active schedule would answer
        // "is my backup set up?" with only half the truth.
        let provider_name = provider.as_deref().map(str::trim).filter(|p| !p.is_empty());
        let account = match provider_name {
            None => ProviderAccount::Unset,
            Some(p) => match backup::provider_meta(p) {
                None => ProviderAccount::UnknownProvider,
                Some(meta) => match backup::provider_readiness(pool, &meta).await {
                    Err(e) => ProviderAccount::LookupFailed(e.to_string()),
                    Ok(r) if r.ready() => ProviderAccount::Ready,
                    Ok(r) if r.connected => ProviderAccount::MissingScope(r.missing_scopes),
                    Ok(_) => ProviderAccount::NotConnected,
                },
            },
        };

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
            }
            _ => out.push_str("Schedule: off (automatic backups disabled)\n"),
        }
        out.push_str(&backup_provider_line(
            provider_name.unwrap_or_default(),
            &account,
        ));
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
             (keys: backup_schedule, backup_provider, backup_retention). Setting \
             backup_provider only picks a destination, it does NOT connect the account: \
             that is connect_oauth_account, or the user in Settings → Accounts. Restore is \
             done from the workspace picker, not from here.",
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
            // An empty stored value is named, never rendered as `(unset)` or
            // as a blank. It states the STATE, which is true of every key: the
            // row exists and holds nothing. What that state MEANS is per-key,
            // and the description is where each key says so. It matters for
            // `voice_resident_sections`, where an empty row means no sections
            // and an absent one means the three defaults.
            let current = match effective.get(spec.key).map(String::as_str) {
                None => "(unset)",
                Some("") => "(empty)",
                Some(v) => v,
            };
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

#[cfg(test)]
mod backup_status_tests {
    use super::{backup_provider_line, ProviderAccount};

    /// The regression this whole line exists for: a provider preference that is
    /// SET is not a provider that WORKS. `backup_provider` only names a
    /// destination, so the agent must be told, in the same breath, that nothing
    /// is connected behind it. The 2026-08-05 session read `Provider: dropbox`
    /// and told the user backups were configured.
    #[test]
    fn not_connected_is_stated_loudly_and_names_accounts() {
        let line = backup_provider_line("dropbox", &ProviderAccount::NotConnected);
        assert!(line.contains("dropbox"));
        assert!(line.contains("NOT CONNECTED"), "{line}");
        assert!(line.contains("FAIL"), "must say uploads fail: {line}");
        assert!(
            line.contains("Settings → Accounts"),
            "must name the page that connects an account: {line}"
        );
        assert!(
            !line.contains("Settings → System → Backup"),
            "must never send the user to the Backup page to connect: {line}"
        );
    }

    #[test]
    fn ready_reports_the_account_as_connected() {
        let line = backup_provider_line("google_drive", &ProviderAccount::Ready);
        assert!(line.contains("google_drive"));
        assert!(line.contains("connected"), "{line}");
        assert!(
            !line.contains("NOT CONNECTED"),
            "a ready provider must not read as broken: {line}"
        );
    }

    /// Connected-but-underscoped is its own state: telling the user to connect
    /// an account they already have would send them in a circle. It also names
    /// WHICH scopes are short, so the agent relays the same specific gap the
    /// Backup page shows rather than a vaguer version of it.
    #[test]
    fn missing_scope_is_distinct_from_not_connected_and_names_the_scopes() {
        let line = backup_provider_line(
            "dropbox",
            &ProviderAccount::MissingScope(vec!["files.content.read", "files.metadata.read"]),
        );
        assert!(line.contains("MISSING"), "{line}");
        assert!(line.contains("files.content.read"), "{line}");
        assert!(line.contains("files.metadata.read"), "{line}");
        assert!(
            !line.contains("NOT CONNECTED"),
            "the account exists; only its scopes are short: {line}"
        );
    }

    #[test]
    fn unset_provider_says_so_without_naming_an_empty_provider() {
        let line = backup_provider_line("", &ProviderAccount::Unset);
        assert!(line.contains("none set"), "{line}");
        assert!(!line.contains("Provider:  "), "no empty name hole: {line}");
    }

    /// A failed lookup is UNKNOWN, never a "no" (CLAUDE.md's no-silent-defaults
    /// rule, same reasoning as the schedule/provider preference reads above).
    #[test]
    fn lookup_failure_refuses_to_report_either_verdict() {
        let line = backup_provider_line(
            "dropbox",
            &ProviderAccount::LookupFailed("connection reset".to_string()),
        );
        assert!(line.contains("connection reset"), "{line}");
        assert!(line.contains("the check failed"), "{line}");
        assert!(
            !line.contains("NOT CONNECTED"),
            "an unreachable DB must not read as a missing account: {line}"
        );
    }

    /// A typo'd provider id must name the valid ids rather than looking like a
    /// connection problem.
    #[test]
    fn unknown_provider_lists_the_valid_ids() {
        let line = backup_provider_line("dropbx", &ProviderAccount::UnknownProvider);
        assert!(line.contains("dropbx"), "{line}");
        for id in crate::core::backup::PROVIDER_IDS {
            assert!(line.contains(id), "must list {id}: {line}");
        }
    }
}
