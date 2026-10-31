//! The ping plan at its margins, for point and distribution estimates, and
//! under both caps.

use super::*;
use chrono::Duration;

const MINUTE: f64 = 60.0;

/// A Lucidos Agent prefill pings every 260 s.
const EVERY: f64 = 260.0;

/// A reread costs 25 pings on Opus.
const CAP: f64 = 25.0;

const SAFETY: f64 = CAP * EVERY;

fn wait_began() -> DateTime<Utc> {
    DateTime::from_timestamp(1_800_000_000, 0).expect("valid")
}

fn after(secs: f64) -> DateTime<Utc> {
    wait_began() + Duration::milliseconds((secs * 1000.0).round() as i64)
}

/// An estimate stated `stated` seconds into the wait, of a wake `expected`
/// seconds later, with `buckets` as `(minutes from the statement, odds)`.
fn estimate(stated: f64, expected: f64, buckets: &[(f64, f64)]) -> WakeEstimate {
    let distribution: Vec<WakeBucket> = buckets
        .iter()
        .map(|(minutes, probability)| WakeBucket {
            by: after(stated + minutes * MINUTE),
            probability: *probability,
        })
        .collect();
    WakeEstimate::new(
        after(stated),
        after(stated + expected),
        &distribution,
        WakeSource::Decision,
        wait_began(),
        ReEstimate::NotAsked,
    )
}

fn plan(timeout: Option<f64>, wake: Option<&WakeEstimate>) -> Plan {
    plan_wait(CAP, EVERY, timeout, wake)
}

fn until(give_up_after_secs: f64, end: PlanEnd) -> Plan {
    Plan {
        give_up_after_secs,
        end,
    }
}

fn assert_plan(got: Plan, want: Plan, label: &str) {
    assert_eq!(got.end, want.end, "{label}: {got:?}");
    assert!(
        (got.give_up_after_secs - want.give_up_after_secs).abs() < 1e-6,
        "{label}: {got:?}, want {want:?}"
    );
}

#[test]
fn with_no_estimate_the_bare_reread_cap_applies() {
    assert_plan(
        plan(None, None),
        until(SAFETY, PlanEnd::RereadCap),
        "no timeout",
    );
    assert_plan(
        plan(Some(30.0 * MINUTE), None),
        until(30.0 * MINUTE, PlanEnd::Timeout),
        "an earlier timeout",
    );
    assert_plan(
        plan(Some(SAFETY + 1.0), None),
        until(SAFETY, PlanEnd::RereadCap),
        "a later timeout",
    );
}

/// The release thread: about 60 minutes plans to 75, about 17 pings of the
/// 20 the saving margin allows. About 90 minutes plans nothing.
#[test]
fn a_point_estimate_plans_to_the_overrun_margin() {
    let hour = estimate(0.0, 60.0 * MINUTE, &[]);
    assert_plan(
        plan(None, Some(&hour)),
        until(75.0 * MINUTE, PlanEnd::Estimate),
        "about 60 min",
    );
    let longer = estimate(0.0, 90.0 * MINUTE, &[]);
    assert_plan(
        plan(None, Some(&longer)),
        until(0.0, PlanEnd::WakePastCap),
        "about 90 min",
    );
}

/// The plan pings at least once past a near wake.
#[test]
fn a_near_wake_gets_one_ping_past_it() {
    let near = estimate(0.0, 300.0, &[]);
    assert_plan(
        plan(None, Some(&near)),
        until(300.0 + EVERY, PlanEnd::Estimate),
        "five minutes",
    );
}

/// The saving margin at its edge: a plan of exactly 80% of one reread runs,
/// and one a second longer sends nothing.
#[test]
fn the_saving_margin_holds_at_its_edge() {
    let limit = PLAN_SPEND_SHARE * SAFETY;
    let edge = limit / OVERRUN_FACTOR;
    assert_plan(
        plan(None, Some(&estimate(0.0, edge, &[]))),
        until(limit, PlanEnd::Estimate),
        "at the edge",
    );
    assert_plan(
        plan(None, Some(&estimate(0.0, edge + 1.0, &[]))),
        until(0.0, PlanEnd::WakePastCap),
        "past the edge",
    );
}

/// A plan looks forward from its statement: earlier pings are sunk. The
/// safety cap still dates from the wait start.
#[test]
fn a_re_estimate_plans_forward_under_the_safety_cap() {
    assert_plan(
        plan(None, Some(&estimate(3000.0, 20.0 * MINUTE, &[]))),
        until(3000.0 + 25.0 * MINUTE, PlanEnd::Estimate),
        "room to spare",
    );
    assert_plan(
        plan(None, Some(&estimate(5000.0, 20.0 * MINUTE, &[]))),
        until(SAFETY, PlanEnd::RereadCap),
        "the overrun reaches the safety cap",
    );
    assert_plan(
        plan(None, Some(&estimate(5500.0, 20.0 * MINUTE, &[]))),
        until(5500.0, PlanEnd::WakePastCap),
        "the wake itself lies past the safety cap",
    );
}

