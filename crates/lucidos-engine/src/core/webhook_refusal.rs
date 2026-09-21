//! Whether a webhook is turning away the deliveries it receives, and why.
//!
//! The sibling of `core/webhook_ingress.rs`, one layer in. That one asks
//! whether a sender can still reach the workspace. This one asks what happens
//! to a delivery that did.
//!
//! The two readings are independent, and the gap between them is the bug this
//! exists to close. A hook that completes every handshake and then refuses
//! every body passes the ingress probe perfectly, because a 401 is the healthy
//! answer there: an unsigned probe is exactly what a live verifier turns away.
//!
//! Everything here is pure. The scheduler reads the rows and hands them over.
//!
//! See `docs/adr/0235-a-refused-delivery-is-an-outage.md`.

use serde::{Deserialize, Serialize};

use crate::core::webhooks::{RefusalRun, Webhook};

/// What is turning the deliveries away.
///
/// Two shapes, reported in different words, because they want different
/// actions from the reader. The split is
/// [`DeliveryRefusal::examined_the_delivery`], the mirror of
/// `Stage::measured_the_ingress` (ADR 0172).
///
/// [`DeliveryRefusal::examined_the_delivery`]: crate::core::webhooks::DeliveryRefusal::examined_the_delivery
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RefusalCause {
    /// The hook is switched off and deliveries keep arriving. Every one is
    /// thrown away before anything is read, so nothing was verified and
    /// nothing is wrong with the secret.
    Disabled,
    /// The delivery reached the verifier and failed it. A rotated secret, a
    /// missing credential, a body nothing can sign.
    Verification,
}

impl RefusalCause {
    /// Every cause, so a test can walk them.
    pub const ALL: [RefusalCause; 2] = [Self::Disabled, Self::Verification];

    /// How `webhooks.refusal_run_cause` stores this, and how the wire spells
    /// it. One value, so the column and the payload cannot drift apart.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Verification => "verification",
        }
    }

    /// Read a stored cause back. `None` for one this engine does not know,
    /// which judges nothing rather than guessing at a fault it cannot name.
    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|cause| cause.key() == key)
    }
}

/// How a declared refusal ended.
///
/// Exhaustive over the ways a hook can leave the refusing state, which is what
/// stops a declaration stranding. [`decide`] derives it, so the retraction and
/// the verdict cannot disagree about what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Resolution {
    /// A delivery verified and emitted. The only positive evidence there is.
    Accepted,
    /// The hook's enabled flag moved, so the fault named is not the live one.
    Reconfigured,
    /// Nothing has arrived for a fortnight. There is nothing left to report.
    Quiet,
    /// The webhook is gone.
    Removed,
}

/// What one hook's refusal run adds up to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalVerdict {
    /// Deliveries are landing, or too few have been turned away to say.
    Clear,
    /// Deliveries are being thrown away, and have been for long enough to say.
    Refusing(RefusalCause),
}

/// How many refusals a run needs before the engine calls a hook broken.
///
/// Three, because one `workflow_run` delivers three times (`requested`,
/// `in_progress`, `completed`). Below three the engine cannot tell a whole run
/// thrown away from one stray body a sender retried.
pub const REFUSALS_BEFORE_DEGRADED: i64 = 3;

/// How many a switched-off hook needs, which is one.
///
/// There is nothing to be uncertain about. A disabled hook is turned away
/// before anything is read, so that delivery was thrown away with certainty.
/// The clock below still applies, so a straggler arriving right after the
/// user's own click cannot page them.
pub const DISABLED_REFUSALS_BEFORE_DEGRADED: i64 = 1;

/// How long a run must also have lasted.
///
/// Thirty minutes, the patience the ingress check already spends (two
/// 15-minute strikes). It is longer than the burst one workflow run produces,
/// so a sender retrying one payload cannot page anyone inside it.
///
/// **The count and the clock are both required, and neither works alone.** A
/// purely time-based window false-negatives on a hook that takes three
/// deliveries a day, which has nothing inside any useful window. A purely
/// count-based one cannot tell three refusals in four seconds from three over
/// three days.
///
/// Together they handle both shapes. A busy hook reaches the count in seconds
/// and then waits out the clock. A quiet hook passes the clock at once and
/// waits for the count.
pub const REFUSAL_RUN_BEFORE_DEGRADED_SECS: i64 = 30 * 60;

