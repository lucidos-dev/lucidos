use super::*;
use axum::http::Request;
use base64::engine::general_purpose::STANDARD as B64;
use tower::ServiceExt as _;

const NOW: u64 = 1_800_000_000;
/// The route B's own picker reaches. A wire contract with `control.ts`.
const UPLOADS_ROUTE: &str = "/~/api/v1/control/restore-uploads";

fn uploads() -> (tempfile::TempDir, RestoreUploads) {
    let dir = tempfile::tempdir().unwrap();
    let uploads = RestoreUploads::new(dir.path().join("restore-uploads"));
    (dir, uploads)
}

fn sha(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn part_len(uploads: &RestoreUploads, id: &str) -> u64 {
    std::fs::metadata(uploads.session_dir(id).join(PART))
        .unwrap()
        .len()
}

async fn status_of<T>(result: Result<T, ApiError>) -> StatusCode {
    match result {
        Ok(_) => StatusCode::OK,
        Err(e) => e.into_response().status(),
    }
}

// ── The session store ───────────────────────────────────────────────────────

#[tokio::test]
async fn chunks_append_in_order_and_the_offset_survives_a_reload() {
    let (dir, uploads) = uploads();
    let id = uploads.create(10, "b.age".into(), NOW).unwrap();
    let after = uploads
        .append(&id, 0, Some(sha(b"hello")), Body::from("hello"), NOW)
        .await
        .unwrap();
    assert_eq!(after.offset, 5);

    // A gateway restart: a fresh store over the same directory.
    let reloaded = RestoreUploads::new(dir.path().join("restore-uploads"));
    assert_eq!(reloaded.claim(&id, NOW).unwrap().record().offset, 5);
    let done = reloaded
        .append(&id, 5, None, Body::from("world"), NOW)
        .await
        .unwrap();
    assert!(done.is_complete());
    let staged = std::fs::read(reloaded.session_dir(&id).join(PART)).unwrap();
    assert_eq!(staged, b"helloworld");
}

#[tokio::test]
async fn a_chunk_at_the_wrong_offset_is_refused_and_stores_nothing() {
    let (_dir, uploads) = uploads();
    let id = uploads.create(10, "b.age".into(), NOW).unwrap();
    uploads
        .append(&id, 0, None, Body::from("abc"), NOW)
        .await
        .unwrap();
    let refused = uploads.append(&id, 0, None, Body::from("abc"), NOW).await;
    assert_eq!(status_of(refused).await, StatusCode::CONFLICT);
    assert_eq!(uploads.claim(&id, NOW).unwrap().record().offset, 3);
    assert_eq!(part_len(&uploads, &id), 3);
}

#[tokio::test]
async fn a_checksum_mismatch_answers_460_and_stores_nothing() {
    let (_dir, uploads) = uploads();
    let id = uploads.create(10, "b.age".into(), NOW).unwrap();
    let refused = uploads
        .append(&id, 0, Some(sha(b"other")), Body::from("hello"), NOW)
        .await;
    assert_eq!(status_of(refused).await, checksum_mismatch());
    assert_eq!(uploads.claim(&id, NOW).unwrap().record().offset, 0);
    assert_eq!(part_len(&uploads, &id), 0);
}

#[tokio::test]
async fn a_chunk_past_the_length_is_refused_and_stores_nothing() {
    let (_dir, uploads) = uploads();
    let id = uploads.create(4, "b.age".into(), NOW).unwrap();
    let refused = uploads.append(&id, 0, None, Body::from("hello"), NOW).await;
    assert_eq!(status_of(refused).await, StatusCode::BAD_REQUEST);
    assert_eq!(part_len(&uploads, &id), 0);
}

#[tokio::test]
async fn bytes_past_the_recorded_offset_are_dropped_before_the_next_chunk() {
    // A crash between the write and the record save leaves a longer part.
    let (_dir, uploads) = uploads();
    let id = uploads.create(6, "b.age".into(), NOW).unwrap();
    std::fs::write(uploads.session_dir(&id).join(PART), b"junk").unwrap();
    uploads
        .append(&id, 0, None, Body::from("abcdef"), NOW)
        .await
        .unwrap();
    let staged = std::fs::read(uploads.session_dir(&id).join(PART)).unwrap();
    assert_eq!(staged, b"abcdef");
}

#[test]
fn a_session_held_by_one_request_refuses_a_second() {
    let (_dir, uploads) = uploads();
    let id = uploads.create(4, "b.age".into(), NOW).unwrap();
    let held = uploads.claim(&id, NOW).unwrap();
    assert!(uploads.claim(&id, NOW).is_err());
    drop(held);
    assert!(uploads.claim(&id, NOW).is_ok());
}

#[test]
fn at_most_four_sessions_are_staged() {
    let (_dir, uploads) = uploads();
    for _ in 0..MAX_SESSIONS {
        uploads.create(4, "b.age".into(), NOW).unwrap();
    }
    let refused = uploads.create(4, "b.age".into(), NOW).unwrap_err();
    assert_eq!(
        refused.into_response().status(),
        StatusCode::TOO_MANY_REQUESTS
    );
}

#[test]
fn a_create_the_disk_cannot_hold_is_refused_up_front() {
    let (_dir, uploads) = uploads();
    let refused = uploads
        .create(u64::MAX / 2, "b.age".into(), NOW)
        .unwrap_err();
    assert!(refused.message().contains("free"), "{}", refused.message());
    assert_eq!(
        refused.into_response().status(),
        StatusCode::INSUFFICIENT_STORAGE
    );
    assert!(uploads.session_ids().is_empty());
}

#[test]
fn a_session_expires_a_day_after_its_last_chunk() {
    let (_dir, uploads) = uploads();
    let id = uploads.create(4, "b.age".into(), NOW).unwrap();
    let later = NOW + EXPIRY.as_secs();
    assert!(uploads.claim(&id, later - 1).is_ok());
    assert!(uploads.claim(&id, later).is_err());
    assert!(!uploads.session_dir(&id).exists());
}

#[test]
fn the_sweep_removes_expired_sessions_and_keeps_live_ones() {
    let (_dir, uploads) = uploads();
    let old = uploads.create(4, "b.age".into(), NOW).unwrap();
    let fresh = uploads.create(4, "b.age".into(), NOW + 60).unwrap();
    let sweep_at = NOW + EXPIRY.as_secs();
    assert_eq!(uploads.sweep(sweep_at), 1);
    assert!(!uploads.session_dir(&old).exists());
    assert!(uploads.session_dir(&fresh).exists());
}

#[test]
fn the_startup_sweep_drops_an_archive_a_restore_left_behind() {
    let (_dir, uploads) = uploads();
    let id = uploads.create(1, "b.age".into(), NOW).unwrap();
    std::fs::write(uploads.session_dir(&id).join(PART), b"x").unwrap();
    let mut claim = uploads.claim(&id, NOW).unwrap();
    claim.record.offset = 1;
    let committed = claim.take().unwrap();
    assert!(committed.exists());
    assert_eq!(uploads.sweep_on_start(NOW), 1);
    assert!(!committed.exists());
}

#[test]
fn take_moves_the_archive_out_and_removes_the_session() {
    let (_dir, uploads) = uploads();
    let id = uploads.create(1, "b.age".into(), NOW).unwrap();
    std::fs::write(uploads.session_dir(&id).join(PART), b"x").unwrap();
    let archive = uploads.claim(&id, NOW).unwrap().take().unwrap();
    assert_eq!(std::fs::read(&archive).unwrap(), b"x");
    assert!(!uploads.session_dir(&id).exists());
    assert!(uploads.claim(&id, NOW).is_err());
}

#[test]
fn an_incomplete_upload_cannot_be_committed() {
    let (_dir, uploads) = uploads();
    let id = uploads.create(4, "b.age".into(), NOW).unwrap();
    let refused = uploads.claim_complete(&id, NOW).err().unwrap();
    assert_eq!(refused.into_response().status(), StatusCode::CONFLICT);
}

#[test]
fn delete_removes_the_session() {
    let (_dir, uploads) = uploads();
    let id = uploads.create(4, "b.age".into(), NOW).unwrap();
    uploads.delete(&id, NOW).unwrap();
    assert!(!uploads.session_dir(&id).exists());
}

#[test]
fn staged_files_are_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;
    let (_dir, uploads) = uploads();
    let id = uploads.create(4, "b.age".into(), NOW).unwrap();
    let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&uploads.root), 0o700);
    assert_eq!(mode(&uploads.session_dir(&id)), 0o700);
    assert_eq!(mode(&uploads.session_dir(&id).join(PART)), 0o600);
    assert_eq!(mode(&uploads.session_dir(&id).join(RECORD_FILE)), 0o600);
}

