//! Verify `GET /api/v1/threads/:id/events` is gzip-compressed when the client
//! offers `Accept-Encoding`, and that the decoded body is byte-identical to the
//! uncompressed response. Heavy coding-agent threads ship multiple MB of event
//! JSON on every open; without compression that transfer dominates load latency
//! over Tailscale / on mobile. The default tower-http predicate leaves the SSE
//! stream untouched (covered by `sse_test.rs`).
//!
//! The e2e reqwest client is built without the `gzip` feature (see
//! `lucidos-e2e/Cargo.toml`), so it neither auto-adds `Accept-Encoding` nor
//! auto-decodes — setting the header here exercises the real wire path and
//! `.bytes()` returns the raw compressed body for us to decode explicitly.

use crate::support::{base_url, db_url, http_client, seed_chat_thread_summary, unique_marker};
use serde_json::Value;
use uuid::Uuid;

async fn pool() -> sqlx::PgPool {
    sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to e2e DB")
}

/// Seed a thread whose snapshot body is large and highly compressible — a long
/// `MessageReceived` text, which is not subject to any payload stripping.
async fn seed_large_thread(pool: &sqlx::PgPool) -> Uuid {
    let thread_id = Uuid::new_v4();
    let marker = unique_marker("api-snapshot-compression");
    seed_chat_thread_summary(pool, thread_id, "idle").await;
    let big_text = format!("compressible body for {marker} ").repeat(4000); // ~120 KB
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, created, aggregate_id, aggregate, thread_id) \
         VALUES ($1, 'MessageReceived', $2, NOW(), $3::text, 'thread', $3)",
    )
    .bind(Uuid::new_v4())
    .bind(serde_json::json!({ "text": big_text, "channel": "chat" }))
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("seed MessageReceived");
    thread_id
}

#[tokio::test]
async fn snapshot_is_gzip_compressed_and_decodes_identically() {
    use async_compression::tokio::bufread::GzipDecoder;
    use tokio::io::{AsyncReadExt, BufReader};

    let pool = pool().await;
    let thread_id = seed_large_thread(&pool).await;
    let url = format!("{}/api/v1/threads/{}/events", base_url(), thread_id);

    // Plain request (no Accept-Encoding) — the reference body.
    let plain_resp = http_client()
        .get(&url)
        .send()
        .await
        .expect("plain snapshot request failed");
    assert_eq!(plain_resp.status(), 200);
    assert!(
        plain_resp.headers().get("content-encoding").is_none(),
        "no Accept-Encoding offered → response must be uncompressed"
    );
    let plain_bytes = plain_resp.bytes().await.expect("plain bytes");
    let plain_json: Value = serde_json::from_slice(&plain_bytes).expect("plain JSON");

    // Same request, offering gzip.
    let gz_resp = http_client()
        .get(&url)
        .header("Accept-Encoding", "gzip")
        .send()
        .await
        .expect("gzip snapshot request failed");
    assert_eq!(gz_resp.status(), 200);
    assert_eq!(
        gz_resp
            .headers()
            .get("content-encoding")
            .and_then(|v| v.to_str().ok()),
        Some("gzip"),
        "Accept-Encoding: gzip → response must be gzip-encoded"
    );
    let gz_bytes = gz_resp.bytes().await.expect("gzip bytes");
    assert!(
        gz_bytes.len() < plain_bytes.len(),
        "compressed body ({} bytes) must be smaller than plain ({} bytes)",
        gz_bytes.len(),
        plain_bytes.len()
    );
    assert_eq!(&gz_bytes[..2], &[0x1f, 0x8b], "gzip magic bytes");

    // Decoded gzip body must equal the plain body exactly.
    let mut decoder = GzipDecoder::new(BufReader::new(&gz_bytes[..]));
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).await.expect("gunzip");
    let decoded_json: Value = serde_json::from_slice(&decoded).expect("decoded JSON");
    assert_eq!(
        decoded_json, plain_json,
        "decoded gzip body must equal the plain body"
    );
}
