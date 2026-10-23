//! End-to-end API tests for widgets (ADRs 0402, 0407, 0415, 0466).
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

/// A reply in `thread`, for a pin to name as where its widget draws.
async fn seed_reply(pool: &sqlx::PgPool, thread: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created, thread_id) \
         VALUES ($1, 'thread', $2::text, 'TextStreamed', $3, NOW(), $2)",
    )
    .bind(id)
    .bind(thread)
    .bind(json!({ "text": "Here are the fares." }))
    .execute(pool)
    .await
    .expect("insert the reply");
    id
}

#[tokio::test]
async fn a_widget_pins_and_unpins_without_touching_disk() {
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

    // A reply embeds a widget now, so nothing shows one (ADR 0466).
    let (status, _) = post(
        &client,
        "/widgets/show",
        json!({ "app_id": id, "thread_id": thread }),
    )
    .await;
    assert!(
        [404, 405].contains(&status),
        "the show route is gone: {status}"
    );

    let history_before = folder_history(&id);
    let manifest_before = std::fs::read_to_string(app_dir(&id).join("manifest.json")).unwrap();

    // A pin names the reply that draws the widget, which must be in the thread.
    let (status, _) = post(
        &client,
        "/widgets/pin",
        json!({ "app_id": id, "thread_id": thread, "shown_event_id": Uuid::new_v4() }),
    )
    .await;
    assert_eq!(status, 409, "a reply from nowhere is refused");
    let reply = seed_reply(&pool, thread).await;
    let (status, body) = post(
        &client,
        "/widgets/pin",
        json!({ "app_id": id, "thread_id": thread, "shown_event_id": reply }),
    )
    .await;
    assert_eq!(status, 200, "pin: {body}");
    let entries = thread_widgets(&client, thread).await;
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(entries[0]["app_id"], id.as_str());
    assert_eq!(entries[0]["pinned"], true);
    assert_eq!(
        entries[0]["shown_event_id"],
        reply.to_string(),
        "Show in thread lands on the reply"
    );

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
    assert_eq!(types, ["WidgetPinned", "WidgetUnpinned"]);
    for (_, payload) in &payloads {
        assert_eq!(payload["app_id"], id.as_str());
        assert!(
            !payload.to_string().contains("<h1>"),
            "no HTML in an event: {payload}"
        );
    }

    // A second thread may not pin a widget that is not reusable.
    let other = Uuid::new_v4();
    seed_thread(&pool, other, None, "idle").await;
    let (status, _) = post(
        &client,
        "/widgets/pin",
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

/// ADR 0415 over real HTTP. The built-in sound player resolves, and two param
/// sets are two instances with their own pins. Bad params are refused, and no
/// route may change the built-in.
#[tokio::test]
async fn built_in_instances_pin_apart_and_the_built_in_stays_read_only() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect");
    let thread = Uuid::new_v4();
    seed_thread(&pool, thread, None, "idle").await;
    let id = "lucidos-sound-player";

    let app: serde_json::Value = client
        .get(format!("{}/api/v1/app?id={id}", base_url()))
        .send()
        .await
        .expect("app request")
        .json()
        .await
        .expect("app JSON");
    assert_eq!(app["built_in"], true, "{app}");
    assert_eq!(app["params"]["clip"]["required"], true, "{app}");

    let marin = json!({ "clip": "artifacts/voices/marin.mp3" });
    let cedar = json!({ "clip": "artifacts/voices/cedar.mp3" });
    for (params, label) in [(&marin, "Marin"), (&cedar, "Cedar")] {
        let (status, body) = post(
            &client,
            "/widgets/pin",
            json!({ "app_id": id, "thread_id": thread, "params": params, "label": label }),
        )
        .await;
        assert_eq!(status, 200, "pin: {body}");
    }
    let (status, body) = post(
        &client,
        "/widgets/unpin",
        json!({ "app_id": id, "thread_id": thread, "params": marin }),
    )
    .await;
    assert_eq!(status, 200, "unpin: {body}");

    let entries = thread_widgets(&client, thread).await;
    assert_eq!(entries.len(), 2, "two instances: {entries:?}");
    assert_eq!(entries[0]["label"], "Marin");
    assert_eq!(entries[0]["pinned"], false);
    assert_eq!(entries[1]["params"], cedar);
    assert_eq!(entries[1]["pinned"], true);

    for bad in [
        json!({}),
        json!({ "clip": "a.mp3", "volume": 3 }),
        json!([1]),
    ] {
        let (status, body) = post(
            &client,
            "/widgets/pin",
            json!({ "app_id": id, "thread_id": thread, "params": bad }),
        )
        .await;
        assert!(
            [400, 409, 422].contains(&status),
            "{bad} must be refused: {status} {body}"
        );
    }

    let (status, body) = post(&client, "/widgets/stop-reusing", json!({ "app_id": id })).await;
    assert_eq!(status, 409, "stop-reusing a built-in: {body}");
    let resp = client
        .put(format!("{}/api/v1/data/apps/{id}/index.html", base_url()))
        .body("<h1>mine</h1>")
        .send()
        .await
        .expect("data write request");
    assert_eq!(resp.status().as_u16(), 403, "a data write to a built-in");

    cleanup(&pool, &[thread]).await;
    pool.close().await;
}

async fn dashboard(client: &reqwest::Client, which: &str) -> serde_json::Value {
    client
        .get(format!("{}/api/v1/dashboard?dashboard={which}", base_url()))
        .send()
        .await
        .expect("dashboard request failed")
        .json()
        .await
        .expect("the dashboard is JSON")
}

/// (app id, size) per tile, keeping only this test's widgets: other tests may
/// add to the same dashboards.
fn tile_sizes(board: &serde_json::Value, ours: &[&str]) -> Vec<(String, String)> {
    board["tiles"]
        .as_array()
        .expect("tiles")
        .iter()
        .filter(|t| ours.contains(&t["app_id"].as_str().unwrap_or_default()))
        .map(|t| {
            (
                t["app_id"].as_str().unwrap().to_string(),
                t["size"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

/// Prompt parity for the dashboards (ADRs 0432, 0496). Each dashboard holds
/// its own tiles, and an arrangement over HTTP reads back the same. A new tile
/// lands at its first declared size, and an undeclared size is refused.
/// Adding a thread's own widget makes it reusable in the same action. The
/// Home thread takes no pins.
#[tokio::test]
async fn the_dashboards_hold_their_own_tiles_over_http() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect");
    let clock = folder("clock");
    let inbox = folder("inbox");
    let owned = folder("owned");
    let origin = Uuid::new_v4();
    seed_thread(&pool, origin, None, "idle").await;
    write_app_folder(
        &client,
        &clock,
        json!({"name": "Clock", "kind": "widget", "reusable": true,
               "sizes": ["small", "large"], "transparent": true}),
    )
    .await;
    write_app_folder(
        &client,
        &inbox,
        json!({"name": "Inbox", "kind": "widget", "reusable": true}),
    )
    .await;
    write_app_folder(&client, &owned, widget_manifest(origin, false)).await;

    for id in [&clock, &inbox] {
        let (status, body) = post(
            &client,
            "/widgets/dashboard-add",
            json!({ "app_id": id, "dashboard": "desktop" }),
        )
        .await;
        assert_eq!(status, 200, "add {id}: {body}");
    }
    let ours = [clock.as_str(), inbox.as_str(), owned.as_str()];
    assert_eq!(
        tile_sizes(&dashboard(&client, "desktop").await, &ours),
        [
            (clock.clone(), "small".into()),
            (inbox.clone(), "medium".into())
        ],
        "a new tile lands at its first declared size"
    );
    assert!(
        tile_sizes(&dashboard(&client, "phone").await, &ours).is_empty(),
        "the desktop's tiles stay on the desktop"
    );

    let (status, arranged) = post(
        &client,
        "/widgets/arrange",
        json!({ "dashboard": "desktop", "tiles": [
            { "app_id": inbox, "size": "medium" },
            { "app_id": clock, "size": "large" },
        ]}),
    )
    .await;
    assert_eq!(status, 200, "arrange: {arranged}");
    let expected = [
        (inbox.clone(), "medium".into()),
        (clock.clone(), "large".into()),
    ];
    assert_eq!(tile_sizes(&arranged, &ours), expected);
    assert_eq!(
        tile_sizes(&dashboard(&client, "desktop").await, &ours),
        expected
    );

    let (status, body) = post(
        &client,
        "/widgets/arrange",
        json!({ "dashboard": "phone", "tiles": [{ "app_id": clock, "size": "small" }] }),
    )
    .await;
    assert_eq!(status, 409, "clock is not on the phone dashboard: {body}");

    let (status, body) = post(
        &client,
        "/widgets/dashboard-add",
        json!({ "app_id": owned, "dashboard": "phone" }),
    )
    .await;
    assert_eq!(status, 200, "add to the phone: {body}");
    assert_eq!(body["made_reusable"], true, "{body}");
    assert_eq!(
        tile_sizes(&dashboard(&client, "phone").await, &ours),
        [(owned.clone(), "medium".into())]
    );

    let home: String =
        sqlx::query_scalar("SELECT thread_id::text FROM thread_summaries WHERE is_home")
            .fetch_one(&pool)
            .await
            .expect("the workspace has a home thread");
    let (status, body) = post(
        &client,
        "/widgets/pin",
        json!({ "app_id": clock, "thread_id": home }),
    )
    .await;
    assert_eq!(status, 409, "Home has no shelf: {body}");

    for (id, which) in [(&clock, "desktop"), (&inbox, "desktop"), (&owned, "phone")] {
        let (status, body) = post(
            &client,
            "/widgets/dashboard-remove",
            json!({ "app_id": id, "dashboard": which }),
        )
        .await;
        assert_eq!(status, 200, "remove {id}: {body}");
    }
    for which in ["desktop", "phone"] {
        assert!(tile_sizes(&dashboard(&client, which).await, &ours).is_empty());
    }
    cleanup(&pool, &[origin]).await;
    remove_folders(&client, &[&clock, &inbox, &owned]).await;
    pool.close().await;
}

/// ADR 0532 over real HTTP. An app that is not a widget joins a dashboard as a
/// quarter-size shortcut through the one checked add. The tile reads back as
/// `kind: "app"`, takes only `quarter`, and refuses params and unknown ids.
#[tokio::test]
async fn an_app_joins_a_dashboard_as_a_quarter_shortcut_over_http() {
    let client = user_client().await;
    let notes = folder("notes");
    write_app_folder(
        &client,
        &notes,
        json!({"name": "Notes", "description": "Quick notes"}),
    )
    .await;

    let (status, body) = post(
        &client,
        "/widgets/dashboard-add",
        json!({ "app_id": notes, "dashboard": "desktop", "params": { "a": 1 } }),
    )
    .await;
    assert_eq!(status, 409, "an app takes no params: {body}");

    let (status, body) = post(
        &client,
        "/widgets/dashboard-add",
        json!({ "app_id": "no-such-app-exists", "dashboard": "desktop" }),
    )
    .await;
    assert_eq!(status, 409, "an unknown id is refused: {body}");

    let (status, body) = post(
        &client,
        "/widgets/dashboard-add",
        json!({ "app_id": notes, "dashboard": "desktop", "label": "My notes" }),
    )
    .await;
    assert_eq!(status, 200, "add the app: {body}");
    assert_ne!(body["made_reusable"], true, "an app is never made reusable");

    let board = dashboard(&client, "desktop").await;
    let tile = board["tiles"]
        .as_array()
        .expect("tiles")
        .iter()
        .find(|t| t["app_id"] == notes.as_str())
        .unwrap_or_else(|| panic!("the shortcut is on the dashboard: {board}"));
    assert_eq!(tile["kind"], "app", "{tile}");
    assert_eq!(tile["size"], "quarter", "{tile}");
    assert_eq!(tile["name"], "Notes", "{tile}");

    let (status, body) = post(
        &client,
        "/widgets/dashboard-add",
        json!({ "app_id": notes, "dashboard": "desktop" }),
    )
    .await;
    assert_eq!(status, 409, "a second add is refused: {body}");

    let (status, body) = post(
        &client,
        "/widgets/arrange",
        json!({ "dashboard": "desktop", "tiles": [{ "app_id": notes, "size": "small" }] }),
    )
    .await;
    assert_eq!(status, 409, "a shortcut takes only quarter: {body}");
    let (status, body) = post(
        &client,
        "/widgets/arrange",
        json!({ "dashboard": "desktop", "tiles": [{ "app_id": notes, "size": "quarter" }] }),
    )
    .await;
    assert_eq!(status, 200, "arrange at quarter: {body}");

    let (status, body) = post(
        &client,
        "/widgets/dashboard-remove",
        json!({ "app_id": notes, "dashboard": "desktop" }),
    )
    .await;
    assert_eq!(status, 200, "remove the shortcut: {body}");
    assert!(
        tile_sizes(&dashboard(&client, "desktop").await, &[notes.as_str()]).is_empty(),
        "the shortcut left the dashboard"
    );
    remove_folders(&client, &[&notes]).await;
}
