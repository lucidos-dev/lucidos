//! E2E tests for the content-addressed blob store HTTP surface.
//!
//! Covers POST /api/v1/threads/:id/blobs (multipart upload, returns hash),
//! GET /api/v1/blobs/:hash (streams original with immutable cache),
//! and GET /api/v1/blobs/:hash/preview (downscaled JPEG for browser display).
//!
//! See `docs/plans/2026-05-07-image-blob-store-design.md`.

use crate::support::{
    base_url, db_url, encoded_jpeg, png_bytes, sha256_hex, user_client, workspace_path,
};
use reqwest::multipart::{Form, Part};
use serde_json::json;
use uuid::Uuid;

fn threads_url() -> String {
    format!("{}/api/v1/threads", base_url())
}

fn blobs_url(thread_id: &Uuid) -> String {
    format!("{}/api/v1/threads/{}/blobs", base_url(), thread_id)
}

fn blob_url(hash: &str) -> String {
    format!("{}/api/v1/blobs/{}", base_url(), hash)
}

fn blob_preview_url(hash: &str) -> String {
    format!("{}/api/v1/blobs/{}/preview", base_url(), hash)
}

fn jpeg_form(bytes: Vec<u8>) -> Form {
    Form::new().part(
        "file",
        Part::bytes(bytes)
            .file_name("test.jpg")
            .mime_str("image/jpeg")
            .unwrap(),
    )
}

async fn create_thread(client: &reqwest::Client, mode: &str) -> Uuid {
    let id = Uuid::new_v4();
    let resp = client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": mode }))
        .send()
        .await
        .expect("POST /threads failed");
    assert!(
        resp.status().is_success(),
        "thread creation failed: {}",
        resp.status()
    );
    id
}

fn png_form() -> Form {
    Form::new().part(
        "file",
        Part::bytes(png_bytes())
            .file_name("test.png")
            .mime_str("image/png")
            .unwrap(),
    )
}

#[tokio::test]
async fn post_blob_returns_hash_and_writes_file() {
    let client = user_client().await;
    let thread_id = create_thread(&client, "lucidos").await;
    let bytes = png_bytes();
    let expected_hash = sha256_hex(&bytes);

    let resp = client
        .post(blobs_url(&thread_id))
        .multipart(png_form())
        .send()
        .await
        .expect("POST /blobs failed");
    assert_eq!(resp.status(), 201, "POST /blobs should return 201");

    let body: serde_json::Value = resp.json().await.expect("response is not JSON");
    assert_eq!(body["hash"], expected_hash);
    assert_eq!(body["mime"], "image/png");
    assert_eq!(body["byte_size"], bytes.len());

    // Verify the blob exists on disk under the workspace's data/blobs/ tree.
    let blob_path = workspace_path()
        .join("data/blobs")
        .join(&expected_hash[..2])
        .join(format!("{expected_hash}.png"));
    assert!(
        blob_path.exists(),
        "blob file should exist on disk at {}",
        blob_path.display()
    );
    let on_disk = std::fs::read(&blob_path).expect("read blob");
    assert_eq!(on_disk, bytes, "on-disk bytes must equal upload");
}

#[tokio::test]
async fn post_blob_idempotent_on_same_bytes() {
    let client = user_client().await;
    let thread_id = create_thread(&client, "lucidos").await;

    let r1 = client
        .post(blobs_url(&thread_id))
        .multipart(png_form())
        .send()
        .await
        .expect("first POST failed");
    assert!(
        r1.status().is_success(),
        "first upload failed: {}",
        r1.status()
    );
    let h1: serde_json::Value = r1.json().await.unwrap();

    let r2 = client
        .post(blobs_url(&thread_id))
        .multipart(png_form())
        .send()
        .await
        .expect("second POST failed");
    assert!(
        r2.status().is_success(),
        "second upload failed: {}",
        r2.status()
    );
    let h2: serde_json::Value = r2.json().await.unwrap();

    assert_eq!(h1["hash"], h2["hash"], "same bytes must produce same hash");
}

