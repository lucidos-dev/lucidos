use serde::{Deserialize, Serialize};

use crate::core::webhooks::{RefusalRun, Webhook};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RefusalCause {
    Disabled,
    Verification,
}

impl RefusalCause {
    pub const ALL: [RefusalCause; 2] = [Self::Disabled, Self::Verification];

    /// INVARIANT: How `webhooks.refusal_run_cause` stores this, and how the wire spells
    /// it. One value, so the column and the payload cannot drift apart.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Verification => "verification",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|cause| cause.key() == key)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Resolution {
    Accepted,
    Reconfigured,
    Quiet,
    Removed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalVerdict {
    Clear,
    Refusing(RefusalCause),
}

pub const REFUSALS_BEFORE_DEGRADED: i64 = 3;

pub const DISABLED_REFUSALS_BEFORE_DEGRADED: i64 = 1;

/// INVARIANT: How long a run must also have lasted.
///
/// Thirty minutes, the patience the ingress check already spends (two
/// 15-minute strikes). It is longer than the burst one workflow run produces,
/// so a sender retrying one payload cannot page anyone inside it.
///
/// **For a hook that is on, the refusals themselves must span it**, first to
/// last. The run's age to now is not enough: three unsigned probes inside one
/// minute, then silence, would pass it half an hour later. A switched-off hook
/// keeps the run's age, since its one certain refusal spans nothing.
///
/// **The count and the clock are both required, and neither works alone.** A
/// purely time-based window false-negatives on a hook that takes three
/// deliveries a day, which has nothing inside any useful window. A purely
/// count-based one cannot tell three refusals in four seconds from three over
/// three days.
///
/// Together they handle both shapes. A busy broken hook reaches the count in
/// seconds and keeps refusing until the span passes the clock. A quiet hook
/// spans the clock between deliveries and waits for the count.
///
/// See `docs/adr/0365-a-burst-is-not-an-outage.md`.
pub const REFUSAL_RUN_BEFORE_DEGRADED_SECS: i64 = 30 * 60;

pub const REFUSAL_RUN_GOES_QUIET_SECS: i64 = 14 * 24 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Nothing,
    Declare(RefusalCause),
    Recover(Resolution),
}

pub fn judge(hook: &Webhook) -> RefusalVerdict {
    let run = &hook.refusal_run;
    if !run.is_running() {
        return RefusalVerdict::Clear;
    }
    let (Some(run_secs), Some(cause)) = (run.run_secs, run.cause) else {
        return RefusalVerdict::Clear;
    };
    if gone_quiet(run) {
        return RefusalVerdict::Clear;
    }
    // INVARIANT: **The live flag wins over the stored cause, in BOTH directions.** The
    // flag is a fact this cycle reads. The cause is what the run was evidence
    // of when it was written, and a switch moves under it.
    //
    // Off, the hook throws every delivery away before reading it, so no run on
    // it can support "none of them verified". Saying otherwise sends the reader
    // at the secret, and re-pointing a hook drops the whole config with it. So
    // a one-click fix becomes a real outage. The floor is the reported cause's:
    // the certainty comes from the flag, not from the count.
    //
    // On, a disabled run names a fault that is over, whatever its tally says.
    //
    // The clock reads a different span per cause. Off, one refusal is certain,
    // so the run's age is enough. On, the refusals themselves must span it,
    // because a burst followed by silence is a stray sender, not an outage.
    let (cause, floor, lasted_secs) = if !hook.enabled {
        (
            RefusalCause::Disabled,
            DISABLED_REFUSALS_BEFORE_DEGRADED,
            run_secs,
        )
    } else if cause == RefusalCause::Verification {
        let Some(span_secs) = refusals_span_secs(run) else {
            return RefusalVerdict::Clear;
        };
        (
            RefusalCause::Verification,
            REFUSALS_BEFORE_DEGRADED,
            span_secs,
        )
    } else {
        return RefusalVerdict::Clear;
    };
    if run.refusals >= floor && lasted_secs >= REFUSAL_RUN_BEFORE_DEGRADED_SECS {
        RefusalVerdict::Refusing(cause)
    } else {
        RefusalVerdict::Clear
    }
}

