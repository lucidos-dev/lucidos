//! E2E tests for the compose state-machine HTTP surface.
//!
//! Covers POST /api/v1/threads, PUT /api/v1/threads/:id/compose, and
//! DELETE /api/v1/threads/:id. The "discard then late PUT returns 410" test
//! is the headline contract — it proves that the state machine prevents
//! resurrection by construction (no tombstone, no LWW) which was the whole
//! point of the redesign.
//!
//! Read-back verification reads `thread_summaries` directly via sqlx — the
//! frontend hydrates composing rows from `/api/v1/threads`'s `composing[]`
//! field, but for these contract tests the row's columns are what we care
//! about, and the projection is what the field reads anyway.
//!
//! See `docs/plans/2026-05-03-threads-as-drafts-design.md`.

use crate::support::{base_url, db_url, http_client};
use serde_json::json;
use uuid::Uuid;

/// Read `source` from `thread_summaries` for one thread. Used to verify that
/// compose-time mode toggles propagate to the source column the drawer pill
/// reads — composing threads that auto-archive without being sent must still
/// surface as Claude Code when CC was toggled.
async fn fetch_source(pool: &sqlx::PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .expect("source query")
}

/// Read the compose-relevant columns for one thread. Returns `(state,
/// compose_text, compose_images, compose_mode)`. Used by the contract tests
/// that verify PUT compose actually persists the requested fields.
async fn fetch_compose_row(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> (String, String, serde_json::Value, Option<String>) {
    sqlx::query_as(
        "SELECT state, compose_text, compose_images, compose_mode \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .expect("compose row query")
}

fn threads_url() -> String {
    format!("{}/api/v1/threads", base_url())
}

fn compose_url(id: &Uuid) -> String {
    format!("{}/api/v1/threads/{}/compose", base_url(), id)
}

fn thread_url(id: &Uuid) -> String {
    format!("{}/api/v1/threads/{}", base_url(), id)
}

#[tokio::test]
async fn post_threads_creates_composing_thread_and_persists_row() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect to e2e db");
    let id = Uuid::new_v4();

    let resp = client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "lucidos" }))
        .send()
        .await
        .expect("POST /api/v1/threads failed");
    assert_eq!(resp.status(), 201, "POST should create with 201");

    let (state, text, _images, mode) = fetch_compose_row(&pool, id).await;
    assert_eq!(state, "composing");
    assert_eq!(text, "");
    assert_eq!(mode.as_deref(), Some("lucidos"));
}

#[tokio::test]
async fn post_threads_idempotent_on_same_mode() {
    let client = http_client();
    let id = Uuid::new_v4();
    let body = json!({ "id": id, "mode": "lucidos" });

    let r1 = client
        .post(threads_url())
        .json(&body)
        .send()
        .await
        .expect("first POST failed");
    assert_eq!(r1.status(), 201, "first POST should be 201 Created");

    let r2 = client
        .post(threads_url())
        .json(&body)
        .send()
        .await
        .expect("second POST failed");
    assert_eq!(
        r2.status(),
        200,
        "second POST with same id+mode should be 200 OK (idempotent)"
    );
}

#[tokio::test]
async fn post_threads_conflict_on_different_mode() {
    let client = http_client();
    let id = Uuid::new_v4();

    let r1 = client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "lucidos" }))
        .send()
        .await
        .expect("first POST failed");
    assert_eq!(r1.status(), 201);

    let r2 = client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "claude_code" }))
        .send()
        .await
        .expect("second POST failed");
    assert_eq!(
        r2.status(),
        409,
        "POST with mismatching mode must be 409 Conflict"
    );
}

#[tokio::test]
async fn put_compose_updates_text_images_and_mode() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect to e2e db");
    let id = Uuid::new_v4();
    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "lucidos" }))
        .send()
        .await
        .expect("POST /threads failed");

    let resp = client
        .put(compose_url(&id))
        .json(&json!({
            "text": "hello world",
            "image_hashes": ["hash-1", "hash-2"],
            "mode": "claude_code",
        }))
        .send()
        .await
        .expect("PUT compose failed");
    assert_eq!(resp.status(), 204, "PUT should be 204 No Content");

    let (_state, text, images, mode) = fetch_compose_row(&pool, id).await;
    assert_eq!(text, "hello world");
    assert_eq!(images[0], "hash-1");
    assert_eq!(images[1], "hash-2");
    assert_eq!(mode.as_deref(), Some("claude_code"));
}

