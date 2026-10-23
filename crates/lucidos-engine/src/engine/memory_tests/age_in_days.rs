use super::age_in_days;
use chrono::{Duration, Utc};

#[test]
fn same_timestamp_is_zero() {
    let now = Utc::now();
    assert!(age_in_days(now, now).abs() < f64::EPSILON);
}

#[test]
fn one_day_ago() {
    let now = Utc::now();
    let yesterday = now - Duration::days(1);
    let age = age_in_days(now, yesterday);
    assert!((age - 1.0).abs() < 0.001, "expected ~1.0, got {}", age);
}

#[test]
fn future_timestamp_clamped_to_zero() {
    let now = Utc::now();
    let future = now + Duration::hours(5);
    let age = age_in_days(now, future);
    assert!(
        age.abs() < f64::EPSILON,
        "future timestamp should give 0.0 age, got {}",
        age
    );
}

#[test]
fn fractional_days() {
    let now = Utc::now();
    let twelve_hours_ago = now - Duration::hours(12);
    let age = age_in_days(now, twelve_hours_ago);
    assert!(
        (age - 0.5).abs() < 0.001,
        "12 hours should be ~0.5 days, got {}",
        age
    );
}

#[test]
fn large_age() {
    let now = Utc::now();
    let five_years_ago = now - Duration::days(5 * 365);
    let age = age_in_days(now, five_years_ago);
    assert!(
        (age - 1825.0).abs() < 1.0,
        "5 years should be ~1825 days, got {}",
        age
    );
}
