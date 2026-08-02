use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

// Background model preference keys
pub const PREF_MODEL_TITLE: &str = "model_title";
pub const PREF_MODEL_IMAGE_DESCRIPTION: &str = "model_image_description";
pub const PREF_MODEL_MEMORY: &str = "model_memory";
/// Model the *command guard*'s LLM *judge* uses to classify the ambiguous
/// middle (ADR 0002, Phase 3). Configurable so a workspace can trade
/// accuracy/cost; defaults to [`DEFAULT_COMMAND_JUDGE_MODEL`] when unset.
pub const PREF_MODEL_COMMAND_JUDGE: &str = "model_command_judge";

/// Default model for the command-guard judge — a cheap, fast model (Haiku) per
/// ADR 0002. Mirrored on the frontend in
/// `crates/lucidos-app/src/store/actions/preferences.ts`
/// (`DEFAULT_COMMAND_JUDGE_MODEL`).
pub const DEFAULT_COMMAND_JUDGE_MODEL: &str = "claude-haiku-4-5";

// Chat preference keys (also written by frontend Settings UI)
pub const PREF_CHAT_MODEL: &str = "chat_model";
pub const PREF_CHAT_REASONING_EFFORT: &str = "chat_reasoning_effort";

// Coding-agent binary path overrides (also written by frontend Settings UI).
// Unset = auto-detect (probe list → PATH); a set path wins outright and a
// wrong one fails the spawn naming the key (see
// `runtime::spawn_env::resolve_binary_override`).
pub const PREF_CODING_AGENT_CLAUDE_PATH: &str = "coding_agent_claude_path";
pub const PREF_CODING_AGENT_CODEX_PATH: &str = "coding_agent_codex_path";

/// Default chat model when neither user preference nor `LUCIDOS_MODEL` env is set.
/// Mirrored on the frontend in `crates/lucidos-app/src/store/models.ts`.
pub const DEFAULT_CHAT_MODEL: &str = "claude-opus-5@default";

// Vertex AI configuration
pub const PREF_VERTEX_REGION: &str = "vertex_region";

// Local OpenAI-compatible provider base URL (also written by frontend Settings
// UI). Points the `local` provider at Ollama / LM Studio / vLLM / llama.cpp.
pub const PREF_LOCAL_BASE_URL: &str = "local_base_url";

/// Default base URL for the `local` provider when neither the `local_base_url`
/// preference nor the `LUCIDOS_LOCAL_BASE_URL` env var is set — Ollama's
/// OpenAI-compatible endpoint. Mirrored on the frontend in
/// `crates/lucidos-app/src/components/settings/LocalProviderSettings.tsx`.
pub const DEFAULT_LOCAL_BASE_URL: &str = "http://localhost:11434/v1";

// Image generation model (also written by frontend Settings UI)
pub const PREF_IMAGE_MODEL: &str = "image_model";

// When off, ContextCaptured still fires per LLM call but section
// bodies are dropped — only the name + char_count survives. Defaults
// to "true" to preserve historical behavior.
pub(crate) const PREF_CAPTURE_CONTEXT: &str = "capture_context";

// Master toggle for the *command guard* (ADR 0002): the pre-dispatch safety
// gate over the Lucidos Agent's bash/python tools. Off by default — the feature
// ships dark and is enabled per-workspace. See `engine::command_guard`.
pub(crate) const PREF_COMMAND_GUARD: &str = "command_guard";

// Sub-toggle for the command-guard *judge* (ADR 0002, Phase 3). When the guard
// is on, the LLM judge classifies the ambiguous middle (everything the static
// fast-path doesn't settle). Default on; set to "false" to fall back to the
// static "dangerous" list for the ask lane (the documented reopen path — accept
// more misses, pay no per-command LLM cost/latency). Only consulted when the
// master `command_guard` toggle is on.
pub(crate) const PREF_COMMAND_GUARD_JUDGE: &str = "command_guard_judge";

