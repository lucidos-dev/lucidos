//! Terminal boot-failure reporting to the workspace gateway (ADR 0014 §11).
//!
//! The sibling [`crate::boot_report`] narrates boot *progress*; this module
//! reports boot *death*. When startup fails in a way no retry can fix — most
//! importantly a database migrated by a NEWER Lucidos than this binary, which
//! is what an app downgrade produces — the engine exits and the gateway is left
//! guessing. Without a reported reason it respawns us to the restart cap and
//! then falls back to the neutral "Workspace starting…" splash, so the user
//! stares at a spinner while the actual, actionable cause sits in a log file
//! they will never open. (Exactly the 2026-07-29 incident: installing the 0.15.0
//! DMG over a database the 0.16.0 RC had migrated produced
//! `Error: VersionMissing(20260713144403)` on every spawn and a permanent
//! splash.)
//!
//! Two things differ from `boot_report`, and both are load-bearing:
//!
//! 1. **The POST is awaited, not detached.** `boot_report` spawns a detached
//!    task because a phase report must never delay the boot. Here the process is
//!    about to *exit*, so a detached task would simply never run — the report has
//!    to land before we return.
//! 2. **Only a CLASSIFIED-terminal failure is reported**
//!    ([`terminal_migration_message`] returns `Option`, and a wiped database
//!    needs the migration marker's say-so). Reporting stops the
//!    gateway respawning, so treating an unclassified error as terminal would
//!    turn a workspace that recovers on its own — a dropped connection during
//!    migrations, say — into a permanently dead one. When in doubt, say nothing
//!    and let the supervisor retry.
//!
//! Still best-effort in every other respect: a short timeout, all transport
//! errors swallowed, and a no-op when we were not spawned by a gateway
//! (`LUCIDOS_GATEWAY_PORT` / `LUCIDOS_WORKSPACE_ID` unset — the
//! `LUCIDOS_NO_GATEWAY` dev mode and the e2e direct-engine harness). Reporting
//! must never change the exit code or invent a failure of its own.

use sqlx::migrate::{MigrateError, Migrator};
use sqlx::PgPool;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Per-process first-report guard, so a second classified-terminal site (or a
/// retried construction) cannot overwrite the message already delivered.
static REPORTED: AtomicBool = AtomicBool::new(false);

/// How long one POST attempt may take. Bounded because we are on the exit path:
/// two schemes × this timeout is the worst case a user waits before the process
/// dies, and a dead gateway must not turn a fast failure into a hang.
const REPORT_TIMEOUT: Duration = Duration::from_secs(2);

/// Take the single report slot, returning `true` for the first caller only.
///
/// Split out (and parameterized over the flag) so the rule is unit testable
/// without touching the process-global [`REPORTED`].
fn claim(reported: &AtomicBool) -> bool {
    !reported.swap(true, Ordering::SeqCst)
}

/// Read the migration versions recorded in the database.
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

/// The migration versions this binary carries.
pub fn embedded_versions(migrator: &Migrator) -> Vec<i64> {
    migrator.iter().map(|m| m.version).collect()
}

/// Turn a **terminal** migration failure into a sentence a non-technical user can
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
        // The downgrade case: the database records migrations this build has
        // never heard of, so sqlx refuses to run rather than guess.
        MigrateError::VersionMissing(version) => {
            let mut unknown: Vec<i64> = applied
                .iter()
                .copied()
                .filter(|v| !embedded.contains(v))
                .collect();
            unknown.sort_unstable();
            // `unknown` is empty only if `_sqlx_migrations` was unreadable; fall
            // back to the single version sqlx named so the message still helps.
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
        // The migration file shipped in this build differs from the one that was
        // actually applied — a modified database, or two builds from different
        // lineages sharing one workspace.
        MigrateError::VersionMismatch(version) => format!(
            "Lucidos {engine_version} cannot open this workspace: migration {version} in this \
             version differs from the one already applied to its database. The database may \
             have been modified, or opened by a different build of Lucidos."
        ),
        // Only reachable on a database without transactional DDL, but the
        // remedy is specific enough to be worth its own wording.
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
        // Everything else, such as an unreadable migration source, may well
        // succeed on the next attempt. Staying silent leaves the supervisor's
        // respawn in charge, which recovers a workspace from a transient fault.
        _ => return None,
    })
}

