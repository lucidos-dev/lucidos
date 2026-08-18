//! User-managed non-secret environment variables.
//!
//! A parallel mechanism to [`crate::core::credentials`]: plaintext key/value
//! pairs the user (or the LLM) defines once and the engine injects as real
//! environment variables into every subprocess it spawns — `run_python`,
//! `run_bash`, background bash, scheduled scripts, triggers, Claude Code, and
//! Codex — alongside the `CRED_*` / `OAUTH_*` injection.
//!
//! The store has a **second consumer**: [`apply_to_process_env`] copies every
//! pair into the ENGINE'S OWN process env, once at startup. The two differ on
//! restart, and its doc comment states the rule for both.
//!
//! These are deliberately **not** secret: the value is carried in the
//! `EnvironmentVariableSet` SystemEvent (broadcast to every device), and may
//! appear in tool-call payloads, logs, and the event store. That is the whole
//! point and the reason they live outside the credential store.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

/// A single user-managed environment variable (name + plaintext value).
///
/// The name root `EnvironmentVariable` is used identically across DB
/// (`environment_variables`), Rust, TS, the wire, and the events so the concept
/// never drifts between layers (see `.claude/rules/glossary.md`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentVariable {
    pub id: Uuid,
    pub name: String,
    pub value: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Reason a candidate name was rejected. Stringified at the API / tool boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameRejection {
    /// Failed the `^[A-Z_][A-Z0-9_]*$` shape check.
    Invalid,
    /// A well-formed name the engine itself owns — injecting it would clobber an
    /// engine-critical var, so creation is refused outright.
    Reserved,
}

impl NameRejection {
    pub fn message(&self, name: &str) -> String {
        match self {
            NameRejection::Invalid => format!(
                "Invalid environment variable name '{name}'. Use uppercase letters, digits, and \
                 underscores only, and don't start with a digit (e.g. MY_FLAG, LUCIDOS_REPO)."
            ),
            NameRejection::Reserved => format!(
                "'{name}' is reserved by the engine and can't be used as a user environment \
                 variable. Names starting with CRED_ or OAUTH_, the PG* database vars, and the \
                 engine-owned LUCIDOS_* / PATH / RUSTC_WRAPPER vars are off-limits."
            ),
        }
    }
}

/// Engine-owned env var names a user variable must never clobber. These are set
/// by the engine on every spawn (see `engine_impl::scripts::build_script_env_vars`
/// and `runtime::spawn_env::apply_lucidos_env`); a user row with one of these
/// names is refused at creation and filtered out again at injection time
/// (defense in depth). `LUCIDOS_REPO` is intentionally NOT here — it is
/// `LUCIDOS_*`-style but user-settable; for coding-agent spawns the engine's
/// value wins by ordering, for plain scripts the user's value applies.
const RESERVED_EXACT: &[&str] = &[
    // Postgres connection (see `core::pg_env_vars`).
    "PGUSER",
    "PGPASSWORD",
    "PGHOST",
    "PGPORT",
    "PGDATABASE",
    // Process / build env the engine controls.
    "PATH",
    "RUSTC_WRAPPER",
    // Engine-owned LUCIDOS_* (see `api::actor` + `runtime::spawn_env`).
    "LUCIDOS_WORKSPACE",
    "LUCIDOS_AGENT_ORIGIN_TOKEN",
    "LUCIDOS_THREAD_ID",
    "LUCIDOS_HOST_PID",
    "LUCIDOS_FRONTEND_PID",
    "LUCIDOS_API_PORT",
    "LUCIDOS_API_BASE_URL",
    "LUCIDOS_EVENT_ID",
    "LUCIDOS_SESSION_KIND",
];

/// Reserved name prefixes — the injection namespaces owned by the credential
/// and OAuth stores.
const RESERVED_PREFIXES: &[&str] = &["CRED_", "OAUTH_"];

/// Whether `name` matches the allowed shape: uppercase ASCII letters, digits,
/// and underscores, and must not start with a digit (`^[A-Z_][A-Z0-9_]*$`).
pub fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Whether `name` is reserved by the engine (exact name or reserved prefix).
/// Assumes a syntactically valid name; callers validate shape first.
pub fn is_reserved_name(name: &str) -> bool {
    RESERVED_EXACT.contains(&name) || RESERVED_PREFIXES.iter().any(|p| name.starts_with(p))
}

