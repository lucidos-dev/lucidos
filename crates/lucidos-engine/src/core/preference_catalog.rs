//! The **preference catalog** — the single source of truth for the user
//! preferences the *Lucidos Agent* may read and write through the
//! `get_preferences` / `set_preference` tools.
//!
//! Lucidos "Settings" spans several backing stores (preferences, models,
//! credentials, MCP servers, repositories). This catalog covers ONLY the
//! `preferences` key→value table — the bulk of Settings. Each [`PrefSpec`]
//! declares a key's scope, allowed values, default, and any write side-effect,
//! so the tool layer can validate the agent's writes and the agent can discover
//! what's settable without dropping to raw HTTP (the motivating failure being a
//! thread where the agent curled `/preferences` and
//! fumbled the device-scope override).
//!
//! What is NOT here is deliberate:
//! - **Internal / managed-elsewhere keys** ([`INTERNAL_KEYS`]) — the command
//!   guard safety toggles, the per-turn tool-call cap, backup config,
//!   keybindings, debug toggles. These are rejected by `set_preference` with a
//!   hint pointing the user at the right Settings surface. The agent must not
//!   be able to disable its own command guard, nor to raise the backstop over
//!   its own agentic loop.
//! - **Secrets** never live in `preferences` (they go through credentials), so
//!   no catalog entry is a secret.
//!
//! The catalog is also the source the `system-knowhow/preferences.md` doc is
//! kept in lockstep with (a sync test fails on drift — see
//! `.claude/rules/system-knowhow.md`).

/// Where a preference is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefScope {
    /// One workspace-wide value (`device_id IS NULL`).
    Global,
    /// A per-device override (keyed by the caller's device id) that wins over the
    /// global value. `set_preference` auto-resolves the caller's device id for
    /// these; the agent never has to pass it.
    Device,
}

/// A side-effect a write triggers beyond the plain store upsert + the persisted
/// `PreferencesChanged` broadcast. Lets the one write chokepoint keep the
/// engine's in-memory caches and the legacy per-key events honest regardless of
/// which entry path (LLM tool or HTTP `PUT /preferences`) made the write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefSideEffect {
    /// Plain preference — upsert + emit `PreferencesChanged`.
    None,
    /// `language` — also refresh the engine's in-memory `user_language` and emit
    /// `LanguageSet` (the frontend live-applies on it).
    Language,
    /// `timezone` — also refresh in-memory `user_timezone` and emit
    /// `TimezoneSet`. (Value is IANA-validated by [`PrefValue::IanaTimezone`].)
    Timezone,
    /// `push_notifications` — also sync the `devices.push_enabled` column and let
    /// the tool layer drive the `[PUSH_NOTIFICATION_REQUEST]` permission
    /// handshake.
    Push,
}

/// The allowed values for a preference — drives both validation and the
/// agent-facing description.
#[derive(Debug, Clone, Copy)]
pub enum PrefValue {
    /// `"true"` / `"false"`.
    Bool,
    /// One of a fixed set of strings.
    Enum(&'static [&'static str]),
    /// A number in `[min, max]` inclusive (parsed as f64; integers accepted).
    Number { min: f64, max: f64 },
    /// An IANA timezone name (validated via `chrono_tz`).
    IanaTimezone,
    /// Free-form non-empty text (e.g. a model id, a URL, a language name).
    Text,
}

/// One agent-settable preference.
pub struct PrefSpec {
    pub key: &'static str,
    pub label: &'static str,
    pub scope: PrefScope,
    pub value: PrefValue,
    /// The effective default when the preference is unset (shown to the agent).
    pub default: &'static str,
    pub description: &'static str,
    pub side_effect: PrefSideEffect,
}

/// Mirrors the OpenAI/Anthropic reasoning tiers in
/// `crates/lucidos-app/src/store/models.ts` (`REASONING_LEVELS`). The engine
/// clamps to what each model supports, so the catalog only checks membership.
pub(crate) const REASONING_EFFORTS: &[&str] = &["none", "low", "medium", "high", "xhigh", "max"];
/// Mirrors `ImageModel` in `crates/lucidos-app/src/store/actions/preferences.ts`.
const IMAGE_MODELS: &[&str] = &[
    "auto",
    "imagen-4",
    "gpt-image-1",
    "gpt-image-1.5",
    "gpt-image-2",
];
/// Mirrors `ExternalLinkTarget` in
/// `crates/lucidos-app/src/store/actions/preferences.ts`, which is also where
/// the routing lives (`utils/openExternalUrl.ts`).
const EXTERNAL_LINK_TARGETS: &[&str] = &["safari", "ask", "in-app"];
/// Mirrors `Theme` / `FontFamily` in the same frontend file.
const THEMES: &[&str] = &["light", "dark", "system"];
const FONT_FAMILIES: &[&str] = &[
    "monospace",
    "system",
    "inter",
    "jetbrains-mono",
    "ibm-plex-mono",
    "fira-code",
];