#[test]
fn a_session_id_that_is_not_ours_reads_as_missing() {
    let (_dir, uploads) = uploads();
    for id in ["../etc", "", "ABCDEF", &"a".repeat(31)] {
        assert!(uploads.claim(id, NOW).is_err(), "{id}");
    }
}

#[test]
fn the_filename_metadata_is_reduced_to_its_last_segment() {
    let meta = format!("filename {}", B64.encode("../../lucidos-backup-myws.age"));
    assert_eq!(
        parse_filename(Some(&meta)).unwrap(),
        "lucidos-backup-myws.age"
    );
    assert_eq!(parse_filename(None).unwrap(), "");
    assert!(parse_filename(Some("filename !!")).is_err());
}

// ── Through the control plane ───────────────────────────────────────────────

fn state() -> (tempfile::TempDir, GatewayState) {
    let dir = tempfile::tempdir().unwrap();
    let state = GatewayState::for_tests_with_static_dir(Some(dir.path().to_owned()));
    (dir, state)
}

fn control(method: &str, path: &str) -> axum::http::request::Builder {
    Request::builder()
        .method(method)
        .uri(path)
        .header(crate::auth::HEADER_LOCAL_TOKEN, "test-local-token")
        .header("host", "localhost:5251")
        .header("sec-fetch-site", "same-origin")
        .header("origin", "https://localhost:5251")
        .header("referer", "https://localhost:5251/")
}

