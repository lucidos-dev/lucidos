//! The verdict, exercised at every boundary it draws.
//!
//! Pure, so each case is a row built by hand. The ages are the ones Postgres
//! would have measured, passed in as seconds, because nothing here reads a
//! clock (ADR 0053). What the scheduler does with a verdict is covered beside
//! the scheduler.

use super::*;
use crate::core::webhooks::DeliveryRefusal;
use std::collections::BTreeMap;
use uuid::Uuid;

const CLEAR: RefusalVerdict = RefusalVerdict::Clear;
const PAST_THE_CLOCK: i64 = REFUSAL_RUN_BEFORE_DEGRADED_SECS + 60;

fn ago(secs: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::Duration::seconds(secs)
}

/// A hook with a refusal run built from the given reasons.
///
/// `run_secs` and `quiet_secs` are the two ages the database measures, so a
/// test says how old a run is without touching a clock.
fn refusing(
    enabled: bool,
    reasons: &[(DeliveryRefusal, i64)],
    run_secs: i64,
    quiet_secs: i64,
) -> Webhook {
    let refusals: i64 = reasons.iter().map(|(_, n)| *n).sum();
    let running = refusals > 0;
    Webhook {
        id: Uuid::nil(),
        name: "GitHub workflow runs".into(),
        event_type: "GithubWorkflowRunStateChanged".into(),
        token_hash: Some("x".into()),
        hmac: None,
        dedupe: None,
        headers: Vec::new(),
        enabled,
        created_at: ago(90_000),
        updated_at: ago(90_000),
        last_accepted_at: None,
        last_refused_at: running.then(|| ago(quiet_secs)),
        last_refusal_reason: reasons.first().map(|(r, _)| r.reason().to_string()),
        refusal_run: RefusalRun {
            refusals,
            since: running.then(|| ago(run_secs)),
            // A run is homogeneous, so its cause is the first reason's. A test
            // that mixes reasons is describing a row the writer cannot produce.
            cause: reasons.first().map(|(r, _)| r.cause()),
            reasons: reasons
                .iter()
                .map(|(r, n)| (r.key().to_string(), *n))
                .collect(),
            run_secs: running.then_some(run_secs),
            quiet_secs: running.then_some(quiet_secs),
        },
    }
}

/// An enabled hook refusing what it verifies, in the shape most cases want.
fn failing_verification(count: i64, run_secs: i64, quiet_secs: i64) -> Webhook {
    refusing(
        true,
        &[(DeliveryRefusal::SignatureMismatch, count)],
        run_secs,
        quiet_secs,
    )
}

/// A hook that accepted something, so its run is over.
fn accepting() -> Webhook {
    Webhook {
        last_accepted_at: Some(ago(60)),
        refusal_run: RefusalRun::default(),
        ..refusing(true, &[], 0, 0)
    }
}

/// A standing declaration, as the timeline holds it.
fn standing(cause: RefusalCause, since_secs_ago: i64) -> Declared {
    Declared {
        cause,
        since: ago(since_secs_ago),
    }
}

/// The whole point of the feature: 18 days of deliveries thrown away by a hook
/// somebody switched off, with a green socket the entire time.
#[test]
fn a_switched_off_hook_still_taking_deliveries_is_declared() {
    let hook = refusing(false, &[(DeliveryRefusal::Disabled, 42)], 18 * 86_400, 300);
    assert_eq!(
        judge(&hook),
        RefusalVerdict::Refusing(RefusalCause::Disabled)
    );

    // One delivery is enough, because a disabled hook threw it away with
    // certainty. Nothing was read, so there is nothing to be unsure about.
    let one = refusing(false, &[(DeliveryRefusal::Disabled, 1)], PAST_THE_CLOCK, 60);
    assert_eq!(
        judge(&one),
        RefusalVerdict::Refusing(RefusalCause::Disabled)
    );
}

/// The verification half: the delivery reached the verifier and failed it.
#[test]
fn a_hook_refusing_what_it_verifies_is_declared_in_different_words() {
    assert_eq!(
        judge(&failing_verification(42, 97_000, 300)),
        RefusalVerdict::Refusing(RefusalCause::Verification),
        "a switched-on hook's 401 is about the delivery, never the flag"
    );
}

