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

/// Read `compose_selection` (the per-draft dropdown selection) for one thread.
async fn fetch_compose_selection(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Option<serde_json::Value> {
    sqlx::query_scalar("SELECT compose_selection FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .expect("compose_selection query")
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
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to e2e db");
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
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to e2e db");
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
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to e2e db");
    let id = Uuid::new_v4();
    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "lucidos" }))
        .send()
        .await
        .expect("POST /threads failed");
    assert_eq!(
        fetch_source(&pool, id).await,
        "chat",
        "lucidos POST seeds source=chat"
    );

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
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to e2e db");
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
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to e2e db");
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

/// PUT compose with a `selection` persists the per-draft dropdown selection to
/// `thread_summaries.compose_selection` AND surfaces it on the `/api/v1/threads`
/// composing list, so a reload rehydrates the draft's picks.
#[tokio::test]
async fn put_compose_persists_and_surfaces_selection() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to e2e db");
    let id = Uuid::new_v4();
    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "claude_code" }))
        .send()
        .await
        .expect("POST /threads failed");

    let selection = json!({
        "scope": { "kind": "app", "appId": "habit-tracker" },
        "codingAgent": "codex",
        "ccModel": "sonnet",
    });
    let resp = client
        .put(compose_url(&id))
        .json(&json!({ "text": "wire it up", "image_hashes": [], "selection": selection }))
        .send()
        .await
        .expect("PUT compose with selection failed");
    assert_eq!(resp.status(), 204);

    // Column persisted.
    assert_eq!(
        fetch_compose_selection(&pool, id).await.as_ref(),
        Some(&selection)
    );

    // Surfaced on the composing list so the frontend rehydrates on reload.
    let listed: serde_json::Value = client
        .get(threads_url())
        .send()
        .await
        .expect("GET /threads failed")
        .json()
        .await
        .expect("threads json");
    let composing = listed["composing"].as_array().expect("composing[] array");
    let row = composing
        .iter()
        .find(|t| t["thread_id"] == json!(id.to_string()))
        .expect("draft present in composing[]");
    assert_eq!(
        row["compose_selection"], selection,
        "selection must be on the list row"
    );
}

/// A text-only keystroke PUT (no `selection` field) must PRESERVE the stored
/// selection via COALESCE — otherwise every keystroke would wipe the picks.
#[tokio::test]
async fn put_compose_text_only_preserves_selection() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to e2e db");
    let id = Uuid::new_v4();
    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "lucidos" }))
        .send()
        .await
        .expect("POST failed");

    let selection = json!({ "model": "opus", "reasoningEffort": "high" });
    client
        .put(compose_url(&id))
        .json(&json!({ "text": "first", "image_hashes": [], "selection": selection }))
        .send()
        .await
        .expect("PUT with selection failed");

    // Text-only PUT — `selection` omitted; COALESCE keeps the stored value.
    client
        .put(compose_url(&id))
        .json(&json!({ "text": "first and more" }))
        .send()
        .await
        .expect("text-only PUT failed");

    assert_eq!(
        fetch_compose_selection(&pool, id).await.as_ref(),
        Some(&selection),
        "text-only PUT must preserve the stored selection"
    );
}

#[tokio::test]
async fn delete_thread_marks_discarded() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to e2e db");
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
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to e2e db");
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
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to e2e db");
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

// --- The compose write fence (`compose_epoch`) ---
//
// A compose write composed BEFORE a submission must never be applied AFTER it.
// Without the fence, a draft PUT stalled by a bad connection lands after the
// message it preceded and rewrites the draft the send just consumed, so the
// message reads as sent while the composer still holds a stale revision of it
// (reported 2026-08-06 from the iOS PWA).
//
// These tests move `compose_epoch` directly rather than sending a real message:
// the endpoint contract is what is under test here, and which projection arms
// advance the epoch is covered by the engine's own projection tests
// (`event_bus_tests::thread_state_and_eviction`). Doing it in SQL also keeps
// the case deterministic and free of an LLM round trip.

