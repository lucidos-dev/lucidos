//! End-to-end API tests for the workspace prompt footprint (ADR 0413).
//!
//! - `POST /api/v1/app/opened` identifies its caller before it records, and
//!   records one `AppOpened` per call for a real app only.
//! - `GET /api/v1/workspace-prompt-footprint` reports every registry section
//!   and judges the app it was just told was opened as used.

use crate::support::{
    base_url, db_url, http_client, remove_data_fixtures, user_client, workspace_tree_lock,
    write_data_fixture,
};
use serde_json::json;
use uuid::Uuid;

fn folder(stem: &str) -> String {
    format!("{stem}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

async fn write_app(client: &reqwest::Client, id: &str, manifest: serde_json::Value) {
    let _tree = workspace_tree_lock().read().await;
    write_data_fixture(
        client,
        &format!("apps/{id}/manifest.json"),
        &manifest.to_string(),
    )
    .await
    .expect("write manifest");
    write_data_fixture(client, &format!("apps/{id}/index.html"), "<h1>app</h1>")
        .await
        .expect("write index.html");
}

async fn opened(client: &reqwest::Client, id: &str) -> u16 {
    client
        .post(format!("{}/api/v1/app/opened?id={id}", base_url()))
        .send()
        .await
        .expect("POST /app/opened")
        .status()
        .as_u16()
}

async fn opens_recorded(pool: &sqlx::PgPool, id: &str) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE event_type = 'AppOpened' AND aggregate_id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .expect("count AppOpened")
}

#[tokio::test]
async fn an_open_is_recorded_once_for_an_identified_caller_and_a_real_app() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect");
    let app = folder("e2e-footprint-app");
    let widget = folder("e2e-footprint-widget");
    write_app(
        &client,
        &app,
        json!({ "name": "Footprint app", "description": "An app." }),
    )
    .await;
    write_app(
        &client,
        &widget,
        json!({ "name": "Footprint widget", "kind": "widget", "reusable": true }),
    )
    .await;

    assert_eq!(
        opened(&http_client(), &app).await,
        401,
        "no credential, no record"
    );
    assert_eq!(opens_recorded(&pool, &app).await, 0);

    assert_eq!(opened(&client, &app).await, 204);
    assert_eq!(opens_recorded(&pool, &app).await, 1);

    assert_eq!(
        opened(&client, &widget).await,
        400,
        "a widget is not opened"
    );
    assert_eq!(opened(&client, "e2e-no-such-app").await, 404);
    assert_eq!(opens_recorded(&pool, &widget).await, 0);

    let report: serde_json::Value = client
        .get(format!("{}/api/v1/workspace-prompt-footprint", base_url()))
        .send()
        .await
        .expect("GET footprint")
        .json()
        .await
        .expect("footprint JSON");
    let sections = report["sections"].as_array().expect("sections");
    for id in [
        "available-apps",
        "reusable-widgets",
        "knowhow-routing",
        "mcp-tool-schemas",
    ] {
        assert!(
            sections.iter().any(|s| s["id"] == id),
            "{id} missing from {report}"
        );
    }
    let apps = sections
        .iter()
        .find(|s| s["id"] == "available-apps")
        .unwrap();
    let item = apps["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == app.as_str())
        .unwrap_or_else(|| panic!("{app} not listed: {apps}"));
    assert_eq!(item["usage"]["verdict"], "used");
    assert!(report["system_prompt_chars"].as_u64().unwrap() > 0);

    for id in [&app, &widget] {
        remove_data_fixtures(
            &client,
            &format!("apps/{id}"),
            &[
                format!("apps/{id}/manifest.json"),
                format!("apps/{id}/index.html"),
            ],
        )
        .await;
    }
}