/// One refusal is not an outage, and neither are three inside the clock.
#[test]
fn one_isolated_refusal_declares_nothing() {
    assert_eq!(judge(&failing_verification(1, PAST_THE_CLOCK, 60)), CLEAR);
    assert_eq!(
        judge(&failing_verification(2, PAST_THE_CLOCK, 60)),
        CLEAR,
        "below the count, however long it has been going"
    );

    // A whole workflow run's worth of refusals, all inside four seconds. That
    // is one bad payload, not an outage, and the clock is what says so.
    assert_eq!(judge(&failing_verification(3, 4, 1)), CLEAR);

    // The same three, once the clock has run out.
    assert_eq!(
        judge(&failing_verification(3, PAST_THE_CLOCK, 60)),
        RefusalVerdict::Refusing(RefusalCause::Verification)
    );
}

/// Both floors are `>=`, so each constant names the first value that counts.
#[test]
fn the_two_floors_are_inclusive() {
    let exactly = failing_verification(
        REFUSALS_BEFORE_DEGRADED,
        REFUSAL_RUN_BEFORE_DEGRADED_SECS,
        60,
    );
    assert_eq!(
        judge(&exactly),
        RefusalVerdict::Refusing(RefusalCause::Verification)
    );
}

/// A hook nobody delivers to any more is not news, however broken it is.
///
/// Without this the 18-day bar would stand for good over a sender that was
/// repointed elsewhere, and nothing could ever retract it.
#[test]
fn a_run_nothing_has_added_to_for_a_fortnight_stops_being_news() {
    let stale = refusing(
        false,
        &[(DeliveryRefusal::Disabled, 42)],
        30 * 86_400,
        REFUSAL_RUN_GOES_QUIET_SECS,
    );
    assert_eq!(judge(&stale), CLEAR);

    // A week of silence is this hook's ordinary state between scheduled runs,
    // so it must not retract there.
    let quiet_week = refusing(
        false,
        &[(DeliveryRefusal::Disabled, 42)],
        30 * 86_400,
        7 * 86_400,
    );
    assert_eq!(
        judge(&quiet_week),
        RefusalVerdict::Refusing(RefusalCause::Disabled)
    );
}

/// A re-enabled hook's disabled run names a fault that is over.
///
/// One of the two directions the live flag decides. Nothing is reported here,
/// however long the run is, because the switch is back on.
#[test]
fn a_re_enabled_hooks_disabled_run_names_a_fault_that_is_over() {
    let re_enabled = refusing(true, &[(DeliveryRefusal::Disabled, 42)], 97_000, 300);
    assert_eq!(judge(&re_enabled), CLEAR);
}

/// A switched-off hook is never reported as a verification fault.
///
/// The other direction, and the one that cost a live outage. A run
/// forms while the hook is on, somebody switches the hook off, and the stored
/// cause still says `verification`. The flag is the fact; the cause is a
/// reading of what the run WAS.
///
/// Reporting the stored cause sends the reader at the secret. Re-pointing a
/// hook replaces the whole config object and drops the secret with it, so the
/// wrong words turn one click into a multi-day outage.
#[test]
fn a_switched_off_hook_is_never_reported_as_a_verification_fault() {
    for reason in [
        DeliveryRefusal::SignatureMissing,
        DeliveryRefusal::SignatureMismatch,
        DeliveryRefusal::Token,
        DeliveryRefusal::CredentialMissing,
    ] {
        let switched_off = refusing(false, &[(reason, 5)], 97_000, 300);
        assert_eq!(
            judge(&switched_off),
            RefusalVerdict::Refusing(RefusalCause::Disabled),
            "{} on a hook that is off is still a switched-off hook",
            reason.key()
        );
    }

    // It declares rather than going quiet. Clearing here was the old answer,
    // and silence on a hook throwing deliveries away is the failure ADR 0235
    // exists to prevent.
    let said = standing(RefusalCause::Verification, 97_000);
    let switched_off = refusing(
        false,
        &[(DeliveryRefusal::SignatureMissing, 5)],
        97_000,
        300,
    );
    assert_eq!(
        decide(judge(&switched_off), Some(&said), Some(&switched_off)),
        Decision::Declare(RefusalCause::Disabled),
        "the standing verification declaration has to be said again, in the other words"
    );
}

