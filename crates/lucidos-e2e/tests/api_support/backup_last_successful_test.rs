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

    assert!(v.get("at").is_some(), "no `at` key: {v}");
    assert!(
        v["stale"].is_boolean(),
        "`stale` must be the engine's verdict, not the picker's: {v}",
    );
    assert!(v["configured"].is_boolean(), "no `configured` bool: {v}");
    assert!(v["overdue"].is_boolean(), "no `overdue` bool: {v}");

    assert_eq!(
        v["at"],
        serde_json::Value::Null,
        "the e2e workspace has no backup history: {v}",
    );
    assert_eq!(v["stale"], true, "no backup at all is the worst kind: {v}");
    assert_eq!(v["overdue"], true, "no backup at all is overdue: {v}");
    assert_eq!(
        v["overdue_at"],
        serde_json::Value::Null,
        "no backup has no overdue instant: {v}",
    );
}
