//! End-to-end API tests for widgets (ADRs 0402, 0407).
//!
//! Three plan invariants an external client is placed to prove over real HTTP:
//!
//! - Pin and Unpin write one thread event each, and no file or commit (5).
//! - Delete (a thread) removes exactly that thread's own widgets (6).
//! - Archive removes no widget (7).
//!
//! Threads are seeded directly, as `delete_thread_test.rs` does. Widget
//! folders are committed fixtures written through the data API.

use crate::delete_thread_test::{cleanup, seed_thread};
use crate::support::{
    base_url, db_url, git, remove_data_fixtures, user_client, workspace_path, workspace_tree_lock,
    write_data_fixture,
};
use serde_json::json;
use uuid::Uuid;

/// A unique folder id, so parallel runs never share one.
fn folder(stem: &str) -> String {
    format!("{stem}-{}", &Uuid::new_v4().simple().to_string()[..8])
}

/// Commit a widget (or, with `kind: "app"`, an app) under `data/apps/<id>/`.
async fn write_app_folder(client: &reqwest::Client, id: &str, manifest: serde_json::Value) {
    let _tree = workspace_tree_lock().read().await;
    write_data_fixture(
        client,
        &format!("apps/{id}/manifest.json"),
        &manifest.to_string(),
    )
    .await
    .expect("write manifest");
    write_data_fixture(client, &format!("apps/{id}/index.html"), "<h1>widget</h1>")
        .await
        .expect("write index.html");
}

fn widget_manifest(origin: Uuid, reusable: bool) -> serde_json::Value {
    json!({
        "name": "Fare grid",
        "description": "Fares by date",
        "kind": "widget",
        "origin_thread_id": origin.to_string(),
        "reusable": reusable,
    })
}

fn app_dir(id: &str) -> std::path::PathBuf {
    workspace_path().join("data/apps").join(id)
}

/// The subjects of every commit that touched an app folder, newest first.
fn folder_history(id: &str) -> String {
    let out = git(&["log", "--format=%s", "--", &format!("data/apps/{id}")]);
    String::from_utf8_lossy(&out.stdout).into_owned()
}

async fn post(
    client: &reqwest::Client,
    path: &str,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let resp = client
        .post(format!("{}/api/v1{path}", base_url()))
        .json(&body)
        .send()
        .await
        .unwrap_or_else(|e| panic!("POST {path} failed: {e}"));
    let status = resp.status().as_u16();
    (status, resp.json().await.unwrap_or(serde_json::Value::Null))
}

async fn thread_widgets(client: &reqwest::Client, thread: Uuid) -> Vec<serde_json::Value> {
    client
        .get(format!(
            "{}/api/v1/widgets/thread?thread_id={thread}",
            base_url()
        ))
        .send()
        .await
        .expect("thread widgets request failed")
        .json::<Vec<serde_json::Value>>()
        .await
        .expect("thread widgets are a JSON array")
}

async fn remove_folders(client: &reqwest::Client, ids: &[&str]) {
    for id in ids {
        if app_dir(id).exists() {
            let files = vec![
                format!("apps/{id}/manifest.json"),
                format!("apps/{id}/index.html"),
            ];
            remove_data_fixtures(client, &format!("apps/{id}"), &files).await;
        }
    }
}