/// Validate a candidate name: shape first, then reserved policy.
pub fn validate_name(name: &str) -> Result<(), NameRejection> {
    if !is_valid_name(name) {
        return Err(NameRejection::Invalid);
    }
    if is_reserved_name(name) {
        return Err(NameRejection::Reserved);
    }
    Ok(())
}

/// Store for user-managed environment variables.
///
/// **No caller can skip the event.** [`Self::upsert`], [`Self::update`] and
/// [`Self::delete`] are the only reachable mutators: the raw row writes
/// ([`Self::upsert_row`], [`Self::update_row`], [`Self::delete_row`]) are
/// private to this module. `EnvironmentVariable{Set,Deleted}` is what reloads
/// the Settings list on every device, and these values are deliberately
/// non-secret, so the event carries the value.
///
/// Same shape and the same reachability-not-atomicity guarantee as
/// `RepositoryStore`; see `core::announced_surfaces`.
pub struct EnvironmentVariableStore;

type Row = (Uuid, String, String, DateTime<Utc>, DateTime<Utc>);

fn parse_row(row: Row) -> EnvironmentVariable {
    let (id, name, value, created_at, updated_at) = row;
    EnvironmentVariable {
        id,
        name,
        value,
        created_at,
        updated_at,
    }
}

impl EnvironmentVariableStore {
    /// Insert or update a variable (upsert by name). Returns the row id.
    ///
    /// **Private on purpose**: [`Self::upsert`] is the reachable mutator, and it
    /// emits. See the type-level doc.
    async fn upsert_row(pool: &PgPool, name: &str, value: &str) -> Result<Uuid, sqlx::Error> {
        sqlx::query_scalar::<_, Uuid>(
            r#"
            INSERT INTO environment_variables (name, value)
            VALUES ($1, $2)
            ON CONFLICT (name) DO UPDATE SET
                value = EXCLUDED.value,
                updated_at = NOW()
            RETURNING id
            "#,
        )
        .bind(name)
        .bind(value)
        .fetch_one(pool)
        .await
    }

