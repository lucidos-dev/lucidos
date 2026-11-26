use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::MessageOrigin;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnvironmentVariable {
    pub id: Uuid,
    pub name: String,
    pub value: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameRejection {
    Invalid,
    Reserved,
    /// INVARIANT: A name that makes a subprocess load code it never names (`BASH_ENV`,
    /// `LD_PRELOAD`, `NODE_OPTIONS`, `GIT_SSH_COMMAND`). The user may set it;
    /// an agent may not, since every later subprocess would run it unguarded.
    CodeInjecting,
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
                 variable. Names starting with CRED_ or OAUTH_ and the engine-owned \
                 LUCIDOS_* / PATH / RUSTC_WRAPPER vars are off-limits."
            ),
            NameRejection::CodeInjecting => format!(
                "'{name}' changes what code every later subprocess loads, so an agent can't \
                 set it. Ask the user to set it in Settings → System → Environment variables."
            ),
        }
    }
}

/// INVARIANT: Engine-owned env var names a user variable must never clobber. These are set
/// by the engine on every spawn (see `engine_impl::scripts::build_script_env_vars`
/// and `runtime::spawn_env::apply_lucidos_env`); a user row with one of these
/// names is refused at creation and filtered out again at injection time
/// (defense in depth). `LUCIDOS_REPO` is intentionally NOT here — it is
/// `LUCIDOS_*`-style but user-settable; for coding-agent spawns the engine's
/// value wins by ordering, for plain scripts the user's value applies.
const RESERVED_EXACT: &[&str] = &[
    crate::core::LUCIDOS_DATABASE_URL,
    crate::core::MANAGED_DATABASE_ENV,
    "PATH",
    "RUSTC_WRAPPER",
    "LUCIDOS_WORKSPACE",
    "LUCIDOS_WORKSPACE_ORIGIN",
    crate::core::subprocess_origin::ENV_AGENT_ORIGIN_TOKEN,
    crate::core::subprocess_origin::ENV_SOURCE_THREAD_ID,
    "LUCIDOS_HOST_PID",
    "LUCIDOS_FRONTEND_PID",
    "LUCIDOS_API_PORT",
    "LUCIDOS_API_BASE_URL",
    "LUCIDOS_EVENT_ID",
    "LUCIDOS_SESSION_KIND",
    AGENT_RUN_ENV,
    // INVARIANT: Network exposure switches. The engine reads them after
    // `apply_to_process_env`, so a stored row would change how the NEXT boot
    // binds, serves TLS, or gates cross-origin browsers. Only the launcher sets
    // these, never a user row an agent can write.
    "LUCIDOS_BIND_LOOPBACK",
    "LUCIDOS_BIND_ALL",
    "LUCIDOS_BIND_ADDR",
    "LUCIDOS_TLS_CERT",
    "LUCIDOS_TLS_KEY",
    "LUCIDOS_PERMISSIVE_CORS",
];

const RESERVED_PREFIXES: &[&str] = &["CRED_", "OAUTH_"];

pub fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

pub fn is_reserved_name(name: &str) -> bool {
    RESERVED_EXACT.contains(&name) || RESERVED_PREFIXES.iter().any(|p| name.starts_with(p))
}

pub fn validate_name(name: &str) -> Result<(), NameRejection> {
    if !is_valid_name(name) {
        return Err(NameRejection::Invalid);
    }
    if is_reserved_name(name) {
        return Err(NameRejection::Reserved);
    }
    Ok(())
}

pub fn validate_agent_name(name: &str) -> Result<(), NameRejection> {
    validate_name(name)?;
    if is_code_injecting_env_name(name) {
        return Err(NameRejection::CodeInjecting);
    }
    Ok(())
}

/// INVARIANT: Store for user-managed environment variables.
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

    async fn delete_row(pool: &PgPool, name: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM environment_variables WHERE name = $1")
            .bind(name)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

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

    /// INVARIANT: Build the `(NAME, value)` pairs to inject into a spawned subprocess.
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

    /// INVARIANT: [`Self::env_pairs`] for a spawn about to happen, logged under `label`.
    ///
    /// A failed read falls back to the startup copy rather than to nothing.
    /// [`apply_to_subprocess_env`] removes every startup name missing here, so
    /// an empty fallback would strip live variables on a DB hiccup.
    pub async fn spawn_pairs(pool: &PgPool, label: &str) -> Vec<(String, String)> {
        Self::env_pairs(pool).await.unwrap_or_else(|e| {
            crate::log_legacy!(
                "[{}] Failed to load user environment variables, using the startup copy: {}",
                label,
                e
            );
            startup_env_vars().to_vec()
        })
    }
}