/// The catalog. Keep this in lockstep with `system-knowhow/preferences.md`
/// (the sync test in this module's tests pins the doc's key table to these
/// keys).
pub const CATALOG: &[PrefSpec] = &[
    // ---- Locale (global, side-effecting) ----
    PrefSpec {
        key: "language",
        label: "Language",
        scope: PrefScope::Global,
        value: PrefValue::Text,
        default: "(detected from the conversation)",
        description: "Preferred language for the Lucidos Agent's responses and session summaries (e.g. 'English', 'Norwegian', 'Spanish').",
        side_effect: PrefSideEffect::Language,
    },
    PrefSpec {
        key: "timezone",
        label: "Timezone",
        scope: PrefScope::Global,
        value: PrefValue::IanaTimezone,
        default: "(unset — ask the user)",
        description: "IANA timezone (e.g. 'Europe/Oslo', 'America/New_York', 'Asia/Tokyo'). Used for trigger scheduling and time display. Set this before creating triggers.",
        side_effect: PrefSideEffect::Timezone,
    },
    // ---- Models (global) ----
    PrefSpec {
        key: "chat_model",
        label: "Chat model",
        scope: PrefScope::Global,
        value: PrefValue::Text,
        default: "claude-opus-5@default",
        description: "Default chat model id for NEW Lucidos Agent threads. A thread that's already running reuses its own last-used model, so changing this does NOT switch the current/running thread on its next turn (use the thread's in-thread picker for that). Must be an enabled model id from the registry — call get_preferences or manage_models(action='list') to see the options (e.g. 'claude-opus-5@default', 'claude-fable-5').",
        side_effect: PrefSideEffect::None,
    },
    PrefSpec {
        key: "chat_reasoning_effort",
        label: "Reasoning effort",
        scope: PrefScope::Global,
        value: PrefValue::Enum(REASONING_EFFORTS),
        default: "high",
        description: "Default thinking budget for NEW Lucidos Agent threads. A thread that's already running reuses its own last-used effort, so changing this does NOT change the current/running thread on its next turn (use the thread's in-thread picker for that). Not every model supports every tier; the engine clamps per model.",
        side_effect: PrefSideEffect::None,
    },
    PrefSpec {
        key: "image_model",
        label: "Image model",
        scope: PrefScope::Global,
        value: PrefValue::Enum(IMAGE_MODELS),
        default: "auto",
        description: "Model used by generate_image. 'auto' lets the engine choose.",
        side_effect: PrefSideEffect::None,
    },
    PrefSpec {
        key: "model_title",
        label: "Title model",
        scope: PrefScope::Global,
        value: PrefValue::Text,
        default: "gemini-3-flash-preview",
        description: "Background model that generates thread titles. A cheap, fast model is appropriate.",
        side_effect: PrefSideEffect::None,
    },
    PrefSpec {
        key: "model_image_description",
        label: "Image-description model",
        scope: PrefScope::Global,
        value: PrefValue::Text,
        default: "gemini-3-flash-preview",
        description: "Background model that describes uploaded images for search/memory.",
        side_effect: PrefSideEffect::None,
    },
    PrefSpec {
        key: "model_memory",
        label: "Memory model",
        scope: PrefScope::Global,
        value: PrefValue::Text,
        default: "gemini-3-flash-preview",
        description: "Background model the memory extractor uses.",
        side_effect: PrefSideEffect::None,
    },
    // ---- Providers (global) ----
    PrefSpec {
        key: "vertex_region",
        label: "Vertex region",
        scope: PrefScope::Global,
        value: PrefValue::Text,
        default: "europe-west1",
        description: "Google Vertex AI region for vertex-provider models (e.g. 'europe-west1', 'us-central1', 'global').",
        side_effect: PrefSideEffect::None,
    },
    PrefSpec {
        key: "local_base_url",
        label: "Local provider base URL",
        scope: PrefScope::Global,
        value: PrefValue::Text,
        default: "http://localhost:11434/v1",
        description: "Base URL for the 'local' OpenAI-compatible provider (Ollama / LM Studio / vLLM / llama.cpp).",
        side_effect: PrefSideEffect::None,
    },
    // ---- Behavior (global) ----
    PrefSpec {
        key: "notifications_filter",
        label: "Notifications filter",
        scope: PrefScope::Global,
        value: PrefValue::Enum(&["all", "unread"]),
        default: "all",
        description: "Which notifications the bell shows.",
        side_effect: PrefSideEffect::None,
    },
    PrefSpec {
        key: "mobile_header_sticky",
        label: "Sticky mobile header",
        scope: PrefScope::Global,
        value: PrefValue::Bool,
        default: "true",
        description: "Keep the mobile header always visible ('true') instead of hiding it on scroll ('false').",
        side_effect: PrefSideEffect::None,
    },
    PrefSpec {
        key: "external_link_target",
        label: "External links open in",
        scope: PrefScope::Global,
        value: PrefValue::Enum(EXTERNAL_LINK_TARGETS),
        default: "safari",
        description: "Where an external http(s) link goes when tapped in an INSTALLED iOS PWA (no effect on desktop, Android, or a normal Safari tab, which all open a new tab). 'safari' hands it to the Safari app; 'ask' opens the OS share sheet so iOS offers every installed browser, including the user's real default; 'in-app' keeps it in the PWA's in-app web view, which has no address bar and no shared Safari session.",
        side_effect: PrefSideEffect::None,
    },
    PrefSpec {
        key: "welcome_suggestions_dismissed",
        label: "Welcome message dismissed",
        scope: PrefScope::Global,
        value: PrefValue::Bool,
        default: "false",
        description: "Whether the new-workspace welcome message + starter suggestions are hidden. Set 'false' to SHOW the welcome message again, 'true' to hide it.",
        side_effect: PrefSideEffect::None,
    },
    PrefSpec {
        key: "coding_agent_default",
        label: "Default coding agent",
        scope: PrefScope::Global,
        value: PrefValue::Enum(&["claude-code", "codex"]),
        default: "claude-code",
        description: "Default coding agent the compose destination picker pre-selects for coding targets.",
        side_effect: PrefSideEffect::None,
    },
    PrefSpec {
        key: "coding_agent_claude_path",
        label: "Claude Code binary path",
        scope: PrefScope::Global,
        value: PrefValue::Text,
        default: "(auto-detected)",
        description: "Absolute path to the `claude` CLI used for Claude Code threads. Leave unset to auto-detect (~/.local/bin, ~/.claude/local, Homebrew, PATH). A set path that doesn't exist fails the spawn with an error naming this setting — it never silently falls back.",
        side_effect: PrefSideEffect::None,
    },
    PrefSpec {
        key: "coding_agent_codex_path",
        label: "Codex binary path",
        scope: PrefScope::Global,
        value: PrefValue::Text,
        default: "(auto-detected)",
        description: "Absolute path to the `codex` CLI used for Codex threads. Leave unset to auto-detect (~/.local/bin, Homebrew, PATH). A set path that doesn't exist fails the spawn with an error naming this setting — it never silently falls back.",
        side_effect: PrefSideEffect::None,
    },
    // ---- Appearance (device-scoped) ----
    PrefSpec {
        key: "theme",
        label: "Theme",
        scope: PrefScope::Device,
        value: PrefValue::Enum(THEMES),
        default: "dark",
        description: "Color theme for THIS device. 'system' follows the OS setting. Device-scoped — it overrides the global value on the device that set it.",
        side_effect: PrefSideEffect::None,
    },
    PrefSpec {
        key: "font-family",
        label: "Font family",
        scope: PrefScope::Device,
        value: PrefValue::Enum(FONT_FAMILIES),
        default: "monospace",
        description: "UI font for THIS device. Device-scoped.",
        side_effect: PrefSideEffect::None,
    },
    PrefSpec {
        key: "ui-scale",
        label: "UI scale",
        scope: PrefScope::Device,
        value: PrefValue::Number { min: 75.0, max: 200.0 },
        default: "100",
        description: "UI scale percent for THIS device (75–200, snapped to 12.5 steps; 100 = default). Device-scoped.",
        side_effect: PrefSideEffect::None,
    },
    // ---- Push (device-scoped, side-effecting) ----
    PrefSpec {
        key: "push_notifications",
        label: "Push notifications",
        scope: PrefScope::Device,
        value: PrefValue::Enum(&["enabled", "declined"]),
        default: "(unset — ask the user)",
        description: "Push notifications for THIS device. Setting 'enabled' triggers the browser/OS permission prompt; 'declined' suppresses it so the user isn't asked again. Device-scoped.",
        side_effect: PrefSideEffect::Push,
    },
    // ---- Backup (global) — re-registers the backup cron via the scheduler's
    // PreferencesChanged subscriber (no engine restart). The provider's ACCOUNT
    // must be connected in Settings → Accounts (NOT on the Backup page, which
    // has no account UI) before a scheduled backup can upload; an enabled
    // schedule auto-generates an encryption key on its first run.
    PrefSpec {
        key: "backup_schedule",
        label: "Backup schedule",
        scope: PrefScope::Global,
        value: PrefValue::Text,
        default: "off",
        description: "Automatic backup schedule as a 6-field cron expression in the user's timezone (second minute hour day-of-month month day-of-week), or 'off' to disable. Examples: '0 0 3 * * *' = daily 03:00, '0 0 3 * * 0' = weekly Sunday 03:00, '0 0 */12 * * *' = every 12h. Requires backup_provider set AND its account connected (see that key).",
        side_effect: PrefSideEffect::None,
    },
    PrefSpec {
        key: "backup_provider",
        label: "Backup provider",
        scope: PrefScope::Global,
        value: PrefValue::Enum(crate::core::backup::PROVIDER_IDS),
        default: "(unset — ask the user)",
        description: "Cloud destination for backups, independent of backup_schedule: it stays set with the schedule off, and the Backup page's provider dropdown both opens on it and writes it. Setting this connects NOTHING: the account is connected separately with connect_oauth_account, or by the user in Settings → Accounts (never on the Backup page, which has no account UI). Until then backups run and the upload fails. Check get_backup_status, which reports whether the account is connected.",
        side_effect: PrefSideEffect::None,
    },
    PrefSpec {
        key: "backup_retention",
        label: "Backup retention",
        scope: PrefScope::Global,
        value: PrefValue::Number { min: 1.0, max: 50.0 },
        default: "5",
        description: "How many of the most recent backups to keep in the provider; older ones are pruned after each successful backup.",
        side_effect: PrefSideEffect::None,
    },
    PrefSpec {
        key: "backup_reminder_dismissed",
        label: "Backup reminder dismissed",
        scope: PrefScope::Global,
        value: PrefValue::Text,
        default: "(unset: the reminder shows)",
        description: "Dismissal state of the app-shell banner shown while backup is off (no active backup_schedule with a backup_provider). Three values: unset or empty means never dismissed, so the banner shows; an RFC 3339 instant means dismissed then, hidden for 30 days from that instant; 'forever' means dismissed a second time, hidden permanently. Set it to empty to bring the reminder back. The banner is only ever shown while backup is off, so enabling a schedule hides it whatever this says.",
        side_effect: PrefSideEffect::None,
    },
];

