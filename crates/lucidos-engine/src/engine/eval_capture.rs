//! The eval-only full capture: `ContextCaptured` bodies, uncut (ADR 0110).
//!
//! A capture normally truncates every section body head and tail at 8,000
//! chars, in two places. Both of a section's sizes stay true either way, so
//! the budget sums are honest. The BODY is not there, and 8 KB cannot debug a
//! 137,000-char message array. That array is the region a benchmark is about.
//!
//! The caps exist for a real reason, recorded in `api::threads::events_snapshot`:
//! 500 captures of full bodies is many megabytes the events list would ship on
//! every load. That endpoint already strips `sections` on the list path and
//! serves them from `GET /api/v1/events/:event_id/context`, so the read side is
//! solved. What is left is a write path an eval arm can opt into.
//!
//! **The gate is one environment variable, read once.** The arm's engine is
//! spawned with it by `scripts/eval-context-mode.sh`, exactly as the query
//! classifier is pinned. Nothing else sets it, and a workspace without it is
//! byte-identical to before.
//!
//! It costs disk. A capture runs near 130 KB with bodies, so a fourteen-task
//! run is roughly 17 MB per arm, in that arm's own database.

/// Turns the two body caps off for this engine process.
pub(crate) const FULL_CAPTURE_ENV: &str = "LUCIDOS_EVAL_FULL_CAPTURE";

/// Whether this process persists whole section bodies.
///
/// Read once. The variable cannot change under a running engine, and every
/// captured round asks.
pub(crate) fn full_bodies() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| enabled_value(std::env::var(FULL_CAPTURE_ENV).ok().as_deref()))
}

/// The same truthy set the cache probe uses, so one habit covers both.
fn enabled_value(value: Option<&str>) -> bool {
    matches!(value.map(str::trim), Some("1" | "true" | "yes" | "on"))
}

/// The cap a capture applies to one section body, or `None` for no cap.
///
/// Callers keep their own constant and hand it here. The number stays beside
/// the code that chose it, and only the decision to ignore it lives here.
pub(crate) fn body_cap(normal: usize) -> Option<usize> {
    match full_bodies() {
        true => None,
        false => Some(normal),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_truthy_value_lifts_the_caps() {
        for on in ["1", "true", "yes", "on", " on "] {
            assert!(enabled_value(Some(on)), "{on} should enable it");
        }
        for off in [None, Some(""), Some("0"), Some("false"), Some("maybe")] {
            assert!(!enabled_value(off), "{off:?} should leave it off");
        }
    }

    /// The caller's constant survives, and only the decision moves here.
    #[test]
    fn a_cap_is_the_callers_number_or_nothing() {
        assert_eq!(body_cap(8_000).is_some(), !full_bodies());
        if !full_bodies() {
            assert_eq!(body_cap(8_000), Some(8_000));
        }
    }
}
