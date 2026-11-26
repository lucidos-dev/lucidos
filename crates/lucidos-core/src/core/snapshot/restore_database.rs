use super::BoxError;
use std::{collections::BTreeMap, path::Path};

const ACCESS_TABLES: [&str; 3] = ["devices", "device_presence", "push_subscriptions"];

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

fn clear_carried_preferences() -> String {
    format!(
        "DELETE FROM public.preferences WHERE key = '{}';",
        crate::core::prefs::SNAPSHOT_RESTORE_BY_AGENTS.key()
    )
}

const ACCESS_ROWS_TIME_ZONE: &str = "SET LOCAL TimeZone = 'UTC';";

/// INVARIANT: Must succeed after normal first-boot migrations and before any auth, worker
/// or HTTP dispatch. Failure leaves staging intact for retry or recovery.
pub async fn finish_access_after_migrations(pool: &sqlx::PgPool) -> Result<(), BoxError> {
    async {
        let mut tx = pool.begin().await?;
        sqlx::raw_sql(ACCESS_ROWS_TIME_ZONE).execute(&mut *tx).await?;
        let staged: bool = sqlx::query_scalar("SELECT to_regclass('snapshot_restore_access.rows') IS NOT NULL")
            .fetch_one(&mut *tx).await?;
        if !staged {
            tx.commit().await?;
            return Ok(());
        }
        sqlx::raw_sql(&format!("LOCK TABLE snapshot_restore_access.rows IN ACCESS EXCLUSIVE MODE; TRUNCATE public.push_subscriptions, public.device_presence, public.devices; {}", clear_carried_preferences()))
            .execute(&mut *tx).await?;
        for (table, filter) in carried_rows() {
            sqlx::raw_sql(&format!(
                "UPDATE snapshot_restore_access.rows r SET value = r.value || COALESCE((
                   SELECT jsonb_object_agg(c.column_name, to_jsonb((r.value->>c.column_name)::timestamptz))
                   FROM information_schema.columns c
                   WHERE c.table_schema = 'public' AND c.table_name = '{table}'
                     AND c.data_type = 'timestamp with time zone'
                     AND jsonb_typeof(r.value->c.column_name) = 'string'
                 ), '{{}}'::jsonb)
                 WHERE r.table_name = '{table}';"
            ))
            .execute(&mut *tx)
            .await?;
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
        sqlx::raw_sql(
            "DELETE FROM public.backup_origin
             WHERE EXISTS (SELECT 1 FROM snapshot_restore_access.rows WHERE table_name = 'backup_origin');
             INSERT INTO public.backup_origin (workspace_path, origin)
             SELECT value->>'workspace_path', value->>'origin'
             FROM snapshot_restore_access.rows
             WHERE table_name = 'backup_origin' AND value <> 'null'::jsonb;",
        )
        .execute(&mut *tx)
        .await?;
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
    let mut before = format!("{ACCESS_ROWS_TIME_ZONE}\nCREATE TEMP TABLE snapshot_current_access (table_name text NOT NULL, value jsonb NOT NULL) ON COMMIT DROP;\n");
    // INVARIANT: A replay interrupted before first boot already has authoritative staging.
    // Rollback must carry that set, not the deliberately empty public tables.
    before.push_str("DO $snapshot$ BEGIN IF to_regclass('snapshot_restore_access.rows') IS NOT NULL THEN INSERT INTO pg_temp.snapshot_current_access SELECT * FROM snapshot_restore_access.rows; ELSE\n");
    for (table, filter) in carried_rows() {
        before.push_str(&format!(
            "INSERT INTO pg_temp.snapshot_current_access SELECT '{table}', to_jsonb(t) FROM public.{table} t WHERE {filter};\n"
        ));
    }
    before.push_str(
        "IF to_regclass('public.backup_origin') IS NULL THEN
             INSERT INTO pg_temp.snapshot_current_access VALUES ('backup_origin', 'null'::jsonb);
         ELSE
             INSERT INTO pg_temp.snapshot_current_access
             SELECT 'backup_origin', COALESCE((SELECT to_jsonb(t) FROM public.backup_origin t), 'null'::jsonb);
         END IF;\n",
    );
    before.push_str("END IF; END $snapshot$;\n");
    // INVARIANT: pg_restore --clean drops only objects named by its archive. Reset the
    // application schema too, so newer tables, views and routines cannot leak
    // into the restored point in time. Recreate the default schema for dumps
    // which omit its creation; temporary access copies survive outside it.
    // This executes inside the same transaction as replay and validation.
    before.push_str("DROP SCHEMA public CASCADE;\nCREATE SCHEMA public;\n");
    let mut after = format!(
        "SELECT 1 / CASE WHEN (SELECT count(*) FROM pg_catalog.pg_tables WHERE schemaname = 'public') = {} THEN 1 ELSE 0 END;\n",
        rows.len()
    );
    for (table, count) in rows {
        let quoted = table.replace('"', "\"\"");
        // INVARIANT: Never interpolate table names into a PL/pgSQL string literal. The
        // count itself is a typed nonnegative integer, not archive-provided SQL.
        after.push_str(&format!(
            "SELECT 1 / CASE WHEN (SELECT count(*) FROM public.\"{quoted}\") = {count} THEN 1 ELSE 0 END;\n"
        ));
    }
    after.push_str("TRUNCATE public.push_subscriptions, public.device_presence, public.devices;\n");
    after.push_str(&clear_carried_preferences());
    after.push_str("DROP SCHEMA IF EXISTS snapshot_restore_access CASCADE; CREATE SCHEMA snapshot_restore_access; REVOKE ALL ON SCHEMA snapshot_restore_access FROM PUBLIC; CREATE TABLE snapshot_restore_access.rows (table_name text NOT NULL, value jsonb NOT NULL);\n");
    after.push_str(
        "INSERT INTO snapshot_restore_access.rows SELECT * FROM pg_temp.snapshot_current_access;\n",
    );
    Ok((before, after))
}

#[cfg(test)]
#[path = "restore_database_tests.rs"]
mod tests;