/// Known preference keys the agent must NOT write, each with a hint pointing at
/// where the human changes it. Distinct from a truly unknown key so the agent
/// gets actionable guidance instead of "unknown key".
pub const INTERNAL_KEYS: &[(&str, &str)] = &[
    ("command_guard", "the command guard is the safety gate over the agent's own bash/python — toggle it in Settings → Permissions, not via set_preference"),
    ("command_guard_judge", "managed in Settings → Permissions (Command safety)"),
    ("model_command_judge", "managed in Settings → Permissions (Command safety)"),
    ("max_tool_calls", "the per-turn tool-call cap is the backstop over your own agentic loop, so you must not raise your own limit; the user changes it in Settings → Models → Chat & triggers"),
    ("capture_context", "a debug-only context-capture toggle; change it in Settings if you really need to"),
    ("backup_last_run", "internal backup state, not a setting"),
    ("keybindings", "keyboard shortcuts are managed in Settings → Keyboard Shortcuts"),
    ("network_bind", "the per-workspace engine network bind (loopback / all interfaces / a specific tailnet IP): a security setting changed in Settings → Access → Network access, not via set_preference"),
    ("engine_switch_dismissed_build", "internal UI state — the on-disk engine build id the user deferred the 'Switch to new version' toast for (workspace-global so a dismiss defers on every device); managed by the version-update toast, not a setting"),
    ("client_refresh_dismissed_build", "internal UI state — the served client build id the user deferred the 'refresh to sync' toast for (workspace-global so a dismiss defers on every device); managed by the version-update toast, not a setting"),
];

