use sqlx::migrate::{MigrateError, Migrator};
use sqlx::PgPool;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static REPORTED: AtomicBool = AtomicBool::new(false);

const REPORT_TIMEOUT: Duration = Duration::from_secs(2);

fn claim(reported: &AtomicBool) -> bool {
    !reported.swap(true, Ordering::SeqCst)
}

/// INVARIANT: Read the migration versions recorded in the database.
///
/// Best-effort: an unreadable `_sqlx_migrations` yields an empty list, which
/// only costs the message its counts — never an error of its own, since we are
/// already reporting someone else's failure. Read-only by construction: the
/// failure path must never mutate the database to make the engine start (that
/// judgment belongs to the user).
pub async fn applied_versions(pool: &PgPool) -> Vec<i64> {
    sqlx::query_scalar::<_, i64>("SELECT version FROM _sqlx_migrations ORDER BY version")
        .fetch_all(pool)
        .await
        .unwrap_or_default()
}

pub fn embedded_versions(migrator: &Migrator) -> Vec<i64> {
    migrator.iter().map(|m| m.version).collect()
}

/// INVARIANT: Turn a **terminal** migration failure into a sentence a non-technical user can
/// act on, or `None` when the failure is retryable.
///
/// The `None` arm is load-bearing, not a fallback. Reporting a boot failure stops
/// the gateway respawning, so classifying a *transient* error as terminal converts
/// a workspace that would have recovered on its own into a dead one. Only failures
/// that re-run identically forever qualify: a database ahead of this binary, a
/// migration whose content no longer matches, a half-applied migration, a
/// statement the server rejects with a deterministic SQLSTATE
/// ([`terminal_database_error_message`]). A dropped connection or an unreadable
/// migration source is what the supervisor's retry exists for. It stays
/// unreported, and the existing crash-recovery path handles it.
///
/// Pure, so the wording is unit-tested without a database. `applied` and
/// `embedded` are the recorded and compiled-in version lists; they are only
/// consulted for the newer-database case, where the *gap* is the useful detail.
///
/// Deliberately never names a target version to upgrade to: migrations carry no
/// app-version tag, so the newest unknown migration id cannot be mapped back to
/// a Lucidos release. Saying "install 0.16.0" would be a guess, so the message
/// gives the counts (which are facts) and says "a newer version" (which is true).
pub fn terminal_migration_message(
    err: &MigrateError,
    applied: &[i64],
    embedded: &[i64],
    engine_version: &str,
) -> Option<String> {
    Some(match err {
        MigrateError::VersionMissing(version) => {
            let mut unknown: Vec<i64> = applied
                .iter()
                .copied()
                .filter(|v| !embedded.contains(v))
                .collect();
            unknown.sort_unstable();
            let (count, newest) = match unknown.last() {
                Some(newest) => (unknown.len(), *newest),
                None => (1, *version),
            };
            let plural = if count == 1 { "" } else { "s" };
            format!(
                "Lucidos {engine_version} cannot open this workspace: its database was \
                 created by a newer version of Lucidos. The database contains \
                 {count} migration{plural} this version does not know about (newest: \
                 {newest}). Install a newer version of Lucidos to open this workspace."
            )
        }
        MigrateError::VersionMismatch(version) => format!(
            "Lucidos {engine_version} cannot open this workspace: migration {version} in this \
             version differs from the one already applied to its database. The database may \
             have been modified, or opened by a different build of Lucidos."
        ),
        MigrateError::Dirty(version) => format!(
            "Lucidos {engine_version} cannot open this workspace: migration {version} is only \
             partially applied to its database and must be repaired by hand."
        ),
        MigrateError::ExecuteMigration(source, version) => {
            return terminal_database_error_message(
                source,
                &format!("migration {version}"),
                engine_version,
            )
        }
        MigrateError::Execute(source) => {
            return terminal_database_error_message(source, "the migration check", engine_version)
        }
        // INVARIANT: Everything else, such as an unreadable migration source, may well
        // succeed on the next attempt. Staying silent leaves the supervisor's
        // respawn in charge, which recovers a workspace from a transient fault.
        _ => return None,
    })
}