/// Store for managing user preferences in the database.
///
/// **Announcing is the default, and the silent door is guarded.**
/// [`Self::set`], [`Self::set_for_device`] and [`Self::delete`] emit
/// `PreferencesChanged` from inside the write path; the raw row writes are
/// private to this module. [`Self::set_silent`] exists for the handful of keys
/// that are engine bookkeeping rather than settings, and it REJECTS any key
/// absent from `preference_catalog::SILENT_PREF_KEYS`, so it cannot be used to
/// write a user-visible preference quietly.
///
/// That inversion matters here more than for the other stores, because
/// `PreferencesChanged` is a MECHANISM and not just a notification: the
/// scheduler re-registers the backup cron off a `backup_schedule` write, and
/// the frontend live-applies theme / font / scale. Two writers used to bypass
/// `apply_preference_write` and hand-roll the emit at their call site (the
/// scheduler's backup schedule and the HTTP retention handler), which is the
/// shape that produced the bug this whole change is about.
///
/// See `core::announced_surfaces`.
pub struct PreferenceStore;

impl PreferenceStore {
    /// Defensive double-write — the migration owns this CREATE TABLE
    /// (see `20260517160627_consolidate_init_schema_tables.sql`). Slated
    /// for removal in `harden-init-schema-tables-vs-migrations-pattern-finish`.
    pub async fn init_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS preferences (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Set a global preference (insert or update, device_id IS NULL).
    ///
    /// **Private on purpose**: [`Self::set`] and [`Self::set_silent`] are the
    /// reachable mutators, and the first of them emits.
    async fn set_row(pool: &PgPool, key: &str, value: &str) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO preferences (key, value, device_id, updated_at)
            VALUES ($1, $2, NULL, NOW())
            ON CONFLICT (key, COALESCE(device_id, '')) DO UPDATE SET
                value = EXCLUDED.value,
                updated_at = NOW()
            "#,
        )
        .bind(key)
        .bind(value)
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Set a per-device preference (insert or update).
    ///
    /// **Private on purpose**: [`Self::set_for_device`] is the reachable
    /// mutator, and it emits.
    async fn set_for_device_row(
        pool: &PgPool,
        key: &str,
        value: &str,
        device_id: &str,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            INSERT INTO preferences (key, value, device_id, updated_at)
            VALUES ($1, $2, $3, NOW())
            ON CONFLICT (key, COALESCE(device_id, '')) DO UPDATE SET
                value = EXCLUDED.value,
                updated_at = NOW()
            "#,
        )
        .bind(key)
        .bind(value)
        .bind(device_id)
        .execute(pool)
        .await?;

