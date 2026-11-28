use super::*;
use crate::core::webhooks::DeliveryRefusal;
use std::collections::BTreeMap;
use uuid::Uuid;

const CLEAR: RefusalVerdict = RefusalVerdict::Clear;
const PAST_THE_CLOCK: i64 = REFUSAL_RUN_BEFORE_DEGRADED_SECS + 60;

fn ago(secs: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::Duration::seconds(secs)
}

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

fn failing_verification(count: i64, run_secs: i64, quiet_secs: i64) -> Webhook {
    refusing(
        true,
        &[(DeliveryRefusal::SignatureMismatch, count)],
        run_secs,
        quiet_secs,
    )
}

fn accepting() -> Webhook {
    Webhook {
        last_accepted_at: Some(ago(60)),
        refusal_run: RefusalRun::default(),
        ..refusing(true, &[], 0, 0)
    }
}

fn standing(cause: RefusalCause, since_secs_ago: i64) -> Declared {
    Declared {
        cause,
        since: ago(since_secs_ago),
    }
}

#[test]
fn a_switched_off_hook_still_taking_deliveries_is_declared() {
    let hook = refusing(false, &[(DeliveryRefusal::Disabled, 42)], 18 * 86_400, 300);
    assert_eq!(
        judge(&hook),
        RefusalVerdict::Refusing(RefusalCause::Disabled)
    );

    let one = refusing(false, &[(DeliveryRefusal::Disabled, 1)], PAST_THE_CLOCK, 60);
    assert_eq!(
        judge(&one),
        RefusalVerdict::Refusing(RefusalCause::Disabled)
    );
}

#[test]
fn a_hook_refusing_what_it_verifies_is_declared_in_different_words() {
    assert_eq!(
        judge(&failing_verification(42, 97_000, 300)),
        RefusalVerdict::Refusing(RefusalCause::Verification),
        "a switched-on hook's 401 is about the delivery, never the flag"
    );
}

#[test]
fn one_isolated_refusal_declares_nothing() {
    assert_eq!(judge(&failing_verification(1, PAST_THE_CLOCK, 60)), CLEAR);
    assert_eq!(
        judge(&failing_verification(2, PAST_THE_CLOCK, 60)),
        CLEAR,
        "below the count, however long it has been going"
    );

    assert_eq!(judge(&failing_verification(3, 4, 1)), CLEAR);

    assert_eq!(
        judge(&failing_verification(3, PAST_THE_CLOCK, 0)),
        RefusalVerdict::Refusing(RefusalCause::Verification)
    );
}

#[test]
fn a_burst_of_refusals_followed_by_silence_declares_nothing() {
    let burst = refusing(
        true,
        &[(DeliveryRefusal::SignatureMissing, 3)],
        37 * 60,
        37 * 60 - 61,
    );
    assert_eq!(judge(&burst), CLEAR);

    let week_later = refusing(
        true,
        &[(DeliveryRefusal::SignatureMissing, 3)],
        7 * 86_400 + 61,
        7 * 86_400,
    );
    assert_eq!(judge(&week_later), CLEAR);

    assert_eq!(
        judge(&failing_verification(3, 37 * 60, 37 * 60 - 61)),
        CLEAR
    );
}

#[test]
fn a_spread_out_mismatch_run_still_declares() {
    assert_eq!(
        judge(&failing_verification(3, 3 * 3600, 600)),
        RefusalVerdict::Refusing(RefusalCause::Verification)
    );
}

#[test]
fn a_switched_off_hook_needs_no_span() {
    let one = refusing(
        false,
        &[(DeliveryRefusal::Disabled, 1)],
        PAST_THE_CLOCK,
        PAST_THE_CLOCK,
    );
    assert_eq!(
        judge(&one),
        RefusalVerdict::Refusing(RefusalCause::Disabled)
    );
}

#[test]
fn a_verification_run_with_no_measured_span_declares_nothing() {
    let mut hook = failing_verification(42, 97_000, 300);
    hook.refusal_run.quiet_secs = None;
    assert_eq!(judge(&hook), CLEAR);
}

#[test]
fn the_two_floors_are_inclusive() {
    let exactly = failing_verification(
        REFUSALS_BEFORE_DEGRADED,
        REFUSAL_RUN_BEFORE_DEGRADED_SECS + 60,
        60,
    );
    assert_eq!(
        judge(&exactly),
        RefusalVerdict::Refusing(RefusalCause::Verification)
    );
    let just_short = failing_verification(
        REFUSALS_BEFORE_DEGRADED,
        REFUSAL_RUN_BEFORE_DEGRADED_SECS + 60,
        61,
    );
    assert_eq!(judge(&just_short), CLEAR);
}

