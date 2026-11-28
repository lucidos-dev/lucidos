use crate::support::{base_url, http_client};
use serde_json::Value;

const BUILDING_AN_APP_ID: &str = "system-knowhow/building-an-app";

#[tokio::test]
async fn knowhow_list_includes_building_an_app() {
    let client = http_client();
    let resp = client
        .get(format!("{}/api/v1/knowhow", base_url()))
        .send()
        .await
        .expect("GET /api/v1/knowhow failed");
    assert_eq!(resp.status(), 200, "knowhow list must return 200");

    let body: Value = resp.json().await.expect("knowhow list must be JSON");
    let entries = body["knowhow"]
        .as_array()
        .expect("response must carry a `knowhow` array");
    assert!(
        entries
            .iter()
            .any(|e| e["id"].as_str() == Some(BUILDING_AN_APP_ID)),
        "catalog must advertise `{BUILDING_AN_APP_ID}`; got ids: {:?}",
        entries
            .iter()
            .filter_map(|e| e["id"].as_str())
            .collect::<Vec<_>>(),
    );
}

#[tokio::test]
async fn knowhow_read_returns_building_an_app_body() {
    let client = http_client();
    let resp = client
        .get(format!("{}/api/v1/knowhow/read", base_url()))
        .query(&[("id", BUILDING_AN_APP_ID)])
        .send()
        .await
        .expect("GET /api/v1/knowhow/read failed");
    assert_eq!(resp.status(), 200, "reading a known id must return 200");

    let text = resp.text().await.expect("read body must be text");
    assert!(
        text.contains("[SYSTEM-KNOWHOW:"),
        "read body must carry the SYSTEM-KNOWHOW wrapper; got: {text:.200}",
    );
    assert!(
        text.contains("Building an App"),
        "read body must carry the building-an-app content; got: {text:.200}",
    );
}

#[tokio::test]
async fn knowhow_read_unknown_id_is_404() {
    let client = http_client();
    let resp = client
        .get(format!("{}/api/v1/knowhow/read", base_url()))
        .query(&[("id", "system-knowhow/this-doc-does-not-exist-xyzzy")])
        .send()
        .await
        .expect("GET /api/v1/knowhow/read failed");
    assert_eq!(resp.status(), 404, "unknown id must return 404");

    let text = resp.text().await.expect("404 body must be text");
    assert!(
        text.contains("not found"),
        "404 body must carry the not-found sentinel; got: {text}",
    );
}

#[tokio::test]
async fn knowhow_read_records_one_read_per_doc_per_day() {
    let client = http_client();
    for _ in 0..2 {
        let resp = client
            .get(format!("{}/api/v1/knowhow/read", base_url()))
            .query(&[("id", BUILDING_AN_APP_ID)])
            .send()
            .await
            .expect("GET /api/v1/knowhow/read failed");
        assert_eq!(resp.status(), 200);
    }
    let pool = sqlx::PgPool::connect(&crate::support::db_url())
        .await
        .expect("connect");
    let recorded: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE event_type = 'KnowhowRead' AND aggregate_id = $1 \
         AND created >= now() - make_interval(days => 1)",
    )
    .bind(BUILDING_AN_APP_ID)
    .fetch_one(&pool)
    .await
    .expect("count KnowhowRead");
    assert_eq!(recorded, 1, "two reads in one day record one KnowhowRead");
}