#[tokio::test]
async fn a_widget_shows_pins_and_unpins_without_touching_disk() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect");
    let thread = Uuid::new_v4();
    seed_thread(&pool, thread, None, "idle").await;
    let id = folder("fare-grid");
    write_app_folder(&client, &id, widget_manifest(thread, false)).await;

    // A widget is never an app in the list, but answers by id.
    let apps: Vec<serde_json::Value> = client
        .get(format!("{}/api/v1/apps", base_url()))
        .send()
        .await
        .expect("apps request")
        .json()
        .await
        .expect("apps JSON");
    assert!(
        apps.iter().all(|a| a["id"] != id.as_str()),
        "a widget leaked into /apps"
    );
    let app: serde_json::Value = client
        .get(format!("{}/api/v1/app?id={id}", base_url()))
        .send()
        .await
        .expect("app request")
        .json()
        .await
        .expect("app JSON");
    assert_eq!(app["kind"], "widget");

    let (status, body) = post(
        &client,
        "/widgets/show",
        json!({ "app_id": id, "thread_id": thread }),
    )
    .await;
    assert_eq!(status, 200, "show: {body}");
    let entries = thread_widgets(&client, thread).await;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["app_id"], id.as_str());
    assert_eq!(entries[0]["pinned"], false, "showing adds no chip");

    let history_before = folder_history(&id);
    let manifest_before = std::fs::read_to_string(app_dir(&id).join("manifest.json")).unwrap();

    let (status, body) = post(
        &client,
        "/widgets/pin",
        json!({ "app_id": id, "thread_id": thread }),
    )
    .await;
    assert_eq!(status, 200, "pin: {body}");
    assert_eq!(thread_widgets(&client, thread).await[0]["pinned"], true);

    let (status, body) = post(
        &client,
        "/widgets/unpin",
        json!({ "app_id": id, "thread_id": thread }),
    )
    .await;
    assert_eq!(status, 200, "unpin: {body}");
    assert_eq!(thread_widgets(&client, thread).await[0]["pinned"], false);

    assert_eq!(
        folder_history(&id),
        history_before,
        "Pin and Unpin must commit nothing"
    );
    assert_eq!(
        std::fs::read_to_string(app_dir(&id).join("manifest.json")).unwrap(),
        manifest_before,
        "Pin and Unpin must change no file"
    );

    // One event each, carrying the app id and nothing else.
    let payloads: Vec<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT event_type, payload FROM events WHERE thread_id = $1 \
         AND event_type LIKE 'Widget%' ORDER BY sequence",
    )
    .bind(thread)
    .fetch_all(&pool)
    .await
    .expect("read the widget events");
    let types: Vec<&str> = payloads.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(types, ["WidgetShown", "WidgetPinned", "WidgetUnpinned"]);
    for (_, payload) in &payloads {
        assert_eq!(payload["app_id"], id.as_str());
        assert!(
            !payload.to_string().contains("<h1>"),
            "no HTML in an event: {payload}"
        );
    }

    // A second thread may not show a widget that is not reusable.
    let other = Uuid::new_v4();
    seed_thread(&pool, other, None, "idle").await;
    let (status, _) = post(
        &client,
        "/widgets/show",
        json!({ "app_id": id, "thread_id": other }),
    )
    .await;
    assert_eq!(
        status, 409,
        "another thread's widget must be reusable first"
    );

    cleanup(&pool, &[thread, other]).await;
    remove_folders(&client, &[&id]).await;
    pool.close().await;
}

#[tokio::test]
async fn delete_removes_exactly_the_threads_own_widgets() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect");
    let doomed = Uuid::new_v4();
    let sibling = Uuid::new_v4();
    seed_thread(&pool, doomed, None, "idle").await;
    seed_thread(&pool, sibling, None, "idle").await;

    let owned = folder("owned");
    let reusable = folder("reusable");
    let made_app = folder("made-app");
    let siblings = folder("siblings");
    write_app_folder(&client, &owned, widget_manifest(doomed, false)).await;
    write_app_folder(&client, &reusable, widget_manifest(doomed, true)).await;
    write_app_folder(
        &client,
        &made_app,
        json!({ "name": "Fares app", "description": "Made from a widget" }),
    )
    .await;
    write_app_folder(&client, &siblings, widget_manifest(sibling, false)).await;

    let (status, body) = post(
        &client,
        "/threads/delete",
        json!({ "thread_id": doomed.to_string() }),
    )
    .await;
    assert_eq!(status, 200, "delete: {body}");
    assert_eq!(body["widgets_removed"], 1, "{body}");

    assert!(!app_dir(&owned).exists(), "the thread's own widget must go");
    assert!(app_dir(&reusable).exists(), "a reusable widget stays");
    assert!(
        app_dir(&made_app).exists(),
        "an app made from a widget stays"
    );
    assert!(app_dir(&siblings).exists(), "another thread's widget stays");

    // One commit removed the owned widget and touched nothing else.
    let subject = folder_history(&owned)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    assert!(
        subject.starts_with("Delete widgets with their thread"),
        "got: {subject}"
    );
    let sha = String::from_utf8_lossy(
        &git(&[
            "log",
            "-1",
            "--format=%H",
            "--",
            &format!("data/apps/{owned}"),
        ])
        .stdout,
    )
    .trim()
    .to_string();
    let touched = String::from_utf8_lossy(&git(&["show", "--name-only", "--format=", &sha]).stdout)
        .into_owned();
    assert!(
        touched
            .lines()
            .all(|p| p.starts_with(&format!("data/apps/{owned}/"))),
        "the delete commit touched more than the owned widget: {touched}"
    );

    cleanup(&pool, &[sibling]).await;
    remove_folders(&client, &[&reusable, &made_app, &siblings]).await;
    pool.close().await;
}

#[tokio::test]
async fn archive_keeps_a_threads_widget() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect");
    let thread = Uuid::new_v4();
    seed_thread(&pool, thread, None, "idle").await;
    let id = folder("kept");
    write_app_folder(&client, &id, widget_manifest(thread, false)).await;

    let (status, body) = post(
        &client,
        "/threads/archive",
        json!({ "thread_id": thread.to_string() }),
    )
    .await;
    assert_eq!(status, 200, "archive: {body}");
    assert!(app_dir(&id).exists(), "Archive must not remove a widget");

    cleanup(&pool, &[thread]).await;
    remove_folders(&client, &[&id]).await;
    pool.close().await;
}