/// Preference keys the engine writes as its own internal state, and which
/// therefore do NOT announce `PreferencesChanged`.
///
/// The `preferences` table is a general key/value store used for two different
/// things: settings the user changes (which every device must learn about) and
/// engine bookkeeping that happens to need a durable home. Announcing is the
/// DEFAULT, and this list is the closed set of exceptions, each with the reason
/// it is not a setting.
///
/// [`PreferenceStore::set_silent`] REJECTS any key absent from this list, so a
/// caller cannot use the silent door to write a user-visible preference by
/// mistake. That inversion is the point: forgetting to announce is the failure
/// this guards against, so silence has to be asked for explicitly and by name.
///
/// Distinct from [`INTERNAL_KEYS`], which is about who may WRITE a key (those
/// are real settings the human edits in Settings; the agent just must not touch
/// them, and they announce normally).
pub const SILENT_PREF_KEYS: &[(&str, &str)] = &[
    (
        "vapid_keys",
        "the workspace's Web Push signing keypair, generated once on demand; a \
         transport secret, not a setting",
    ),
    (
        "backup_last_run",
        "the outcome of the most recent backup, written by the backup job itself \
         so the Settings page survives a restart; the run already announces \
         BackupCompleted / BackupFailed",
    ),
    (
        "backfill_trigger_id_from_events_done",
        "a one-shot migration marker, so a completed backfill is not re-run",
    ),
    (
        "backfill_trigger_id_v5_to_config_id_done",
        "a one-shot migration marker, so a completed backfill is not re-run",
    ),
    (
        "backfill_repo_names_from_changes_done",
        "a one-shot migration marker, so a completed backfill is not re-run",
    ),
    (
        "backfill_cc_repo_id_to_deterministic_done",
        "a one-shot migration marker, so a completed backfill is not re-run",
    ),
];