/// A run whose cause this engine cannot read judges nothing.
///
/// The one thing it certainly must not do is guess `Disabled` and tell a user
/// their live hook is switched off.
#[test]
fn a_cause_this_engine_cannot_read_declares_nothing() {
    let mut hook = refusing(false, &[(DeliveryRefusal::Disabled, 42)], 97_000, 300);
    hook.refusal_run.cause = None;
    assert_eq!(judge(&hook), CLEAR);
}

/// An age the query could not produce judges nothing either.
///
/// It cannot happen while a run is going, since the column and the age are
/// written together. The point is that the fallback errs toward silence rather
/// than toward a fault nobody can date.
#[test]
fn a_run_with_no_measured_age_declares_nothing() {
    let mut hook = refusing(false, &[(DeliveryRefusal::Disabled, 42)], 97_000, 300);
    hook.refusal_run.run_secs = None;
    assert_eq!(judge(&hook), CLEAR);
}

#[test]
fn a_hook_that_accepted_something_is_clear() {
    assert_eq!(judge(&accepting()), CLEAR);
}

/// Edge-triggered: a standing declaration is neither repeated nor retracted.
#[test]
fn a_standing_declaration_is_said_once() {
    let down = RefusalVerdict::Refusing(RefusalCause::Disabled);
    assert_eq!(
        decide(down, None, None),
        Decision::Declare(RefusalCause::Disabled)
    );
    let said = standing(RefusalCause::Disabled, 97_000);
    assert_eq!(decide(down, Some(&said), None), Decision::Nothing);
}

/// A cause that changed re-declares, because the two want different actions.
#[test]
fn a_changed_cause_is_said_again() {
    let hook = failing_verification(3, 97_000, 300);
    let said = standing(RefusalCause::Disabled, 97_000);
    assert_eq!(
        decide(
            RefusalVerdict::Refusing(RefusalCause::Verification),
            Some(&said),
            Some(&hook),
        ),
        Decision::Declare(RefusalCause::Verification)
    );
}

/// Every way out of the refusing state retracts, and each names itself.
#[test]
fn a_declaration_is_never_stranded() {
    let said = standing(RefusalCause::Verification, 97_000);
    let off = standing(RefusalCause::Disabled, 97_000);

    // A delivery verified. The only positive evidence there is.
    assert_eq!(
        decide(CLEAR, Some(&said), Some(&accepting())),
        Decision::Recover(Resolution::Accepted)
    );

    // Switched back on, with the disabled run still on the row.
    let re_enabled = refusing(true, &[(DeliveryRefusal::Disabled, 42)], 97_000, 300);
    assert_eq!(
        decide(CLEAR, Some(&off), Some(&re_enabled)),
        Decision::Recover(Resolution::Reconfigured)
    );

    // Switched off while a verification declaration stood. `update` ends the
    // run in the same statement that moves the flag, so the row the next cycle
    // reads carries no run at all.
    //
    // It names the SWITCH, not silence. A cleared run has no age, which reads
    // as quiet, and that would report a deliberate switch-off as a sender that
    // went away. A trigger routing on the resolution would act on the wrong one.
    let switched_off = refusing(false, &[], 0, 0);
    assert_eq!(judge(&switched_off), CLEAR);
    assert_eq!(
        decide(CLEAR, Some(&said), Some(&switched_off)),
        Decision::Recover(Resolution::Reconfigured)
    );

    // Nothing has arrived for a fortnight.
    let stale = refusing(
        false,
        &[(DeliveryRefusal::Disabled, 42)],
        30 * 86_400,
        REFUSAL_RUN_GOES_QUIET_SECS + 60,
    );
    assert_eq!(
        decide(CLEAR, Some(&off), Some(&stale)),
        Decision::Recover(Resolution::Quiet)
    );

    // The hook is gone.
    assert_eq!(
        decide(CLEAR, Some(&off), None),
        Decision::Recover(Resolution::Removed)
    );

    // Nothing stands, so nothing is retracted.
    assert_eq!(decide(CLEAR, None, Some(&accepting())), Decision::Nothing);
}

/// An acceptance is read off the row, never inferred from an empty run.
///
/// A hook with two senders accepts from one, then a straggler from the other
/// is refused minutes later. A delivery DID verify, so the retraction has to
/// say so, and a trigger routing on `accepted` has to fire. Inferring it from
/// an empty run would miss this: the fresh refusal fills the run again.
#[test]
fn a_recovery_after_an_acceptance_says_so_even_with_a_fresh_run() {
    let mut hook = failing_verification(1, 600, 300);
    hook.last_accepted_at = Some(ago(780));
    let said = standing(RefusalCause::Verification, 97_000);

    assert_eq!(judge(&hook), CLEAR, "the new run is young");
    assert_eq!(
        decide(CLEAR, Some(&said), Some(&hook)),
        Decision::Recover(Resolution::Accepted)
    );
}