/// A wake expected after the wait's timeout plans to the timeout, which wakes
/// the thread anyway.
#[test]
fn a_wake_past_the_timeout_plans_to_the_timeout() {
    let timeout = 40.0 * MINUTE;
    assert_plan(
        plan(Some(timeout), Some(&estimate(0.0, 60.0 * MINUTE, &[]))),
        until(timeout, PlanEnd::Timeout),
        "past the timeout",
    );
    assert_plan(
        plan(Some(timeout), Some(&estimate(0.0, 35.0 * MINUTE, &[]))),
        until(timeout, PlanEnd::Timeout),
        "the overrun reaches the timeout",
    );
}

/// Whatever the estimate, no plan pings past the timeout or the safety cap,
/// and none spends past the saving margin from its statement.
#[test]
fn no_plan_passes_either_cap_or_the_saving_margin() {
    let timeout = 70.0 * MINUTE;
    for stated in [0.0, 600.0, 3000.0, 6000.0] {
        for minutes in [1.0, 5.0, 20.0, 45.0, 60.0, 69.0, 80.0, 200.0] {
            let wake = estimate(stated, minutes * MINUTE, &[]);
            for timeout in [None, Some(timeout)] {
                let got = plan(timeout, Some(&wake));
                let label = format!("stated {stated}, {minutes} min, timeout {timeout:?}");
                assert!(got.give_up_after_secs <= SAFETY + 1e-6, "{label}: {got:?}");
                if let Some(timeout) = timeout {
                    assert!(got.give_up_after_secs <= timeout + 1e-6, "{label}: {got:?}");
                }
                if got.end == PlanEnd::Estimate {
                    let pings = (got.give_up_after_secs - stated) / EVERY;
                    assert!(pings <= PLAN_SPEND_SHARE * CAP + 1e-6, "{label}: {got:?}");
                }
            }
        }
    }
}

/// The decision's buckets, every one listed, most of the odds on one span.
fn buckets(weights: &[(f64, f64)]) -> Vec<(f64, f64)> {
    [
        5.0, 10.0, 15.0, 20.0, 30.0, 45.0, 60.0, 90.0, 120.0, 240.0, 1440.0,
    ]
    .into_iter()
    .map(|end| {
        let weight = weights.iter().find(|(e, _)| *e == end).map_or(0.0, |w| w.1);
        (end, weight)
    })
    .collect()
}

/// A distribution pings to the bucket end with the best expected saving.
/// Here the 45-minute end covers every wake for the same pings as any later
/// one, and the earlier tie wins.
#[test]
fn a_distribution_plans_the_best_expected_saving() {
    let wake = estimate(0.0, 25.0 * MINUTE, &buckets(&[(30.0, 0.9), (45.0, 0.1)]));
    assert_plan(
        plan(None, Some(&wake)),
        until(45.0 * MINUTE * OVERRUN_FACTOR, PlanEnd::Estimate),
        "most odds on 20 to 30 minutes",
    );
}

/// The release thread's odds as a distribution: most of them past an hour.
/// No bucket end saves anything under the margins, so no pings go out.
#[test]
fn a_distribution_that_saves_nothing_sends_no_pings() {
    let wake = estimate(
        0.0,
        70.0 * MINUTE,
        &buckets(&[(60.0, 0.2), (90.0, 0.6), (120.0, 0.2)]),
    );
    assert_plan(
        plan(None, Some(&wake)),
        until(0.0, PlanEnd::WakePastCap),
        "most odds past an hour",
    );
}

/// A distribution with no weight reads as its point.
#[test]
fn a_weightless_distribution_reads_as_its_point() {
    let wake = estimate(0.0, 60.0 * MINUTE, &buckets(&[]));
    assert!(wake.buckets.is_empty());
    assert_plan(
        plan(None, Some(&wake)),
        until(75.0 * MINUTE, PlanEnd::Estimate),
        "no weight",
    );
}

#[test]
fn the_uniform_helpers_hold_at_their_edges() {
    assert_eq!(uniform_cdf(10.0, 20.0, 5.0), 0.0);
    assert_eq!(uniform_cdf(10.0, 20.0, 15.0), 0.5);
    assert_eq!(uniform_cdf(10.0, 20.0, 25.0), 1.0);
    assert_eq!(uniform_capped_mean(10.0, 20.0, 5.0), 5.0);
    assert_eq!(uniform_capped_mean(10.0, 20.0, 30.0), 15.0);
    // Half the wakes come by 15 (mean 12.5), and half are capped there.
    assert!((uniform_capped_mean(10.0, 20.0, 15.0) - 13.75).abs() < 1e-9);
}

/// The guard claims one re-estimate per estimate. A finished ask stays asked,
/// and a newer estimate may ask again.
#[test]
fn the_re_estimate_guard_asks_once_per_estimate() {
    let guard = ReEstimates::default();
    let thread = Uuid::new_v4();
    let (first, second) = (after(0.0), after(60.0));
    assert_eq!(guard.of(thread, first), ReEstimate::NotAsked);
    assert!(guard.claim(thread, first));
    assert!(!guard.claim(thread, first), "one ask per estimate");
    assert_eq!(guard.of(thread, first), ReEstimate::Asking);
    guard.finish(thread, first);
    assert_eq!(guard.of(thread, first), ReEstimate::Asked);
    assert!(!guard.claim(thread, first));
    assert_eq!(guard.of(thread, second), ReEstimate::NotAsked);
    assert!(guard.claim(thread, second), "a newer estimate asks again");
    guard.retain(&[]);
    assert_eq!(guard.of(thread, second), ReEstimate::NotAsked);
}
