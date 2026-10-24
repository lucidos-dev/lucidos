use super::*;
use crate::test_support::{setup_test_db, teardown_test_db};

const DAY: f64 = 86_400.0;
const MIN: f64 = 60.0;

fn run(age_days: f64, duration_min: f64) -> PhaseRun {
    PhaseRun {
        age_secs: age_days * DAY,
        duration_secs: duration_min * MIN,
    }
}

#[test]
fn too_few_runs_give_no_estimate() {
    let runs: Vec<PhaseRun> = (0..4).map(|_| run(1.0, 10.0)).collect();
    assert_eq!(estimate(&runs), None);
}

#[test]
fn equal_weights_give_the_plain_median() {
    let runs: Vec<PhaseRun> = [5.0, 10.0, 15.0, 20.0, 25.0]
        .iter()
        .map(|d| run(0.0, *d))
        .collect();
    assert_eq!(
        estimate(&runs),
        Some(PhaseEstimate {
            typical_secs: 900,
            runs: 5
        })
    );
}

/// The process gets faster, so old slow runs must give way to new fast ones
/// even when they outnumber them.
#[test]
fn recent_runs_outweigh_older_ones() {
    let mut runs: Vec<PhaseRun> = (0..6).map(|_| run(60.0, 40.0)).collect();
    runs.extend((0..4).map(|_| run(1.0, 10.0)));
    assert_eq!(estimate(&runs).map(|e| e.typical_secs), Some(600));
}

/// A run parked overnight measures the user, not the phase.
#[test]
fn runs_past_four_hours_are_dropped() {
    let mut runs: Vec<PhaseRun> = (0..5).map(|_| run(1.0, 10.0)).collect();
    runs.extend((0..5).map(|_| run(1.0, 600.0)));
    assert_eq!(
        estimate(&runs),
        Some(PhaseEstimate {
            typical_secs: 600,
            runs: 5
        })
    );
}

/// Insert one event `ago_secs` in the past. Events for one thread must be
/// inserted oldest first, since `sequence` orders them.
async fn event(pool: &PgPool, thread: Uuid, event_type: &str, ago_secs: f64) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, thread_id, aggregate, aggregate_id) \
         VALUES ($1, $2, '{}'::jsonb, now() - make_interval(secs => $3), $4, 'thread', $5)",
    )
    .bind(Uuid::new_v4())
    .bind(event_type)
    .bind(ago_secs)
    .bind(thread)
    .bind(thread.to_string())
    .execute(pool)
    .await
    .expect("insert event");
}

/// A phase run on its own thread, `ago_days` back, lasting `minutes`.
async fn phase_run(pool: &PgPool, start: &str, end: &str, ago_days: f64, minutes: f64) {
    let thread = Uuid::new_v4();
    let started = ago_days * DAY;
    event(pool, thread, start, started).await;
    event(pool, thread, end, started - minutes * MIN).await;
}

#[tokio::test]
async fn only_runs_that_succeeded_count() {
    let (pool, db) = setup_test_db().await;
    for minutes in [10.0, 12.0, 14.0, 16.0, 18.0] {
        phase_run(
            &pool,
            "MergeConflictDetected",
            "ChangeApplied",
            1.0,
            minutes,
        )
        .await;
        phase_run(
            &pool,
            "MissingHardeningDetected",
            "ChangeHardened",
            1.0,
            minutes + 10.0,
        )
        .await;
    }
    // Failed runs would drag both estimates down if they counted.
    for _ in 0..5 {
        phase_run(
            &pool,
            "MergeConflictDetected",
            "ChangeApplyFailed",
            1.0,
            1.0,
        )
        .await;
        phase_run(
            &pool,
            "MissingHardeningDetected",
            "ChangeApplyFailed",
            1.0,
            1.0,
        )
        .await;
    }
    // Outside the window.
    for _ in 0..5 {
        phase_run(&pool, "MergeConflictDetected", "ChangeApplied", 120.0, 90.0).await;
    }

    let estimates = compute(&pool).await.expect("compute");
    assert_eq!(
        estimates.resolving_conflict,
        Some(PhaseEstimate {
            typical_secs: 14 * 60,
            runs: 5
        })
    );
    assert_eq!(
        estimates.hardening,
        Some(PhaseEstimate {
            typical_secs: 24 * 60,
            runs: 5
        })
    );
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_open_hardening_reports_when_it_began() {
    let (pool, db) = setup_test_db().await;
    let (open, finished, never) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    event(&pool, open, "MissingHardeningDetected", 300.0).await;
    event(&pool, finished, "MissingHardeningDetected", 600.0).await;
    event(&pool, finished, "ChangeHardened", 60.0).await;

    let starts = open_hardening_starts(&pool, &[open, finished, never])
        .await
        .expect("starts");
    assert_eq!(starts.keys().collect::<Vec<_>>(), vec![&open]);
    let age = (chrono::Utc::now() - starts[&open]).num_seconds();
    assert!(
        (250..=400).contains(&age),
        "began about 5 min ago, got {age}s"
    );
    teardown_test_db(&db).await;
}
