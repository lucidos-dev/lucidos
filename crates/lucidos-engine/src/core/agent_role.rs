//! The agent role: the database role behind `LUCIDOS_DATABASE_URL` (ADR 0509).
//!
//! Agents, scripts and triggers sign in as their workspace's agent role, never
//! as the engine. It reads every table except secret columns and can connect
//! to no other database. Writes use an allow-list unless the gateway marks
//! the workspace as machine-made.
//! The engine sets it up at every boot, so a new table or secret column gets
//! the right grants on the next start.

use sqlx::PgPool;

use crate::core::secret_store::{MOVED_SECRET_COLUMNS, SECRET_COLUMNS};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Tables an agent may read but never write. An agent that could write the
/// backup recipient could seal every later backup to a key of its own. One
/// that could write a mailbox's hosts could send its password to a server of
/// its own at the next login. One that could clear the restore boundary could
/// spend an owner approval the restore expired (ADR 0612).
const READ_ONLY_TABLES: &[&str] = &[
    "_sqlx_migrations",
    "backup_recipient",
    "email_accounts",
    "snapshot_restore_boundary",
];

/// Content and UI state that grant no authority. New tables fail closed.
const AGENT_WRITABLE_TABLES: &[&str] = &[
    "preferences",
    "memory_entries",
    "summary_tree_nodes",
    "summary_tree_scopes",
    "notification_reads",
    "pinned_apps",
    "browser_logins",
];

fn is_machine_made(origin: Option<&str>) -> bool {
    matches!(origin, Some("agent" | "demo" | "eval" | "e2e"))
}

/// A role, database, table or column name, quoted for SQL. Postgres cannot
/// bind an identifier. A workspace database name holds a hyphen, so quoting,
/// not a plain-name check, is what keeps the name whole and safe.
fn ident(name: &str) -> Result<String, BoxError> {
    if name.is_empty() || name.len() > lucidos_pg_url::PG_MAX_IDENTIFIER_BYTES {
        return Err(format!("{name:?} cannot name a database object").into());
    }
    Ok(format!("\"{}\"", name.replace('"', "\"\"")))
}

/// Create or refresh the agent role for the database `database_url` names,
/// with a new password, and return the read-only URL that signs in as it.
pub async fn provision(
    pool: &PgPool,
    database_url: &str,
    origin: Option<&str>,
) -> Result<String, BoxError> {
    let mut url = lucidos_pg_url::PgUrl::parse(database_url)?;
    let role = lucidos_pg_url::agent_role(&url.database);
    let password = lucidos_local_token::mint_hex(32)?;
    let (role_ident, group, database) = (
        ident(&role)?,
        ident(lucidos_pg_url::AGENT_ROLE_GROUP)?,
        ident(&url.database)?,
    );

    // Two engines booting at once may both create the group or a role; the
    // loser's duplicate is the state it wanted.
    for statement in [
        format!(
            "DO $$ BEGIN CREATE ROLE {group} NOLOGIN; \
             EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$"
        ),
        format!(
            "DO $$ BEGIN CREATE ROLE {role_ident} LOGIN; \
             EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL; END $$"
        ),
        format!(
            "ALTER ROLE {role_ident} WITH LOGIN NOSUPERUSER NOCREATEDB NOCREATEROLE PASSWORD '{}'",
            scram_verifier(&password)
        ),
        format!("GRANT {group} TO {role_ident}"),
        format!("REVOKE ALL ON DATABASE {database} FROM PUBLIC"),
        format!("GRANT CONNECT, TEMPORARY ON DATABASE {database} TO {role_ident}"),
        format!("GRANT USAGE ON SCHEMA public TO {role_ident}"),
        format!("GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO {role_ident}"),
    ] {
        sqlx::query(&statement).execute(pool).await?;
    }
    grant_tables(pool, &role_ident, is_machine_made(origin)).await?;
    guard_human_only_preferences(pool, &role).await?;

    url.user = Some(role);
    url.password = Some(password);
    Ok(url.read_only_url())
}

const SCRAM_ITERATIONS: u32 = 4096;

/// The SCRAM-SHA-256 verifier Postgres stores for `password` (RFC 5802). A
/// failed `ALTER ROLE` lands in the server log whole, so the statement carries
/// this verifier, which signs nobody in, never the password itself.
fn scram_verifier(password: &str) -> String {
    use base64::Engine as _;
    use hmac::{Hmac, Mac};
    use rand::RngCore as _;
    use sha2::{Digest, Sha256};

    let hmac = |key: &[u8], parts: &[&[u8]]| -> [u8; 32] {
        let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key length");
        for part in parts {
            mac.update(part);
        }
        mac.finalize().into_bytes().into()
    };
    let mut salt = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut salt);

    // PBKDF2-HMAC-SHA-256 with one output block, the length SCRAM needs.
    let mut block = hmac(password.as_bytes(), &[&salt, &1u32.to_be_bytes()]);
    let mut salted = block;
    for _ in 1..SCRAM_ITERATIONS {
        block = hmac(password.as_bytes(), &[&block]);
        salted.iter_mut().zip(block).for_each(|(s, b)| *s ^= b);
    }

    let stored_key = Sha256::digest(hmac(&salted, &[b"Client Key"]));
    let server_key = hmac(&salted, &[b"Server Key"]);
    let b64 = base64::engine::general_purpose::STANDARD;
    format!(
        "SCRAM-SHA-256${SCRAM_ITERATIONS}:{}${}:{}",
        b64.encode(salt),
        b64.encode(stored_key),
        b64.encode(server_key)
    )
}

