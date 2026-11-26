use crate::support::{base_url, http_client, user_client};
use lucidos_engine::core::backup::crypto;
use serde_json::json;

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

#[tokio::test]
async fn the_configured_provider_survives_an_inactive_schedule() {
    let _lock = crate::support::backup_key_lock().lock().await;
    let (_, recipient) = crypto::generate_key_pair();
    let resp = user_client()
        .await
        .put(format!("{}/api/v1/backup/recipient", base_url()))
        .json(&json!({ "recipient": recipient.to_string() }))
        .send()
        .await
        .expect("PUT /backup/recipient");
    assert!(resp.status().is_success(), "{}", resp.status());

    let put = put_schedule("dropbox", "0 0 3 * * *").await;
    assert_eq!(put["schedule"], "0 0 3 * * *");
    assert_eq!(put["provider"], "dropbox");

    let (schedule, provider) = read_schedule().await.expect("GET /backup/schedule");
    assert_eq!(schedule.as_deref(), Some("0 0 3 * * *"));
    assert_eq!(provider.as_deref(), Some("dropbox"));

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

    put_schedule("google_drive", "off").await;
    let (schedule, provider) = read_schedule().await.expect("GET /backup/schedule");
    assert_eq!(schedule, None);
    assert_eq!(provider.as_deref(), Some("google_drive"));

    put_schedule("google_drive", "0 0 4 * * *").await;
    let (schedule, provider) = read_schedule().await.expect("GET /backup/schedule");
    assert_eq!(schedule.as_deref(), Some("0 0 4 * * *"));
    assert_eq!(provider.as_deref(), Some("google_drive"));
}