async fn send(state: &GatewayState, request: Request<Body>) -> Response {
    crate::server::gateway_router(state.clone())
        .oneshot(request)
        .await
        .unwrap()
}

async fn create_through_router(state: &GatewayState, length: usize) -> String {
    let response = send(
        state,
        control("POST", UPLOADS_ROUTE)
            .header("tus-resumable", TUS_VERSION)
            .header("upload-length", length.to_string())
            .header(
                "upload-metadata",
                format!(
                    "filename {}",
                    B64.encode("lucidos-backup-myws-20260601-040254.age")
                ),
            )
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let location = response.headers()["location"].to_str().unwrap().to_string();
    location.rsplit('/').next().unwrap().to_string()
}

fn chunk(id: &str, offset: usize, bytes: Vec<u8>) -> Request<Body> {
    let digest = B64.encode(sha(&bytes));
    control("PATCH", &format!("{UPLOADS_ROUTE}/{id}"))
        .header("tus-resumable", TUS_VERSION)
        .header("content-type", OFFSET_CONTENT_TYPE)
        .header("upload-offset", offset.to_string())
        .header("upload-checksum", format!("sha256 {digest}"))
        .body(Body::from(bytes))
        .unwrap()
}

#[tokio::test]
async fn a_chunk_far_above_the_default_body_limit_streams_through() {
    let (_dir, state) = state();
    let size = 5 * 1024 * 1024;
    let id = create_through_router(&state, size).await;
    let response = send(&state, chunk(&id, 0, vec![7u8; size])).await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(
        response.headers()["upload-offset"],
        size.to_string().as_str()
    );
    assert!(response.headers().contains_key("upload-expires"));

    let head = send(
        &state,
        control("HEAD", &format!("{UPLOADS_ROUTE}/{id}"))
            .header("tus-resumable", TUS_VERSION)
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()["upload-offset"], size.to_string().as_str());
    assert_eq!(head.headers()["upload-length"], size.to_string().as_str());
}

#[tokio::test]
async fn a_request_without_the_tus_version_or_chunk_type_is_refused() {
    let (_dir, state) = state();
    let no_version = send(
        &state,
        control("POST", UPLOADS_ROUTE)
            .header("upload-length", "4")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(no_version.status(), StatusCode::PRECONDITION_FAILED);

    let id = create_through_router(&state, 4).await;
    let wrong_type = send(
        &state,
        control("PATCH", &format!("{UPLOADS_ROUTE}/{id}"))
            .header("tus-resumable", TUS_VERSION)
            .header("content-type", "application/json")
            .header("upload-offset", "0")
            .body(Body::from("abcd"))
            .unwrap(),
    )
    .await;
    assert_eq!(wrong_type.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

async fn commit(state: &GatewayState, body: Value) -> Response {
    send(
        state,
        control("POST", "/~/api/v1/control/workspaces/restore")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap(),
    )
    .await
}

use serde_json::{json, Value};

#[tokio::test]
async fn the_commit_refuses_an_incomplete_upload() {
    let (_dir, state) = state();
    let id = create_through_router(&state, 4).await;
    let response = commit(&state, json!({ "upload": id, "key": "k", "name": "myws" })).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn the_commit_refuses_an_unknown_upload_and_a_missing_key() {
    let (_dir, state) = state();
    let unknown = commit(
        &state,
        json!({ "upload": "0".repeat(32), "key": "k", "name": "myws" }),
    )
    .await;
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    let id = create_through_router(&state, 1).await;
    let no_key = commit(&state, json!({ "upload": id, "key": " ", "name": "myws" })).await;
    assert_eq!(no_key.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_refused_commit_keeps_the_upload_and_never_writes_the_key() {
    let (dir, state) = state();
    state.register_for_test(crate::registry::Workspace::gateway_provisioned(
        "myws".into(),
        "myws".into(),
        5600,
    ));
    let id = create_through_router(&state, 3).await;
    assert_eq!(
        send(&state, chunk(&id, 0, b"abc".to_vec())).await.status(),
        StatusCode::NO_CONTENT
    );
    let key = "AGE-SECRET-KEY-1UNIQUEMARKERFORTHISTEST";
    let refused = commit(&state, json!({ "upload": id, "key": key, "name": "myws" })).await;
    assert_eq!(refused.status(), StatusCode::CONFLICT);

    // The name collision left the staged archive in place for a second try.
    let uploads = state.restore_uploads();
    assert!(uploads.claim_complete(&id, now_secs()).is_ok());
    assert!(
        !grep_tree(dir.path(), key),
        "the backup key reached the disk"
    );
}

fn grep_tree(root: &Path, needle: &str) -> bool {
    let Ok(entries) = std::fs::read_dir(root) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let path = entry.path();
        if path.is_dir() {
            grep_tree(&path, needle)
        } else {
            std::fs::read(&path)
                .is_ok_and(|bytes| bytes.windows(needle.len()).any(|w| w == needle.as_bytes()))
        }
    })
}