/// Stand in for a submission consuming the thread's compose slot. Writes what
/// the `MessageReceived` projection writes in one transaction: the row leaves
/// `composing`, the stored draft goes, and the epoch advances.
///
/// The `state` flip is load-bearing, not decoration. It is the column the mode
/// lock reads, so a stand-in that left the row `composing` could never reach
/// the lock. That omission is why this suite passed while a real send answered
/// a stale write with 409.
async fn consume_compose_slot(pool: &sqlx::PgPool, thread_id: Uuid) {
    sqlx::query(
        "UPDATE thread_summaries \
         SET compose_epoch = compose_epoch + 1, \
             compose_text = '', \
             compose_mode = NULL, \
             state = 'active' \
         WHERE thread_id = $1",
    )
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("consume compose slot");
}

async fn fetch_compose_epoch(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT compose_epoch FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .expect("compose_epoch query")
}

#[tokio::test]
async fn put_compose_at_a_consumed_epoch_is_refused_and_changes_nothing() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to e2e db");
    let id = Uuid::new_v4();
    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "lucidos" }))
        .send()
        .await
        .expect("POST /api/v1/threads failed");

    // The client's draft PUT, composed at epoch 0.
    let stale_body = json!({ "text": "You can have both", "compose_epoch": 0 });
    let resp = client
        .put(compose_url(&id))
        .json(&stale_body)
        .send()
        .await
        .expect("PUT compose failed");
    assert_eq!(resp.status(), 204, "the first write is at the live epoch");

    consume_compose_slot(&pool, id).await;

    // The same write, arriving late. This is the replay the stalled link
    // produces, and it must not resurrect the draft.
    let resp = client
        .put(compose_url(&id))
        .json(&stale_body)
        .send()
        .await
        .expect("PUT compose failed");
    assert_eq!(
        resp.status(),
        412,
        "a write composed before the submission must be refused, got {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await.expect("412 body");
    assert_eq!(
        body["compose_epoch"].as_i64(),
        Some(1),
        "the refusal must hand back the current epoch so the client can re-issue"
    );

    let (_state, text, _images, _mode) = fetch_compose_row(&pool, id).await;
    assert_eq!(text, "", "the refused write must not have been applied");
}

#[tokio::test]
async fn put_compose_at_the_current_epoch_is_applied() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to e2e db");
    let id = Uuid::new_v4();
    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "lucidos" }))
        .send()
        .await
        .expect("POST /api/v1/threads failed");
    consume_compose_slot(&pool, id).await;

    // The client heard about the submission and re-composed against epoch 1,
    // which is what the 412 above tells it to do.
    let resp = client
        .put(compose_url(&id))
        .json(&json!({ "text": "a genuinely new follow-up", "compose_epoch": 1 }))
        .send()
        .await
        .expect("PUT compose failed");
    assert_eq!(resp.status(), 204);

    let (_state, text, _images, _mode) = fetch_compose_row(&pool, id).await;
    assert_eq!(text, "a genuinely new follow-up");
}

#[tokio::test]
async fn consecutive_compose_puts_at_one_epoch_are_all_accepted() {
    // The keystroke path. The epoch counts SUBMISSIONS, not writes, so every
    // PUT between two submissions carries the same value and none of them may
    // fence out the next. A per-write counter would break typing outright.
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to e2e db");
    let id = Uuid::new_v4();
    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "lucidos" }))
        .send()
        .await
        .expect("POST /api/v1/threads failed");

    for text in ["a", "ab", "abc"] {
        let resp = client
            .put(compose_url(&id))
            .json(&json!({ "text": text, "compose_epoch": 0 }))
            .send()
            .await
            .expect("PUT compose failed");
        assert_eq!(
            resp.status(),
            204,
            "keystroke write `{text}` was fenced out"
        );
    }

    let (_state, text, _images, _mode) = fetch_compose_row(&pool, id).await;
    assert_eq!(text, "abc");
    assert_eq!(
        fetch_compose_epoch(&pool, id).await,
        0,
        "a compose write must not move the epoch"
    );
}