    /// Save a variable and announce it. The only way to create one.
    ///
    /// Announces unconditionally, unlike `RepositoryStore::register`: setting a
    /// variable to the value it already holds is a deliberate user action on a
    /// surface nothing re-runs on a loop, so there is no restart-spam hazard to
    /// suppress and the timeline should show that the user did it.
    pub async fn upsert(
        pool: &PgPool,
        event_bus: &EventBus,
        name: &str,
        value: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<Uuid, sqlx::Error> {
        let id = Self::upsert_row(pool, name, value).await?;
        event_bus
            .emit_or_log(
                BusEvent::System(SystemEvent::EnvironmentVariableSet {
                    name: name.to_string(),
                    value: value.to_string(),
                    actor,
                }),
                "[EnvVars] EnvironmentVariableSet",
            )
            .await;
        Ok(id)
    }

    /// Get a single variable by name.
    pub async fn get(
        pool: &PgPool,
        name: &str,
    ) -> Result<Option<EnvironmentVariable>, sqlx::Error> {
        let row = sqlx::query_as::<_, Row>(
            r#"
            SELECT id, name, value, created_at, updated_at
            FROM environment_variables
            WHERE name = $1
            "#,
        )
        .bind(name)
        .fetch_optional(pool)
        .await?;
        Ok(row.map(parse_row))
    }

    /// List all variables, ordered by name.
    pub async fn list(pool: &PgPool) -> Result<Vec<EnvironmentVariable>, sqlx::Error> {
        let rows = sqlx::query_as::<_, Row>(
            r#"
            SELECT id, name, value, created_at, updated_at
            FROM environment_variables
            ORDER BY name ASC
            "#,
        )
        .fetch_all(pool)
        .await?;
        Ok(rows.into_iter().map(parse_row).collect())
    }

    /// Update the value of an existing variable. Returns `true` if a row existed.
    ///
    /// **Private on purpose**: [`Self::update`] is the reachable mutator.
    async fn update_row(pool: &PgPool, name: &str, value: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            r#"
            UPDATE environment_variables
            SET value = $2, updated_at = NOW()
            WHERE name = $1
            "#,
        )
        .bind(name)
        .bind(value)
        .execute(pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Update an existing variable and announce it. Announces only when a row
    /// was actually updated, so an edit aimed at a missing name stays silent.
    pub async fn update(
        pool: &PgPool,
        event_bus: &EventBus,
        name: &str,
        value: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, sqlx::Error> {
        let updated = Self::update_row(pool, name, value).await?;
        if updated {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::EnvironmentVariableSet {
                        name: name.to_string(),
                        value: value.to_string(),
                        actor,
                    }),
                    "[EnvVars] EnvironmentVariableSet",
                )
                .await;
        }
        Ok(updated)
    }

    /// Delete a variable by name. Returns `true` if a row existed.
    ///
    /// **Private on purpose**: [`Self::delete`] is the reachable mutator.
    async fn delete_row(pool: &PgPool, name: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM environment_variables WHERE name = $1")
            .bind(name)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Delete a variable and announce it. The only way to remove one.
    ///
    /// `EnvironmentVariableDeleted` fires only when a row was actually deleted,
    /// so a repeated or racing delete announces once.
    pub async fn delete(
        pool: &PgPool,
        event_bus: &EventBus,
        name: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, sqlx::Error> {
        let removed = Self::delete_row(pool, name).await?;
        if removed {
            event_bus
                .emit_or_log(
                    BusEvent::System(SystemEvent::EnvironmentVariableDeleted {
                        name: name.to_string(),
                        actor,
                    }),
                    "[EnvVars] EnvironmentVariableDeleted",
                )
                .await;
        }
        Ok(removed)
    }

    /// Build the `(NAME, value)` pairs to inject into a spawned subprocess.
    ///
    /// Reserved-named rows are filtered out as a defense-in-depth backstop: the
    /// API and the LLM tool already refuse to create them, but a row that
    /// pre-dates a future reserved-list addition (or one written out-of-band)
    /// must never be injected, since the engine relies on owning those names.
    /// The caller adds these to the env map BEFORE the engine-owned vars so a
    /// soft collision (e.g. a user `LUCIDOS_REPO` on a coding-agent spawn) is
    /// overwritten by the engine.
    pub async fn env_pairs(pool: &PgPool) -> Result<Vec<(String, String)>, sqlx::Error> {
        let vars = Self::list(pool).await?;
        Ok(vars
            .into_iter()
            .filter(|v| !is_reserved_name(&v.name))
            .map(|v| (v.name, v.value))
            .collect())
    }
}

/// Strip one layer of matching surrounding single or double quotes from a
/// `.env` value (`"foo"` / `'foo'` → `foo`). Leaves unquoted/mismatched as-is.
fn unquote(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' || first == b'\'') && first == last {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

/// One-time startup migration: import any `<workspace>/data/.env` `KEY=VALUE`
/// lines into the `environment_variables` table, then **delete** the file. This
/// is the consolidation that retires the legacy per-workspace `.env` mechanism
/// (`load_workspace_env`) in favour of the single DB-backed store.
///
/// Self-idempotent: the file is removed after a successful import, so a later
/// boot finds nothing to do. Invalid- or reserved-named lines are skipped with a
/// log line (the engine owns those names). Each imported row announces
/// `EnvironmentVariableSet` like any other write, because the write path owns
/// the emit and a backfill is not special: these variables genuinely start
/// existing here. It cannot spam a restart, since the file is deleted after the
/// import. The store is non-secret, which is correct here: `.env` held only
/// non-secret config (paths, CLI config dirs, identifiers); real secrets live in
/// the credential / OAuth stores.
///
/// Best-effort: read/parse/delete errors are logged, never fatal — a broken
/// `.env` must not stop the engine from booting.
pub async fn migrate_env_file_to_db(
    pool: &PgPool,
    event_bus: &EventBus,
    workspace: &std::path::Path,
) {
    let env_path = workspace.join(crate::core::DATA_DIR).join(".env");
    if !env_path.exists() {
        return;
    }
    let contents = match std::fs::read_to_string(&env_path) {
        Ok(c) => c,
        Err(e) => {
            crate::log!(
                "[EnvMigration] Failed to read {} — leaving it in place: {}",
                env_path.display(),
                e
            );
            return;
        }
    };

    let mut imported = 0usize;
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((raw_key, raw_val)) = line.split_once('=') else {
            continue;
        };
        let key = raw_key.trim();
        let value = unquote(raw_val.trim());
        match validate_name(key) {
            Ok(()) => {
                match EnvironmentVariableStore::upsert(pool, event_bus, key, &value, None).await {
                    Ok(_) => imported += 1,
                    Err(e) => crate::log!("[EnvMigration] Failed to import '{}': {}", key, e),
                }
            }
            Err(rejection) => crate::log!(
                "[EnvMigration] Skipping '{}' from data/.env: {}",
                key,
                rejection.message(key)
            ),
        }
    }

    match std::fs::remove_file(&env_path) {
        Ok(()) => crate::log!(
            "[EnvMigration] Imported {} var(s) from data/.env into the DB and removed the file",
            imported
        ),
        Err(e) => crate::log!(
            "[EnvMigration] Imported {} var(s) but failed to delete {}: {}",
            imported,
            env_path.display(),
            e
        ),
    }
}

/// The engine-process half of the store. Applies the stored variables to the
/// ENGINE'S OWN process env, once at startup. That restores the inheritance the
/// retired per-workspace `data/.env` provided (`load_workspace_env` used to
/// `set_var` these at startup).
///
/// Two kinds of reader see them. Engine-internal shell-outs inherit the process
/// env: the engine's own `git push`/`fetch`/`clone` (Apply time, repo add),
/// stdio MCP servers, `open`. Engine code calling `std::env::var` reads them
/// directly.
///
/// Reserved names are filtered (via `env_pairs`), so this can NEVER overwrite an
/// engine-critical process var (`PG*`, `PATH`, internal `LUCIDOS_*`, …).
///
/// Restart semantics mirror the old `data/.env`, and this half is the one that
/// needs a restart. Running once at startup means a caller inside the engine
/// reads whatever the store held when the engine last started. A `OnceLock`
/// around such a read pins that value for the process lifetime. The per-spawn
/// injection (`build_script_env_vars` / `apply_lucidos_env`) is the no-restart
/// path, and already covers every tool/agent subprocess, where freshness matters.
pub async fn apply_to_process_env(pool: &PgPool) {
    let pairs = match EnvironmentVariableStore::env_pairs(pool).await {
        Ok(pairs) => pairs,
        Err(e) => {
            crate::log!(
                "[EnvVars] Failed to load environment variables for process env: {}",
                e
            );
            return;
        }
    };
    for (name, value) in pairs {
        // SAFETY: called once at startup, before the engine begins serving
        // requests or running recovery sweeps. This mirrors the prior
        // `dotenvy::from_path_override` startup load (which also mutated process
        // env via `set_var`); no request-handling thread reads these vars at
        // this point.
        unsafe {
            std::env::set_var(&name, &value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names_accepted() {
        for name in [
            "FOO",
            "MY_FLAG",
            "_LEADING_UNDERSCORE",
            "A1",
            "LUCIDOS_REPO",
            "CLAUDE_CODE_USE_VERTEX",
            "X_2_Y_3",
        ] {
            assert!(is_valid_name(name), "{name} should be valid");
        }
    }

    #[test]
    fn invalid_names_rejected() {
        for name in [
            "",           // empty
            "1ABC",       // leading digit
            "lower",      // lowercase
            "Mixed",      // mixed case
            "WITH-DASH",  // dash
            "WITH SPACE", // space
            "WITH.DOT",   // dot
            "TRAILING\n", // newline
        ] {
            assert!(!is_valid_name(name), "{name:?} should be invalid");
        }
    }

    #[test]
    fn reserved_names_blocked() {
        for name in [
            "PGUSER",
            "PGPASSWORD",
            "PATH",
            "RUSTC_WRAPPER",
            "LUCIDOS_WORKSPACE",
            "LUCIDOS_HOST_PID",
            "LUCIDOS_AGENT_ORIGIN_TOKEN",
            "CRED_OPENAI",
            "OAUTH_GOOGLE_ACCESS_TOKEN",
        ] {
            assert!(is_reserved_name(name), "{name} should be reserved");
            assert_eq!(validate_name(name), Err(NameRejection::Reserved));
        }
    }

    #[test]
    fn lucidos_repo_is_allowed() {
        // The user explicitly wants LUCIDOS_REPO-style vars settable; the engine
        // wins by ordering on coding-agent spawns, so it's not hard-reserved.
        assert!(!is_reserved_name("LUCIDOS_REPO"));
        assert_eq!(validate_name("LUCIDOS_REPO"), Ok(()));
        assert!(!is_reserved_name("LUCIDOS_CUSTOM_THING"));
        assert_eq!(validate_name("LUCIDOS_CUSTOM_THING"), Ok(()));
    }

    #[test]
    fn validate_name_reports_invalid_before_reserved() {
        // A lowercase reserved-ish name fails the shape check first.
        assert_eq!(validate_name("path"), Err(NameRejection::Invalid));
    }

    /// The load-bearing guarantee: a variable write and its announcement are one
    /// operation, so the Settings list on every device reloads no matter which
    /// entry path made the write. Deletion announces only when a row was really
    /// removed, so a racing double-delete speaks once.
    #[tokio::test]
    async fn every_mutation_announces_and_a_no_op_delete_does_not() {
        let (pool, db) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        async fn emitted(pool: &PgPool, event_type: &str) -> i64 {
            sqlx::query_scalar("SELECT count(*) FROM events WHERE event_type = $1")
                .bind(event_type)
                .fetch_one(pool)
                .await
                .unwrap()
        }

        EnvironmentVariableStore::upsert(&pool, &bus, "MY_FLAG", "1", None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "EnvironmentVariableSet").await, 1);

        EnvironmentVariableStore::update(&pool, &bus, "MY_FLAG", "2", None)
            .await
            .unwrap();
        assert_eq!(emitted(&pool, "EnvironmentVariableSet").await, 2);

        assert!(
            !EnvironmentVariableStore::update(&pool, &bus, "MISSING", "x", None)
                .await
                .unwrap()
        );
        assert_eq!(
            emitted(&pool, "EnvironmentVariableSet").await,
            2,
            "an update that matched no row must not announce"
        );

        assert!(
            EnvironmentVariableStore::delete(&pool, &bus, "MY_FLAG", None)
                .await
                .unwrap()
        );
        assert_eq!(emitted(&pool, "EnvironmentVariableDeleted").await, 1);
        assert!(
            !EnvironmentVariableStore::delete(&pool, &bus, "MY_FLAG", None)
                .await
                .unwrap()
        );
        assert_eq!(
            emitted(&pool, "EnvironmentVariableDeleted").await,
            1,
            "second delete removes nothing and therefore announces nothing"
        );

        crate::test_support::teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn env_pairs_round_trips_and_filters_reserved() {
        let (pool, _db) = crate::test_support::setup_test_db().await;

        let (bus, _callback_rx) = EventBus::new(pool.clone());
        EnvironmentVariableStore::upsert(&pool, &bus, "MY_FLAG", "1", None)
            .await
            .expect("upsert MY_FLAG");
        EnvironmentVariableStore::upsert(&pool, &bus, "LUCIDOS_REPO", "my-repo", None)
            .await
            .expect("upsert LUCIDOS_REPO");
        // A reserved-named row written out-of-band (the API/tool would refuse it)
        // must be filtered out of the injected pairs as a defense-in-depth net.
        EnvironmentVariableStore::upsert(&pool, &bus, "PGUSER", "evil", None)
            .await
            .expect("upsert reserved row");

        let pairs = EnvironmentVariableStore::env_pairs(&pool)
            .await
            .expect("env_pairs");

        assert!(
            pairs.contains(&("MY_FLAG".to_string(), "1".to_string())),
            "user var must be injected"
        );
        assert!(
            pairs.contains(&("LUCIDOS_REPO".to_string(), "my-repo".to_string())),
            "non-reserved LUCIDOS_* var must be injected"
        );
        assert!(
            !pairs.iter().any(|(k, _)| k == "PGUSER"),
            "reserved engine-owned name must never reach injection"
        );
    }

    #[tokio::test]
    async fn upsert_replaces_value_and_update_requires_existing() {
        let (pool, _db) = crate::test_support::setup_test_db().await;

        let (bus, _callback_rx) = EventBus::new(pool.clone());
        EnvironmentVariableStore::upsert(&pool, &bus, "FOO", "one", None)
            .await
            .expect("insert");
        EnvironmentVariableStore::upsert(&pool, &bus, "FOO", "two", None)
            .await
            .expect("upsert replace");
        let got = EnvironmentVariableStore::get(&pool, "FOO")
            .await
            .expect("get")
            .expect("present");
        assert_eq!(got.value, "two");

        assert!(
            EnvironmentVariableStore::update(&pool, &bus, "FOO", "three", None)
                .await
                .expect("update existing"),
            "update of existing row returns true"
        );
        assert!(
            !EnvironmentVariableStore::update(&pool, &bus, "MISSING", "x", None)
                .await
                .expect("update missing"),
            "update of missing row returns false"
        );
        assert!(EnvironmentVariableStore::delete(&pool, &bus, "FOO", None)
            .await
            .expect("delete"),);
        assert!(EnvironmentVariableStore::get(&pool, "FOO")
            .await
            .expect("get after delete")
            .is_none());
    }
}