/// Toggling compose_mode via PUT /compose must also update `source` so the
/// drawer pill matches the user's selection while the thread is composing.
/// Without this, a CC-toggled draft that auto-archives without being sent
/// shows as "Lucidos" in the archive section.
#[tokio::test]
async fn put_compose_mode_toggle_updates_source() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect to e2e db");
    let id = Uuid::new_v4();
    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "lucidos" }))
        .send()
        .await
        .expect("POST /threads failed");
    assert_eq!(fetch_source(&pool, id).await, "chat", "lucidos POST seeds source=chat");

    client
        .put(compose_url(&id))
        .json(&json!({ "text": "fix the bug", "images": [], "mode": "claude_code" }))
        .send()
        .await
        .expect("PUT compose to claude_code failed");
    assert_eq!(
        fetch_source(&pool, id).await,
        "claude_code",
        "toggling to CC must update source"
    );

    client
        .put(compose_url(&id))
        .json(&json!({ "text": "never mind", "images": [], "mode": "lucidos" }))
        .send()
        .await
        .expect("PUT compose back to lucidos failed");
    assert_eq!(
        fetch_source(&pool, id).await,
        "chat",
        "toggling back to lucidos must restore source=chat"
    );
}

/// Text-only PUTs don't carry a mode field; source must stay put when only
/// the text changes (otherwise we'd churn the drawer pill on every keystroke).
#[tokio::test]
async fn put_compose_text_only_preserves_source() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect to e2e db");
    let id = Uuid::new_v4();
    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "claude_code" }))
        .send()
        .await
        .expect("POST failed");
    assert_eq!(fetch_source(&pool, id).await, "claude_code");

    client
        .put(compose_url(&id))
        .json(&json!({ "text": "typed some words", "images": [] }))
        .send()
        .await
        .expect("PUT failed");
    assert_eq!(
        fetch_source(&pool, id).await,
        "claude_code",
        "text-only PUT must not touch source"
    );
}

#[tokio::test]
async fn put_compose_text_only_preserves_mode() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect to e2e db");
    let id = Uuid::new_v4();
    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "claude_code" }))
        .send()
        .await
        .expect("POST failed");

    // Text-only PUT — mode field omitted. The handler COALESCEs to keep the
    // existing compose_mode rather than clobbering it.
    client
        .put(compose_url(&id))
        .json(&json!({ "text": "draft text", "images": [] }))
        .send()
        .await
        .expect("PUT failed");

    let (_state, _text, _images, mode) = fetch_compose_row(&pool, id).await;
    assert_eq!(mode.as_deref(), Some("claude_code"), "mode must persist");
}

#[tokio::test]
async fn delete_thread_marks_discarded() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect to e2e db");
    let id = Uuid::new_v4();
    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "lucidos" }))
        .send()
        .await
        .expect("POST failed");

    let resp = client
        .delete(thread_url(&id))
        .send()
        .await
        .expect("DELETE failed");
    assert_eq!(resp.status(), 204);

    let (state, _text, _images, _mode) = fetch_compose_row(&pool, id).await;
    assert_eq!(state, "discarded", "DELETE must flip state to discarded");
}

/// The headline contract: once a thread is discarded, no compose PUT can
/// resurrect it. Old design relied on LWW + tombstones to defend against
/// in-flight echoes; new design rejects at the API boundary by construction.
#[tokio::test]
async fn discard_then_late_put_returns_gone() {
    let client = http_client();
    let id = Uuid::new_v4();

    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "lucidos" }))
        .send()
        .await
        .expect("POST failed");

    client
        .delete(thread_url(&id))
        .send()
        .await
        .expect("DELETE failed");

    let resp = client
        .put(compose_url(&id))
        .json(&json!({ "text": "ghost text", "images": [] }))
        .send()
        .await
        .expect("late PUT failed");
    assert_eq!(
        resp.status(),
        410,
        "PUT to a discarded thread must return 410 Gone (got {})",
        resp.status()
    );
}

#[tokio::test]
async fn delete_then_delete_is_idempotent() {
    let client = http_client();
    let id = Uuid::new_v4();
    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "lucidos" }))
        .send()
        .await
        .expect("POST failed");

    let r1 = client
        .delete(thread_url(&id))
        .send()
        .await
        .expect("first DELETE failed");
    assert_eq!(r1.status(), 204);

    let r2 = client
        .delete(thread_url(&id))
        .send()
        .await
        .expect("second DELETE failed");
    assert_eq!(r2.status(), 204, "second DELETE must be idempotent (204)");
}