const MAX_SERVER_MESSAGE_BYTES: usize = 300;

/// INVARIANT: Report a **deterministic** database error, or return `None`.
///
/// `None` means a retry could clear it (a dropped connection, a deadlock) or
/// that `err` carries no SQLSTATE.
///
/// The report names the step, the SQLSTATE and the server's primary message. It
/// never carries `detail`, `hint` or `where`. For a constraint violation the
/// detail is the failing row, which can hold an email address or a token.
pub fn terminal_database_error_message(
    err: &sqlx::Error,
    step: &str,
    engine_version: &str,
) -> Option<String> {
    let sqlx::Error::Database(db) = err else {
        return None;
    };
    let code = db.code()?;
    if crate::core::db_retry::is_transient_sqlstate(&code) {
        return None;
    }
    let message = db.message().lines().next().unwrap_or_default();
    let message = &message[..message.floor_char_boundary(MAX_SERVER_MESSAGE_BYTES)];
    Some(format!(
        "Lucidos {engine_version} cannot open this workspace: {step} failed with database \
         error {code}: {message}. Starting again will not change this."
    ))
}

pub fn wiped_database_message(workspace_dir: &std::path::Path) -> String {
    let marker = crate::core::migration_marker::path(workspace_dir);
    format!(
        "This workspace's database was emptied outside Lucidos, so Lucidos did not open it \
         blank. To recover, first use Restore from backup in the workspace picker to restore \
         your latest backup. Then ask the restored workspace's Lucidos Agent to copy over the \
         files changed since that backup from {}. To start this workspace empty instead, \
         delete {} and open it again.",
        workspace_dir.display(),
        marker.display()
    )
}

/// INVARIANT: Report a terminal boot failure to the gateway and WAIT for it to land.
///
/// Returns once the gateway has been reached (any response — a 404 from an older
/// gateway that lacks the endpoint counts, since retrying the other scheme would
/// not help), once both schemes have failed, or once the timeout expires. A
/// no-op when not gateway-spawned, and after the first call (see [`claim`]).
pub async fn report(message: &str) {
    crate::internal_faults::record_pending(crate::internal_faults::FaultKind::BootFailure);
    if !claim(&REPORTED) {
        return;
    }
    // INVARIANT: Log unconditionally: a non-gateway engine (dev / e2e) has no splash to
    // render this, and even under a gateway the log is the durable copy.
    crate::log_legacy!("[Startup] Boot failed: {}", message);
    let (Ok(port), Ok(id)) = (
        std::env::var("LUCIDOS_GATEWAY_PORT"),
        std::env::var("LUCIDOS_WORKSPACE_ID"),
    ) else {
        return;
    };
    post_failure(&port, &id, message).await;
}