/// SQLSTATE classes a retry can clear: connection loss (08), rollback (40),
/// resource limits (53), a lock or busy object (55), operator shutdown (57)
/// and system faults (58). Every other class fails the same way each time.
const TRANSIENT_SQLSTATE_CLASSES: [&str; 6] = ["08", "40", "53", "55", "57", "58"];

/// The longest server message a report carries.
const MAX_SERVER_MESSAGE_BYTES: usize = 300;

/// Report a **deterministic** database error, or return `None`.
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
    if TRANSIENT_SQLSTATE_CLASSES
        .iter()
        .any(|class| code.starts_with(class))
    {
        return None;
    }
    let message = db.message().lines().next().unwrap_or_default();
    let message = &message[..message.floor_char_boundary(MAX_SERVER_MESSAGE_BYTES)];
    Some(format!(
        "Lucidos {engine_version} cannot open this workspace: {step} failed with database \
         error {code}: {message}. Starting again will not change this."
    ))
}

/// The message for a database whose schema was wiped after it was migrated
/// (ADR 0481). Terminal: migrating would boot a blank workspace.
///
/// Recovery comes first, then the escape hatch. The workspace's files are
/// intact, so the restored workspace's agent can copy over what changed.
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

/// Report a terminal boot failure to the gateway and WAIT for it to land.
///
/// Returns once the gateway has been reached (any response — a 404 from an older
/// gateway that lacks the endpoint counts, since retrying the other scheme would
/// not help), once both schemes have failed, or once the timeout expires. A
/// no-op when not gateway-spawned, and after the first call (see [`claim`]).
pub async fn report(message: &str) {
    if !claim(&REPORTED) {
        return;
    }
    // Log unconditionally: a non-gateway engine (dev / e2e) has no splash to
    // render this, and even under a gateway the log is the durable copy.
    crate::log!("[Startup] Boot failed: {}", message);
    let (Ok(port), Ok(id)) = (
        std::env::var("LUCIDOS_GATEWAY_PORT"),
        std::env::var("LUCIDOS_WORKSPACE_ID"),
    ) else {
        return; // not gateway-spawned — the log above is the whole report
    };
    post_failure(&port, &id, message).await;
}