#[tokio::test]
async fn post_blob_rejects_non_image_with_unsupported_mime() {
    let client = user_client().await;
    let thread_id = create_thread(&client, "lucidos").await;

    let form = Form::new().part(
        "file",
        Part::bytes(b"plain text not an image".to_vec())
            .file_name("notes.txt")
            .mime_str("text/plain")
            .unwrap(),
    );
    let resp = client
        .post(blobs_url(&thread_id))
        .multipart(form)
        .send()
        .await
        .expect("POST failed");
    assert_eq!(
        resp.status(),
        415,
        "non-image upload should be 415 Unsupported Media Type"
    );
}

/// The 415 must say what these bytes turned out to be. Reciting the allowlist
/// leaves the caller to guess, which is what the paste bug did to the user.
#[tokio::test]
async fn post_blob_names_the_uploaded_format_in_its_415() {
    let client = user_client().await;
    let thread_id = create_thread(&client, "lucidos").await;

    // A little-endian TIFF header. The declared mime is a lie the server
    // ignores: it reads the bytes.
    let mut tiff = vec![0x49, 0x49, 0x2a, 0x00, 0x08, 0x00, 0x00, 0x00];
    tiff.resize(64, 0);
    let form = Form::new().part(
        "file",
        Part::bytes(tiff)
            .file_name("screenshot.tiff")
            .mime_str("image/png")
            .unwrap(),
    );
    let resp = client
        .post(blobs_url(&thread_id))
        .multipart(form)
        .send()
        .await
        .expect("POST failed");
    assert_eq!(resp.status(), 415, "a TIFF is outside the allowlist");

    let body: serde_json::Value = resp.json().await.unwrap();
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("TIFF"),
        "the 415 must name the format, got: {error}"
    );
    assert!(
        !error.contains("webp"),
        "the 415 must not recite the allowlist, got: {error}"
    );
}

/// An empty upload is its own verdict, not "we could not recognize this".
#[tokio::test]
async fn post_blob_calls_an_empty_upload_empty() {
    let client = user_client().await;
    let thread_id = create_thread(&client, "lucidos").await;

    let form = Form::new().part(
        "file",
        Part::bytes(Vec::new())
            .file_name("image.png")
            .mime_str("image/png")
            .unwrap(),
    );
    let resp = client
        .post(blobs_url(&thread_id))
        .multipart(form)
        .send()
        .await
        .expect("POST failed");
    assert_eq!(resp.status(), 415, "zero bytes are not an image");

    let body: serde_json::Value = resp.json().await.unwrap();
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("empty"),
        "the 415 must say the upload was empty, got: {error}"
    );
}

#[tokio::test]
async fn post_blob_rejects_missing_thread() {
    let client = user_client().await;
    let bogus = Uuid::new_v4();
    let resp = client
        .post(blobs_url(&bogus))
        .multipart(png_form())
        .send()
        .await
        .expect("POST failed");
    assert_eq!(
        resp.status(),
        404,
        "upload to nonexistent thread should be 404"
    );
}

#[tokio::test]
async fn post_blob_rejects_discarded_thread() {
    let client = user_client().await;
    let thread_id = create_thread(&client, "lucidos").await;
    // Discard it.
    let del = client
        .delete(format!("{}/api/v1/threads/{}", base_url(), thread_id))
        .send()
        .await
        .expect("DELETE failed");
    assert!(del.status().is_success(), "DELETE failed: {}", del.status());

    let resp = client
        .post(blobs_url(&thread_id))
        .multipart(png_form())
        .send()
        .await
        .expect("POST failed");
    assert_eq!(
        resp.status(),
        410,
        "upload to discarded thread should be 410 Gone"
    );
}

#[tokio::test]
async fn post_blob_emits_image_uploaded_event() {
    let client = user_client().await;
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to e2e db");
    let thread_id = create_thread(&client, "lucidos").await;
    let expected_hash = sha256_hex(&png_bytes());

    let resp = client
        .post(blobs_url(&thread_id))
        .multipart(png_form())
        .send()
        .await
        .expect("POST failed");
    assert!(resp.status().is_success());

    let row: Option<(serde_json::Value,)> = sqlx::query_as(
        "SELECT payload FROM events WHERE event_type = 'ImageUploaded' AND aggregate_id = $1",
    )
    .bind(thread_id.to_string())
    .fetch_optional(&pool)
    .await
    .expect("query");
    let (payload,) = row.expect("ImageUploaded event must exist for the thread");
    assert_eq!(payload["hash"], expected_hash);
    assert_eq!(payload["mime"], "image/png");
    assert_eq!(payload["byte_size"], png_bytes().len());
}

