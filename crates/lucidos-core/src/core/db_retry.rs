use std::future::Future;
use std::time::Duration;

const TRANSIENT_SQLSTATE_CLASSES: [&str; 6] = ["08", "40", "53", "55", "57", "58"];

pub const READ_ATTEMPTS: u32 = 3;

const FIRST_PAUSE: Duration = Duration::from_millis(200);

pub fn is_transient_sqlstate(code: &str) -> bool {
    TRANSIENT_SQLSTATE_CLASSES
        .iter()
        .any(|class| code.starts_with(class))
}

pub fn is_transient(err: &sqlx::Error) -> bool {
    match err {
        sqlx::Error::Io(_) => true,
        sqlx::Error::Database(db) => db.code().is_some_and(|code| is_transient_sqlstate(&code)),
        _ => false,
    }
}

/// INVARIANT: Run `read` until it succeeds, fails for good, or has run [`READ_ATTEMPTS`]
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
                log_legacy!(
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