/// The bare POST, split from [`report`] so it is testable against a stub server
/// without setting process-global env vars.
async fn post_failure(port: &str, id: &str, message: &str) {
    // Same posture as `boot_report` / `api/history.rs::restart_via_gateway`:
    // loopback call to the co-located gateway, accept its self-signed dev cert,
    // bypass any ambient proxy.
    let Ok(client) = crate::gateway_auth::client_builder()
        .timeout(REPORT_TIMEOUT)
        .build()
    else {
        return;
    };
    // Scheme via `net_config::peer_scheme_order` (never hardcoded — the dev
    // gateway serves TLS, packaged serves plain http).
    for scheme in crate::net_config::peer_scheme_order() {
        let url =
            format!("{scheme}://127.0.0.1:{port}/~/api/v1/control/workspaces/{id}/boot-failure");
        if client
            .post(&url)
            .json(&serde_json::json!({ "message": message }))
            .send()
            .await
            .is_ok()
        {
            return; // reached the gateway (any status) — done
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    const VERSION: &str = "0.15.0";

    /// The 2026-07-29 incident, as a test: 159 applied vs 154 embedded.
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
        // The invariant that keeps the message honest — migrations carry no
        // app-version tag, so no upgrade target may be invented.
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

    /// An unreadable `_sqlx_migrations` leaves the lists empty; the message must
    /// still name the version sqlx reported rather than claiming "0 migrations".
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

    /// The load-bearing negative case. Reporting stops the gateway respawning, so
    /// a RETRYABLE migration error must stay unreported — classifying a dropped
    /// connection as terminal would turn a workspace that recovers on its own into
    /// a permanently dead one.
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

    #[derive(Debug)]
    struct ServerError {
        code: &'static str,
        message: String,
    }

    impl std::fmt::Display for ServerError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.message)
        }
    }

    impl std::error::Error for ServerError {}

    impl sqlx::error::DatabaseError for ServerError {
        fn message(&self) -> &str {
            &self.message
        }
        fn code(&self) -> Option<std::borrow::Cow<'_, str>> {
            Some(self.code.into())
        }
        fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
            self
        }
        fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
            self
        }
        fn kind(&self) -> sqlx::error::ErrorKind {
            sqlx::error::ErrorKind::Other
        }
    }

    fn server_error(code: &'static str, message: &str) -> sqlx::Error {
        sqlx::Error::Database(Box::new(ServerError {
            code,
            message: message.to_string(),
        }))
    }

    /// Reporting stops the gateway respawning, so each of these must stay
    /// retryable, including an error that carries no SQLSTATE.
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

    /// The 07:46 incident: a `CHECK` violation whose `detail` held a contributor's
    /// email and a token prefix. The real server error must yield a message with
    /// none of it.
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

    /// Serve plain HTTP, reply with `status`, and hand back the first real HTTP
    /// request seen. A hand-rolled listener keeps this dependency-free.
    ///
    /// It must ACCEPT IN A LOOP rather than serve exactly one connection:
    /// `peer_scheme_order()` puts https first whenever this process resolves to
    /// TLS (which it does under the dev env), so the reporter's first attempt is a
    /// TLS ClientHello into this plaintext socket. That connection has to be
    /// consumed and discarded so the http retry — the one carrying the actual
    /// request — still finds a listener.
    async fn stub_gateway(status: &'static str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().expect("addr").port().to_string();
        let handle = tokio::spawn(async move {
            // Bounded so a genuinely broken reporter fails the test instead of
            // hanging it.
            for _ in 0..4 {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return String::new();
                };
                let mut buf = vec![0u8; 4096];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let text = String::from_utf8_lossy(&buf[..n]).to_string();
                if !text.starts_with("POST ") {
                    continue; // TLS handshake bytes from the https-first attempt
                }
                let _ = sock
                    .write_all(format!("HTTP/1.1 {status}\r\ncontent-length: 0\r\n\r\n").as_bytes())
                    .await;
                let _ = sock.flush().await;
                return text;
            }
            String::new()
        });
        (port, handle)
    }

    /// The report must be OBSERVED by the gateway before `post_failure` returns —
    /// a detached send would let the process exit first and lose it entirely.
    #[tokio::test]
    async fn post_reaches_the_gateway_before_returning() {
        let (port, handle) = stub_gateway("204 No Content").await;
        post_failure(&port, "ws-id", "database is newer").await;
        let request = handle.await.expect("stub");
        assert!(
            request.contains("POST /~/api/v1/control/workspaces/ws-id/boot-failure"),
            "{request}"
        );
        assert!(request.contains("database is newer"), "{request}");
    }

    /// An older gateway has no such route. A 404 is "reached the gateway" — we
    /// must not error, hang, or fall through to a pointless second attempt.
    #[tokio::test]
    async fn older_gateway_404_is_tolerated() {
        let (port, handle) = stub_gateway("404 Not Found").await;
        post_failure(&port, "ws-id", "database is newer").await;
        assert!(handle.await.expect("stub").contains("boot-failure"));
    }

    /// Nothing listening: both schemes fail fast and the call still returns.
    #[tokio::test]
    async fn unreachable_gateway_returns_without_erroring() {
        // Bind then drop, so the port is almost certainly free.
        let port = {
            let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
            l.local_addr().expect("addr").port().to_string()
        };
        post_failure(&port, "ws-id", "database is newer").await;
    }
}
