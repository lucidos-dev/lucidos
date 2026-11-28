use crate::support::{b64, db_url, png_bytes, sha256_hex};
use lucidos_engine::core::image_migration::migrate_legacy_image_payloads;
use serde_json::{json, Value};
use uuid::Uuid;

#[tokio::test]
async fn migration_rewrites_legacy_message_received_payload_to_hashes() {
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect");
    let workspace = tempfile::tempdir().expect("tempdir");

    let thread_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    let payload = json!({
        "text": "legacy image test",
        "user_images": [{
            "base64": b64(&png_bytes()),
            "mime_type": "image/png"
        }]
    });
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate) \
         VALUES ($1, 'MessageReceived', $2, NOW(), $3, 'thread')",
    )
    .bind(event_id)
    .bind(&payload)
    .bind(thread_id.to_string())
    .execute(&pool)
    .await
    .expect("seed insert");

    migrate_legacy_image_payloads(&pool, workspace.path())
        .await
        .expect("migration");

    let (rewritten,): (Value,) = sqlx::query_as("SELECT payload FROM events WHERE id = $1")
        .bind(event_id)
        .fetch_one(&pool)
        .await
        .expect("read back");
    assert!(
        rewritten.get("user_images").is_none(),
        "legacy user_images must be removed, got: {rewritten}"
    );
    let expected_hash = sha256_hex(&png_bytes());
    assert_eq!(
        rewritten["user_image_hashes"],
        json!([expected_hash]),
        "user_image_hashes must contain sha256 of the bytes"
    );

    sqlx::query("DELETE FROM events WHERE id = $1")
        .bind(event_id)
        .execute(&pool)
        .await
        .ok();
}

#[tokio::test]
async fn migration_rewrites_production_images_field_to_hashes() {
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect");
    let workspace = tempfile::tempdir().expect("tempdir");

    let thread_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    let payload = json!({
        "text": "production-shape image payload",
        "images": [{
            "base64": b64(&png_bytes()),
            "mime_type": "image/png"
        }]
    });
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate) \
         VALUES ($1, 'MessageReceived', $2, NOW(), $3, 'thread')",
    )
    .bind(event_id)
    .bind(&payload)
    .bind(thread_id.to_string())
    .execute(&pool)
    .await
    .expect("seed insert");

    migrate_legacy_image_payloads(&pool, workspace.path())
        .await
        .expect("migration");

    let (rewritten,): (Value,) = sqlx::query_as("SELECT payload FROM events WHERE id = $1")
        .bind(event_id)
        .fetch_one(&pool)
        .await
        .expect("read back");
    assert!(
        rewritten.get("images").is_none(),
        "legacy images field must be removed, got: {rewritten}"
    );
    let expected_hash = sha256_hex(&png_bytes());
    assert_eq!(
        rewritten["user_image_hashes"],
        json!([expected_hash]),
        "user_image_hashes must contain sha256 of the bytes"
    );

    sqlx::query("DELETE FROM events WHERE id = $1")
        .bind(event_id)
        .execute(&pool)
        .await
        .ok();
}

#[tokio::test]
async fn migration_skips_non_legacy_images_arrays() {
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect");
    let workspace = tempfile::tempdir().expect("tempdir");

    let response_id = Uuid::new_v4();
    let response_payload = json!({
        "text": "...",
        "images": ["memory_hit_id_1", "memory_hit_id_2"]
    });
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate) \
         VALUES ($1, 'ResponseGenerated', $2, NOW(), $3, 'thread')",
    )
    .bind(response_id)
    .bind(&response_payload)
    .bind(Uuid::new_v4().to_string())
    .execute(&pool)
    .await
    .expect("seed response_generated");

    let _ = migrate_legacy_image_payloads(&pool, workspace.path())
        .await
        .expect("migration");

    let (after,): (Value,) = sqlx::query_as("SELECT payload FROM events WHERE id = $1")
        .bind(response_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        after, response_payload,
        "ResponseGenerated must not be touched"
    );

    sqlx::query("DELETE FROM events WHERE id = $1")
        .bind(response_id)
        .execute(&pool)
        .await
        .ok();
}

#[tokio::test]
async fn migration_is_noop_on_already_migrated_payload() {
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect");
    let workspace = tempfile::tempdir().expect("tempdir");

    let thread_id = Uuid::new_v4();
    let event_id = Uuid::new_v4();
    let payload = json!({
        "text": "already migrated",
        "user_image_hashes": ["abcd1234"]
    });
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate) \
         VALUES ($1, 'MessageReceived', $2, NOW(), $3, 'thread')",
    )
    .bind(event_id)
    .bind(&payload)
    .bind(thread_id.to_string())
    .execute(&pool)
    .await
    .expect("seed insert");

    let _ = migrate_legacy_image_payloads(&pool, workspace.path())
        .await
        .expect("migration");

    let (after,): (Value,) = sqlx::query_as("SELECT payload FROM events WHERE id = $1")
        .bind(event_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after, payload, "already-migrated row must not be touched");

    sqlx::query("DELETE FROM events WHERE id = $1")
        .bind(event_id)
        .execute(&pool)
        .await
        .ok();
}

#[tokio::test]
async fn migration_preserves_already_migrated_compose_images() {
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect");
    let workspace = tempfile::tempdir().expect("tempdir");

    let thread_id = Uuid::new_v4();
    let hashes = json!(["abcd1234567890", "ef0987654321"]);
    sqlx::query(
        "INSERT INTO thread_summaries \
            (thread_id, source, is_coding_agent, created_at, last_activity, message_count, status, \
             state, compose_text, compose_images) \
         VALUES ($1, 'chat', FALSE, NOW(), NOW(), 0, 'idle', \
                 'composing', '', $2) \
         ON CONFLICT (thread_id) DO UPDATE \
           SET state = 'composing', compose_images = EXCLUDED.compose_images",
    )
    .bind(thread_id)
    .bind(&hashes)
    .execute(&pool)
    .await
    .expect("seed thread_summaries");

    let _ = migrate_legacy_image_payloads(&pool, workspace.path())
        .await
        .expect("migration");

    let (after,): (Value,) =
        sqlx::query_as("SELECT compose_images FROM thread_summaries WHERE thread_id = $1")
            .bind(thread_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        after, hashes,
        "already-migrated compose_images hash array must survive intact"
    );

    sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .execute(&pool)
        .await
        .ok();
}