/// An acceptance from BEFORE the declared run is not evidence of anything.
#[test]
fn an_older_acceptance_does_not_retract() {
    let mut hook = refusing(true, &[(DeliveryRefusal::Disabled, 42)], 97_000, 300);
    hook.last_accepted_at = Some(ago(200_000));
    let said = standing(RefusalCause::Disabled, 97_000);
    assert_eq!(
        decide(CLEAR, Some(&said), Some(&hook)),
        Decision::Recover(Resolution::Reconfigured),
        "the hook was switched on again, which is what ended it"
    );
}

/// The age a payload carries is the one the database measured.
#[test]
fn the_reported_age_is_the_one_postgres_measured() {
    let hook = refusing(false, &[(DeliveryRefusal::Disabled, 1)], 1800, 60);
    assert_eq!(refusing_secs(&hook.refusal_run), 1800);

    // A clock that moved backwards must not report a negative run.
    let backwards = RefusalRun {
        refusals: 1,
        run_secs: Some(-600),
        ..RefusalRun::default()
    };
    assert_eq!(refusing_secs(&backwards), 0);
    assert_eq!(refusing_secs(&RefusalRun::default()), 0);
}

/// A recovery an acceptance caused ends AT that acceptance.
///
/// The cycle runs every 15 minutes, so aging the run to the moment it was
/// noticed overstates by up to that long. On a fault just past the half-hour
/// floor, that is half the number again.
#[test]
fn a_recovery_is_dated_by_the_acceptance_rather_than_by_the_cycle() {
    let said = standing(RefusalCause::Verification, 3600);
    let mut hook = accepting();
    // The run started an hour ago and a delivery verified 10 minutes in. The
    // cycle only notices now, 50 minutes after the fault was actually over.
    hook.last_accepted_at = Some(ago(3000));

    let secs = recovered_secs(&said, Resolution::Accepted, Some(&hook), 3600);
    assert!(
        (595..=605).contains(&secs),
        "expected about 600 seconds, got {secs}"
    );

    // Every other way out has no instant the engine saw, so it reports the run
    // up to now, which is the closest honest answer.
    for other in [
        Resolution::Reconfigured,
        Resolution::Quiet,
        Resolution::Removed,
    ] {
        assert_eq!(recovered_secs(&said, other, Some(&hook), 3600), 3600);
    }

    // A deleted hook has no row to read, so the standing age stands.
    assert_eq!(
        recovered_secs(&said, Resolution::Accepted, None, 3600),
        3600
    );

    // An acceptance the database stamped before the run began cannot make the
    // run negative.
    let mut older = accepting();
    older.last_accepted_at = Some(ago(9000));
    assert_eq!(
        recovered_secs(&said, Resolution::Accepted, Some(&older), 3600),
        0
    );
}

/// Every cause round-trips through the value the column and the wire share.
#[test]
fn a_cause_reads_back_off_its_stored_key() {
    let mut keys = std::collections::HashSet::new();
    for cause in RefusalCause::ALL {
        assert!(keys.insert(cause.key()), "duplicate key {cause:?}");
        assert_eq!(RefusalCause::parse(cause.key()), Some(cause));
    }
    assert_eq!(RefusalCause::parse("from-the-future"), None);

    // The split is one predicate, so a refusal's cause and the enum's own
    // reading of it cannot disagree.
    for refusal in DeliveryRefusal::ALL {
        let expected = if refusal.examined_the_delivery() {
            RefusalCause::Verification
        } else {
            RefusalCause::Disabled
        };
        assert_eq!(refusal.cause(), expected, "{refusal:?}");
    }
}

/// The tally is not what classifies a run, but it is what a reader sees.
#[test]
fn a_run_carries_the_breakdown_its_cause_was_built_from() {
    let hook = failing_verification(41, 97_000, 300);
    assert_eq!(
        hook.refusal_run.reasons,
        BTreeMap::from([("signature-mismatch".to_string(), 41)])
    );
    assert_eq!(hook.refusal_run.cause, Some(RefusalCause::Verification));
}