const PER_SPAWN_ORIGIN_VARS: &[&str] = &[
    crate::core::subprocess_origin::ENV_AGENT_ORIGIN_TOKEN,
    crate::core::subprocess_origin::ENV_SOURCE_THREAD_ID,
];

static STARTUP_ENV_VARS: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();

fn startup_env_vars() -> &'static [(String, String)] {
    STARTUP_ENV_VARS.get().map_or(&[], Vec::as_slice)
}

pub fn apply_to_subprocess_env(
    cmd: &mut tokio::process::Command,
    pairs: &[(String, String)],
    inherited: super::InheritedSecrets<'_>,
) {
    apply_env_over_startup(cmd, startup_env_vars(), pairs, inherited);
}

pub fn inherited_env_var(name: &str) -> Option<String> {
    inherited_over_startup(name, startup_env_vars(), std::env::var(name).ok())
}

fn inherited_over_startup(
    name: &str,
    startup: &[(String, String)],
    process_value: Option<String>,
) -> Option<String> {
    if startup.iter().any(|(k, _)| k == name) {
        return None;
    }
    process_value.filter(|v| !v.is_empty())
}

fn apply_env_over_startup(
    cmd: &mut tokio::process::Command,
    startup: &[(String, String)],
    pairs: &[(String, String)],
    inherited: super::InheritedSecrets<'_>,
) {
    let current: std::collections::HashSet<&str> = pairs.iter().map(|(k, _)| k.as_str()).collect();
    let stripped = match inherited {
        super::InheritedSecrets::Keep => None,
        super::InheritedSecrets::Strip(snapshot) => Some(snapshot.names()),
    };
    let removed = startup
        .iter()
        .map(|(name, _)| name.as_str())
        .chain(PER_SPAWN_ORIGIN_VARS.iter().copied())
        .chain(stripped.into_iter().flatten());
    for name in removed {
        if !current.contains(name) {
            cmd.env_remove(name);
        }
    }
    for (key, value) in pairs {
        cmd.env(key, value);
    }
}

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