#[tokio::test]
async fn get_blob_returns_bytes_with_mime_after_upload() {
    let client = user_client().await;
    let thread_id = create_thread(&client, "lucidos").await;
    let bytes = png_bytes();

    let upload = client
        .post(blobs_url(&thread_id))
        .multipart(png_form())
        .send()
        .await
        .expect("upload failed");
    assert!(upload.status().is_success());
    let hash = upload.json::<serde_json::Value>().await.unwrap()["hash"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = client
        .get(blob_url(&hash))
        .send()
        .await
        .expect("GET failed");
    assert_eq!(resp.status(), 200, "GET should be 200");
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("image/png"),
        "GET must set Content-Type from sniffed mime"
    );
    let returned = resp.bytes().await.expect("read body").to_vec();
    assert_eq!(returned, bytes, "GET must return the uploaded bytes");
}

#[tokio::test]
async fn get_blob_sets_immutable_cache_header() {
    let client = user_client().await;
    let thread_id = create_thread(&client, "lucidos").await;
    let upload = client
        .post(blobs_url(&thread_id))
        .multipart(png_form())
        .send()
        .await
        .expect("upload failed");
    let hash = upload.json::<serde_json::Value>().await.unwrap()["hash"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = client
        .get(blob_url(&hash))
        .send()
        .await
        .expect("GET failed");
    let cache = resp
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    // Content-addressed = immutable; max-age=1 year is the convention.
    assert!(
        cache.contains("immutable") && cache.contains("max-age=31536000"),
        "Cache-Control must declare immutable and 1y max-age, got: {cache:?}"
    );
}

#[tokio::test]
async fn get_blob_returns_404_for_unknown_hash() {
    let client = user_client().await;
    // Valid-shaped hash but unused.
    let bogus = "0".repeat(64);
    let resp = client
        .get(blob_url(&bogus))
        .send()
        .await
        .expect("GET failed");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn get_blob_returns_404_for_malformed_hash() {
    let client = user_client().await;
    // Path traversal attempt + short string — must not 200.
    let resp = client
        .get(blob_url("..%2F..%2Fetc%2Fpasswd"))
        .send()
        .await
        .expect("GET failed");
    assert_ne!(resp.status(), 200);
    let resp = client
        .get(blob_url("short"))
        .send()
        .await
        .expect("GET failed");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn get_blob_preview_downscales_large_image_to_jpeg() {
    let client = user_client().await;
    let thread_id = create_thread(&client, "lucidos").await;
    // 4000 long edge — exceeds the 2048-px preview cap, so the endpoint
    // must serve a downscaled JPEG instead of the original.
    let bytes = encoded_jpeg(4000, 3000);
    let upload = client
        .post(blobs_url(&thread_id))
        .multipart(jpeg_form(bytes.clone()))
        .send()
        .await
        .expect("upload failed");
    assert!(upload.status().is_success());
    let hash = upload.json::<serde_json::Value>().await.unwrap()["hash"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = client
        .get(blob_preview_url(&hash))
        .send()
        .await
        .expect("GET preview failed");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("image/jpeg"),
        "preview is always JPEG"
    );

    let returned = resp.bytes().await.expect("read body").to_vec();
    assert!(
        returned.len() < bytes.len(),
        "preview should be smaller than original ({} >= {})",
        returned.len(),
        bytes.len()
    );

    let img = image::load_from_memory(&returned).expect("preview must decode as JPEG");
    assert_eq!(
        (img.width(), img.height()),
        (2048, 1536),
        "long edge clamped to 2048, aspect preserved"
    );
}

#[tokio::test]
async fn get_blob_preview_serves_original_when_image_within_cap() {
    let client = user_client().await;
    let thread_id = create_thread(&client, "lucidos").await;
    // 800×600 is well under the 2048-px cap — endpoint must serve the
    // original bytes (and the original PNG mime, in this case).
    let bytes = png_bytes();
    let upload = client
        .post(blobs_url(&thread_id))
        .multipart(png_form())
        .send()
        .await
        .expect("upload failed");
    let hash = upload.json::<serde_json::Value>().await.unwrap()["hash"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = client
        .get(blob_preview_url(&hash))
        .send()
        .await
        .expect("GET preview failed");
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("image/png"),
        "small image: serve original mime"
    );
    let returned = resp.bytes().await.expect("read body").to_vec();
    assert_eq!(
        returned, bytes,
        "small image: returned bytes match original"
    );
}

#[tokio::test]
async fn get_blob_preview_returns_404_for_unknown_hash() {
    let client = user_client().await;
    let bogus = "0".repeat(64);
    let resp = client
        .get(blob_preview_url(&bogus))
        .send()
        .await
        .expect("GET failed");
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn get_blob_preview_sets_immutable_cache_header() {
    let client = user_client().await;
    let thread_id = create_thread(&client, "lucidos").await;
    let upload = client
        .post(blobs_url(&thread_id))
        .multipart(png_form())
        .send()
        .await
        .expect("upload failed");
    let hash = upload.json::<serde_json::Value>().await.unwrap()["hash"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = client
        .get(blob_preview_url(&hash))
        .send()
        .await
        .expect("GET preview failed");
    let cache = resp
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    // Preview is content-addressed (deterministic from original + cap), so
    // same immutable cache contract as the original.
    assert!(
        cache.contains("immutable") && cache.contains("max-age=31536000"),
        "Cache-Control must declare immutable and 1y max-age, got: {cache:?}"
    );
}

/// Regression: lazy preview generation took 16-25 s on first display for
/// large iPhone JPEGs in dev. iOS Safari aborted the fetch and the page
/// was left with broken `<img>` elements until reload. POST /blobs must
/// pre-generate the preview in the background so the first display GET
/// always hits the warm cache.
#[tokio::test]
async fn post_blob_pregenerates_preview_in_background_for_large_image() {
    let client = user_client().await;
    let thread_id = create_thread(&client, "lucidos").await;
    let bytes = encoded_jpeg(4000, 3000);

    let upload = client
        .post(blobs_url(&thread_id))
        .multipart(jpeg_form(bytes))
        .send()
        .await
        .expect("upload failed");
    assert!(
        upload.status().is_success(),
        "upload failed: {}",
        upload.status()
    );
    let hash = upload.json::<serde_json::Value>().await.unwrap()["hash"]
        .as_str()
        .unwrap()
        .to_string();

    let cache_path = workspace_path()
        .join(".lucidos/blob-previews")
        .join(&hash[..2])
        .join(format!(
            "{hash}-{}.jpg",
            lucidos_engine::core::blobs::PREVIEW_MAX_EDGE
        ));

    // Background task starts after upload returns; poll for the cache file
    // to appear within a generous window (debug build, Lanczos3 4k→2k).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if cache_path.exists() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!(
        "preview cache file should exist after background pre-generation: {}",
        cache_path.display()
    );
}

/// The pre-generation must NOT block the upload response — the user's
/// upload latency stays bounded by network + multipart parse, never by
/// decode + resize.
#[tokio::test]
async fn post_blob_returns_quickly_even_when_pregeneration_is_slow() {
    let client = user_client().await;
    let thread_id = create_thread(&client, "lucidos").await;
    let bytes = encoded_jpeg(4000, 3000);

    let start = std::time::Instant::now();
    let upload = client
        .post(blobs_url(&thread_id))
        .multipart(jpeg_form(bytes))
        .send()
        .await
        .expect("upload failed");
    let elapsed = start.elapsed();
    assert!(upload.status().is_success());

    // Synchronous Lanczos3 4k→2k on a debug build is ~5-10 s. The upload
    // response must come back well under that. 3 s leaves comfortable
    // headroom for slow CI without masking a regression.
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "POST /blobs took {elapsed:?} — preview pre-generation must not block the response"
    );
}
