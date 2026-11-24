use std::io::{Error as IoError, ErrorKind};
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;
use crate::test_support::server_error;

/// The error from the report: Postgres restarted and closed the socket.
fn eof() -> sqlx::Error {
    sqlx::Error::Io(IoError::new(
        ErrorKind::UnexpectedEof,
        "expected to read 5 bytes, got 0 bytes at EOF",
    ))
}

#[test]
fn a_lost_connection_is_transient() {
    for kind in [
        ErrorKind::UnexpectedEof,
        ErrorKind::ConnectionReset,
        ErrorKind::BrokenPipe,
    ] {
        assert!(
            is_transient(&sqlx::Error::Io(IoError::from(kind))),
            "{kind:?}"
        );
    }
}

/// 57P01 is a terminated backend, 57P02 a crash restart, 57P03 a server
/// still recovering, and 08006 a connection failure.
#[test]
fn a_server_error_of_a_transient_class_is_transient() {
    for code in [
        "08006", "40001", "53300", "55P03", "57P01", "57P02", "57P03",
    ] {
        assert!(is_transient(&server_error(code, "transient")), "{code}");
    }
}

#[test]
fn a_deterministic_error_is_not_transient() {
    for code in ["42P01", "23505", "42601"] {
        assert!(
            !is_transient(&server_error(code, "deterministic")),
            "{code}"
        );
    }
    assert!(!is_transient(&sqlx::Error::PoolClosed));
    assert!(!is_transient(&sqlx::Error::RowNotFound));
}

/// The acquire already waited its full timeout, so two more attempts would
/// only triple how long the turn hangs before it fails.
#[test]
fn a_timed_out_acquire_is_not_retried() {
    assert!(!is_transient(&sqlx::Error::PoolTimedOut));
}

#[tokio::test(start_paused = true)]
async fn a_read_that_drops_its_connection_once_succeeds_on_the_retry() {
    let calls = AtomicU32::new(0);
    let outcome = retry_transient_read("the test read", || async {
        match calls.fetch_add(1, Ordering::SeqCst) {
            0 => Err(eof()),
            _ => Ok("rows"),
        }
    })
    .await;
    assert_eq!(outcome.unwrap(), "rows");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn a_deterministic_error_fails_at_once() {
    let calls = AtomicU32::new(0);
    let outcome: Result<(), _> = retry_transient_read("the test read", || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Err(server_error("42P01", "relation does not exist"))
    })
    .await;
    assert!(matches!(outcome, Err(sqlx::Error::Database(_))));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(start_paused = true)]
async fn a_connection_that_stays_down_fails_after_the_last_attempt() {
    let calls = AtomicU32::new(0);
    let started = tokio::time::Instant::now();
    let outcome: Result<(), _> = retry_transient_read("the test read", || async {
        calls.fetch_add(1, Ordering::SeqCst);
        Err(eof())
    })
    .await;
    assert!(matches!(outcome, Err(sqlx::Error::Io(_))));
    assert_eq!(calls.load(Ordering::SeqCst), READ_ATTEMPTS);
    // Each pause doubles the one before, and none follows the last attempt.
    let pauses: Duration = (0..READ_ATTEMPTS - 1)
        .map(|i| FIRST_PAUSE * 2u32.pow(i))
        .sum();
    assert_eq!(started.elapsed(), pauses);
}
