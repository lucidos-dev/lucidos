use crate::core::prefs;
use crate::core::proxy_timeout_bounds::{MAX_SECS, MIN_SECS};
use axum::http::StatusCode;
use std::time::Duration;
use tokio::time::Instant;

pub(crate) fn default_wait() -> Duration {
    Duration::from_secs_f64(prefs::PROXY_TIMEOUT_SECS.default_number())
}

pub(crate) fn range_rejection(field: &str, secs: f64) -> Option<String> {
    if secs.is_finite() && secs >= MIN_SECS as f64 && secs <= MAX_SECS as f64 {
        return None;
    }
    Some(format!(
        "'{field}' must be a number of seconds between {MIN_SECS} and {MAX_SECS} (got {secs})"
    ))
}

pub(crate) fn effective(
    entry_secs: Option<f64>,
    workspace: Option<&str>,
) -> Result<Duration, String> {
    if let Some(secs) = entry_secs {
        return match range_rejection("timeout_secs", secs) {
            None => Ok(Duration::from_secs_f64(secs)),
            Some(reason) => Err(reason),
        };
    }
    let Some(raw) = workspace else {
        return Ok(default_wait());
    };
    let key = prefs::PROXY_TIMEOUT_SECS.key();
    let secs: f64 = raw
        .trim()
        .parse()
        .map_err(|_| format!("the stored '{key}' is not a number: '{raw}'"))?;
    match range_rejection(key, secs) {
        None => Ok(Duration::from_secs_f64(secs)),
        Some(reason) => Err(format!("the stored {reason}")),
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CallBudget {
    per_request: Duration,
    deadline: Instant,
}

impl CallBudget {
    pub(crate) fn start(per_request: Duration) -> Self {
        Self::with_cap(per_request, Duration::from_secs(MAX_SECS))
    }

    pub(crate) fn with_cap(per_request: Duration, call_cap: Duration) -> Self {
        Self {
            per_request,
            deadline: Instant::now() + call_cap,
        }
    }

    pub(crate) fn time_left(&self) -> Option<Duration> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        (!left.is_zero()).then_some(left)
    }

    pub(crate) fn next_request(&self) -> Option<Duration> {
        self.time_left().map(|left| left.min(self.per_request))
    }
}

pub(crate) async fn resolve(
    pool: &sqlx::PgPool,
    entry_secs: Option<f64>,
) -> Result<Duration, (StatusCode, String)> {
    let internal = |msg: String| (StatusCode::INTERNAL_SERVER_ERROR, msg);
    if entry_secs.is_some() {
        return effective(entry_secs, None).map_err(internal);
    }
    let stored = prefs::PROXY_TIMEOUT_SECS
        .try_stored(pool)
        .await
        .map_err(|e| internal(format!("could not read the proxy timeout: {e}")))?;
    effective(None, stored.as_deref()).map_err(internal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_configured_waits_the_catalog_default() {
        assert_eq!(effective(None, None).unwrap(), default_wait());
    }

    #[test]
    fn the_workspace_value_applies_when_the_entry_sets_none() {
        assert_eq!(
            effective(None, Some("300")).unwrap(),
            Duration::from_secs(300)
        );
    }

    #[test]
    fn the_entry_wins_over_the_workspace_value() {
        assert_eq!(
            effective(Some(45.0), Some("300")).unwrap(),
            Duration::from_secs(45)
        );
        assert_eq!(
            effective(Some(45.0), None).unwrap(),
            Duration::from_secs(45)
        );
    }

    #[test]
    fn an_entry_value_out_of_range_is_an_error_not_a_panic() {
        assert!(effective(Some(-1.0), None).is_err());
        assert!(effective(Some(f64::NAN), None).is_err());
        assert!(effective(Some(MAX_SECS as f64 + 1.0), Some("300")).is_err());
    }

    #[test]
    fn a_fractional_value_is_honoured() {
        assert_eq!(
            effective(None, Some("1.5")).unwrap(),
            Duration::from_millis(1500)
        );
    }

    #[test]
    fn a_stored_value_out_of_range_fails_loudly() {
        let err = effective(None, Some(&(MAX_SECS + 1).to_string())).unwrap_err();
        assert!(err.contains(prefs::PROXY_TIMEOUT_SECS.key()), "{err}");
        assert!(err.contains(&MAX_SECS.to_string()), "{err}");
        assert!(effective(None, Some("0")).is_err());
        assert!(effective(None, Some("soon")).is_err());
    }

    #[test]
    fn the_range_is_inclusive_at_both_ends() {
        let (min, max) = (MIN_SECS as f64, MAX_SECS as f64);
        assert!(range_rejection("timeout_secs", min).is_none());
        assert!(range_rejection("timeout_secs", max).is_none());
        assert!(range_rejection("timeout_secs", min - 0.5).is_some());
        assert!(range_rejection("timeout_secs", max + 0.5).is_some());
        assert!(range_rejection("timeout_secs", -5.0).is_some());
        let reason = range_rejection("timeout_secs", max + 1.0).unwrap();
        assert!(reason.contains("'timeout_secs'"), "{reason}");
        let range = format!("between {MIN_SECS} and {MAX_SECS}");
        assert!(reason.contains(&range), "{reason}");
    }

    #[tokio::test]
    async fn a_stored_out_of_range_preference_is_refused_not_clamped() {
        use crate::test_support::{seed_preference, setup_test_db, teardown_test_db};
        let (pool, db) = setup_test_db().await;
        for stored in [(MAX_SECS + 1).to_string(), "0".to_string()] {
            seed_preference(&pool, prefs::PROXY_TIMEOUT_SECS.key(), &stored)
                .await
                .unwrap();
            let (status, message) = resolve(&pool, None)
                .await
                .expect_err("an out-of-range stored value must not resolve");
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{stored}");
            assert!(
                message.contains(prefs::PROXY_TIMEOUT_SECS.key()),
                "{message}"
            );
        }
        pool.close().await;
        teardown_test_db(&db).await;
    }

    #[test]
    fn a_request_waits_the_setting_while_the_call_has_time() {
        let budget = CallBudget::start(default_wait());
        let wait = budget.next_request().expect("a fresh call has time");
        assert_eq!(wait, default_wait());
    }

    #[tokio::test]
    async fn the_last_request_gets_only_what_is_left_of_the_call() {
        let budget = CallBudget::with_cap(default_wait(), Duration::from_millis(200));
        let wait = budget.next_request().expect("a fresh call has time");
        assert!(wait <= Duration::from_millis(200), "{wait:?}");
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(budget.next_request(), None, "the call is out of time");
    }

    mod guard_tests {

        #[test]
        fn the_cli_waits_the_client_wait() {
            let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            let cli = std::fs::read_to_string(root.join("crates/lucidos-cli/src/proxy.rs"))
                .expect("read the CLI proxy source");
            use crate::core::proxy_timeout_bounds::CLIENT_WAIT_SECS;
            let needle = format!("const CLIENT_WAIT_SECS: u64 = {CLIENT_WAIT_SECS};");
            assert!(cli.contains(&needle), "lucidos proxy must carry `{needle}`");
        }
    }
}