/// INVARIANT: How far apart the run's first and last refusals are, in seconds.
///
/// The difference of the two ages Postgres measured against one `now()`, so it
/// is exactly `last_refused_at - refusal_run_since` and reads no host clock.
/// `last_refused_at` is always the run's newest refusal, because one statement
/// writes both, and the probe skip skips both.
fn refusals_span_secs(run: &RefusalRun) -> Option<i64> {
    Some(run.run_secs? - run.quiet_secs?)
}

fn gone_quiet(run: &RefusalRun) -> bool {
    run.quiet_secs
        .is_none_or(|secs| secs >= REFUSAL_RUN_GOES_QUIET_SECS)
}

/// INVARIANT: Decide from this cycle's verdict and what the timeline already carries.
///
/// Edge-triggered, exactly as the ingress check is. The caller reads `declared`
/// out of the events table each cycle rather than holding it in memory. So a
/// restarted engine cannot announce a fault the timeline already has.
///
/// A CHANGED cause re-declares. The two causes take different words and want
/// different actions. Leaving the first standing would keep pointing the user
/// at the wrong thing.
///
/// There is no debounce counter here, unlike the ingress check's two strikes.
/// The run IS the debounce: it counts real deliveries rather than probe cycles,
/// and it survives a restart because it lives on the row.
pub fn decide(
    observed: RefusalVerdict,
    declared: Option<&Declared>,
    hook: Option<&Webhook>,
) -> Decision {
    match (observed, declared) {
        (RefusalVerdict::Refusing(cause), None) => Decision::Declare(cause),
        (RefusalVerdict::Refusing(cause), Some(said)) if cause != said.cause => {
            Decision::Declare(cause)
        }
        (RefusalVerdict::Refusing(_), Some(_)) => Decision::Nothing,
        (RefusalVerdict::Clear, Some(said)) => Decision::Recover(resolution(said, hook)),
        (RefusalVerdict::Clear, None) => Decision::Nothing,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Declared {
    pub cause: RefusalCause,
    /// INVARIANT: When the declared run started. Compared only against `last_accepted_at`,
    /// which the same database clock wrote, so no host clock is involved.
    pub since: chrono::DateTime<chrono::Utc>,
}

/// INVARIANT: Why a standing declaration stopped holding.
///
/// Exhaustive over the four ways out, in the order that makes each one certain.
/// A missing row is a deletion.
///
/// The positive evidence is an acceptance dated after the declared run began,
/// read off the row. It is NOT inferred from the run being empty: a fresh
/// refusal restarts the run, so "empty" goes false again within minutes of the
/// delivery that verified.
fn resolution(declared: &Declared, hook: Option<&Webhook>) -> Resolution {
    let Some(hook) = hook else {
        return Resolution::Removed;
    };
    if hook.last_accepted_at.is_some_and(|at| at > declared.since) {
        return Resolution::Accepted;
    }
    // INVARIANT: A run that is GONE, with no acceptance to date it, was ended by the flag
    // moving: `WebhookStore::update` clears it in that same statement. Quiet
    // has to read the run's own silence, so a cleared run must not reach it.
    // An absent age answers `gone_quiet` yes, which would report a deliberate
    // switch-off as a sender that went away.
    if hook.refusal_run.is_running() && gone_quiet(&hook.refusal_run) {
        return Resolution::Quiet;
    }
    Resolution::Reconfigured
}

pub fn refusing_secs(run: &RefusalRun) -> i64 {
    run.run_secs.unwrap_or(0).max(0)
}

pub fn recovered_secs(
    declared: &Declared,
    resolution: Resolution,
    hook: Option<&Webhook>,
    standing_secs: i64,
) -> i64 {
    if resolution != Resolution::Accepted {
        return standing_secs;
    }
    hook.and_then(|h| h.last_accepted_at)
        .map_or(standing_secs, |at| {
            (at - declared.since).num_seconds().max(0)
        })
}

#[cfg(test)]
#[path = "webhook_refusal_tests.rs"]
mod tests;
