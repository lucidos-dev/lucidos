//! Production HTTP recovery after the full restored constructor, on an
//! isolated child HOME and disposable database. No live workspace is touched.
use std::sync::Arc;

use serde_json::{json, Value};
use uuid::Uuid;

pub(super) async fn verify(engine: Arc<crate::engine::LucidosEngine>, generation: Uuid) {
    let pool = engine.pool().clone();
    let device = "recovery-test-device";
    // This device was paired after capture; never recreate it after restore.
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("x-lucidos-device-id", device.parse().unwrap());
    assert!(
        crate::api::actor::require_owner_device_for(&headers, &pool, None, "owner-required")
            .await
            .is_ok()
    );
    headers.insert(
        "x-lucidos-device-id",
        "revoked-snapshot-device".parse().unwrap(),
    );
    assert!(
        crate::api::actor::require_owner_device_for(&headers, &pool, None, "owner-required")
            .await
            .is_err()
    );
    let scheduler = crate::scheduler::SchedulerManager::new(engine.clone(), pool.clone())
        .await
        .unwrap();
    let app = crate::api::create_router(
        engine.clone(),
        pool.clone(),
        engine.event_store().clone(),
        crate::memory::EmbedderSlot::empty(),
        None,
        engine.workspace_path().to_path_buf(),
        Arc::new(tokio::sync::Mutex::new(scheduler)),
        chrono::Utc::now(),
        &crate::net_config::BindChoice::Loopback,
    );
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let url = format!("http://{}/api/v1/threads", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::new();
    // Hold a real capture session while the HTTP writers and restore admission
    // compete for the same workspace-wide archive slot.
    let database_url = crate::core::database_url();
    let capture = crate::core::snapshot::service::Session::open_for_database(
        &database_url,
        Some("constructor-workspace"),
        &engine.archive_job_slot,
        crate::core::snapshot::ArchiveJobKind::SnapshotCapture,
    )
    .unwrap();
    let base = url.trim_end_matches("/threads");
    for (path, body) in [
        ("/snapshots", json!({})),
        ("/backup", json!({"provider":"local"})),
    ] {
        let response = client
            .post(format!("{base}{path}"))
            .header("x-lucidos-device-id", device)
            .header("x-lucidos-restore-generation", generation.to_string())
            .json(&body)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 409, "{path}");
    }
    crate::api::assert_restore_slot_busy(&database_url, &engine.archive_job_slot);
    drop(capture);
    // A revoked archived device also fails the real mutating HTTP gate.
    let refused = client
        .post(&url)
        .header("x-lucidos-device-id", "revoked-snapshot-device")
        .header("x-lucidos-restore-generation", generation.to_string())
        .json(&json!({"id":Uuid::new_v4(),"mode":"lucidos"}))
        .send()
        .await
        .unwrap();
    assert_eq!(refused.status(), 401);
    let id = Uuid::new_v4();
    let selection = json!({"model": "example-model", "reasoningEffort": "high"});
    let body = json!({"id": id, "mode": "lucidos", "recovered_compose": {
        "generation": generation, "text": "Recovered text with its model picks",
        "image_hashes": [], "selection": selection,
    }});
    let post = || {
        client
            .post(&url)
            .header("x-lucidos-device-id", device)
            .header("x-lucidos-restore-generation", generation.to_string())
            .json(&body)
    };
    // Race real HTTP requests through actor resolution, generation admission,
    // and the event-store lock. Both must observe one atomic destination.
    let (first, concurrent) = tokio::join!(post().send(), post().send());
    let first = first.unwrap();
    let concurrent = concurrent.unwrap();
    let mut statuses = [first.status().as_u16(), concurrent.status().as_u16()];
    statuses.sort();
    assert_eq!(statuses, [200, 201]);
    let status = first.status();
    let first: Value = first.json().await.unwrap();
    assert!(status.is_success(), "{first}");
    assert_eq!(concurrent.json::<Value>().await.unwrap(), first);
    assert_eq!(first["text"], body["recovered_compose"]["text"]);
    assert_eq!(first["selection"], selection);
    let row: (String, Value) = sqlx::query_as(
        "SELECT compose_text, compose_selection FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        row,
        ("Recovered text with its model picks".into(), selection)
    );
    // Repeating the request must observe the same destination without a write.
    let repeat = post().send().await.unwrap();
    assert_eq!(repeat.status(), 200);
    assert_eq!(repeat.json::<Value>().await.unwrap(), first);
    let edited_selection = json!({"model": "another-model"});
    let edit = client
        .put(format!("{url}/{id}/compose"))
        .header("x-lucidos-device-id", device)
        .header("x-lucidos-restore-generation", generation.to_string())
        .json(
            &json!({"text": "Edited after recovery", "selection": edited_selection,
            "compose_epoch": first["compose_epoch"]}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(edit.status(), 200);
    let edit: Value = edit.json().await.unwrap();
    let repeat = post().send().await.unwrap();
    assert_eq!(repeat.status(), 200);
    assert_eq!(repeat.json::<Value>().await.unwrap(), edit);
    let row: (String, Value) = sqlx::query_as(
        "SELECT compose_text, compose_selection FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, ("Edited after recovery".into(), edited_selection));
    let starts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ThreadStarted'",
    )
    .bind(id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(starts, 1, "recovery retries must not duplicate creation");
    let sends: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE aggregate_id = $1 AND event_type = 'MessageReceived'",
    )
    .bind(id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(sends, 0, "recovery must never send the retained text");
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
}
