//! E2E coverage for the backup *destination* half of the schedule contract.
//!
//! Pins the behavior behind a user report: an install configured for Dropbox
//! opened Settings → Backup on Google Drive, so the health card, the
//! connected / ready verdict, *Grant access* and *Back up now* all addressed a
//! provider the user had never chosen.
//!
//! Two engine defects fed that, and both are only observable across a
//! round trip, which is why this is an API test rather than a unit test:
//!
//! 1. `GET /backup/schedule` collapsed to `{schedule: null, provider: null}`
//!    whenever the schedule was inactive, so the page had nothing to seed from.
//! 2. `Scheduler::set_backup_schedule` wrote `backup_provider` ONLY in the
//!    schedule-active branch, so with the schedule off the destination could not
//!    be persisted at all.
//!
//! `schedule` keeps its old meaning throughout: the ACTIVE cron, null when
//! automatic backups are off. Only `provider` was decoupled from it.
//!
//! No cleanup guard, deliberately. The suite runs against the disposable
//! `e2e-test` workspace, which the harness creates and tears down per run, and
//! no other test in the suite reads the backup schedule, so there is nothing to
//! restore for and nothing to interfere with. A `Drop` guard would also have to
//! issue an HTTP request while the runtime is winding down, which this crate's
//! `reqwest` (no `blocking` feature) cannot do without re-entering the reactor.

use crate::support::{base_url, http_client, user_client};
use serde_json::json;

/// `(schedule, provider)` as the engine reports them, or `None` if the request
/// itself failed.
async fn read_schedule() -> Option<(Option<String>, Option<String>)> {
    let resp = http_client()
        .get(format!("{}/api/v1/backup/schedule", base_url()))
        .send()
        .await
        .ok()?;
    let v: serde_json::Value = resp.json().await.ok()?;
    Some((
        v["schedule"].as_str().map(str::to_string),
        v["provider"].as_str().map(str::to_string),
    ))
}

async fn put_schedule(provider: &str, schedule: &str) -> serde_json::Value {
    let resp = user_client()
        .await
        .put(format!("{}/api/v1/backup/schedule", base_url()))
        .json(&json!({ "provider": provider, "schedule": schedule }))
        .send()
        .await
        .expect("PUT /backup/schedule");
    assert!(
        resp.status().is_success(),
        "PUT /backup/schedule failed: {}",
        resp.status()
    );
    resp.json().await.expect("a JSON schedule response")
}

/// One pass over the whole contract, in a single test so the assertions cannot
/// race each other on the shared preference pair.
///
/// Takes the backup-key lock because enabling a schedule is not only a
/// preference write: `PUT /backup/schedule` calls `crypto::ensure_key` for an
/// active schedule, which re-mints the key `backup_key_test` deletes to prove a
/// reveal never mints one. Without the lock this test reds THAT one.
#[tokio::test]
async fn the_configured_provider_survives_an_inactive_schedule() {
    let _lock = crate::support::backup_key_lock().lock().await;

    // An active schedule reports both halves, which is the pre-existing
    // behavior and stays untouched.
    let put = put_schedule("dropbox", "0 0 3 * * *").await;
    assert_eq!(put["schedule"], "0 0 3 * * *");
    assert_eq!(put["provider"], "dropbox");

    let (schedule, provider) = read_schedule().await.expect("GET /backup/schedule");
    assert_eq!(schedule.as_deref(), Some("0 0 3 * * *"));
    assert_eq!(provider.as_deref(), Some("dropbox"));

    // Turning the schedule OFF must not lose the destination. This is the
    // regression: the disable branch never wrote the provider, and the GET
    // nulled it, so the page had no way to know it was configured for Dropbox.
    let put = put_schedule("dropbox", "off").await;
    assert_eq!(
        put["schedule"],
        serde_json::Value::Null,
        "an inactive schedule reports no cron"
    );
    assert_eq!(
        put["provider"], "dropbox",
        "the PUT must report back the destination it just wrote"
    );

    let (schedule, provider) = read_schedule().await.expect("GET /backup/schedule");
    assert_eq!(schedule, None, "automatic backups are off");
    assert_eq!(
        provider.as_deref(),
        Some("dropbox"),
        "the configured destination outlives the cron"
    );

    // Changing ONLY the destination while the schedule is off persists it, and
    // still does not resurrect a cron.
    put_schedule("google_drive", "off").await;
    let (schedule, provider) = read_schedule().await.expect("GET /backup/schedule");
    assert_eq!(schedule, None);
    assert_eq!(provider.as_deref(), Some("google_drive"));

    // And re-enabling a schedule keeps the destination that was picked while it
    // was off, rather than reverting to whatever was last active.
    put_schedule("google_drive", "0 0 4 * * *").await;
    let (schedule, provider) = read_schedule().await.expect("GET /backup/schedule");
    assert_eq!(schedule.as_deref(), Some("0 0 4 * * *"));
    assert_eq!(provider.as_deref(), Some("google_drive"));
}
