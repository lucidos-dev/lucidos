//! Retry policy shared by the build script and its offline regression tests.
use std::io;

#[derive(Debug)]
pub struct FetchError {
    pub message: String,
    pub transient: bool,
}

impl FetchError {
    pub fn http(status: u16, message: String) -> Self {
        Self {
            message,
            transient: (500..600).contains(&status) || matches!(status, 408 | 429),
        }
    }

    pub fn transport(error: &(dyn std::error::Error + 'static), mut transient: bool) -> Self {
        let mut source = Some(error);
        while let Some(cause) = source {
            if let Some(io) = cause.downcast_ref::<io::Error>() {
                transient |= Self::io(io::Error::from(io.kind())).transient;
            }
            source = cause.source();
        }
        Self {
            message: error.to_string(),
            transient,
        }
    }

    pub fn io(error: io::Error) -> Self {
        let transient = matches!(
            error.kind(),
            io::ErrorKind::TimedOut
                | io::ErrorKind::ConnectionReset
                | io::ErrorKind::ConnectionAborted
                | io::ErrorKind::Interrupted
                | io::ErrorKind::UnexpectedEof
        );
        Self {
            message: error.to_string(),
            transient,
        }
    }
}

pub fn fetch_sources(
    sources: impl IntoIterator<Item = String>,
    mut fetch: impl FnMut(&str) -> Result<Vec<u8>, FetchError>,
    mut verify: impl FnMut(&str, &[u8]) -> bool,
    mut sleep: impl FnMut(u64),
) -> Result<Vec<u8>, Vec<String>> {
    let mut failures = Vec::new();
    for url in sources {
        for attempt in 1..=3 {
            let transient = match fetch(&url) {
                Ok(bytes) if verify(&url, &bytes) => return Ok(bytes),
                Ok(_) => {
                    failures.push(format!(
                        "{url}, attempt {attempt}: SHA-256 does not match dist.txt"
                    ));
                    false
                }
                Err(error) => {
                    failures.push(format!("{url}, attempt {attempt}: {}", error.message));
                    error.transient
                }
            };
            if !transient {
                break;
            }
            if let Some(seconds) = [2, 8].get(attempt - 1) {
                sleep(*seconds);
            }
        }
    }
    Err(failures)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_failure(mut failure: impl FnMut() -> Result<Vec<u8>, FetchError>, attempts: usize) {
        let mut calls = Vec::new();
        let mut sleeps = Vec::new();
        let bytes = fetch_sources(
            ["mirror".into(), "upstream".into()],
            |url| {
                calls.push(url.to_owned());
                if url == "mirror" {
                    failure()
                } else {
                    Ok(vec![42])
                }
            },
            |_, bytes| bytes == [42],
            |seconds| sleeps.push(seconds),
        )
        .unwrap();
        assert_eq!(bytes, [42]);
        assert_eq!(calls, [vec!["mirror"; attempts], vec!["upstream"]].concat());
        assert_eq!(sleeps, if attempts == 3 { vec![2, 8] } else { vec![] });
    }

    #[test]
    fn not_found_falls_back_without_retry() {
        check_failure(|| Err(FetchError::http(404, "not found".into())), 1);
    }

    #[test]
    fn hash_mismatch_falls_back_without_retry() {
        check_failure(|| Ok(vec![0]), 1);
    }

    #[test]
    fn server_errors_retry_before_fallback() {
        for status in [500, 502, 503, 504] {
            check_failure(|| Err(FetchError::http(status, "server error".into())), 3);
        }
    }

    #[test]
    fn timeouts_and_connection_resets_retry_before_fallback() {
        for kind in [io::ErrorKind::TimedOut, io::ErrorKind::ConnectionReset] {
            check_failure(|| Err(FetchError::io(io::Error::from(kind))), 3);
        }
    }

    #[test]
    fn timeout_and_rate_limit_responses_retry() {
        for status in [408, 429] {
            check_failure(|| Err(FetchError::http(status, "transient HTTP".into())), 3);
        }
    }

    #[test]
    fn transport_category_preserves_transient_io_sources() {
        for kind in [io::ErrorKind::Other, io::ErrorKind::ConnectionRefused] {
            check_failure(
                || {
                    let error = io::Error::from(kind);
                    Err(FetchError::transport(&error, true))
                },
                3,
            );
        }
    }

    #[test]
    fn other_client_errors_do_not_retry() {
        for status in [400, 401, 403, 410] {
            check_failure(|| Err(FetchError::http(status, "client error".into())), 1);
        }
    }

    #[test]
    fn both_sources_failing_returns_diagnostics_without_sleep() {
        let errors = fetch_sources(
            ["mirror".into(), "upstream".into()],
            |_| Ok(vec![0]),
            |_, _| false,
            |_| panic!("unexpected sleep"),
        )
        .unwrap_err();
        assert_eq!(errors.len(), 2);
        assert!(errors[0].contains("mirror"));
        assert!(errors[1].contains("upstream"));
    }
}
