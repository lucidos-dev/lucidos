use std::path::{Path, PathBuf};

use sqlx::PgPool;

pub const FILE_NAME: &str = "database-migrated";

pub fn path(workspace: &Path) -> PathBuf {
    workspace.join(".lucidos").join(FILE_NAME)
}

pub fn read(workspace: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path(workspace)).ok()?;
    let identity = text.trim();
    (!identity.is_empty()).then(|| identity.to_string())
}

pub fn write(workspace: &Path, identity: &str) -> std::io::Result<()> {
    let marker = path(workspace);
    if let Some(dir) = marker.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(marker, format!("{identity}\n"))
}

pub fn forget(workspace: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path(workspace)) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationTable {
    Present,
    Missing,
    Unknown,
}

pub async fn migration_table(pool: &PgPool) -> MigrationTable {
    match sqlx::query_scalar::<_, bool>("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
        .fetch_one(pool)
        .await
    {
        Ok(true) => MigrationTable::Present,
        Ok(false) => MigrationTable::Missing,
        Err(_) => MigrationTable::Unknown,
    }
}

fn identity_of(cluster: Option<&str>, database: &str) -> String {
    match cluster {
        Some(cluster) => format!("{cluster}/{database}"),
        None => database.to_string(),
    }
}

async fn database_identity(database_url: &str, pool: &PgPool) -> Option<String> {
    let database = lucidos_pg_url::PgUrl::parse(database_url).ok()?.database;
    let cluster =
        sqlx::query_scalar::<_, String>("SELECT system_identifier::text FROM pg_control_system()")
            .fetch_one(pool)
            .await
            .ok();
    Some(identity_of(cluster.as_deref(), &database))
}

pub async fn marker_names(workspace: &Path, database_url: &str, pool: &PgPool) -> bool {
    let Some(marker) = read(workspace) else {
        return false;
    };
    database_identity(database_url, pool).await.as_deref() == Some(marker.as_str())
}

/// INVARIANT: The terminal boot-failure message when the database behind `database_url`
/// was wiped, else `None`. Runs before migrating. A wipe needs both facts:
/// the marker names this database, and its migration table is confirmed gone.
pub async fn refuse_wiped_database(
    workspace: &Path,
    database_url: &str,
    pool: &PgPool,
) -> Option<String> {
    let wiped = marker_names(workspace, database_url, pool).await
        && migration_table(pool).await == MigrationTable::Missing;
    wiped.then(|| crate::boot_failure::wiped_database_message(workspace))
}

pub async fn record_migrated(workspace: &Path, database_url: &str, pool: &PgPool) {
    let Some(identity) = database_identity(database_url, pool).await else {
        return;
    };
    if let Err(e) = write(workspace, &identity) {
        crate::log_legacy!(
            "[Startup] Could not write the migration marker {}: {}",
            path(workspace).display(),
            e
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_identity_names_the_cluster_when_it_is_known() {
        assert_eq!(identity_of(Some("7695"), "lucidos_dev"), "7695/lucidos_dev");
        assert_eq!(identity_of(None, "lucidos_dev"), "lucidos_dev");
    }

    #[test]
    fn the_marker_round_trips_and_forgetting_is_idempotent() {
        let workspace = tempfile::TempDir::new().expect("tempdir");
        assert_eq!(read(workspace.path()), None);
        write(workspace.path(), "lucidos_dev").expect("write");
        assert_eq!(read(workspace.path()).as_deref(), Some("lucidos_dev"));
        forget(workspace.path()).expect("forget");
        assert_eq!(read(workspace.path()), None);
        forget(workspace.path()).expect("forgetting twice is fine");
    }

    #[test]
    fn an_empty_marker_names_nothing() {
        let workspace = tempfile::TempDir::new().expect("tempdir");
        write(workspace.path(), "").expect("write");
        assert_eq!(read(workspace.path()), None);
    }

    #[tokio::test]
    async fn a_dropped_schema_is_refused_only_once_the_marker_names_it() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let url = crate::test_support::test_db_url(&db_name);
        let workspace = tempfile::TempDir::new().expect("tempdir");

        assert_eq!(migration_table(&pool).await, MigrationTable::Present);
        assert_eq!(
            refuse_wiped_database(workspace.path(), &url, &pool).await,
            None
        );
        record_migrated(workspace.path(), &url, &pool).await;
        let identity = read(workspace.path()).expect("the marker was written");
        assert!(identity.ends_with(&format!("/{db_name}")), "{identity}");
        assert!(marker_names(workspace.path(), &url, &pool).await);
        assert_eq!(
            refuse_wiped_database(workspace.path(), &url, &pool).await,
            None
        );

        sqlx::raw_sql("DROP SCHEMA public CASCADE")
            .execute(&pool)
            .await
            .expect("drop schema");
        assert_eq!(migration_table(&pool).await, MigrationTable::Missing);
        let message = refuse_wiped_database(workspace.path(), &url, &pool)
            .await
            .expect("a wiped database is refused");
        assert_eq!(
            message,
            crate::boot_failure::wiped_database_message(workspace.path())
        );

        let cluster = identity.trim_end_matches(&format!("/{db_name}"));
        for (other, why) in [
            (
                format!("{cluster}/lucidos_some_other_db"),
                "another database",
            ),
            (format!("1/{db_name}"), "the same name in another cluster"),
            (db_name.clone(), "a marker that never learned its cluster"),
        ] {
            write(workspace.path(), &other).expect("write");
            assert_eq!(
                refuse_wiped_database(workspace.path(), &url, &pool).await,
                None,
                "a marker for {why} never blocks"
            );
        }
        forget(workspace.path()).expect("forget");
        assert_eq!(
            refuse_wiped_database(workspace.path(), &url, &pool).await,
            None,
            "no marker never blocks"
        );

        pool.close().await;
        crate::test_support::teardown_test_db(&db_name).await;
    }

    mod guard_tests {
        use super::*;

        #[test]
        fn the_e2e_database_reset_forgets_the_marker() {
            let script =
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/lib/e2e.sh");
            let text = std::fs::read_to_string(&script).expect("read e2e.sh");
            let reset = text
                .split("reset_e2e_database() {")
                .nth(1)
                .and_then(|rest| rest.split("\n}\n").next())
                .expect("reset_e2e_database exists");
            assert!(
                reset.contains(&format!("rm -f \"$E2E_WORKSPACE/.lucidos/{FILE_NAME}\"")),
                "reset_e2e_database must delete the migration marker"
            );
        }
    }
}
