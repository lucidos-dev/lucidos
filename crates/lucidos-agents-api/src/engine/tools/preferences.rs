use super::super::LucidosEngine;
use crate::core::prefs::{self, PrefAccess, PrefScope};
use crate::core::PreferenceStore;
use crate::llm::tool_names as tn;

const PUSH_ENABLED_REPLY: &str = "[PUSH_NOTIFICATION_REQUEST][ACTION COMPLETED] Push notifications enabled for this device. Tell the user what to expect, keyed off the last used device in [USER DEVICE & PREFERENCES]: if its details say \"Lucidos desktop app\" they are in the native desktop app: notifications arrive as native macOS notifications governed by System Settings → Notifications (macOS asks for permission on first launch; if no banner appears, allow Lucidos there); do NOT mention browser permission or site settings. Otherwise they are in a web browser or installed PWA, and the browser will now ask for notification permission and they should click Allow. Note: in a development build (tauri-dev) native desktop banners don't appear at all, so run Lucidos in a browser/PWA to receive notifications while developing. Either way, they'll get notifications for triggered tasks and alerts.";

const MAX_LISTED_VALUE_CHARS: usize = 200;

fn summarize_pref_value(value: &str) -> String {
    let len = value.chars().count();
    if len <= MAX_LISTED_VALUE_CHARS {
        return value.to_string();
    }
    let head: String = value.chars().take(MAX_LISTED_VALUE_CHARS).collect();
    format!(
        "{head}… (truncated here; {len} characters stored. This key REPLACES the \
         whole value, so read it in full first with `lucidos preferences get` via \
         run_bash, or you will overwrite what you cannot see.)"
    )
}

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