async fn post_failure(port: &str, id: &str, message: &str) {
    let Ok(client) = crate::gateway_auth::client_builder()
        .timeout(REPORT_TIMEOUT)
        .build()
    else {
        return;
    };
    let url = format!(
        "{}/~/api/v1/control/workspaces/{id}/boot-failure",
        lucidos_local_token::client::origin(port)
    );
    let _ = client
        .post(&url)
        .json(&serde_json::json!({ "message": message }))
        .send()
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::server_error;
    use tokio::net::TcpListener;

    const VERSION: &str = "0.15.0";

    #[test]
    fn newer_database_message_names_the_gap_not_a_target_version() {
        let embedded: Vec<i64> = (1..=154).collect();
        let mut applied = embedded.clone();
        applied.extend([
            20260713144403,
            20260725071626,
            20260725200708,
            20260725211150,
            20260728091039,
        ]);

        let msg = terminal_migration_message(
            &MigrateError::VersionMissing(20260713144403),
            &applied,
            &embedded,
            VERSION,
        )
        .expect("a newer database is terminal");

        assert!(msg.contains("newer version of Lucidos"), "{msg}");
        assert!(msg.contains("5 migrations"), "{msg}");
        assert!(
            msg.contains("20260728091039"),
            "newest unknown version: {msg}"
        );
        assert!(msg.contains(VERSION), "names this build: {msg}");
        assert!(!msg.contains("0.16"), "must not fabricate a target: {msg}");
    }

    #[test]
    fn single_unknown_migration_is_not_pluralized() {
        let msg = terminal_migration_message(
            &MigrateError::VersionMissing(20260713144403),
            &[1, 20260713144403],
            &[1],
            VERSION,
        )
        .expect("terminal");
        assert!(msg.contains("1 migration this version"), "{msg}");
    }

    #[test]
    fn unreadable_migration_table_falls_back_to_the_reported_version() {
        let msg = terminal_migration_message(
            &MigrateError::VersionMissing(20260713144403),
            &[],
            &[],
            VERSION,
        )
        .expect("terminal");
        assert!(msg.contains("1 migration this version"), "{msg}");
        assert!(msg.contains("20260713144403"), "{msg}");
    }

    #[test]
    fn mismatch_and_dirty_each_get_their_own_wording() {
        let mismatch =
            terminal_migration_message(&MigrateError::VersionMismatch(42), &[], &[], VERSION)
                .expect("terminal");
        assert!(
            mismatch.contains("differs from the one already applied"),
            "{mismatch}"
        );

        let dirty = terminal_migration_message(&MigrateError::Dirty(42), &[], &[], VERSION)
            .expect("terminal");
        assert!(dirty.contains("partially applied"), "{dirty}");
    }

    #[test]
    fn retryable_migration_errors_are_not_reported_as_terminal() {
        assert!(
            terminal_migration_message(&MigrateError::ForceNotSupported, &[], &[], VERSION)
                .is_none(),
            "an unclassified error must leave the supervisor's retry in charge",
        );
        assert!(
            terminal_migration_message(
                &MigrateError::Source("connection reset".into()),
                &[],
                &[],
                VERSION,
            )
            .is_none(),
            "a transient source/IO fault must stay retryable",
        );
    }

    #[test]
    fn transient_database_errors_stay_retryable() {
        for code in [
            "08006", "40001", "40P01", "53300", "55P03", "57P01", "58030",
        ] {
            let err = server_error(code, "transient");
            assert!(
                terminal_database_error_message(&err, "migration 1", VERSION).is_none(),
                "{code} must stay retryable"
            );
            assert!(
                terminal_migration_message(
                    &MigrateError::ExecuteMigration(server_error(code, "transient"), 1),
                    &[],
                    &[],
                    VERSION
                )
                .is_none(),
                "{code} in a migration must stay retryable"
            );
        }
        let io = sqlx::Error::Io(std::io::Error::other("connection reset"));
        assert!(terminal_database_error_message(&io, "migration 1", VERSION).is_none());
        assert!(
            terminal_database_error_message(&sqlx::Error::PoolTimedOut, "x", VERSION).is_none()
        );
    }

    #[test]
    fn a_failing_migration_statement_is_terminal_and_names_version_code_and_message() {
        let msg = terminal_migration_message(
            &MigrateError::ExecuteMigration(
                server_error(
                    "P0001",
                    "retire_model: successor claude-fable-5-1 has no models row",
                ),
                20261018074151,
            ),
            &[],
            &[],
            VERSION,
        )
        .expect("a deterministic SQL error is terminal");
        assert!(msg.contains("migration 20261018074151"), "{msg}");
        assert!(msg.contains("P0001"), "{msg}");
        assert!(
            msg.contains("successor claude-fable-5-1 has no models row"),
            "{msg}"
        );
        assert!(msg.contains(VERSION), "{msg}");
    }

    #[test]
    fn a_long_multi_line_server_message_is_cut_to_one_bounded_line() {
        let long = format!("first line\nsecond line\n{}", "é".repeat(400));
        let msg = terminal_database_error_message(&server_error("23514", &long), "step", VERSION)
            .expect("terminal");
        assert!(msg.contains("first line"), "{msg}");
        assert!(!msg.contains("second line"), "{msg}");
        let long_line = "é".repeat(400);
        let msg =
            terminal_database_error_message(&server_error("23514", &long_line), "step", VERSION)
                .expect("terminal");
        assert!(msg.len() < 450, "{msg}");
    }

    #[tokio::test]
    async fn a_check_violation_report_carries_no_row_data() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        sqlx::raw_sql(
            "CREATE TABLE boot_failure_probe (email text, token text, \
             CONSTRAINT probe_sealed CHECK (token LIKE 'sealed:%'))",
        )
        .execute(&pool)
        .await
        .unwrap();
        let err = sqlx::query(
            "INSERT INTO boot_failure_probe VALUES ('person@example.test', 'plain-refresh-token')",
        )
        .execute(&pool)
        .await
        .expect_err("the CHECK refuses a plain token");
        let sqlx::Error::Database(db) = &err else {
            panic!("expected a database error: {err:?}");
        };
        let pg = db.downcast_ref::<sqlx::postgres::PgDatabaseError>();
        assert!(
            pg.detail()
                .is_some_and(|d| d.contains("person@example.test")),
            "the server detail carries the row, which is why the report drops it"
        );

        let msg = terminal_database_error_message(&err, "the secret check at boot", VERSION)
            .expect("23514 is terminal");
        assert!(msg.contains("23514"), "{msg}");
        assert!(msg.contains("probe_sealed"), "{msg}");
        assert!(!msg.contains("person@example.test"), "{msg}");
        assert!(!msg.contains("plain-refresh-token"), "{msg}");

        pool.close().await;
        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[test]
    fn the_wiped_database_message_puts_recovery_first_and_names_both_paths() {
        let workspace = std::path::Path::new("/home/user/workspaces/myws");
        let msg = wiped_database_message(workspace);
        let restore = msg.find("Restore from backup").expect("names the restore");
        let copy = msg.find("Lucidos Agent").expect("names the agent step");
        let escape = msg
            .find("start this workspace empty")
            .expect("names the escape");
        assert!(restore < copy && copy < escape, "{msg}");
        assert!(msg.contains("from /home/user/workspaces/myws."), "{msg}");
        assert!(
            msg.contains("delete /home/user/workspaces/myws/.lucidos/database-migrated"),
            "{msg}"
        );
    }

    #[test]
    fn only_the_first_report_is_claimed() {
        let flag = AtomicBool::new(false);
        assert!(claim(&flag), "first caller takes the slot");
        assert!(!claim(&flag), "specific message must not be overwritten");
        assert!(!claim(&flag));
    }

    type Seen = std::sync::Arc<std::sync::Mutex<Option<String>>>;

    fn stub_gateway(status: axum::http::StatusCode) -> (String, Seen) {
        let seen: Seen = Default::default();
        let record = seen.clone();
        let app = axum::Router::new().fallback(move |uri: axum::http::Uri, body: String| {
            let record = record.clone();
            async move {
                *record.lock().unwrap() = Some(format!("POST {} {body}", uri.path()));
                status
            }
        });
        let port = lucidos_local_token::test_support::serve_https(app).port();
        (port.to_string(), seen)
    }

    #[tokio::test]
    async fn post_reaches_the_gateway_before_returning() {
        let (port, seen) = stub_gateway(axum::http::StatusCode::NO_CONTENT);
        post_failure(&port, "ws-id", "database is newer").await;
        let request = seen.lock().unwrap().clone().unwrap_or_default();
        assert!(
            request.contains("POST /~/api/v1/control/workspaces/ws-id/boot-failure"),
            "{request}"
        );
        assert!(request.contains("database is newer"), "{request}");
    }

    #[tokio::test]
    async fn older_gateway_404_is_tolerated() {
        let (port, seen) = stub_gateway(axum::http::StatusCode::NOT_FOUND);
        post_failure(&port, "ws-id", "database is newer").await;
        let request = seen.lock().unwrap().clone().unwrap_or_default();
        assert!(request.contains("boot-failure"), "{request}");
    }

    #[tokio::test]
    async fn unreachable_gateway_returns_without_erroring() {
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            l.local_addr().expect("addr").port().to_string()
        };
        post_failure(&port, "ws-id", "database is newer").await;
    }
}