/// The reported card. A keystroke write carrying the draft's mode is stalled by
/// a bad link and lands after the send it preceded. The epoch is why it was not
/// applied, so the answer is the 412 the client resyncs from silently. The mode
/// lock's 409 surfaced as a "Compose sync failed" card on an ordinary send.
#[tokio::test]
async fn a_stale_write_carrying_a_mode_is_refused_as_stale_not_mode_locked() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to e2e db");
    let id = Uuid::new_v4();
    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "lucidos" }))
        .send()
        .await
        .expect("POST /api/v1/threads failed");

    // The draft write the client had in flight when the user hit Send.
    let stale_body = json!({
        "text": "half a sentence",
        "mode": "lucidos",
        "compose_epoch": 0,
    });
    let resp = client
        .put(compose_url(&id))
        .json(&stale_body)
        .send()
        .await
        .expect("PUT compose failed");
    assert_eq!(resp.status(), 204, "the first write is at the live epoch");

    consume_compose_slot(&pool, id).await;

    let resp = client
        .put(compose_url(&id))
        .json(&stale_body)
        .send()
        .await
        .expect("PUT compose failed");
    assert_eq!(
        resp.status(),
        412,
        "a stale write must be refused as stale, whatever else it carried, got {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await.expect("412 body");
    assert_eq!(
        body["compose_epoch"].as_i64(),
        Some(1),
        "the refusal must hand back the current epoch so the client can re-issue"
    );

    let (state, text, _images, mode) = fetch_compose_row(&pool, id).await;
    // The mode lock's precondition. Without this the case is toothless: a row
    // left `composing` takes the 412 anyway, so the test would pass while never
    // reaching the branch it exists to order.
    assert_eq!(state, "active", "the row must be post-send for this case");
    assert_eq!(text, "", "the refused write must not have been applied");
    assert_eq!(mode, None, "the refused write must not restore a mode");
}

/// The mode lock itself, at the CURRENT epoch. A client toggling the channel on
/// a thread the engine has as sent really has diverged, and 409 says so. This
/// is what the reordering above must not weaken.
#[tokio::test]
async fn a_current_write_carrying_a_mode_on_a_sent_thread_is_mode_locked() {
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to e2e db");
    let id = Uuid::new_v4();
    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "lucidos" }))
        .send()
        .await
        .expect("POST /api/v1/threads failed");
    consume_compose_slot(&pool, id).await;

    let resp = client
        .put(compose_url(&id))
        .json(&json!({
            "text": "x",
            "mode": "claude_code",
            "compose_epoch": 1,
        }))
        .send()
        .await
        .expect("PUT compose failed");
    assert_eq!(
        resp.status(),
        409,
        "a mode change at the live epoch on a sent thread must stay 409 (got {})",
        resp.status()
    );

    let (_state, _text, _images, mode) = fetch_compose_row(&pool, id).await;
    assert_eq!(mode, None, "the refused write must not set a mode");
}

#[tokio::test]
async fn put_compose_without_an_epoch_is_unfenced() {
    // Permanent back-compat: a cached PWA bundle running against a newer engine
    // cannot know to send an epoch, and refusing its writes would break draft
    // sync outright for it.
    let client = http_client();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect to e2e db");
    let id = Uuid::new_v4();
    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "lucidos" }))
        .send()
        .await
        .expect("POST /api/v1/threads failed");
    consume_compose_slot(&pool, id).await;

    let resp = client
        .put(compose_url(&id))
        .json(&json!({ "text": "from a client that predates the fence" }))
        .send()
        .await
        .expect("PUT compose failed");
    assert_eq!(resp.status(), 204);

    let (_state, text, _images, _mode) = fetch_compose_row(&pool, id).await;
    assert_eq!(text, "from a client that predates the fence");
}