fn human_size(bytes: Option<u64>) -> String {
    match bytes {
        Some(b) if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        Some(b) if b >= 1 << 20 => format!("{:.0} MB", b as f64 / (1u64 << 20) as f64),
        Some(b) => format!("{b} B"),
        None => "?".to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ProviderAccount {
    Unset,
    UnknownProvider,
    LookupFailed(String),
    NotConnected,
    /// INVARIANT: Connected, but the grant is short one or more of the scopes the provider
    /// needs. Carries WHICH, so the agent can name them exactly as the Backup
    /// page does: the two read the same `provider_readiness` verdict and must
    /// not be able to describe it differently.
    MissingScope(Vec<&'static str>),
    Ready,
}

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

#[easy_ext::ext(PreferencesExt)]
pub(crate) impl LucidosEngine {
    async fn execute_get_backup_status(
        &self,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        use crate::core::backup::{self, is_schedule_active, BackupRunStatus};

        let pool = &self.pool;
        let cron = match prefs::BACKUP_SCHEDULE.try_stored(pool).await {
            Ok(v) => v,
            Err(e) => {
                return Ok(format!(
                    "Error: could not read the backup schedule preference: {}. Backup status is unknown; do not tell the user backups are off.",
                    e
                ))
            }
        };
        let provider = match prefs::BACKUP_PROVIDER.try_stored(pool).await {
            Ok(v) => v,
            Err(e) => {
                return Ok(format!(
                    "Error: could not read the backup provider preference: {}. Backup status is unknown; do not tell the user backups are off.",
                    e
                ))
            }
        };
        let retention = prefs::BACKUP_RETENTION.read(pool).await.round() as usize;
        let tz: chrono_tz::Tz = self.user_timezone().await.parse().unwrap_or(chrono_tz::UTC);

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
                if let Ok(schedule) = crate::scheduler::cron_rules::parse_standard_cron(c) {
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

        out.push_str(&format!(
            "\nChange the schedule/provider/retention with set_preference \
             (keys: {schedule}, {provider}, {retention}). Setting \
             {provider} only picks a destination, it does NOT connect the account: \
             that is connect_oauth_account, or the user in Settings → Accounts. Restore is \
             done from the workspace picker, not from here.",
            schedule = prefs::BACKUP_SCHEDULE.key(),
            provider = prefs::BACKUP_PROVIDER.key(),
            retention = prefs::BACKUP_RETENTION.key(),
        ));
        Ok(out)
    }

    async fn execute_preferences_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
        device_id: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match name {
            tn::SET_PREFERENCE => self.execute_set_preference(args, device_id).await,
            tn::GET_PREFERENCES => self.execute_get_preferences(device_id).await,
            _ => Err(format!("Unknown preferences tool: {}", name).into()),
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

        // INVARIANT: Catalog gate: only agent-settable keys, with validated values. Internal
        // keys (command_guard, backup config, keybindings, …) are rejected here —
        // the agent must not disable its own command guard.
        let spec = match prefs::lookup(key) {
            Some(s) => s,
            None => {
                return Ok(match prefs::internal_hint(key) {
                    Some(hint) => format!("Error: '{}' can't be changed with set_preference — {}.", key, hint),
                    None => format!(
                        "Error: unknown preference '{}'. Call get_preferences to see the settable keys.",
                        key
                    ),
                });
            }
        };
        if let Err(e) = prefs::validate(spec, value) {
            return Ok(format!("Error: {}", e));
        }

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
        let effect_note = match key {
            k if k == prefs::CHAT_MODEL.key() => " This is the default for NEW Lucidos Agent threads. A thread that's already running — including this one — keeps its current model (whatever it last used), so this preference change does NOT switch the current thread's model on its next turn. To change a running thread's model, use its in-thread model picker.",
            k if k == prefs::TRIGGER_MODEL.key() => " This is the default for intent triggers that pin no model. It applies from the trigger's next fire, and a trigger's own model still wins. It does not change any chat thread.",
            k if k == prefs::CHAT_REASONING_EFFORTS.key() => " These are the defaults for NEW Lucidos Agent threads. A thread that's already running, including this one, keeps its current reasoning effort while it stays on the same model, so this change does NOT move the current thread's effort on its next turn. To change a running thread's effort, use its in-thread picker.",
            k if k == prefs::RESPONSE_STYLE.key() || k == prefs::RESPONSE_STYLES.key() => " It applies from the next message, in this thread and every other: a turn builds its prompt once at the start, so this one finishes in the style it began in. It covers triggers too, and it does NOT change coding-agent sessions.",
            _ => " Open Lucidos views pick this up automatically.",
        };
        Ok(format!(
            "[ACTION COMPLETED] {} set to '{}'{}.{}",
            spec.label,
            summarize_pref_value(value),
            scope_note,
            effect_note
        ))
    }

    async fn execute_get_preferences(
        &self,
        device_id: Option<&str>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let global = PreferenceStore::get_all(&self.pool).await?;
        let effective = match device_id {
            Some(did) => PreferenceStore::get_all_for_device(&self.pool, did).await?,
            None => global.clone(),
        };

        let mut out = String::from(
            "Settable preferences — set with set_preference(key, value). Device-scoped keys apply to the calling device only.\n",
        );
        let agent_specs = prefs::CATALOG
            .iter()
            .filter(|spec| spec.access == PrefAccess::Agent);
        for spec in agent_specs {
            let current = match effective.get(spec.key).map(String::as_str) {
                None => "(unset)".to_string(),
                Some("") => "(empty)".to_string(),
                Some(v) => summarize_pref_value(v),
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
                prefs::allowed_values_hint(spec),
                spec.default_label(),
            ));
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

        // INVARIANT: Human-only settings that exist in the store, shown read-only so the
        // agent can explain them but knows not to set them. Engine bookkeeping
        // is never printed: it holds a transport secret and migration markers.
        let internal: Vec<String> = prefs::internal_specs()
            .filter_map(|spec| {
                let prefs::PrefAccess::Human { hint } = spec.access else {
                    return None;
                };
                effective
                    .get(spec.key)
                    .map(|v| format!("- {} = {} (read-only: {})", spec.key, v, hint))
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

    #[test]
    fn unknown_provider_lists_the_valid_ids() {
        let line = backup_provider_line("dropbx", &ProviderAccount::UnknownProvider);
        assert!(line.contains("dropbx"), "{line}");
        for id in crate::core::backup::PROVIDER_IDS {
            assert!(line.contains(id), "must list {id}: {line}");
        }
    }
}

#[cfg(test)]
mod push_marker_tests {
    use super::PUSH_ENABLED_REPLY;

    #[test]
    fn the_push_reply_opens_with_the_marker_the_sse_loop_keys_off() {
        const MARKER: &str = "[PUSH_NOTIFICATION_REQUEST]";
        assert!(
            PUSH_ENABLED_REPLY.starts_with(MARKER),
            "marker must come first"
        );
        let run_loop = include_str!("../agentic_loop/run.rs");
        assert!(
            run_loop.contains(&format!("result.starts_with(\"{MARKER}\")")),
            "the agentic loop no longer keys off the marker"
        );
    }
}
