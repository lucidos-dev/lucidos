//! How long the proxy waits on one upstream request.
//!
//! Two layers decide it, in this order: an `apis.json` entry's own
//! `timeout_secs`, then the workspace preference `proxy_timeout_secs`, then
//! [`DEFAULT_SECS`]. The builtin model providers have no entry, so the
//! preference is their only knob. See ADR 0277.
//!
//! The wait applies per upstream request, so a redirect hop and the one 401
//! retry each start their own. [`CallBudget`] caps the whole proxied call at
//! [`MAX_SECS`], which is what lets a client wait a fixed time and never cut
//! first. At the default the cap never binds: ten requests at 30 s is 300 s.

use axum::http::StatusCode;
use std::time::Duration;
use tokio::time::Instant;

/// The wait when nothing is configured.
pub(crate) const DEFAULT_SECS: u64 = 30;

/// The largest wait either layer accepts. The Anthropic and OpenAI SDKs both
/// default to ten minutes.
pub(crate) const MAX_SECS: u64 = 600;

/// The smallest wait either layer accepts.
pub(crate) const MIN_SECS: u64 = 1;

/// Why `secs` is not a usable wait for `field`, if it is not.
pub(crate) fn range_rejection(field: &str, secs: f64) -> Option<String> {
    if secs.is_finite() && secs >= MIN_SECS as f64 && secs <= MAX_SECS as f64 {
        return None;
    }
    Some(format!(
        "'{field}' must be a number of seconds between {MIN_SECS} and {MAX_SECS} (got {secs})"
    ))
}

/// The wait for one proxied call, from the entry's override and the stored
/// workspace preference.
///
/// A value that fails the range check is an error, never a silent clamp.
/// Both stores validate on the way in, so only a hand edit gets here. The
/// check also keeps `Duration::from_secs_f64` from panicking.
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
        return Ok(Duration::from_secs(DEFAULT_SECS));
    };
    let key = crate::core::PREF_PROXY_TIMEOUT_SECS;
    let secs: f64 = raw
        .trim()
        .parse()
        .map_err(|_| format!("the stored '{key}' is not a number: '{raw}'"))?;
    match range_rejection(key, secs) {
        None => Ok(Duration::from_secs_f64(secs)),
        Some(reason) => Err(format!("the stored {reason}")),
    }
}

/// The waits one proxied call may spend: each upstream request up to
/// `per_request`, and all of them together up to a fixed cap.
#[derive(Debug, Clone, Copy)]
pub(crate) struct CallBudget {
    per_request: Duration,
    deadline: Instant,
}

impl CallBudget {
    /// A budget starting now, capped at [`MAX_SECS`] for the whole call.
    pub(crate) fn start(per_request: Duration) -> Self {
        Self::with_cap(per_request, Duration::from_secs(MAX_SECS))
    }

    pub(crate) fn with_cap(per_request: Duration, call_cap: Duration) -> Self {
        Self {
            per_request,
            deadline: Instant::now() + call_cap,
        }
    }

    /// What is left of the whole call, or `None` once it is out of time.
    pub(crate) fn time_left(&self) -> Option<Duration> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        (!left.is_zero()).then_some(left)
    }

    /// The wait for the next upstream request, or `None` once the call is
    /// out of time.
    pub(crate) fn next_request(&self) -> Option<Duration> {
        self.time_left().map(|left| left.min(self.per_request))
    }
}

/// [`effective`], with the preference read from the database.
pub(crate) async fn resolve(
    pool: &sqlx::PgPool,
    entry_secs: Option<f64>,
) -> Result<Duration, (StatusCode, String)> {
    let internal = |msg: String| (StatusCode::INTERNAL_SERVER_ERROR, msg);
    if entry_secs.is_some() {
        return effective(entry_secs, None).map_err(internal);
    }
    let stored = crate::core::PreferenceStore::get(pool, crate::core::PREF_PROXY_TIMEOUT_SECS)
        .await
        .map_err(|e| internal(format!("could not read the proxy timeout: {e}")))?;
    effective(None, stored.as_deref()).map_err(internal)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_configured_waits_thirty_seconds() {
        assert_eq!(effective(None, None).unwrap(), Duration::from_secs(30));
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
        assert!(effective(Some(601.0), Some("30")).is_err());
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
        let err = effective(None, Some("601")).unwrap_err();
        assert!(err.contains("proxy_timeout_secs"), "{err}");
        assert!(err.contains("600"), "{err}");
        assert!(effective(None, Some("0")).is_err());
        assert!(effective(None, Some("soon")).is_err());
    }

    #[test]
    fn the_range_is_inclusive_at_both_ends() {
        assert!(range_rejection("timeout_secs", 1.0).is_none());
        assert!(range_rejection("timeout_secs", 600.0).is_none());
        assert!(range_rejection("timeout_secs", 0.5).is_some());
        assert!(range_rejection("timeout_secs", 600.5).is_some());
        assert!(range_rejection("timeout_secs", -5.0).is_some());
        let reason = range_rejection("timeout_secs", 601.0).unwrap();
        assert!(reason.contains("'timeout_secs'"), "{reason}");
        assert!(reason.contains("between 1 and 600"), "{reason}");
    }

    #[test]
    fn a_request_waits_the_setting_while_the_call_has_time() {
        let budget = CallBudget::start(Duration::from_secs(30));
        let wait = budget.next_request().expect("a fresh call has time");
        assert_eq!(wait, Duration::from_secs(30));
    }

    #[tokio::test]
    async fn the_last_request_gets_only_what_is_left_of_the_call() {
        let budget = CallBudget::with_cap(Duration::from_secs(30), Duration::from_millis(200));
        let wait = budget.next_request().expect("a fresh call has time");
        assert!(wait <= Duration::from_millis(200), "{wait:?}");
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(budget.next_request(), None, "the call is out of time");
    }

    /// How much longer than [`MAX_SECS`] a client waits on the engine. Every
    /// pipeline step and request is bounded by the cap, so this covers only
    /// the work before the budget starts: reading the preference, binding the
    /// pipeline, and building its layers.
    const CLIENT_MARGIN_SECS: u64 = 60;

    /// A client deadline below the engine's maximum would cut a call the
    /// engine still means to finish, which is the bug a raised setting fixes.
    /// `lucidos proxy` and the SDK's `lucidos.proxy` each carry the sum as a
    /// literal, since neither can import this crate.
    #[test]
    fn the_clients_wait_longer_than_the_engine_maximum() {
        let wait = MAX_SECS + CLIENT_MARGIN_SECS;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let cli = std::fs::read_to_string(root.join("crates/lucidos-cli/src/proxy.rs"))
            .expect("read the CLI proxy source");
        let needle = format!("const CLIENT_WAIT_SECS: u64 = {wait};");
        assert!(cli.contains(&needle), "lucidos proxy must carry `{needle}`");
        let sdk = std::fs::read_to_string(root.join("packages/lucidos-sdk/src/proxy.ts"))
            .expect("read the SDK proxy source");
        let needle = format!("const PROXY_TIMEOUT_MS = {};", wait * 1000);
        assert!(sdk.contains(&needle), "lucidos.proxy must carry `{needle}`");
    }
}