/// INVARIANT: One-time startup migration: import any `<workspace>/data/.env` `KEY=VALUE`
/// lines into the `environment_variables` table, then **delete** the file. This
/// is the consolidation that retires the legacy per-workspace `.env` mechanism
/// (`load_workspace_env`) in favour of the single DB-backed store.
///
/// Self-idempotent: the file is removed once every line has landed, so a later
/// boot finds nothing to do. Invalid- or reserved-named lines are skipped with a
/// log line (the engine owns those names). A code-injecting name is held: an
/// agent can write this file, so only the user may set that name, in Settings.
/// The file stays until they have. Each imported row announces
/// `EnvironmentVariableSet` like any other write, because the write path owns
/// the emit and a backfill is not special: these variables genuinely start
/// existing here. It cannot spam a restart: the file is deleted once the import
/// is clean, and a retained one re-imports only the keys the DB still lacks.
/// The store is non-secret, which is correct here: `.env` held
/// only non-secret config (paths, CLI config dirs, identifiers); real secrets
/// live in the credential / OAuth stores.
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
            crate::log_legacy!(
                "[EnvMigration] Failed to read {} — leaving it in place: {}",
                env_path.display(),
                e
            );
            return;
        }
    };

    let mut imported = 0usize;
    let mut failed = 0usize;
    let mut already = 0usize;
    let mut held = 0usize;
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
        match validate_agent_name(key) {
            Ok(()) => {
                match EnvironmentVariableStore::get(pool, key).await {
                    Ok(Some(_)) => {
                        already += 1;
                        continue;
                    }
                    Ok(None) => {}
                    Err(e) => {
                        failed += 1;
                        crate::log_legacy!(
                            "[EnvMigration] Could not read '{}' from the DB: {}",
                            key,
                            e
                        );
                        continue;
                    }
                }
                match EnvironmentVariableStore::upsert(pool, event_bus, key, &value, None).await {
                    Ok(_) => imported += 1,
                    Err(e) => {
                        failed += 1;
                        crate::log_legacy!("[EnvMigration] Failed to import '{}': {}", key, e);
                    }
                }
            }
            // INVARIANT: An agent can write data/.env, so this name waits for the user.
            // Keeping the file keeps their legacy value until they set it.
            Err(NameRejection::CodeInjecting) => {
                match EnvironmentVariableStore::get(pool, key).await {
                    Ok(Some(_)) => already += 1,
                    _ => {
                        held += 1;
                        crate::log_legacy!(
                            "[EnvMigration] Not importing '{}' from data/.env: {}",
                            key,
                            NameRejection::CodeInjecting.message(key)
                        );
                    }
                }
            }
            Err(rejection) => crate::log_legacy!(
                "[EnvMigration] Skipping '{}' from data/.env: {}",
                key,
                rejection.message(key)
            ),
        }
    }

    // INVARIANT: A line that failed to import still exists only in this file, so deleting
    // it now would lose the variable for good. A transient DB error at boot is
    // exactly when that happens. Keep the file and retry next boot.
    //
    // The retry is quiet. The loop skips a key the DB already holds, so a later
    // boot imports only what is still missing and announces nothing for the
    // rest. That also bounds a permanently failing line: the file stays, and
    // every boot after the first does the same small read and no writes.
    if failed > 0 || held > 0 {
        crate::log_legacy!(
            "[EnvMigration] Imported {} var(s), skipped {} already in the DB, {} failed, {} held for Settings, so data/.env stays for the next boot",
            imported,
            already,
            failed,
            held
        );
        return;
    }

    match std::fs::remove_file(&env_path) {
        Ok(()) => crate::log_legacy!(
            "[EnvMigration] Imported {} var(s) from data/.env into the DB, skipped {} already there, and removed the file",
            imported,
            already
        ),
        Err(e) => crate::log_legacy!(
            "[EnvMigration] Imported {} var(s) but failed to delete {}: {}",
            imported,
            env_path.display(),
            e
        ),
    }
}

pub async fn apply_to_process_env(pool: &PgPool) {
    // INVARIANT: Before any row lands in the process env, so the snapshot holds only
    // what the engine inherited.
    super::inherited_secret_env();
    let pairs = match EnvironmentVariableStore::env_pairs(pool).await {
        Ok(pairs) => pairs,
        Err(e) => {
            crate::log_legacy!(
                "[EnvVars] Failed to load environment variables for process env: {}",
                e
            );
            return;
        }
    };
    for (name, value) in &pairs {
        // SAFETY: called once at startup, before the engine begins serving
        // requests or running recovery sweeps. This mirrors the prior
        // `dotenvy::from_path_override` startup load (which also mutated process
        // env via `set_var`); no request-handling thread reads these vars at
        // this point.
        unsafe {
            std::env::set_var(name, value);
        }
    }
    if STARTUP_ENV_VARS.set(pairs).is_err() {
        crate::log_legacy!(
            "[EnvVars] apply_to_process_env ran twice; keeping the first startup copy"
        );
    }
}

/// INVARIANT: The variables that tell the `lucidos` CLI which engine to reach. A line
/// that sets one can send a `data write --from` file anywhere.
pub const LUCIDOS_CLI_ENV_NAMES: [&str; 3] = [
    "LUCIDOS_API_BASE_URL",
    "LUCIDOS_WORKSPACE",
    "LUCIDOS_API_PORT",
];

