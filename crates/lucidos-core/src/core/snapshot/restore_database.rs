//! Offline database replay. The caller authenticates the dump and retains the
//! archive locks. Replay clears archived grants and durably stages the current
//! carried rows. First boot installs them after migrations, before dispatch.
use super::BoxError;
use std::{collections::BTreeMap, path::Path};

const ACCESS_TABLES: [&str; 3] = ["devices", "device_presence", "push_subscriptions"];

/// The rows a restore takes from the current timeline, never the archive: the
/// access tables whole, and the preference that lets agents restore.
fn carried_rows() -> Vec<(&'static str, String)> {
    let mut carried: Vec<_> = ACCESS_TABLES
        .into_iter()
        .map(|table| (table, "TRUE".to_owned()))
        .collect();
    carried.push((
        "preferences",
        format!(
            "key = '{}'",
            crate::core::prefs::SNAPSHOT_RESTORE_BY_AGENTS.key()
        ),
    ));
    carried
}

/// Removes every carried preference, archived or current.
fn clear_carried_preferences() -> String {
    format!(
        "DELETE FROM public.preferences WHERE key = '{}';",
        crate::core::prefs::SNAPSHOT_RESTORE_BY_AGENTS.key()
    )
}

/// Must succeed after normal first-boot migrations and before any auth, worker
/// or HTTP dispatch. Failure leaves staging intact for retry or recovery.
pub async fn finish_access_after_migrations(pool: &sqlx::PgPool) -> Result<(), BoxError> {
    async {
        let mut tx = pool.begin().await?;
        // A crash after this transaction commits but before first boot removes
        // its external marker must be safe to retry. Replay creates staging
        // atomically, and only this transaction consumes it.
        let staged: bool = sqlx::query_scalar("SELECT to_regclass('snapshot_restore_access.rows') IS NOT NULL")
            .fetch_one(&mut *tx).await?;
        if !staged {
            tx.commit().await?;
            return Ok(());
        }
        sqlx::raw_sql(&format!("LOCK TABLE snapshot_restore_access.rows IN ACCESS EXCLUSIVE MODE; TRUNCATE public.push_subscriptions, public.device_presence, public.devices; {}", clear_carried_preferences()))
            .execute(&mut *tx).await?;
        for (table, filter) in carried_rows() {
            // Populate by name, never by the column order of an older archive.
            // EXCEPT ALL in both directions checks exact values AND cardinality;
            // jsonb_populate_record otherwise silently ignores missing columns.
            let statement = format!(
                "INSERT INTO public.{table} SELECT (jsonb_populate_record(NULL::public.{table}, value)).* FROM snapshot_restore_access.rows WHERE table_name = '{table}';
                 SELECT 1 / CASE WHEN NOT EXISTS (
                   (SELECT to_jsonb(t) FROM public.{table} t WHERE {filter} EXCEPT ALL SELECT value FROM snapshot_restore_access.rows WHERE table_name = '{table}')
                   UNION ALL
                   (SELECT value FROM snapshot_restore_access.rows WHERE table_name = '{table}' EXCEPT ALL SELECT to_jsonb(t) FROM public.{table} t WHERE {filter})
                 ) THEN 1 ELSE 0 END;"
            );
            sqlx::raw_sql(&statement).execute(&mut *tx).await?;
        }
        sqlx::raw_sql("DROP SCHEMA snapshot_restore_access CASCADE;")
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(())
    }.await.map_err(|_| "snapshot_access_restore_failed".into())
}

pub fn restore(
    database_url: &str,
    dump: &Path,
    expected_rows: &BTreeMap<String, i64>,
) -> Result<(), BoxError> {
    let (before, after) =
        transaction_checks(expected_rows).map_err(|_| "snapshot_database_restore_failed")?;
    crate::core::backup::pg_restore_with_checks(database_url, dump, &before, &after)
        .map_err(|_| "snapshot_database_restore_failed".into())
}

fn transaction_checks(rows: &BTreeMap<String, i64>) -> Result<(String, String), BoxError> {
    if rows.is_empty()
        || rows.values().any(|count| *count < 0)
        || ACCESS_TABLES.iter().any(|table| !rows.contains_key(*table))
    {
        return Err("invalid snapshot table counts".into());
    }
    let mut before = String::from("CREATE TEMP TABLE snapshot_current_access (table_name text NOT NULL, value jsonb NOT NULL) ON COMMIT DROP;\n");
    // A replay interrupted before first boot already has authoritative staging.
    // Rollback must carry that set, not the deliberately empty public tables.
    before.push_str("DO $snapshot$ BEGIN IF to_regclass('snapshot_restore_access.rows') IS NOT NULL THEN INSERT INTO pg_temp.snapshot_current_access SELECT * FROM snapshot_restore_access.rows; ELSE\n");
    for (table, filter) in carried_rows() {
        before.push_str(&format!(
            "INSERT INTO pg_temp.snapshot_current_access SELECT '{table}', to_jsonb(t) FROM public.{table} t WHERE {filter};\n"
        ));
    }
    before.push_str("END IF; END $snapshot$;\n");
    // pg_restore --clean drops only objects named by its archive. Reset the
    // application schema too, so newer tables, views and routines cannot leak
    // into the restored point in time. Recreate the default schema for dumps
    // which omit its creation; temporary access copies survive outside it.
    // This executes inside the same transaction as replay and validation.
    before.push_str("DROP SCHEMA public CASCADE;\nCREATE SCHEMA public;\n");
    // Keep an independent cardinality check after replay rather than assuming
    // cleanup or an archive's schema section produced the expected table set.
    let mut after = format!(
        "SELECT 1 / CASE WHEN (SELECT count(*) FROM pg_catalog.pg_tables WHERE schemaname = 'public') = {} THEN 1 ELSE 0 END;\n",
        rows.len()
    );
    for (table, count) in rows {
        let quoted = table.replace('"', "\"\"");
        // Never interpolate table names into a PL/pgSQL string literal. The
        // count itself is a typed nonnegative integer, not archive-provided SQL.
        after.push_str(&format!(
            "SELECT 1 / CASE WHEN (SELECT count(*) FROM public.\"{quoted}\") = {count} THEN 1 ELSE 0 END;\n"
        ));
    }
    after.push_str("TRUNCATE public.push_subscriptions, public.device_presence, public.devices;\n");
    after.push_str(&clear_carried_preferences());
    // Keep staging outside public so a crash/replay cannot mix archived grants
    // with current access. No keys or plaintext secrets leave PostgreSQL.
    after.push_str("DROP SCHEMA IF EXISTS snapshot_restore_access CASCADE; CREATE SCHEMA snapshot_restore_access; REVOKE ALL ON SCHEMA snapshot_restore_access FROM PUBLIC; CREATE TABLE snapshot_restore_access.rows (table_name text NOT NULL, value jsonb NOT NULL);\n");
    after.push_str(
        "INSERT INTO snapshot_restore_access.rows SELECT * FROM pg_temp.snapshot_current_access;\n",
    );
    Ok((before, after))
}

#[cfg(test)]
#[path = "restore_database_tests.rs"]
mod tests;