/// Whether a preference key is engine-internal state rather than a setting, and
/// so writes silently. See [`SILENT_PREF_KEYS`].
pub fn is_silent_key(key: &str) -> bool {
    SILENT_PREF_KEYS.iter().any(|(k, _)| *k == key)
}

/// Find the catalog spec for an agent-settable key.
pub fn lookup(key: &str) -> Option<&'static PrefSpec> {
    CATALOG.iter().find(|s| s.key == key)
}

/// If `key` is a known-but-not-agent-settable preference, return the hint
/// explaining where the human changes it.
pub fn internal_hint(key: &str) -> Option<&'static str> {
    INTERNAL_KEYS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, hint)| *hint)
}

/// Validate `value` against a spec's [`PrefValue`]. Returns a human-readable
/// error (suitable to hand back to the agent) on rejection.
pub fn validate(spec: &PrefSpec, value: &str) -> Result<(), String> {
    match spec.value {
        PrefValue::Bool => {
            if value == "true" || value == "false" {
                Ok(())
            } else {
                Err(format!(
                    "'{}' must be 'true' or 'false' (got '{}')",
                    spec.key, value
                ))
            }
        }
        PrefValue::Enum(allowed) => {
            if allowed.contains(&value) {
                Ok(())
            } else {
                Err(format!(
                    "'{}' must be one of [{}] (got '{}')",
                    spec.key,
                    allowed.join(", "),
                    value
                ))
            }
        }
        PrefValue::Number { min, max } => match value.parse::<f64>() {
            Ok(n) if n >= min && n <= max => Ok(()),
            Ok(n) => Err(format!(
                "'{}' must be a number between {} and {} (got {})",
                spec.key, min, max, n
            )),
            Err(_) => Err(format!("'{}' must be a number (got '{}')", spec.key, value)),
        },
        PrefValue::IanaTimezone => {
            if value.parse::<chrono_tz::Tz>().is_ok() {
                Ok(())
            } else {
                Err(format!(
                    "'{}' must be an IANA timezone name like 'Europe/Oslo' or 'America/New_York' (got '{}')",
                    spec.key, value
                ))
            }
        }
        PrefValue::Text => {
            if value.trim().is_empty() {
                Err(format!("'{}' must not be empty", spec.key))
            } else {
                Ok(())
            }
        }
    }
}