#[test]
fn a_run_nothing_has_added_to_for_a_fortnight_stops_being_news() {
    let stale = refusing(
        false,
        &[(DeliveryRefusal::Disabled, 42)],
        30 * 86_400,
        REFUSAL_RUN_GOES_QUIET_SECS,
    );
    assert_eq!(judge(&stale), CLEAR);

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

#[test]
fn a_re_enabled_hooks_disabled_run_names_a_fault_that_is_over() {
    let re_enabled = refusing(true, &[(DeliveryRefusal::Disabled, 42)], 97_000, 300);
    assert_eq!(judge(&re_enabled), CLEAR);
}

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

#[test]
fn a_cause_this_engine_cannot_read_declares_nothing() {
    let mut hook = refusing(false, &[(DeliveryRefusal::Disabled, 42)], 97_000, 300);
    hook.refusal_run.cause = None;
    assert_eq!(judge(&hook), CLEAR);
}

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

#[test]
fn a_declaration_is_never_stranded() {
    let said = standing(RefusalCause::Verification, 97_000);
    let off = standing(RefusalCause::Disabled, 97_000);

    assert_eq!(
        decide(CLEAR, Some(&said), Some(&accepting())),
        Decision::Recover(Resolution::Accepted)
    );

    let re_enabled = refusing(true, &[(DeliveryRefusal::Disabled, 42)], 97_000, 300);
    assert_eq!(
        decide(CLEAR, Some(&off), Some(&re_enabled)),
        Decision::Recover(Resolution::Reconfigured)
    );

    let switched_off = refusing(false, &[], 0, 0);
    assert_eq!(judge(&switched_off), CLEAR);
    assert_eq!(
        decide(CLEAR, Some(&said), Some(&switched_off)),
        Decision::Recover(Resolution::Reconfigured)
    );

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

    assert_eq!(
        decide(CLEAR, Some(&off), None),
        Decision::Recover(Resolution::Removed)
    );

    assert_eq!(decide(CLEAR, None, Some(&accepting())), Decision::Nothing);
}

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

#[test]
fn the_reported_age_is_the_one_postgres_measured() {
    let hook = refusing(false, &[(DeliveryRefusal::Disabled, 1)], 1800, 60);
    assert_eq!(refusing_secs(&hook.refusal_run), 1800);

    let backwards = RefusalRun {
        refusals: 1,
        run_secs: Some(-600),
        ..RefusalRun::default()
    };
    assert_eq!(refusing_secs(&backwards), 0);
    assert_eq!(refusing_secs(&RefusalRun::default()), 0);
}

#[test]
fn a_recovery_is_dated_by_the_acceptance_rather_than_by_the_cycle() {
    let said = standing(RefusalCause::Verification, 3600);
    let mut hook = accepting();
    hook.last_accepted_at = Some(ago(3000));

    let secs = recovered_secs(&said, Resolution::Accepted, Some(&hook), 3600);
    assert!(
        (595..=605).contains(&secs),
        "expected about 600 seconds, got {secs}"
    );

    for other in [
        Resolution::Reconfigured,
        Resolution::Quiet,
        Resolution::Removed,
    ] {
        assert_eq!(recovered_secs(&said, other, Some(&hook), 3600), 3600);
    }

    assert_eq!(
        recovered_secs(&said, Resolution::Accepted, None, 3600),
        3600
    );

    let mut older = accepting();
    older.last_accepted_at = Some(ago(9000));
    assert_eq!(
        recovered_secs(&said, Resolution::Accepted, Some(&older), 3600),
        0
    );
}

#[test]
fn a_cause_reads_back_off_its_stored_key() {
    let mut keys = std::collections::HashSet::new();
    for cause in RefusalCause::ALL {
        assert!(keys.insert(cause.key()), "duplicate key {cause:?}");
        assert_eq!(RefusalCause::parse(cause.key()), Some(cause));
    }
    assert_eq!(RefusalCause::parse("from-the-future"), None);

    for refusal in DeliveryRefusal::ALL {
        let expected = if refusal.examined_the_delivery() {
            RefusalCause::Verification
        } else {
            RefusalCause::Disabled
        };
        assert_eq!(refusal.cause(), expected, "{refusal:?}");
    }
}

#[test]
fn a_run_carries_the_breakdown_its_cause_was_built_from() {
    let hook = failing_verification(41, 97_000, 300);
    assert_eq!(
        hook.refusal_run.reasons,
        BTreeMap::from([("signature-mismatch".to_string(), 41)])
    );
    assert_eq!(hook.refusal_run.cause, Some(RefusalCause::Verification));
}