/// INVARIANT: Environment-variable names that make the process the command starts run
/// code the command head says nothing about. The variable either names code
/// to load, or moves where a config naming code is read from. Matched case-sensitively, because the
/// loader and these interpreters read exact upper-case names.
///
/// **An entry ending in `=` is an EXACT name; every other entry is a prefix.**
/// The three-letter names begin many ordinary application variables, and
/// matching those as prefixes drops routine commands out of the Safe fast path
/// for nothing.
///
/// Not a completeness claim: it covers the loader hooks (`LD_*`, `DYLD_*`) and
/// the startup-file hooks of the interpreters we ship or that any dev box has.
/// A miss costs a judge call that did not happen, so add rather than debate.
const CODE_INJECTING_ENV_NAMES: &[&str] = &[
    "LD_",
    "DYLD_",
    "BASH_ENV",
    "ENV=",
    // INVARIANT: The most direct one. The agent can write its own binary with an ordinary
    // in-workspace write, itself Safe. A `PATH=` preamble then runs it with no
    // card, no checkpoint and no judge call. Exact, so `PATHEXT=` and the
    // various build variables keep the fast path.
    "PATH=",
    "SHELLOPTS",
    "BASHOPTS",
    "PS4",
    "IFS=",
    "PYTHONSTARTUP",
    "PYTHONPATH",
    "PERL5OPT",
    "PERL5LIB",
    "RUBYOPT",
    "NODE_OPTIONS",
    "GIT_SSH",
    "GIT_EXTERNAL_DIFF",
    "GIT_PAGER",
    "GIT_EDITOR",
    // INVARIANT: Covers GIT_CONFIG_GLOBAL / _SYSTEM / _COUNT / _KEY_n / _VALUE_n. These are
    // the environment equivalents of `git -c` and `--config-env`, which
    // `git_subcommand_safety` already refuses by name as "an executable
    // config", and they reach the same place: a config file naming
    // `diff.external` makes a plain `git diff` run it. The same write-then-run
    // path as `PATH=` applies, since writing `data/g` is itself Safe.
    "GIT_CONFIG",
    // INVARIANT: These move where git, curl and wget look for their config, onto a file
    // an in-workspace write can plant. `HOME=. git diff` reads `./.gitconfig`
    // and runs its `diff.external`. `GIT_DIR=x` reads `x/config`.
    "HOME=",
    "XDG_CONFIG_HOME=",
    "GIT_DIR=",
    "GIT_COMMON_DIR=",
    "CURL_HOME=",
    "WGETRC=",
    "SYSTEM_WGETRC=",
    "GIT_EXEC_PATH=",
    "RIPGREP_CONFIG_PATH=",
    "LESSOPEN=",
    "LESSCLOSE=",
    "ACKRC=",
    "ACK_OPTIONS=",
    "ACK_PAGER",
    "PAGER=",
    "GH_CONFIG_DIR=",
    "GH_PAGER=",
    "GH_BROWSER=",
    "LESSKEY",
    "LESS=",
];

/// INVARIANT: True when the variable `name` is one of [`CODE_INJECTING_ENV_NAMES`], or
/// one of [`LUCIDOS_CLI_ENV_NAMES`], matched exactly.
///
/// Also the agent-side refusal for a STORED variable
/// (`environment_variables::validate_agent_name`). A stored one reaches every
/// later subprocess with no preamble for this guard to see.
pub fn is_code_injecting_env_name(name: &str) -> bool {
    if name.is_empty() || name.starts_with('-') {
        return false;
    }
    LUCIDOS_CLI_ENV_NAMES.contains(&name)
        || CODE_INJECTING_ENV_NAMES
            .iter()
            .any(|p| match p.strip_suffix('=') {
                Some(exact) => name == exact,
                None => name.starts_with(p),
            })
}