/// Human-readable summary of a spec's allowed values, used in the agent-facing
/// description for both `set_preference`'s error path and `get_preferences`.
pub fn allowed_values_hint(spec: &PrefSpec) -> String {
    match spec.value {
        PrefValue::Bool => "true | false".to_string(),
        PrefValue::Enum(allowed) => allowed.join(" | "),
        PrefValue::Number { min, max } => format!("number {}–{}", min, max),
        PrefValue::IanaTimezone => "IANA timezone name".to_string(),
        PrefValue::Text => "text".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_keys_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for spec in CATALOG {
            assert!(seen.insert(spec.key), "duplicate catalog key: {}", spec.key);
        }
    }

    #[test]
    fn catalog_and_internal_keys_are_disjoint() {
        for (k, _) in INTERNAL_KEYS {
            assert!(
                lookup(k).is_none(),
                "key {} is both agent-settable and internal",
                k
            );
        }
    }

    #[test]
    fn command_guard_is_not_settable_but_has_a_hint() {
        assert!(lookup("command_guard").is_none());
        assert!(internal_hint("command_guard").is_some());
    }

    /// Same shape as the command-guard test above, and for the same reason: the
    /// per-turn tool-call cap is the backstop over the agent's own loop, so the
    /// agent must not be able to raise it. It still gets an actionable hint
    /// rather than "unknown key", because explaining where the human changes it
    /// is exactly what the agent should do when asked.
    #[test]
    fn max_tool_calls_is_not_settable_but_has_a_hint() {
        assert!(lookup("max_tool_calls").is_none());
        let hint = internal_hint("max_tool_calls").expect("must carry a hint");
        assert!(
            hint.contains("Settings"),
            "the hint must point at the Settings surface, got: {hint}"
        );
    }

    #[test]
    fn bool_validation() {
        let spec = lookup("welcome_suggestions_dismissed").unwrap();
        assert!(validate(spec, "true").is_ok());
        assert!(validate(spec, "false").is_ok());
        assert!(validate(spec, "yes").is_err());
    }

    #[test]
    fn enum_validation() {
        let spec = lookup("theme").unwrap();
        assert!(validate(spec, "dark").is_ok());
        assert!(validate(spec, "system").is_ok());
        assert!(validate(spec, "blue").is_err());
    }

    #[test]
    fn number_validation_respects_bounds() {
        let spec = lookup("ui-scale").unwrap();
        assert!(validate(spec, "100").is_ok());
        assert!(validate(spec, "137.5").is_ok());
        assert!(validate(spec, "50").is_err());
        assert!(validate(spec, "300").is_err());
        assert!(validate(spec, "big").is_err());
    }

    #[test]
    fn timezone_validation() {
        let spec = lookup("timezone").unwrap();
        assert!(validate(spec, "Europe/Oslo").is_ok());
        assert!(validate(spec, "Not/AZone").is_err());
    }

    #[test]
    fn theme_is_device_scoped_and_language_is_global() {
        assert_eq!(lookup("theme").unwrap().scope, PrefScope::Device);
        assert_eq!(lookup("language").unwrap().scope, PrefScope::Global);
    }

    /// Drift guard (`.claude/rules/system-knowhow.md`): every catalog key and
    /// every internal key must be documented in `system-knowhow/preferences.md`,
    /// the agent-facing reference. Forward direction only — a doc row for a
    /// retired key is harmless, a settable key with no doc is the dangerous drift.
    #[test]
    fn doc_lists_every_catalog_and_internal_key() {
        let doc = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../system-knowhow/preferences.md"
        ))
        .expect("system-knowhow/preferences.md must exist");
        for spec in CATALOG {
            assert!(
                doc.contains(spec.key),
                "preference '{}' is settable but not documented in system-knowhow/preferences.md",
                spec.key
            );
        }
        for (k, _) in INTERNAL_KEYS {
            assert!(
                doc.contains(k),
                "internal preference '{}' is not mentioned in system-knowhow/preferences.md",
                k
            );
        }
    }
}