/// Read every table, leaving secret columns out. Writes follow the workspace
/// origin and allow-list. Tables holding secrets get only column-level reads.
async fn grant_tables(pool: &PgPool, role: &str, machine_made: bool) -> Result<(), BoxError> {
    let tables: Vec<(String, String)> = sqlx::query_as(
        "SELECT table_name::text, table_type::text FROM information_schema.tables \
         WHERE table_schema = 'public'",
    )
    .fetch_all(pool)
    .await?;
    let mut tx = pool.begin().await?;
    for (table, kind) in tables {
        let table_ident = ident(&table)?;
        sqlx::query(&format!("REVOKE ALL ON {table_ident} FROM {role}"))
            .execute(&mut *tx)
            .await?;
        // A moved column still holds sealed values until the boot step drops
        // it, so it stays hidden for as long as it exists.
        let secret: Vec<&str> = SECRET_COLUMNS
            .iter()
            .chain(MOVED_SECRET_COLUMNS)
            .filter(|spec| spec.table == table)
            .map(|spec| spec.column)
            .collect();
        let columns: Vec<String> = if secret.is_empty() {
            Vec::new()
        } else {
            sqlx::query_scalar(
                "SELECT column_name::text FROM information_schema.columns \
                 WHERE table_schema = 'public' AND table_name = $1",
            )
            .bind(&table)
            .fetch_all(&mut *tx)
            .await?
        };
        let (hidden, readable): (Vec<String>, Vec<String>) = columns
            .into_iter()
            .partition(|column| secret.contains(&column.as_str()));
        let grant = if !hidden.is_empty() {
            sqlx::query(&format!(
                "REVOKE ALL ({}) ON {table_ident} FROM {role}",
                idents(&hidden)?
            ))
            .execute(&mut *tx)
            .await?;
            format!(
                "GRANT SELECT ({}) ON {table_ident} TO {role}",
                idents(&readable)?
            )
        } else if kind != "BASE TABLE"
            || READ_ONLY_TABLES.contains(&table.as_str())
            || (!machine_made && !AGENT_WRITABLE_TABLES.contains(&table.as_str()))
        {
            format!("GRANT SELECT ON {table_ident} TO {role}")
        } else {
            format!("GRANT SELECT, INSERT, UPDATE, DELETE ON {table_ident} TO {role}")
        };
        sqlx::query(&grant).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Refuse an agent-role write to a preference the catalog keeps from agents.
///
/// `set_preference` and the HTTP route refuse these keys already, but the
/// handed database credential reached them directly. One of them,
/// `snapshot_restore_by_agents`, lets agents restore without a card.
/// The trigger is rebuilt on every boot, so the key list follows the catalog.
/// It names the agent role exactly: a superuser counts as a member of every
/// group, so a membership test would refuse the engine itself.
async fn guard_human_only_preferences(pool: &PgPool, role: &str) -> Result<(), BoxError> {
    let literal = |text: &str| format!("'{}'", text.replace('\'', "''"));
    let migrated: bool = sqlx::query_scalar("SELECT to_regclass('public.preferences') IS NOT NULL")
        .fetch_one(pool)
        .await?;
    if !migrated {
        return Ok(());
    }
    let keys = crate::core::prefs::internal_specs()
        .map(|spec| literal(spec.key))
        .collect::<Vec<_>>()
        .join(", ");
    let role = literal(role);
    let mut tx = pool.begin().await?;
    sqlx::raw_sql(&format!(
        "CREATE OR REPLACE FUNCTION public.lucidos_refuse_agent_preference_write() \
         RETURNS trigger LANGUAGE plpgsql AS $guard$ BEGIN \
           IF current_user = {role} \
              AND (OLD.key = ANY (ARRAY[{keys}]) OR NEW.key = ANY (ARRAY[{keys}])) THEN \
             RAISE EXCEPTION 'permission denied: only the owner can change this preference' \
               USING ERRCODE = 'insufficient_privilege'; \
           END IF; \
           IF TG_OP = 'DELETE' THEN RETURN OLD; END IF; \
           RETURN NEW; \
         END $guard$; \
         DROP TRIGGER IF EXISTS lucidos_refuse_agent_preference_write ON public.preferences; \
         CREATE TRIGGER lucidos_refuse_agent_preference_write \
           BEFORE INSERT OR UPDATE OR DELETE ON public.preferences \
           FOR EACH ROW EXECUTE FUNCTION public.lucidos_refuse_agent_preference_write();"
    ))
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

fn idents(names: &[String]) -> Result<String, BoxError> {
    Ok(names
        .iter()
        .map(|name| ident(name))
        .collect::<Result<Vec<_>, _>>()?
        .join(", "))
}

#[cfg(test)]
#[path = "agent_role_tests.rs"]
mod tests;