pub const AGENT_RUN_ENV: &str = "LUCIDOS_AGENT_RUN";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{InheritedSecretEnv, InheritedSecrets};

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
            "",
            "1ABC",
            "lower",
            "Mixed",
            "WITH-DASH",
            "WITH SPACE",
            "WITH.DOT",
            "TRAILING\n",
        ] {
            assert!(!is_valid_name(name), "{name:?} should be invalid");
        }
    }

    #[test]
    fn reserved_names_blocked() {
        for name in [
            "LUCIDOS_DATABASE_URL",
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
    fn network_exposure_switches_are_reserved() {
        for name in [
            "LUCIDOS_BIND_LOOPBACK",
            "LUCIDOS_BIND_ALL",
            "LUCIDOS_BIND_ADDR",
            "LUCIDOS_TLS_CERT",
            "LUCIDOS_TLS_KEY",
            "LUCIDOS_PERMISSIVE_CORS",
        ] {
            assert_eq!(
                validate_name(name),
                Err(NameRejection::Reserved),
                "{name} must not be storable as a user variable"
            );
        }
    }

    #[test]
    fn workspace_origin_is_reserved() {
        assert_eq!(
            validate_name("LUCIDOS_WORKSPACE_ORIGIN"),
            Err(NameRejection::Reserved)
        );
    }

    #[test]
    fn managed_database_handoff_is_reserved() {
        assert_eq!(
            validate_name("LUCIDOS_MANAGED_DATABASE"),
            Err(NameRejection::Reserved)
        );
    }

    #[test]
    fn database_names_the_engine_does_not_own_are_storable() {
        for name in [
            "DATABASE_URL",
            "PGUSER",
            "PGPASSWORD",
            "PGHOST",
            "PGPORT",
            "PGDATABASE",
        ] {
            assert_eq!(validate_name(name), Ok(()), "{name} must be storable");
        }
    }

    #[test]
    fn lucidos_repo_is_allowed() {
        assert!(!is_reserved_name("LUCIDOS_REPO"));
        assert_eq!(validate_name("LUCIDOS_REPO"), Ok(()));
        assert!(!is_reserved_name("LUCIDOS_CUSTOM_THING"));
        assert_eq!(validate_name("LUCIDOS_CUSTOM_THING"), Ok(()));
    }

    #[test]
    fn validate_name_reports_invalid_before_reserved() {
        assert_eq!(validate_name("path"), Err(NameRejection::Invalid));
    }

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

    fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn a_variable_deleted_since_startup_is_removed_from_the_subprocess() {
        let startup = pairs(&[("ANTHROPIC_MODEL", "old"), ("MY_FLAG", "1")]);
        let current = pairs(&[("MY_FLAG", "2"), ("ADDED_LATER", "x")]);
        let mut cmd = tokio::process::Command::new("true");
        apply_env_over_startup(&mut cmd, &startup, &current, InheritedSecrets::Keep);

        let env: std::collections::HashMap<String, Option<String>> = cmd
            .as_std()
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();
        assert_eq!(env.get("ANTHROPIC_MODEL"), Some(&None), "deleted: removed");
        assert_eq!(env.get("MY_FLAG"), Some(&Some("2".to_string())));
        assert_eq!(env.get("ADDED_LATER"), Some(&Some("x".to_string())));
    }

    #[tokio::test]
    async fn a_spawn_inherits_no_origin_it_does_not_set() {
        use crate::core::subprocess_origin::{ENV_AGENT_ORIGIN_TOKEN, ENV_SOURCE_THREAD_ID};
        let child_env = |spawn_pairs: Vec<(String, String)>| async move {
            let mut cmd = tokio::process::Command::new("env");
            cmd.env(ENV_AGENT_ORIGIN_TOKEN, "outer-session-token")
                .env(ENV_SOURCE_THREAD_ID, "outer-session-thread");
            apply_env_over_startup(&mut cmd, &[], &spawn_pairs, InheritedSecrets::Keep);
            let output = cmd.output().await.expect("env runs");
            String::from_utf8(output.stdout).expect("utf-8 env")
        };

        let threadless = child_env(vec![]).await;
        assert!(!threadless.contains("outer-session"), "{threadless}");

        let own = child_env(pairs(&[
            (ENV_AGENT_ORIGIN_TOKEN, "own-token"),
            (ENV_SOURCE_THREAD_ID, "own-thread"),
        ]))
        .await;
        assert!(
            own.contains(&format!("{ENV_AGENT_ORIGIN_TOKEN}=own-token")),
            "{own}"
        );
        assert!(
            own.contains(&format!("{ENV_SOURCE_THREAD_ID}=own-thread")),
            "{own}"
        );
        assert!(!own.contains("outer-session"), "{own}");
    }

    async fn child_env_after(inherited: InheritedSecrets<'_>) -> String {
        let mut cmd = tokio::process::Command::new("env");
        cmd.env("SLACK_USER_TOKEN", "xoxp-from-the-launch-shell")
            .env("GITHUB_TOKEN", "inherited-gh-value");
        let spawn_pairs = pairs(&[("GITHUB_TOKEN", "row-gh-value")]);
        apply_env_over_startup(&mut cmd, &[], &spawn_pairs, inherited);
        let output = cmd.output().await.expect("env runs");
        String::from_utf8(output.stdout).expect("utf-8 env")
    }

    fn launch_shell_snapshot() -> InheritedSecretEnv {
        InheritedSecretEnv::capture(pairs(&[
            ("SLACK_USER_TOKEN", "xoxp-from-the-launch-shell"),
            ("GITHUB_TOKEN", "inherited-gh-value"),
        ]))
    }

    #[tokio::test]
    async fn a_stripped_spawn_inherits_no_secret_it_does_not_set() {
        let snapshot = launch_shell_snapshot();
        let env = child_env_after(InheritedSecrets::Strip(&snapshot)).await;
        assert!(!env.contains("SLACK_USER_TOKEN"), "{env}");
        assert!(env.contains("GITHUB_TOKEN=row-gh-value"), "{env}");
    }

    #[tokio::test]
    async fn a_kept_spawn_still_inherits_the_engine_secrets() {
        let env = child_env_after(InheritedSecrets::Keep).await;
        assert!(
            env.contains("SLACK_USER_TOKEN=xoxp-from-the-launch-shell"),
            "{env}"
        );
    }

    #[test]
    fn a_startup_workspace_var_is_not_inherited() {
        let startup = pairs(&[("CLAUDE_CONFIG_DIR", "/home/u/.claude-personal")]);
        let stale = Some("/home/u/.claude-personal".to_string());
        assert_eq!(
            inherited_over_startup("CLAUDE_CONFIG_DIR", &startup, stale),
            None
        );
        let shell = Some("/home/u/.claude".to_string());
        assert_eq!(
            inherited_over_startup("CLAUDE_CONFIG_DIR", &[], shell.clone()),
            shell,
            "a value from the engine's own launch env is still inherited"
        );
    }

    #[tokio::test]
    async fn env_pairs_round_trips_and_filters_reserved() {
        let (pool, db) = crate::test_support::setup_test_db().await;

        let (bus, _callback_rx) = EventBus::new(pool.clone());
        EnvironmentVariableStore::upsert(&pool, &bus, "MY_FLAG", "1", None)
            .await
            .expect("upsert MY_FLAG");
        EnvironmentVariableStore::upsert(&pool, &bus, "LUCIDOS_REPO", "my-repo", None)
            .await
            .expect("upsert LUCIDOS_REPO");
        EnvironmentVariableStore::upsert(&pool, &bus, "LUCIDOS_DATABASE_URL", "evil", None)
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
            !pairs.iter().any(|(k, _)| k == "LUCIDOS_DATABASE_URL"),
            "reserved engine-owned name must never reach injection"
        );

        crate::test_support::teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn upsert_replaces_value_and_update_requires_existing() {
        let (pool, db) = crate::test_support::setup_test_db().await;

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

        crate::test_support::teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn the_env_file_import_holds_a_code_injecting_name_for_the_user() {
        let (pool, db) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());
        let workspace = tempfile::tempdir().expect("tempdir");
        let data = workspace.path().join(crate::core::DATA_DIR);
        std::fs::create_dir_all(&data).expect("data dir");
        let env_file = data.join(".env");
        std::fs::write(&env_file, "BASH_ENV=data/artifacts/x.sh\nMY_FLAG=1\n").expect("write");

        migrate_env_file_to_db(&pool, &bus, workspace.path()).await;
        assert!(EnvironmentVariableStore::get(&pool, "BASH_ENV")
            .await
            .expect("get")
            .is_none());
        assert!(EnvironmentVariableStore::get(&pool, "MY_FLAG")
            .await
            .expect("get")
            .is_some());
        assert!(env_file.exists(), "the held name keeps the file");

        EnvironmentVariableStore::upsert(&pool, &bus, "BASH_ENV", "/etc/profile", None)
            .await
            .expect("the user sets it in Settings");
        migrate_env_file_to_db(&pool, &bus, workspace.path()).await;
        assert!(!env_file.exists(), "nothing is left to import");

        crate::test_support::teardown_test_db(&db).await;
    }
}