        Ok(())
    }

    /// Get a global preference by key (device_id IS NULL)
    pub async fn get(pool: &PgPool, key: &str) -> Result<Option<String>, sqlx::Error> {
        let result = sqlx::query_scalar::<_, String>(
            "SELECT value FROM preferences WHERE key = $1 AND device_id IS NULL",
        )
        .bind(key)
        .fetch_optional(pool)
        .await?;

        Ok(result)
    }

    /// Get a preference for a specific device, falling back to the global value
    pub async fn get_for_device(
        pool: &PgPool,
        key: &str,
        device_id: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        // Try device-specific first
        let result = sqlx::query_scalar::<_, String>(
            "SELECT value FROM preferences WHERE key = $1 AND device_id = $2",
        )
        .bind(key)
        .bind(device_id)
        .fetch_optional(pool)
        .await?;

        if result.is_some() {
            return Ok(result);
        }

        // Fall back to global
        Self::get(pool, key).await
    }

    /// Get all global preferences as a HashMap
    pub async fn get_all(pool: &PgPool) -> Result<HashMap<String, String>, sqlx::Error> {
        let results = sqlx::query_as::<_, (String, String)>(
            "SELECT key, value FROM preferences WHERE device_id IS NULL ORDER BY key ASC",
        )
        .fetch_all(pool)
        .await?;

        Ok(results.into_iter().collect())
    }

    /// Get merged preferences for a device: global values overridden by device-specific ones
    pub async fn get_all_for_device(
        pool: &PgPool,
        device_id: &str,
    ) -> Result<HashMap<String, String>, sqlx::Error> {
        // Start with global preferences
        let mut map = Self::get_all(pool).await?;

        // Override with device-specific preferences
        let device_results = sqlx::query_as::<_, (String, String)>(
            "SELECT key, value FROM preferences WHERE device_id = $1 ORDER BY key ASC",
        )
        .bind(device_id)
        .fetch_all(pool)
        .await?;

        for (key, value) in device_results {
            map.insert(key, value);
        }

        Ok(map)
    }

    /// Delete a global preference row. **Private on purpose**:
    /// [`Self::delete`] is the reachable mutator, and it emits.
    async fn delete_row(pool: &PgPool, key: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM preferences WHERE key = $1 AND device_id IS NULL")
            .bind(key)
            .execute(pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    /// Write a global preference and announce it.
    ///
    /// Announces unconditionally, including when the value is unchanged: a
    /// preference write is a deliberate user action, and `PreferencesChanged`
    /// is what re-applies the setting (the scheduler re-registers the backup
    /// cron on it), so suppressing a same-value write could skip the
    /// re-application the user was asking for.
    pub async fn set(
        pool: &PgPool,
        event_bus: &EventBus,
        key: &str,
        value: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<(), sqlx::Error> {
        Self::set_row(pool, key, value).await?;
        Self::announce(event_bus, key, Some(value.to_string()), actor).await;
        Ok(())
    }

    /// Write a per-device preference override and announce it.
    pub async fn set_for_device(
        pool: &PgPool,
        event_bus: &EventBus,
        key: &str,
        value: &str,
        device_id: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<(), sqlx::Error> {
        Self::set_for_device_row(pool, key, value, device_id).await?;
        Self::announce(event_bus, key, Some(value.to_string()), actor).await;
        Ok(())
    }

    /// Delete a global preference and announce it. Announces only when a row
    /// existed; `value: None` on the event means "back to the default".
    pub async fn delete(
        pool: &PgPool,
        event_bus: &EventBus,
        key: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, sqlx::Error> {
        let removed = Self::delete_row(pool, key).await?;
        if removed {
            Self::announce(event_bus, key, None, actor).await;
        }
        Ok(removed)
    }

    /// Write a preference key that is engine bookkeeping rather than a setting,
    /// without announcing.
    ///
    /// **Rejects any key not listed in
    /// [`preference_catalog::SILENT_PREF_KEYS`]**, which is what stops this
    /// from becoming the easy way to skip an announcement. Reach for
    /// [`Self::set`] for anything a user can see; if a genuinely internal key
    /// is missing from the list, add it there with its reason.
    pub async fn set_silent(pool: &PgPool, key: &str, value: &str) -> Result<(), sqlx::Error> {
        if !crate::core::preference_catalog::is_silent_key(key) {
            // A protocol violation by the caller, not a database failure. sqlx's
            // error type has no variant for that, so `Protocol` carries the
            // message: the alternative is a second error type for one call site.
            return Err(sqlx::Error::Protocol(format!(
                "'{key}' is not an engine-internal preference key, so it must be written through \
                 PreferenceStore::set (which announces PreferencesChanged). Add it to \
                 SILENT_PREF_KEYS with a reason if it really is internal state."
            )));
        }
        Self::set_row(pool, key, value).await
    }

    /// One place the three announcing paths share, so a write and a delete
    /// cannot drift in what they say.
    async fn announce(
        event_bus: &EventBus,
        key: &str,
        value: Option<String>,
        actor: Option<MessageOrigin>,
    ) {
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::PreferencesChanged {
                    key: key.to_string(),
                    value,
                    actor,
                }),
                "[Preferences] PreferencesChanged",
            )
            .await;
    }

    /// Check if a global preference exists
    pub async fn exists(pool: &PgPool, key: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM preferences WHERE key = $1 AND device_id IS NULL)",
        )
        .bind(key)
        .fetch_one(pool)
        .await?;

        Ok(result)
    }

    /// Read the per-step context-capture toggle. Returns `Ok(false)` when
    /// unset — the debugging capture ships dark and is enabled per-workspace;
    /// set the preference to `"true"` to opt in. DB errors propagate as `Err`
    /// so callers can surface a real failure instead of silently defaulting.
    pub async fn capture_context(pool: &PgPool) -> Result<bool, sqlx::Error> {
        Self::get(pool, PREF_CAPTURE_CONTEXT)
            .await
            .map(|opt| opt.map(|v| v == "true").unwrap_or(false))
    }

    /// Read the *command guard* toggle (ADR 0002). The command guard is a
    /// pre-dispatch safety gate over the Lucidos Agent's bash/python tools.
    /// Returns `Ok(false)` when unset — the feature ships dark and is enabled
    /// per-workspace. DB errors propagate as `Err` so a real failure surfaces
    /// rather than silently disabling the guard.
    pub async fn command_guard(pool: &PgPool) -> Result<bool, sqlx::Error> {
        Self::get(pool, PREF_COMMAND_GUARD)
            .await
            .map(|opt| opt.map(|v| v == "true").unwrap_or(false))
    }

    /// Read the command-guard *judge* sub-toggle (ADR 0002, Phase 3). Returns
    /// `Ok(true)` when unset — when the master guard is on, the judge is the
    /// real classifier by default; set the preference to `"false"` to fall back
    /// to the static dangerous list. DB errors propagate as `Err`. Only
    /// meaningful when [`command_guard`](Self::command_guard) is on.
    pub async fn command_guard_judge(pool: &PgPool) -> Result<bool, sqlx::Error> {
        Self::get(pool, PREF_COMMAND_GUARD_JUDGE)
            .await
            .map(|opt| opt.map(|v| v != "false").unwrap_or(true))
    }

    /// The model the command-guard judge runs on, defaulting to
    /// [`DEFAULT_COMMAND_JUDGE_MODEL`] (Haiku) when unset. DB errors are logged
    /// and treated as unset — the judge falls back to the default model rather
    /// than failing the whole classification.
    pub async fn command_judge_model(pool: &PgPool) -> String {
        match Self::get(pool, PREF_MODEL_COMMAND_JUDGE).await {
            Ok(Some(m)) if !m.trim().is_empty() => m,
            Ok(_) => DEFAULT_COMMAND_JUDGE_MODEL.to_string(),
            Err(e) => {
                log!(
                    "[Preferences] Failed to read {}: {} — using default judge model",
                    PREF_MODEL_COMMAND_JUDGE,
                    e
                );
                DEFAULT_COMMAND_JUDGE_MODEL.to_string()
            }
        }
    }

    /// Read the user's chat model + reasoning effort preferences for code
    /// paths that originate a chat without an explicit user request
    /// (spawn_thread, process_trigger). DB errors are logged and treated as
    /// "unset" — callers fall back to the engine default.
    pub async fn user_chat_settings(pool: &PgPool) -> (Option<String>, Option<String>) {
        let model = Self::get(pool, PREF_CHAT_MODEL).await.unwrap_or_else(|e| {
            log!("[Preferences] Failed to read {}: {}", PREF_CHAT_MODEL, e);
            None
        });
        let effort = Self::get(pool, PREF_CHAT_REASONING_EFFORT)
            .await
            .unwrap_or_else(|e| {
                log!(
                    "[Preferences] Failed to read {}: {}",
                    PREF_CHAT_REASONING_EFFORT,
                    e
                );
                None
            });
        (model, effort)
    }

    /// Read the model + reasoning effort a thread last ran with — the values
    /// stamped on the thread's most recent `MessageReceived` that carried them.
    /// This is the per-thread memory: a follow-up with no explicit override
    /// reuses these instead of snapping back to the account default. Resolved
    /// per field independently (a legacy message with only one set still
    /// contributes that field), newest-first by `sequence`.
    ///
    /// `exclude_event_id` drops the in-flight turn's own `MessageReceived` when
    /// it was pre-emitted upstream (`pre_emitted_origin` == `events.id`), so we
    /// never read the current turn as its own "previous" value. DB errors are
    /// logged and treated as "no record" — callers fall through to preferences.
    pub async fn last_thread_chat_settings(
        pool: &PgPool,
        thread_id: Uuid,
        exclude_event_id: Option<Uuid>,
    ) -> (Option<String>, Option<String>) {
        // `MessageReceived` thread-event payloads are flat (see
        // `ThreadEvent::to_payload`), so `payload->>'model'` reads the field
        // directly. `aggregate_id` is text; bind the thread id as its string.
        let row = sqlx::query_as::<_, (Option<String>, Option<String>)>(
            r#"
            SELECT
              (SELECT payload->>'model'
                 FROM events
                WHERE aggregate_id = $1
                  AND event_type = 'MessageReceived'
                  AND payload->>'model' IS NOT NULL
                  AND payload->>'model' <> ''
                  AND ($2::uuid IS NULL OR id <> $2)
                ORDER BY sequence DESC
                LIMIT 1) AS model,
              (SELECT payload->>'reasoning_effort'
                 FROM events
                WHERE aggregate_id = $1
                  AND event_type = 'MessageReceived'
                  AND payload->>'reasoning_effort' IS NOT NULL
                  AND payload->>'reasoning_effort' <> ''
                  AND ($2::uuid IS NULL OR id <> $2)
                ORDER BY sequence DESC
                LIMIT 1) AS effort
            "#,
        )
        .bind(thread_id.to_string())
        .bind(exclude_event_id)
        .fetch_one(pool)
        .await;
        match row {
            Ok((model, effort)) => (model, effort),
            Err(e) => {
                log!(
                    "[Preferences] Failed to read last thread chat settings for {}: {}",
                    thread_id,
                    e
                );
                (None, None)
            }
        }
    }

    /// Resolve the (model, effort) pair stamped on a chat exchange when the
    /// caller didn't fully specify them, honoring per-thread memory. Order per
    /// field: explicit caller override → the thread's last recorded value →
    /// the user's account chat preference. Skips each DB read it doesn't need.
    pub async fn resolve_chat_overrides_for_thread(
        pool: &PgPool,
        thread_id: Option<Uuid>,
        exclude_event_id: Option<Uuid>,
        model_override: Option<String>,
        effort_override: Option<String>,
    ) -> (Option<String>, Option<String>) {
        if model_override.is_some() && effort_override.is_some() {
            return (model_override, effort_override);
        }
        // Per-thread memory: reuse what this thread last ran with. Only worth a
        // query for a follow-up (a new thread has no prior message).
        let (thread_model, thread_effort) = match thread_id {
            Some(tid) => Self::last_thread_chat_settings(pool, tid, exclude_event_id).await,
            None => (None, None),
        };
        let model = model_override.or(thread_model);
        let effort = effort_override.or(thread_effort);
        if model.is_some() && effort.is_some() {
            return (model, effort);
        }
        let (pref_model, pref_effort) = Self::user_chat_settings(pool).await;
        (model.or(pref_model), effort.or(pref_effort))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    async fn emitted(pool: &PgPool, event_type: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1")
            .bind(event_type)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// The load-bearing guarantee. `PreferencesChanged` is a MECHANISM here,
    /// not just a notification: the scheduler re-registers the backup cron off
    /// it and the frontend live-applies theme / font / scale, so a preference
    /// written without it silently fails to take effect.
    ///
    /// A same-value write still announces. The event re-applies the setting, so
    /// suppressing it would skip the re-application the user asked for.
    #[tokio::test]
    async fn every_preference_write_announces_including_a_same_value_rewrite() {
        let (pool, db_name) = setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        PreferenceStore::set(&pool, &bus, "theme", "dark", None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "PreferencesChanged").await, 1);

        PreferenceStore::set(&pool, &bus, "theme", "dark", None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "PreferencesChanged").await, 2);

        PreferenceStore::set_for_device(&pool, &bus, "theme", "light", "d1", None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "PreferencesChanged").await, 3);

        assert!(PreferenceStore::delete(&pool, &bus, "theme", None)
            .await
            .unwrap());
        assert_eq!(emitted(&pool, "PreferencesChanged").await, 4);
        assert!(!PreferenceStore::delete(&pool, &bus, "theme", None)
            .await
            .unwrap());
        assert_eq!(
            emitted(&pool, "PreferencesChanged").await,
            4,
            "deleting a key that was already gone changes nothing, so it says nothing"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The silent door is guarded, which is what stops it from becoming the
    /// easy way to skip an announcement. A listed engine-internal key writes
    /// quietly; anything else is refused rather than written.
    #[tokio::test]
    async fn set_silent_writes_internal_keys_and_refuses_real_preferences() {
        let (pool, db_name) = setup_test_db().await;

        PreferenceStore::set_silent(&pool, "vapid_keys", "{}")
            .await
            .expect("a listed internal key writes");
        assert_eq!(
            PreferenceStore::get(&pool, "vapid_keys").await.unwrap(),
            Some("{}".to_string())
        );
        assert_eq!(
            emitted(&pool, "PreferencesChanged").await,
            0,
            "an internal key is not a setting and must not announce"
        );

        let refused = PreferenceStore::set_silent(&pool, "theme", "dark").await;
        assert!(
            refused.is_err(),
            "a user-visible preference must not be writable through the silent door"
        );
        assert_eq!(
            PreferenceStore::get(&pool, "theme").await.unwrap(),
            None,
            "the refusal must happen before the write, not after"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn user_chat_settings_returns_none_when_unset() {
        let (pool, db_name) = setup_test_db().await;
        let (model, effort) = PreferenceStore::user_chat_settings(&pool).await;
        assert_eq!(model, None);
        assert_eq!(effort, None);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn capture_context_defaults_to_false_when_unset() {
        let (pool, db_name) = setup_test_db().await;
        assert!(!PreferenceStore::capture_context(&pool).await.unwrap());
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn capture_context_returns_false_when_disabled() {
        let (pool, db_name) = setup_test_db().await;
        crate::test_support::seed_preference(&pool, PREF_CAPTURE_CONTEXT, "false")
            .await
            .unwrap();
        assert!(!PreferenceStore::capture_context(&pool).await.unwrap());
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn capture_context_returns_true_when_explicitly_true() {
        let (pool, db_name) = setup_test_db().await;
        crate::test_support::seed_preference(&pool, PREF_CAPTURE_CONTEXT, "true")
            .await
            .unwrap();
        assert!(PreferenceStore::capture_context(&pool).await.unwrap());
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    // Pins the row-absent-vs-error distinction in one place: splitting the
    // two assertions across separate tests would let a regression that
    // collapses both branches back into the same `Ok(false)` (the original
    // bug) pass each test individually.
    #[tokio::test]
    async fn capture_context_distinguishes_error_from_unset() {
        let (unset_pool, unset_db) = setup_test_db().await;
        let unset_result = PreferenceStore::capture_context(&unset_pool).await;
        unset_pool.close().await;
        teardown_test_db(&unset_db).await;

        let (err_pool, err_db) = setup_test_db().await;
        err_pool.close().await;
        let err_result = PreferenceStore::capture_context(&err_pool).await;
        teardown_test_db(&err_db).await;

        assert_eq!(
            unset_result.ok(),
            Some(false),
            "row-absent must remain the ships-dark `false` default",
        );
        assert!(
            err_result.is_err(),
            "DB error must propagate as Err so callers can render a failed state, got {:?}",
            err_result,
        );
    }

    #[tokio::test]
    async fn user_chat_settings_returns_stored_values() {
        let (pool, db_name) = setup_test_db().await;
        crate::test_support::seed_preference(&pool, PREF_CHAT_MODEL, "claude-opus-4-7[1m]")
            .await
            .unwrap();
        crate::test_support::seed_preference(&pool, PREF_CHAT_REASONING_EFFORT, "max")
            .await
            .unwrap();
        let (model, effort) = PreferenceStore::user_chat_settings(&pool).await;
        assert_eq!(model.as_deref(), Some("claude-opus-4-7[1m]"));
        assert_eq!(effort.as_deref(), Some("max"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    async fn seed_chat_prefs(pool: &PgPool, model: &str, effort: &str) {
        crate::test_support::seed_preference(pool, PREF_CHAT_MODEL, model)
            .await
            .unwrap();
        crate::test_support::seed_preference(pool, PREF_CHAT_REASONING_EFFORT, effort)
            .await
            .unwrap();
    }

    // The thread-less resolution path (thread_id = None): caller override →
    // account preference, no per-thread lookup.
    #[tokio::test]
    async fn resolve_chat_overrides_falls_back_to_prefs_when_none() {
        let (pool, db_name) = setup_test_db().await;
        seed_chat_prefs(&pool, "claude-opus-4-7[1m]", "xhigh").await;
        let (model, effort) =
            PreferenceStore::resolve_chat_overrides_for_thread(&pool, None, None, None, None).await;
        assert_eq!(model.as_deref(), Some("claude-opus-4-7[1m]"));
        assert_eq!(effort.as_deref(), Some("xhigh"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_chat_overrides_keeps_explicit_caller_values() {
        let (pool, db_name) = setup_test_db().await;
        seed_chat_prefs(&pool, "claude-opus-4-7[1m]", "xhigh").await;
        let (model, effort) = PreferenceStore::resolve_chat_overrides_for_thread(
            &pool,
            None,
            None,
            Some("claude-sonnet-4-6".to_string()),
            Some("medium".to_string()),
        )
        .await;
        assert_eq!(model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(effort.as_deref(), Some("medium"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_chat_overrides_mixes_caller_and_prefs() {
        let (pool, db_name) = setup_test_db().await;
        seed_chat_prefs(&pool, "claude-opus-4-7[1m]", "xhigh").await;
        let (model, effort) = PreferenceStore::resolve_chat_overrides_for_thread(
            &pool,
            None,
            None,
            Some("claude-sonnet-4-6".to_string()),
            None,
        )
        .await;
        assert_eq!(model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(effort.as_deref(), Some("xhigh"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_chat_overrides_returns_none_when_no_caller_no_prefs() {
        let (pool, db_name) = setup_test_db().await;
        let (model, effort) =
            PreferenceStore::resolve_chat_overrides_for_thread(&pool, None, None, None, None).await;
        assert_eq!(model, None);
        assert_eq!(effort, None);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    // --- Per-thread model/effort memory
    // (docs/plans/2026-07-03-per-thread-model-memory.md) ---

    /// Insert a flat `MessageReceived` events row the way `ThreadEvent::to_payload`
    /// serializes it (model/effort at the top level), so the resolution query is
    /// tested against the real payload shape. Returns the event id (`events.id`).
    async fn insert_message_received(
        pool: &PgPool,
        thread_id: Uuid,
        model: Option<&str>,
        effort: Option<&str>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        let mut payload = serde_json::json!({ "text": "hi", "mode": "human" });
        if let Some(m) = model {
            payload["model"] = serde_json::json!(m);
        }
        if let Some(e) = effort {
            payload["reasoning_effort"] = serde_json::json!(e);
        }
        sqlx::query(
            "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
             VALUES ($1, $2, $3, $4, $5, now(), $6)",
        )
        .bind(id)
        .bind("thread")
        .bind(thread_id.to_string())
        .bind("MessageReceived")
        .bind(payload)
        .bind(thread_id)
        .execute(pool)
        .await
        .unwrap();
        id
    }

    #[tokio::test]
    async fn last_thread_chat_settings_returns_most_recent_recorded() {
        let (pool, db_name) = setup_test_db().await;
        let tid = Uuid::new_v4();
        insert_message_received(&pool, tid, Some("model-old"), Some("low")).await;
        insert_message_received(&pool, tid, Some("model-new"), Some("high")).await;
        let (model, effort) = PreferenceStore::last_thread_chat_settings(&pool, tid, None).await;
        assert_eq!(model.as_deref(), Some("model-new"));
        assert_eq!(effort.as_deref(), Some("high"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn last_thread_chat_settings_none_for_thread_without_records() {
        let (pool, db_name) = setup_test_db().await;
        let (model, effort) =
            PreferenceStore::last_thread_chat_settings(&pool, Uuid::new_v4(), None).await;
        assert_eq!(model, None);
        assert_eq!(effort, None);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn last_thread_chat_settings_resolves_each_field_independently() {
        // A newer message carrying only a model must NOT erase an older effort —
        // each field is the latest non-empty value on its own.
        let (pool, db_name) = setup_test_db().await;
        let tid = Uuid::new_v4();
        insert_message_received(&pool, tid, Some("model-a"), Some("high")).await;
        insert_message_received(&pool, tid, Some("model-b"), None).await;
        let (model, effort) = PreferenceStore::last_thread_chat_settings(&pool, tid, None).await;
        assert_eq!(model.as_deref(), Some("model-b"));
        assert_eq!(effort.as_deref(), Some("high"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn last_thread_chat_settings_excludes_in_flight_event() {
        // The current turn's own MessageReceived (pre-emitted upstream) must not
        // be read as its own "previous" value.
        let (pool, db_name) = setup_test_db().await;
        let tid = Uuid::new_v4();
        insert_message_received(&pool, tid, Some("prior-model"), Some("low")).await;
        let current =
            insert_message_received(&pool, tid, Some("current-model"), Some("high")).await;
        let (model, effort) =
            PreferenceStore::last_thread_chat_settings(&pool, tid, Some(current)).await;
        assert_eq!(model.as_deref(), Some("prior-model"));
        assert_eq!(effort.as_deref(), Some("low"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_for_thread_reuses_thread_value_over_preference() {
        let (pool, db_name) = setup_test_db().await;
        seed_chat_prefs(&pool, "pref-model", "pref-effort").await;
        let tid = Uuid::new_v4();
        insert_message_received(&pool, tid, Some("thread-model"), Some("thread-effort")).await;
        let (model, effort) =
            PreferenceStore::resolve_chat_overrides_for_thread(&pool, Some(tid), None, None, None)
                .await;
        assert_eq!(model.as_deref(), Some("thread-model"));
        assert_eq!(effort.as_deref(), Some("thread-effort"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_for_thread_explicit_override_beats_thread_memory() {
        let (pool, db_name) = setup_test_db().await;
        let tid = Uuid::new_v4();
        insert_message_received(&pool, tid, Some("thread-model"), Some("thread-effort")).await;
        // Override the model only → effort still comes from the thread (per field).
        let (model, effort) = PreferenceStore::resolve_chat_overrides_for_thread(
            &pool,
            Some(tid),
            None,
            Some("override-model".to_string()),
            None,
        )
        .await;
        assert_eq!(model.as_deref(), Some("override-model"));
        assert_eq!(effort.as_deref(), Some("thread-effort"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_for_thread_falls_back_to_preference_without_thread_record() {
        let (pool, db_name) = setup_test_db().await;
        seed_chat_prefs(&pool, "pref-model", "pref-effort").await;
        // A brand-new thread (no messages) → account preference.
        let (model, effort) = PreferenceStore::resolve_chat_overrides_for_thread(
            &pool,
            Some(Uuid::new_v4()),
            None,
            None,
            None,
        )
        .await;
        assert_eq!(model.as_deref(), Some("pref-model"));
        assert_eq!(effort.as_deref(), Some("pref-effort"));
        // Thread-less resolve behaves the same (caller override → preference).
        let (m2, e2) =
            PreferenceStore::resolve_chat_overrides_for_thread(&pool, None, None, None, None).await;
        assert_eq!(m2.as_deref(), Some("pref-model"));
        assert_eq!(e2.as_deref(), Some("pref-effort"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