/// How long a run may go without a new refusal before it stops being news.
///
/// Fourteen days. A silent week is this hook's expected state between
/// scheduled runs, so two weeks is longer than any quiet stretch it has shown.
/// Past it an alarm would have outlived the traffic that raised it, and would
/// stand for good over a sender that was repointed elsewhere.
pub const REFUSAL_RUN_GOES_QUIET_SECS: i64 = 14 * 24 * 60 * 60;

/// What the cycle should emit for one hook, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Nothing,
    /// Say that this hook is refusing, for this cause.
    Declare(RefusalCause),
    /// Retract a standing declaration, because of this.
    Recover(Resolution),
}

/// Judge one hook from its own row.
///
/// **It takes no clock.** Postgres measured every age it reads, beside the
/// timestamp that age came from (ADR 0053). So this module holds no host clock
/// for a drifting database to disagree with.
///
/// Both floors are `>=`, so each constant names the first value that counts.
pub fn judge(hook: &Webhook) -> RefusalVerdict {
    let run = &hook.refusal_run;
    if !run.is_running() {
        // No run at all. Something was accepted, or nothing has ever arrived.
        return RefusalVerdict::Clear;
    }
    // A run whose age or cause could not be read judges nothing. The next
    // refusal rewrites both, so the silence is one cycle rather than for good.
    let (Some(run_secs), Some(cause)) = (run.run_secs, run.cause) else {
        return RefusalVerdict::Clear;
    };
    if run_secs < REFUSAL_RUN_BEFORE_DEGRADED_SECS || gone_quiet(run) {
        return RefusalVerdict::Clear;
    }
    // The live flag has to still agree with what the run is evidence of. A
    // disabled run on a hook somebody switched back on names a fault that is
    // over, whatever the tally behind it says.
    let floor = match cause {
        RefusalCause::Disabled if !hook.enabled => DISABLED_REFUSALS_BEFORE_DEGRADED,
        RefusalCause::Verification if hook.enabled => REFUSALS_BEFORE_DEGRADED,
        _ => return RefusalVerdict::Clear,
    };
    if run.refusals >= floor {
        RefusalVerdict::Refusing(cause)
    } else {
        RefusalVerdict::Clear
    }
}

/// Has the run stopped growing for long enough to stop being news?
///
/// Read from the last refusal rather than from the run's start. A run that is
/// still being added to is live however old it is, which is the 18-day case:
/// deliveries arrived throughout it.
fn gone_quiet(run: &RefusalRun) -> bool {
    run.quiet_secs
        .is_none_or(|secs| secs >= REFUSAL_RUN_GOES_QUIET_SECS)
}

/// Decide from this cycle's verdict and what the timeline already carries.
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

/// The declaration a cycle is judging against, as the timeline holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Declared {
    pub cause: RefusalCause,
    /// When the declared run started. Compared only against `last_accepted_at`,
    /// which the same database clock wrote, so no host clock is involved.
    pub since: chrono::DateTime<chrono::Utc>,
}

/// Why a standing declaration stopped holding.
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
    if gone_quiet(&hook.refusal_run) {
        return Resolution::Quiet;
    }
    // Nothing verified and deliveries are still arriving, so the run must have
    // changed shape under the declaration: the flag moved, or the cause did.
    Resolution::Reconfigured
}

/// How long the run has been going, for a payload a reader dates.
///
/// Postgres measured it (ADR 0053). Zero when the run has no readable age,
/// which is the same case `judge` refuses to pronounce on.
pub fn refusing_secs(run: &RefusalRun) -> i64 {
    run.run_secs.unwrap_or(0).max(0)
}

/// How long the declared run actually lasted, for the retraction.
///
/// An acceptance is an instant the database stamped, so a recovery it caused
/// ended THERE rather than at the cycle that noticed. Aging to now instead
/// overstates by up to one 15-minute interval, which on a fault just past the
/// half-hour floor is half the number again.
///
/// Both instants are database readings, so subtracting them reads no host
/// clock (ADR 0053).
///
/// No other way out has such an instant. A flag moved, or a sender went away,
/// at some moment the engine never saw. Those report the run up to now, which
/// is the closest honest answer available.
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
