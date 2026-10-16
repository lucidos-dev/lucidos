//! E2E coverage for `GET /api/v1/backup/last-successful`, the per-workspace
//! backup line the gateway's picker draws on every row.
//!
//! An API test because the thing worth pinning is the WIRE. The gateway parses
//! this answer with a hand-rolled reader, `stack::parse_last_successful_backup`,
//! since it links no engine crate to share types with. It treats anything it
//! cannot read whole as "unknown", which renders as a silent row. So a renamed
//! field does not fail loudly anywhere: every picker row just stops saying
//! whether the workspace is backed up.

// Three properties, and all three are cross-process:
//
// 1. The route exists and takes no `provider`, unlike the rest of `/backup/*`.
//    The gateway calls it with no query string at all.
// 2. It answers the three keys the gateway reads, at those exact names.
// 3. It reads the workspace's own run history, so a workspace that has never
//    backed up says so rather than erroring.
//
// The e2e workspace never runs a backup, so `at` is null here and `stale` is
// true. The populated shape is unit-tested at both ends: `build_last_successful`
// in the engine, and `parse_last_successful_backup` in the gateway.
//
// See `docs/plans/2026-08-27-picker-last-successful-backup.md`.

use crate::support::{base_url, http_client};

#[tokio::test]
async fn the_backup_line_answers_without_a_provider() {
    let resp = http_client()
        .get(format!("{}/api/v1/backup/last-successful", base_url()))
        .send()
        .await
        .expect("GET /backup/last-successful");
    assert!(
        resp.status().is_success(),
        "the picker's per-row read must not need a provider: {}",
        resp.status(),
    );
    let v: serde_json::Value = resp.json().await.expect("a JSON answer");

    // Every key the gateway reads, present and correctly typed. Its parser
    // discards the whole answer on a miss, and a discarded answer is a silent
    // row, so a rename here has to fail HERE.
    assert!(v.get("at").is_some(), "no `at` key: {v}");
    assert!(
        v["stale"].is_boolean(),
        "`stale` must be the engine's verdict, not the picker's: {v}",
    );
    assert!(v["configured"].is_boolean(), "no `configured` bool: {v}");

    // The disposable e2e workspace has never run a backup, so this is the
    // "never backed up" answer: a real verdict, not a missing one.
    assert_eq!(
        v["at"],
        serde_json::Value::Null,
        "the e2e workspace has no backup history: {v}",
    );
    assert_eq!(v["stale"], true, "no backup at all is the worst kind: {v}");
}
