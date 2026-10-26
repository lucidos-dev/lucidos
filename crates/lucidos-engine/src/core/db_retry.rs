//! Retry a database read through a dropped connection.
//!
//! A Postgres crash restart closes every connection at once. A read in flight
//! then fails with "expected to read 5 bytes, got 0 bytes at EOF", and the
//! next attempt succeeds. The pool's acquire already waits for a recovering
//! server, so a short pause between attempts is enough.

use std::future::Future;
use std::time::Duration;

/// SQLSTATE classes a retry can clear: connection loss (08), rollback (40),
/// resource limits (53), a lock or busy object (55), operator shutdown (57)
/// and system faults (58). Every other class fails the same way each time.
const TRANSIENT_SQLSTATE_CLASSES: [&str; 6] = ["08", "40", "53", "55", "57", "58"];

/// How many times [`retry_transient_read`] runs a read before it gives up.
pub const READ_ATTEMPTS: u32 = 3;

/// The pause before the second attempt. Each later pause doubles it.
const FIRST_PAUSE: Duration = Duration::from_millis(200);

/// Whether a retry could clear the SQLSTATE `code`.
pub fn is_transient_sqlstate(code: &str) -> bool {
    TRANSIENT_SQLSTATE_CLASSES
        .iter()
        .any(|class| code.starts_with(class))
}

/// Whether a retry could clear `err`: a lost connection (EOF, reset, broken
/// pipe) or a server error of a transient class. A timed-out acquire has
/// already waited the pool's full acquire timeout, so it is not transient.
/// Nor is a closed pool, which means the engine is shutting down.
pub fn is_transient(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Io(_) => true,
        sqlx::Error::Database(db) => db.code().is_some_and(|code| is_transient_sqlstate(&code)),
        _ => false,
    }
}

/// Run `read` until it succeeds, fails for good, or has run [`READ_ATTEMPTS`]
/// times. Returns the last error. `what` names the read in the log.
///
/// Reads only: a write may have landed before its connection dropped, so
/// running it again could apply it twice.
pub async fn retry_transient_read<T, F, Fut>(what: &str, mut read: F) -> Result<T, sqlx::Error>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, sqlx::Error>>,
{
    let mut attempt = 1;
    let mut pause = FIRST_PAUSE;
    loop {
        match read().await {
            Err(e) if attempt < READ_ATTEMPTS && is_transient(&e) => {
                log!(
                    "[DbRetry] {} failed on attempt {}/{}, retrying in {}ms: {}",
                    what,
                    attempt,
                    READ_ATTEMPTS,
                    pause.as_millis(),
                    e
                );
                tokio::time::sleep(pause).await;
                attempt += 1;
                pause *= 2;
            }
            outcome => return outcome,
        }
    }
}

#[cfg(test)]
#[path = "db_retry_tests.rs"]
mod tests;