#[tokio::test]
async fn delete_unknown_thread_is_idempotent_no_op() {
    let client = http_client();
    let id = Uuid::new_v4();
    let resp = client
        .delete(thread_url(&id))
        .send()
        .await
        .expect("DELETE failed");
    assert_eq!(
        resp.status(),
        204,
        "DELETE on unknown id is no-op (idempotent)"
    );
}

/// Following up on an archived thread must work — typing keystrokes after
/// re-opening an archived conversation otherwise toast-spammed
/// "Compose sync failed: 409 thread archived" once per debounced PUT.
/// MessageReceived already revives an archived thread (state→active), so the
/// compose layer must accept the keystrokes that lead up to send.
#[tokio::test]
async fn put_compose_on_archived_thread_returns_no_content() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect to e2e db");
    let id = Uuid::new_v4();

    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "lucidos" }))
        .send()
        .await
        .expect("POST /threads failed");

    // Skip the live message + archive endpoints — directly seed the terminal
    // state we need to gate. The archive endpoint also tears down CC sessions
    // and would couple this contract test to unrelated machinery.
    //
    // Post the state↔archive_state collapse, `archive_state` is the sole
    // archive flag; a post-send archived row carries `state='active'` plus
    // `archive_state='archived'`. The handler-side `ThreadState::from_db_str`
    // rejects `'archived'` loudly, so we never write that to `state`. POST
    // /threads above creates `state='composing'`; flip it to `'active'` here
    // to mirror what MessageReceived (the real path to post-send) would have
    // produced, then flip archive_state to match ThreadArchived.
    sqlx::query(
        "UPDATE thread_summaries SET state = 'active', archive_state = 'archived' WHERE thread_id = $1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .expect("seed archived state");

    let resp = client
        .put(compose_url(&id))
        .json(&json!({ "text": "follow up", "images": [] }))
        .send()
        .await
        .expect("PUT compose on archived failed");
    assert_eq!(
        resp.status(),
        204,
        "PUT compose on an archived thread must accept the draft (got {})",
        resp.status()
    );

    let row: (String, String, String) = sqlx::query_as(
        "SELECT state, archive_state, compose_text FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .expect("read back row");
    assert_eq!(row.0, "active", "compose write must not flip compose state");
    assert_eq!(
        row.1, "archived",
        "compose write must not un-archive the row"
    );
    assert_eq!(row.2, "follow up", "compose text must persist");
}

/// Mode toggle is locked once the thread leaves composing — that lock must
/// still apply to archived threads (mode reflects history, not current intent).
#[tokio::test]
async fn put_compose_on_archived_rejects_mode_change() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url()).await.expect("connect to e2e db");
    let id = Uuid::new_v4();

    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "lucidos" }))
        .send()
        .await
        .expect("POST failed");

    // Mirror the post-send archived row: state='active' (what MessageReceived
    // would have flipped to) plus archive_state='archived' (what ThreadArchived
    // would have set). POST above leaves state='composing', so flip it here.
    // Seeding state='archived' would 500 via `from_db_str("archived")`.
    sqlx::query(
        "UPDATE thread_summaries SET state = 'active', archive_state = 'archived' WHERE thread_id = $1",
    )
    .bind(id)
    .execute(&pool)
    .await
    .expect("seed archived state");

    let resp = client
        .put(compose_url(&id))
        .json(&json!({ "text": "x", "images": [], "mode": "claude_code" }))
        .send()
        .await
        .expect("PUT failed");
    assert_eq!(
        resp.status(),
        409,
        "mode change on archived (post-send) must stay 409 (got {})",
        resp.status()
    );
}

#[tokio::test]
async fn put_compose_unknown_thread_returns_not_found() {
    let client = http_client();
    let id = Uuid::new_v4();
    let resp = client
        .put(compose_url(&id))
        .json(&json!({ "text": "x", "images": [] }))
        .send()
        .await
        .expect("PUT failed");
    assert_eq!(
        resp.status(),
        404,
        "PUT to unknown id must be 404, not silently create"
    );
}

#[tokio::test]
async fn post_threads_rejects_unknown_mode() {
    let client = http_client();
    let resp = client
        .post(threads_url())
        .json(&json!({ "id": Uuid::new_v4(), "mode": "bogus" }))
        .send()
        .await
        .expect("POST failed");
    assert_eq!(resp.status(), 400, "unknown mode must be 400 Bad Request");
}
