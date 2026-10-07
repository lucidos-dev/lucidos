use sqlx::PgPool;
use std::collections::HashMap;
use uuid::Uuid;

use crate::core::preference_catalog::{
    self as prefs, Flag, Number, Optional, Pref, PrefDefault, PrefSpec, Text,
};
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

/// Why `url` cannot be the `local_base_url` preference, or `None` when it can.
///
/// The host must be on this machine or the user's own network: loopback, a
/// private or tailnet address, or a name public DNS cannot answer. A public
/// host would read every prompt and write the replies the agent runs as tool
/// calls. Link-local is refused too, since it holds the cloud metadata endpoint.
///
/// Checked where the value is written and again where it is read, so a value
/// stored before the check existed is refused as well. `LUCIDOS_LOCAL_BASE_URL`
/// is not checked: only the operator can set process env.
pub fn local_base_url_rejection(url: &str) -> Option<String> {
    let url = url.trim();
    let refuse = |why: &str| {
        Some(format!(
            "{} '{url}' {why}. Use an address on this machine or your own network, such as {}",
            prefs::LOCAL_BASE_URL.key(),
            prefs::LOCAL_BASE_URL.default_text()
        ))
    };
    let Ok(parsed) = reqwest::Url::parse(url) else {
        return refuse("is not a URL");
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        return refuse("must use http or https");
    }
    let host = parsed.host_str().unwrap_or_default().to_ascii_lowercase();
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let local = match host.parse::<std::net::IpAddr>() {
        Ok(ip) => ip_is_on_own_network(ip),
        Err(_) => name_is_on_own_network(host),
    };
    if local {
        None
    } else {
        refuse("is not on this machine or a private network")
    }
}

/// Loopback, RFC 1918, the tailnet range, IPv6 unique-local, or the unspecified
/// address, which connects to this machine.
fn ip_is_on_own_network(ip: std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    let v4 = |ip: std::net::Ipv4Addr| {
        ip.is_loopback()
            || ip.is_private()
            || lucidos_tailscale::is_tailnet_addr(ip)
            || ip.is_unspecified()
    };
    match ip {
        IpAddr::V4(ip) => v4(ip),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(mapped) => v4(mapped),
            None => ip.is_loopback() || ip.is_unspecified() || ip.segments()[0] & 0xfe00 == 0xfc00,
        },
    }
}

/// `localhost`, or a name under a suffix reserved for private use, so no public
/// DNS answer can point it elsewhere: `.localhost` (RFC 6761), `.local` (mDNS,
/// RFC 6762) and `.home.arpa` (RFC 8375). Not `.internal`: cloud hosts answer
/// `metadata.google.internal` with the link-local metadata address.
fn name_is_on_own_network(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host);
    host == "localhost"
        || [".localhost", ".local", ".home.arpa"]
            .iter()
            .any(|suffix| host.ends_with(suffix))
}

/// Store for managing user preferences in the database.
///
/// **Announcing is the default, and the silent door is guarded.**
/// [`Self::set`], [`Self::set_for_device`] and [`Self::delete`] emit
/// `PreferencesChanged` from inside the write path; the raw row writes are
/// private to this module. [`Self::set_silent`] exists for the handful of keys
/// that are engine bookkeeping rather than settings, and it REJECTS any key
/// that is not a `PrefAccess::Engine` catalog spec, so it cannot be used to
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

/// The *model selection* a turn resolved to, owned.
///
/// The same three fields as the borrowed `llm::ModelSelection` that goes to the
/// wire. This form crosses an await and is stamped on a starter event. So it
/// holds `String`s, and names its provider by the registry's own value.
///
/// Every field is resolved INDEPENDENTLY, so a turn can take its model from the
/// thread's memory and its effort from the account preference.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ResolvedModelSelection {
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    /// The backend to serve it. `None` lets the model's own *preferred
    /// provider* decide, then its first configured route.
    pub provider: Option<String>,
}

impl ResolvedModelSelection {
    /// Borrow this into the wire-side selection the provider trait takes.
    pub fn as_selection(&self) -> crate::llm::ModelSelection<'_> {
        crate::llm::ModelSelection {
            model: self.model.as_deref(),
            reasoning_effort: self.reasoning_effort.as_deref(),
            provider: self
                .provider
                .as_deref()
                .and_then(crate::llm::model_registry::ProviderKind::from_name),
            // A turn: never capped, so a long valid stream runs to its end.
            attempt_timeout: None,
        }
    }
}

impl PreferenceStore {
    /// Write a global preference row with no announcement.
    ///
    /// Tests only. The announcing [`Self::set`] needs an `EventBus`, and a test
    /// arranging a precondition has nothing to announce it to.
    #[cfg(test)]
    pub async fn set_row_for_test(
        pool: &PgPool,
        key: &str,
        value: &str,
    ) -> Result<(), sqlx::Error> {
        Self::set_row(pool, key, value).await
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

    /// Get a global preference by key (device_id IS NULL).
    ///
    /// **Private on purpose**: a reader goes through a typed [`Pref`] handle,
    /// which resolves an unset key to its catalog default.
    async fn get(pool: &PgPool, key: &str) -> Result<Option<String>, sqlx::Error> {
        let result = sqlx::query_scalar::<_, String>(
            "SELECT value FROM preferences WHERE key = $1 AND device_id IS NULL",
        )
        .bind(key)
        .fetch_optional(pool)
        .await?;

        Ok(result)
    }

    /// Get a preference for a specific device, falling back to the global value.
    /// Private for the same reason as [`Self::get`].
    async fn get_for_device(
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
    /// **Rejects any key that is not a `PrefAccess::Engine` catalog spec**
    /// (`preference_catalog::is_silent_key`), which is what stops this from
    /// becoming the easy way to skip an announcement. Reach for [`Self::set`]
    /// for anything a user can see. A genuinely internal key takes an `Engine`
    /// spec with its reason.
    pub async fn set_silent(pool: &PgPool, key: &str, value: &str) -> Result<(), sqlx::Error> {
        if !crate::core::preference_catalog::is_silent_key(key) {
            // A protocol violation by the caller, not a database failure. sqlx's
            // error type has no variant for that, so `Protocol` carries the
            // message: the alternative is a second error type for one call site.
            return Err(sqlx::Error::Protocol(format!(
                "'{key}' is not an engine-internal preference key, so it must be written through \
                 PreferenceStore::set (which announces PreferencesChanged). Give it a \
                 PrefAccess::Engine catalog spec with a reason if it really is internal state."
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

    /// The pair of numbers that schedules the self-curated context sweep.
    ///
    /// Returns the schedule itself rather than a pair of bare `usize`. The two
    /// numbers are adjacent and interchangeable to the compiler. A
    /// transposition at the call site would run the whole workspace on the
    /// wrong schedule and say nothing.
    pub(crate) async fn self_curated_context_schedule(
        pool: &PgPool,
    ) -> crate::engine::SweepSchedule {
        crate::engine::SweepSchedule::new(
            prefs::SELF_CURATED_CONTEXT_EXPIRE_AFTER_ROUNDS
                .read(pool)
                .await
                .round() as usize,
            prefs::SELF_CURATED_CONTEXT_SWEEP_EVERY_ROUNDS
                .read(pool)
                .await
                .round() as usize,
        )
    }

    /// The per-turn tool-call cap for one turn. Total, because it is the
    /// loop's runaway backstop: an absent, unparseable or unreadable row runs
    /// the turn at the catalog default rather than refusing it.
    pub async fn max_tool_calls(pool: &PgPool) -> usize {
        prefs::MAX_TOOL_CALLS.read(pool).await as usize
    }

    /// The account's chat model and reasoning effort, for code paths that start
    /// a chat without an explicit user request (spawn_thread, process_trigger).
    ///
    /// The model is `None` while unset, because the provider layers
    /// `LUCIDOS_MODEL` over the catalog default at boot. The effort always
    /// resolves, to the catalog default while unset, so every route runs a
    /// fresh thread at the same effort.
    pub async fn user_chat_settings(pool: &PgPool) -> (Option<String>, String) {
        (
            prefs::CHAT_MODEL.stored(pool).await,
            prefs::CHAT_REASONING_EFFORT.read(pool).await,
        )
    }

    /// Read the model + reasoning effort a thread last ran with — the values
    /// stamped on the thread's most recent starter event that carried them.
    /// This is the per-thread memory: a follow-up with no explicit override
    /// reuses these instead of snapping back to the account default. Resolved
    /// per field independently (a legacy message with only one set still
    /// contributes that field), newest-first by `sequence`.
    ///
    /// Both starter kinds count. A chat turn starts with `MessageReceived`; a
    /// **trigger** fire starts with `TriggerStarted` and emits no
    /// `MessageReceived` at all, so reading only the former would make a
    /// trigger thread report the account default however the trigger was
    /// pinned, and a human follow-up there would silently switch models.
    ///
    /// `exclude_event_id` drops the in-flight turn's own `MessageReceived` when
    /// it was pre-emitted upstream (`pre_emitted_origin` == `events.id`), so we
    /// never read the current turn as its own "previous" value. DB errors are
    /// logged and treated as "no record" — callers fall through to preferences.
    ///
    /// The provider is remembered WITH its model. It is the newest one stamped
    /// beside the model this turn runs on: `model_override` when the caller
    /// named one, else the remembered model. So a thread switched from Opus to
    /// Sonnet drops the backend picked for Opus, and Sonnet's own row decides.
    pub async fn last_thread_chat_settings(
        pool: &PgPool,
        thread_id: Uuid,
        exclude_event_id: Option<Uuid>,
        model_override: Option<&str>,
    ) -> ResolvedModelSelection {
        // Starter thread-event payloads are flat (see `ThreadEvent::to_payload`),
        // so `payload->>'model'` reads the field directly on both variants.
        // `aggregate_id` is text; bind the thread id as its string.
        //
        // One sub-select per field, because each is remembered independently: a
        // thread can carry a model from one turn and an effort from another.
        let row = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>)>(
            r#"
            SELECT
              (SELECT payload->>'model'
                 FROM events
                WHERE aggregate_id = $1
                  AND event_type IN ('MessageReceived', 'TriggerStarted')
                  AND payload->>'model' IS NOT NULL
                  AND payload->>'model' <> ''
                  AND ($2::uuid IS NULL OR id <> $2)
                ORDER BY sequence DESC
                LIMIT 1) AS model,
              (SELECT payload->>'reasoning_effort'
                 FROM events
                WHERE aggregate_id = $1
                  AND event_type IN ('MessageReceived', 'TriggerStarted')
                  AND payload->>'reasoning_effort' IS NOT NULL
                  AND payload->>'reasoning_effort' <> ''
                  AND ($2::uuid IS NULL OR id <> $2)
                ORDER BY sequence DESC
                LIMIT 1) AS effort,
              (SELECT payload->>'provider'
                 FROM events
                WHERE aggregate_id = $1
                  AND event_type IN ('MessageReceived', 'TriggerStarted')
                  AND payload->>'provider' IS NOT NULL
                  AND payload->>'provider' <> ''
                  AND payload->>'model' = COALESCE($3, (
                        SELECT payload->>'model'
                          FROM events
                         WHERE aggregate_id = $1
                           AND event_type IN ('MessageReceived', 'TriggerStarted')
                           AND payload->>'model' IS NOT NULL
                           AND payload->>'model' <> ''
                           AND ($2::uuid IS NULL OR id <> $2)
                         ORDER BY sequence DESC
                         LIMIT 1))
                  AND ($2::uuid IS NULL OR id <> $2)
                ORDER BY sequence DESC
                LIMIT 1) AS provider
            "#,
        )
        .bind(thread_id.to_string())
        .bind(exclude_event_id)
        .bind(model_override)
        .fetch_one(pool)
        .await;
        match row {
            Ok((model, reasoning_effort, provider)) => ResolvedModelSelection {
                model,
                reasoning_effort,
                provider,
            },
            Err(e) => {
                log!(
                    "[Preferences] Failed to read last thread chat settings for {}: {}",
                    thread_id,
                    e
                );
                ResolvedModelSelection::default()
            }
        }
    }

    /// Resolve the *model selection* stamped on a chat exchange when the caller
    /// did not fully specify it, honoring per-thread memory.
    ///
    /// Order per field: explicit caller override → the thread's last recorded
    /// value → the user's account chat preference. Each DB read it does not
    /// need is skipped.
    ///
    /// The provider has no account preference of its own, deliberately. It is
    /// remembered per MODEL, on the registry row, because a global one means
    /// nothing across families that share no backend: picking Grok on xAI would
    /// otherwise move Opus off Anthropic. So an unpinned turn leaves it `None`
    /// and the row decides.
    pub async fn resolve_chat_overrides_for_thread(
        pool: &PgPool,
        thread_id: Option<Uuid>,
        exclude_event_id: Option<Uuid>,
        overrides: ResolvedModelSelection,
    ) -> ResolvedModelSelection {
        let ResolvedModelSelection {
            model: model_override,
            reasoning_effort: effort_override,
            provider: provider_override,
        } = overrides;
        if model_override.is_some() && effort_override.is_some() && provider_override.is_some() {
            return ResolvedModelSelection {
                model: model_override,
                reasoning_effort: effort_override,
                provider: provider_override,
            };
        }
        // Per-thread memory: reuse what this thread last ran with. Only worth a
        // query for a follow-up (a new thread has no prior message).
        let remembered = match thread_id {
            Some(tid) => {
                Self::last_thread_chat_settings(
                    pool,
                    tid,
                    exclude_event_id,
                    model_override.as_deref(),
                )
                .await
            }
            None => ResolvedModelSelection::default(),
        };
        let model = model_override.or(remembered.model);
        let effort = effort_override.or(remembered.reasoning_effort);
        let provider = provider_override.or(remembered.provider);
        if model.is_some() && effort.is_some() {
            return ResolvedModelSelection {
                model,
                reasoning_effort: effort,
                provider,
            };
        }
        let (pref_model, pref_effort) = Self::user_chat_settings(pool).await;
        ResolvedModelSelection {
            model: model.or(pref_model),
            reasoning_effort: effort.or(Some(pref_effort)),
            provider,
        }
    }
}

/// Logs a failed read, for a handle read that then falls back to its default.
fn log_read_failure(key: &str, e: &sqlx::Error) {
    log!(
        "[Preferences] Failed to read {}: {}. Using its default",
        key,
        e
    );
}

/// A stored value trimmed, with a blank one read as unset.
fn nonblank(stored: Option<String>) -> Option<String> {
    stored
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

impl<K> Pref<K> {
    /// The stored global row, untouched. `Ok(None)` when no row exists.
    pub async fn try_stored(&self, pool: &PgPool) -> Result<Option<String>, sqlx::Error> {
        PreferenceStore::get(pool, self.key()).await
    }

    /// [`Self::try_stored`], with a failed read logged and read as unset.
    pub async fn stored(&self, pool: &PgPool) -> Option<String> {
        self.try_stored(pool).await.unwrap_or_else(|e| {
            log_read_failure(self.key(), &e);
            None
        })
    }

    /// The stored row for `device_id`, else the global one.
    pub async fn try_stored_for_device(
        &self,
        pool: &PgPool,
        device_id: &str,
    ) -> Result<Option<String>, sqlx::Error> {
        PreferenceStore::get_for_device(pool, self.key(), device_id).await
    }
}

impl Pref<Flag> {
    /// `stored` read as a switch, or the default when absent or neither on nor
    /// off.
    pub fn resolve(&self, stored: Option<&str>) -> bool {
        stored
            .and_then(prefs::parse_flag)
            .unwrap_or_else(|| self.default_flag())
    }

    pub async fn try_read(&self, pool: &PgPool) -> Result<bool, sqlx::Error> {
        Ok(self.resolve(self.try_stored(pool).await?.as_deref()))
    }

    /// Total: a failed read is logged and resolves to the default.
    pub async fn read(&self, pool: &PgPool) -> bool {
        self.resolve(self.stored(pool).await.as_deref())
    }
}

impl Pref<Number> {
    /// `stored` parsed, or the default when it is absent, not a number, or
    /// outside the spec's bounds. An out-of-range value is not a setting at
    /// all, so it reads as unset like any other value the catalog refuses.
    pub fn resolve(&self, stored: Option<&str>) -> f64 {
        let (min, max) = self.bounds();
        stored
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|n| (min..=max).contains(n))
            .unwrap_or_else(|| self.default_number())
    }

    pub async fn try_read(&self, pool: &PgPool) -> Result<f64, sqlx::Error> {
        Ok(self.resolve(self.try_stored(pool).await?.as_deref()))
    }

    /// Total: a failed read is logged and resolves to the default.
    pub async fn read(&self, pool: &PgPool) -> f64 {
        self.resolve(self.stored(pool).await.as_deref())
    }
}

/// `spec`'s value: the stored row when it is valid, else its default,
/// following an inherited default to the spec that holds the value.
async fn resolve_text(pool: &PgPool, spec: &PrefSpec) -> Result<String, sqlx::Error> {
    let stored = nonblank(PreferenceStore::get(pool, spec.key).await?)
        .filter(|v| prefs::validate(spec, v).is_ok());
    if let Some(value) = stored {
        return Ok(value);
    }
    match spec.default {
        PrefDefault::Value(value) => Ok(value.to_string()),
        PrefDefault::Inherits(other) => Box::pin(resolve_text(pool, other)).await,
        PrefDefault::Unset(_) => {
            unreachable!("a text handle's default chain ends in a value")
        }
    }
}

impl Pref<Text> {
    /// `stored` when it is a valid value, else the default. Only for a spec
    /// whose default is a value: an inherited default needs the database, so
    /// it goes through [`Self::try_read`].
    pub fn resolve(&self, stored: Option<&str>) -> String {
        stored
            .map(str::trim)
            .filter(|v| !v.is_empty() && prefs::validate(&self.spec, v).is_ok())
            .unwrap_or_else(|| self.default_text())
            .to_string()
    }

    pub async fn try_read(&self, pool: &PgPool) -> Result<String, sqlx::Error> {
        resolve_text(pool, &self.spec).await
    }

    /// Total: a failed read is logged and resolves to the default.
    pub async fn read(&self, pool: &PgPool) -> String {
        self.try_read(pool).await.unwrap_or_else(|e| {
            log_read_failure(self.key(), &e);
            self.default_text().to_string()
        })
    }

    /// The first value stored along the inheritance chain, without the
    /// chain's final default. For a reader whose unset case means something
    /// other than that default: an unset background model resolves against
    /// the configured providers (`engine::aux_purpose`).
    pub async fn stored_or_inherited(&self, pool: &PgPool) -> Option<String> {
        let mut spec = &self.spec;
        loop {
            let stored = PreferenceStore::get(pool, spec.key)
                .await
                .unwrap_or_else(|e| {
                    log_read_failure(spec.key, &e);
                    None
                });
            if let Some(value) = nonblank(stored) {
                return Some(value);
            }
            match spec.default {
                PrefDefault::Inherits(other) => spec = other,
                PrefDefault::Value(_) | PrefDefault::Unset(_) => return None,
            }
        }
    }

    /// The device's value, else the global one, else the default.
    pub async fn read_for_device(&self, pool: &PgPool, device_id: &str) -> String {
        let stored = self
            .try_stored_for_device(pool, device_id)
            .await
            .unwrap_or_else(|e| {
                log_read_failure(self.key(), &e);
                None
            });
        self.resolve(stored.as_deref())
    }
}

impl Pref<Optional> {
    /// The stored value trimmed, `None` while unset or blank.
    pub async fn try_read(&self, pool: &PgPool) -> Result<Option<String>, sqlx::Error> {
        Ok(nonblank(self.try_stored(pool).await?))
    }

    /// Total: a failed read is logged and reads as unset.
    pub async fn read(&self, pool: &PgPool) -> Option<String> {
        nonblank(self.stored(pool).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{setup_test_db, teardown_test_db};

    /// The real local setups keep working: Ollama and LM Studio on loopback, a
    /// LAN box on an RFC 1918 address, a tailnet box, and a private-use name.
    #[test]
    fn a_local_base_url_on_this_machine_or_own_network_is_accepted() {
        for url in [
            prefs::LOCAL_BASE_URL.default_text(),
            "http://127.0.0.1:1234/v1",
            "http://[::1]:8080/v1",
            "http://0.0.0.0:11434/v1",
            "http://192.168.1.20:11434/v1",
            "http://10.0.0.5:8000/v1",
            "https://172.16.4.2/v1",
            "http://100.101.102.103:11434/v1",
            "http://[fd12:3456::1]:11434/v1",
            "http://gpu-box.local:11434/v1",
            "http://ollama.home.arpa/v1",
            "  http://localhost:11434/v1  ",
        ] {
            assert_eq!(local_base_url_rejection(url), None, "{url}");
        }
    }

    /// A host the user's network does not own would receive every prompt.
    /// Link-local is refused for the metadata endpoint it holds.
    #[test]
    fn a_local_base_url_off_the_own_network_is_refused() {
        for url in [
            "https://attacker.example/v1",
            "http://8.8.8.8/v1",
            "http://169.254.169.254/latest",
            "http://metadata.google.internal/computeMetadata/v1",
            "http://[fe80::1]/v1",
            "http://[2001:db8::1]/v1",
            "http://[::ffff:8.8.8.8]/v1",
            "http://localhost.attacker.example/v1",
            "http://gpubox:11434/v1",
            "http://a.local@attacker.example/v1",
            "ftp://localhost/v1",
            "not a url",
        ] {
            let reason = local_base_url_rejection(url).unwrap_or_else(|| panic!("{url} passed"));
            assert!(reason.contains(prefs::LOCAL_BASE_URL.key()), "{reason}");
        }
    }

    /// The override triple a caller passes in. The cases below only ever pin
    /// a model or an effort, so the provider stays unset.
    fn overrides(model: Option<&str>, effort: Option<&str>) -> ResolvedModelSelection {
        ResolvedModelSelection {
            model: model.map(str::to_string),
            reasoning_effort: effort.map(str::to_string),
            provider: None,
        }
    }

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

        PreferenceStore::set(&pool, &bus, prefs::THEME_MODE.key(), "dark", None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "PreferencesChanged").await, 1);

        PreferenceStore::set(&pool, &bus, prefs::THEME_MODE.key(), "dark", None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "PreferencesChanged").await, 2);

        PreferenceStore::set_for_device(&pool, &bus, prefs::THEME_MODE.key(), "light", "d1", None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "PreferencesChanged").await, 3);

        assert!(
            PreferenceStore::delete(&pool, &bus, prefs::THEME_MODE.key(), None)
                .await
                .unwrap()
        );
        assert_eq!(emitted(&pool, "PreferencesChanged").await, 4);
        assert!(
            !PreferenceStore::delete(&pool, &bus, prefs::THEME_MODE.key(), None)
                .await
                .unwrap()
        );
        assert_eq!(
            emitted(&pool, "PreferencesChanged").await,
            4,
            "deleting a key that was already gone changes nothing, so it says nothing"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The theme rename moves every stored appearance choice to its new key,
    /// global and per device, so no device paints a default after the upgrade.
    ///
    /// The migrations already ran on this database, so the test seeds the old
    /// keys and runs the files again, in order. Each is written to re-run.
    #[tokio::test]
    async fn the_theme_rename_keeps_every_stored_choice() {
        const THEME_TO_THEME_MODE: &str = include_str!(
            "../../migrations/20260928054901_rename_theme_preference_to_theme_mode.sql"
        );
        const LOOK_TO_THEME: &str =
            include_str!("../../migrations/20260928061424_rename_look_preference_to_theme.sql");
        let (pool, db_name) = setup_test_db().await;
        sqlx::query(
            "INSERT INTO preferences (key, value, device_id) VALUES \
             ('theme', 'dark', NULL), \
             ('theme', 'light', 'd1'), \
             ('theme', 'dark', 'd2'), \
             ('theme-mode', 'system', 'd2'), \
             ('look', 'mono', NULL), \
             ('look', 'nord', 'd1'), \
             ('look-effects', 'reduce', 'd1'), \
             ('font-family', 'look', 'd1'), \
             ('font-family', 'inter', 'd2')",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(THEME_TO_THEME_MODE)
            .execute(&pool)
            .await
            .expect("the theme mode rename re-runs");
        sqlx::raw_sql(LOOK_TO_THEME)
            .execute(&pool)
            .await
            .expect("the theme rename re-runs");

        let value = |key: &'static str, device: Option<&'static str>| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, String>(
                    "SELECT value FROM preferences \
                     WHERE key = $1 AND COALESCE(device_id, '') = COALESCE($2, '')",
                )
                .bind(key)
                .bind(device)
                .fetch_optional(&pool)
                .await
                .unwrap()
            }
        };
        assert_eq!(
            value(prefs::THEME_MODE.key(), None).await.as_deref(),
            Some("dark")
        );
        assert_eq!(
            value(prefs::THEME_MODE.key(), Some("d1")).await.as_deref(),
            Some("light")
        );
        assert_eq!(
            value(prefs::THEME_MODE.key(), Some("d2")).await.as_deref(),
            Some("system"),
            "a row already under the new key wins"
        );
        assert_eq!(value("theme", None).await.as_deref(), Some("mono"));
        assert_eq!(value("theme", Some("d1")).await.as_deref(), Some("nord"));
        assert_eq!(
            value("theme", Some("d2")).await,
            None,
            "no mode lands under the new key"
        );
        assert_eq!(
            value(prefs::THEME_EFFECTS.key(), Some("d1"))
                .await
                .as_deref(),
            Some("reduce")
        );
        assert_eq!(
            value(prefs::FONT_FAMILY.key(), Some("d1")).await.as_deref(),
            Some("theme")
        );
        assert_eq!(
            value(prefs::FONT_FAMILY.key(), Some("d2")).await.as_deref(),
            Some("inter")
        );
        let left: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM preferences WHERE key IN ('look', 'look-effects')",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(left, 0, "nothing is left under an old key");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The dynamic bars rename keeps every stored choice with the polarity
    /// flipped, so a device that had unpinned its header still hides it.
    #[tokio::test]
    async fn the_dynamic_bars_rename_flips_every_stored_choice() {
        const STICKY_TO_DYNAMIC_BARS: &str = include_str!(
            "../../migrations/20260930060747_rename_mobile_header_sticky_to_mobile_dynamic_bars.sql"
        );
        let (pool, db_name) = setup_test_db().await;
        sqlx::query(
            "INSERT INTO preferences (key, value, device_id) VALUES \
             ('mobile_header_sticky', 'false', NULL), \
             ('mobile_header_sticky', 'true', 'd1'), \
             ('mobile_header_sticky', 'false', 'd2'), \
             ('mobile_dynamic_bars', 'false', 'd2')",
        )
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(STICKY_TO_DYNAMIC_BARS)
            .execute(&pool)
            .await
            .expect("the dynamic bars rename re-runs");

        let value = |device: Option<&'static str>| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, String>(
                    "SELECT value FROM preferences WHERE key = 'mobile_dynamic_bars' \
                     AND COALESCE(device_id, '') = COALESCE($1, '')",
                )
                .bind(device)
                .fetch_optional(&pool)
                .await
                .unwrap()
            }
        };
        assert_eq!(value(None).await.as_deref(), Some("true"));
        assert_eq!(value(Some("d1")).await.as_deref(), Some("false"));
        assert_eq!(
            value(Some("d2")).await.as_deref(),
            Some("false"),
            "a row already under the new key wins"
        );
        let left: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM preferences WHERE key = 'mobile_header_sticky'",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(left, 0, "nothing is left under the old key");

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// The silent door is guarded, which is what stops it from becoming the
    /// easy way to skip an announcement. A listed engine-internal key writes
    /// quietly; anything else is refused rather than written.
    #[tokio::test]
    async fn set_silent_writes_internal_keys_and_refuses_real_preferences() {
        let (pool, db_name) = setup_test_db().await;

        PreferenceStore::set_silent(&pool, prefs::VAPID_KEYS.key(), "{}")
            .await
            .expect("a listed internal key writes");
        assert_eq!(
            PreferenceStore::get(&pool, prefs::VAPID_KEYS.key())
                .await
                .unwrap(),
            Some("{}".to_string())
        );
        assert_eq!(
            emitted(&pool, "PreferencesChanged").await,
            0,
            "an internal key is not a setting and must not announce"
        );

        let refused = PreferenceStore::set_silent(&pool, prefs::THEME_MODE.key(), "dark").await;
        assert!(
            refused.is_err(),
            "a user-visible preference must not be writable through the silent door"
        );
        assert_eq!(
            PreferenceStore::get(&pool, prefs::THEME_MODE.key())
                .await
                .unwrap(),
            None,
            "the refusal must happen before the write, not after"
        );

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// On an empty table every read is the catalog's default, and nothing
    /// else: a text handle follows an inherited default to its source, and an
    /// optional handle reads as unset.
    #[tokio::test]
    async fn every_handle_reads_its_catalog_default_on_an_empty_table() {
        use crate::core::preference_catalog::{PrefValue, CATALOG};
        let (pool, db_name) = setup_test_db().await;
        for spec in CATALOG {
            let text_shaped = !matches!(spec.value, PrefValue::Bool | PrefValue::Number { .. });
            match spec.default {
                PrefDefault::Unset(_) => {
                    assert_eq!(PreferenceStore::get(&pool, spec.key).await.unwrap(), None);
                }
                PrefDefault::Value(_) | PrefDefault::Inherits(_) if text_shaped => {
                    assert_eq!(
                        Some(resolve_text(&pool, spec).await.unwrap().as_str()),
                        spec.default_value(),
                        "{}",
                        spec.key
                    );
                }
                PrefDefault::Value(_) | PrefDefault::Inherits(_) => {}
            }
        }
        assert_eq!(
            prefs::COMMAND_GUARD.read(&pool).await,
            prefs::COMMAND_GUARD.default_flag()
        );
        assert_eq!(
            prefs::PROVIDER_ENABLED_VERTEX.read(&pool).await,
            prefs::PROVIDER_ENABLED_VERTEX.default_flag()
        );
        assert_eq!(
            prefs::BACKUP_RETENTION.read(&pool).await,
            prefs::BACKUP_RETENTION.default_number()
        );
        assert_eq!(prefs::LANGUAGE.read(&pool).await, None);
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn user_chat_settings_resolves_an_unset_effort_to_the_catalog_default() {
        let (pool, db_name) = setup_test_db().await;
        let (model, effort) = PreferenceStore::user_chat_settings(&pool).await;
        assert_eq!(model, None);
        assert_eq!(effort, prefs::CHAT_REASONING_EFFORT.default_text());
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn capture_context_defaults_to_false_when_unset() {
        let (pool, db_name) = setup_test_db().await;
        assert!(!prefs::CAPTURE_CONTEXT.try_read(&pool).await.unwrap());
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn capture_context_returns_false_when_disabled() {
        let (pool, db_name) = setup_test_db().await;
        crate::test_support::seed_preference(&pool, prefs::CAPTURE_CONTEXT.key(), "false")
            .await
            .unwrap();
        assert!(!prefs::CAPTURE_CONTEXT.try_read(&pool).await.unwrap());
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn capture_context_returns_true_when_explicitly_true() {
        let (pool, db_name) = setup_test_db().await;
        crate::test_support::seed_preference(&pool, prefs::CAPTURE_CONTEXT.key(), "true")
            .await
            .unwrap();
        assert!(prefs::CAPTURE_CONTEXT.try_read(&pool).await.unwrap());
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
        let unset_result = prefs::CAPTURE_CONTEXT.try_read(&unset_pool).await;
        unset_pool.close().await;
        teardown_test_db(&unset_db).await;

        let (err_pool, err_db) = setup_test_db().await;
        err_pool.close().await;
        let err_result = prefs::CAPTURE_CONTEXT.try_read(&err_pool).await;
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

    // --- Per-turn tool-call cap
    // (docs/plans/2026-08-03-configurable-max-tool-calls.md) ---

    /// One case per resolution branch, in one test: the reader is total, so the
    /// property worth pinning is that EVERY input lands on a usable cap. Split
    /// across separate tests, a regression that collapsed the branches into a
    /// single `unwrap_or(default)` (losing the floor, or losing large values)
    /// would still pass most of them.
    #[tokio::test]
    async fn max_tool_calls_resolves_every_input_to_a_usable_cap() {
        let (pool, db_name) = setup_test_db().await;
        let default = prefs::MAX_TOOL_CALLS.default_number() as usize;

        assert_eq!(
            PreferenceStore::max_tool_calls(&pool).await,
            prefs::MAX_TOOL_CALLS.default_number() as usize,
            "an untouched workspace runs at the catalog default"
        );

        for (stored, expected, why) in [
            ("2000", 2000, "a plain value is honored"),
            (" 750 ", 750, "surrounding whitespace is tolerated"),
            (
                "1000000",
                1_000_000,
                "a huge cap inside the catalog bound is the user's call to make",
            ),
            (
                "0",
                default,
                "0 is below the catalog floor, so it reads as unset rather than firing the backstop before the first LLM call",
            ),
            (
                "-5",
                default,
                "a negative is not a cap at all, so it falls back rather than becoming the floor",
            ),
            ("abc", default, "garbage falls back"),
            ("", default, "an empty value falls back"),
        ] {
            crate::test_support::seed_preference(&pool, prefs::MAX_TOOL_CALLS.key(), stored)
                .await
                .unwrap();
            assert_eq!(
                PreferenceStore::max_tool_calls(&pool).await,
                expected,
                "stored {:?}: {}",
                stored,
                why
            );
        }

        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// A DB error must not fail the turn. Unlike `capture_context`, which
    /// propagates `Err` so a caller can render a failed state, this read has no
    /// caller that could do anything useful with the error: the loop needs a
    /// number to run at all.
    #[tokio::test]
    async fn max_tool_calls_falls_back_to_the_default_on_a_db_error() {
        let (pool, db_name) = setup_test_db().await;
        pool.close().await;

        assert_eq!(
            PreferenceStore::max_tool_calls(&pool).await,
            prefs::MAX_TOOL_CALLS.default_number() as usize
        );

        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn user_chat_settings_returns_stored_values() {
        let (pool, db_name) = setup_test_db().await;
        crate::test_support::seed_preference(&pool, prefs::CHAT_MODEL.key(), "claude-opus-4-7[1m]")
            .await
            .unwrap();
        crate::test_support::seed_preference(&pool, prefs::CHAT_REASONING_EFFORT.key(), "max")
            .await
            .unwrap();
        let (model, effort) = PreferenceStore::user_chat_settings(&pool).await;
        assert_eq!(model.as_deref(), Some("claude-opus-4-7[1m]"));
        assert_eq!(effort, "max");
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    async fn seed_chat_prefs(pool: &PgPool, model: &str, effort: &str) {
        crate::test_support::seed_preference(pool, prefs::CHAT_MODEL.key(), model)
            .await
            .unwrap();
        crate::test_support::seed_preference(pool, prefs::CHAT_REASONING_EFFORT.key(), effort)
            .await
            .unwrap();
    }

    // The thread-less resolution path (thread_id = None): caller override →
    // account preference, no per-thread lookup.
    #[tokio::test]
    async fn resolve_chat_overrides_falls_back_to_prefs_when_none() {
        let (pool, db_name) = setup_test_db().await;
        seed_chat_prefs(&pool, "claude-opus-4-7[1m]", "xhigh").await;
        let resolved = PreferenceStore::resolve_chat_overrides_for_thread(
            &pool,
            None,
            None,
            overrides(None, None),
        )
        .await;
        assert_eq!(resolved.model.as_deref(), Some("claude-opus-4-7[1m]"));
        assert_eq!(resolved.reasoning_effort.as_deref(), Some("xhigh"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_chat_overrides_keeps_explicit_caller_values() {
        let (pool, db_name) = setup_test_db().await;
        seed_chat_prefs(&pool, "claude-opus-4-7[1m]", "xhigh").await;
        let resolved = PreferenceStore::resolve_chat_overrides_for_thread(
            &pool,
            None,
            None,
            overrides(Some("claude-sonnet-4-6"), Some("medium")),
        )
        .await;
        assert_eq!(resolved.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(resolved.reasoning_effort.as_deref(), Some("medium"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_chat_overrides_mixes_caller_and_prefs() {
        let (pool, db_name) = setup_test_db().await;
        seed_chat_prefs(&pool, "claude-opus-4-7[1m]", "xhigh").await;
        let resolved = PreferenceStore::resolve_chat_overrides_for_thread(
            &pool,
            None,
            None,
            overrides(Some("claude-sonnet-4-6"), None),
        )
        .await;
        assert_eq!(resolved.model.as_deref(), Some("claude-sonnet-4-6"));
        assert_eq!(resolved.reasoning_effort.as_deref(), Some("xhigh"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_chat_overrides_falls_to_the_catalog_effort_when_nothing_is_set() {
        let (pool, db_name) = setup_test_db().await;
        let resolved = PreferenceStore::resolve_chat_overrides_for_thread(
            &pool,
            None,
            None,
            overrides(None, None),
        )
        .await;
        assert_eq!(resolved.model, None);
        assert_eq!(
            resolved.reasoning_effort.as_deref(),
            Some(prefs::CHAT_REASONING_EFFORT.default_text())
        );
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
        insert_starter(pool, thread_id, model, effort, None).await
    }

    /// A starter that also carries the backend the turn was pinned to.
    async fn insert_starter(
        pool: &PgPool,
        thread_id: Uuid,
        model: Option<&str>,
        effort: Option<&str>,
        provider: Option<&str>,
    ) -> Uuid {
        let id = Uuid::new_v4();
        let mut payload = serde_json::json!({ "text": "hi", "mode": "human" });
        if let Some(p) = provider {
            payload["provider"] = serde_json::json!(p);
        }
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
        let resolved = PreferenceStore::last_thread_chat_settings(&pool, tid, None, None).await;
        assert_eq!(resolved.model.as_deref(), Some("model-new"));
        assert_eq!(resolved.reasoning_effort.as_deref(), Some("high"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn last_thread_chat_settings_none_for_thread_without_records() {
        let (pool, db_name) = setup_test_db().await;
        let resolved =
            PreferenceStore::last_thread_chat_settings(&pool, Uuid::new_v4(), None, None).await;
        assert_eq!(resolved.model, None);
        assert_eq!(resolved.reasoning_effort, None);
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
        let resolved = PreferenceStore::last_thread_chat_settings(&pool, tid, None, None).await;
        assert_eq!(resolved.model.as_deref(), Some("model-b"));
        assert_eq!(resolved.reasoning_effort.as_deref(), Some("high"));
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
        let resolved =
            PreferenceStore::last_thread_chat_settings(&pool, tid, Some(current), None).await;
        assert_eq!(resolved.model.as_deref(), Some("prior-model"));
        assert_eq!(resolved.reasoning_effort.as_deref(), Some("low"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_for_thread_reuses_thread_value_over_preference() {
        let (pool, db_name) = setup_test_db().await;
        seed_chat_prefs(&pool, "pref-model", "low").await;
        let tid = Uuid::new_v4();
        insert_message_received(&pool, tid, Some("thread-model"), Some("thread-effort")).await;
        let resolved = PreferenceStore::resolve_chat_overrides_for_thread(
            &pool,
            Some(tid),
            None,
            overrides(None, None),
        )
        .await;
        assert_eq!(resolved.model.as_deref(), Some("thread-model"));
        assert_eq!(resolved.reasoning_effort.as_deref(), Some("thread-effort"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_for_thread_explicit_override_beats_thread_memory() {
        let (pool, db_name) = setup_test_db().await;
        let tid = Uuid::new_v4();
        insert_message_received(&pool, tid, Some("thread-model"), Some("thread-effort")).await;
        // Override the model only → effort still comes from the thread (per field).
        let resolved = PreferenceStore::resolve_chat_overrides_for_thread(
            &pool,
            Some(tid),
            None,
            overrides(Some("override-model"), None),
        )
        .await;
        assert_eq!(resolved.model.as_deref(), Some("override-model"));
        assert_eq!(resolved.reasoning_effort.as_deref(), Some("thread-effort"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn resolve_for_thread_falls_back_to_preference_without_thread_record() {
        let (pool, db_name) = setup_test_db().await;
        seed_chat_prefs(&pool, "pref-model", "low").await;
        // A brand-new thread (no messages) → account preference.
        let resolved = PreferenceStore::resolve_chat_overrides_for_thread(
            &pool,
            Some(Uuid::new_v4()),
            None,
            overrides(None, None),
        )
        .await;
        assert_eq!(resolved.model.as_deref(), Some("pref-model"));
        assert_eq!(resolved.reasoning_effort.as_deref(), Some("low"));
        // Thread-less resolve behaves the same (caller override → preference).
        let threadless = PreferenceStore::resolve_chat_overrides_for_thread(
            &pool,
            None,
            None,
            overrides(None, None),
        )
        .await;
        let (m2, e2) = (threadless.model, threadless.reasoning_effort);
        assert_eq!(m2.as_deref(), Some("pref-model"));
        assert_eq!(e2.as_deref(), Some("low"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn a_remembered_provider_never_outlives_its_model() {
        let (pool, db_name) = setup_test_db().await;
        let tid = Uuid::new_v4();
        insert_starter(&pool, tid, Some("opus"), Some("high"), Some("vertex")).await;

        // The same model keeps the backend it was pinned to.
        let same = PreferenceStore::resolve_chat_overrides_for_thread(
            &pool,
            Some(tid),
            None,
            overrides(None, None),
        )
        .await;
        assert_eq!(same.provider.as_deref(), Some("vertex"));

        // Switching the model drops it, so the new model's row decides.
        let switched = PreferenceStore::resolve_chat_overrides_for_thread(
            &pool,
            Some(tid),
            None,
            overrides(Some("sonnet"), None),
        )
        .await;
        assert_eq!(switched.provider, None);

        // A later turn on the new model reads no pick either.
        insert_starter(&pool, tid, Some("sonnet"), Some("high"), None).await;
        let later = PreferenceStore::last_thread_chat_settings(&pool, tid, None, None).await;
        assert_eq!(later.model.as_deref(), Some("sonnet"));
        assert_eq!(later.provider, None);

        // Switching back finds the pick recorded with that model.
        let back = PreferenceStore::last_thread_chat_settings(&pool, tid, None, Some("opus")).await;
        assert_eq!(back.provider.as_deref(), Some("vertex"));
        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
