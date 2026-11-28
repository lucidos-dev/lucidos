use crate::support::{base_url, db_url, user_client};
use serde_json::json;
use uuid::Uuid;

async fn fetch_source(pool: &sqlx::PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT source FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .expect("source query")
}

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
    let client = user_client().await;
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
    let client = user_client().await;
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
    let client = user_client().await;
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
    let client = user_client().await;
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
            "image_hashes": ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],
            "mode": "claude_code",
        }))
        .send()
        .await
        .expect("PUT compose failed");
    assert_eq!(resp.status(), 200, "PUT answers with the compose report");
    let report: serde_json::Value = resp.json().await.expect("compose report body");
    let stored_version: i64 =
        sqlx::query_scalar("SELECT summary_version FROM thread_summaries WHERE thread_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("summary_version query");
    assert_eq!(
        report["summary_version"].as_i64(),
        Some(stored_version),
        "the report's revision is the row's version after the write: {report}"
    );
    assert_eq!(report["text"], "hello world");
    assert_eq!(report["mode"], "claude_code");

    let (_state, text, images, mode) = fetch_compose_row(&pool, id).await;
    assert_eq!(text, "hello world");
    assert_eq!(
        images[0],
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    );
    assert_eq!(
        images[1],
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
    );
    assert_eq!(mode.as_deref(), Some("claude_code"));
}

#[tokio::test]
async fn put_compose_mode_toggle_updates_source() {
    let client = user_client().await;
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

#[tokio::test]
async fn put_compose_text_only_preserves_source() {
    let client = user_client().await;
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
    let client = user_client().await;
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
async fn put_compose_persists_and_surfaces_selection() {
    let client = user_client().await;
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
    assert_eq!(resp.status(), 200);

    assert_eq!(
        fetch_compose_selection(&pool, id).await.as_ref(),
        Some(&selection)
    );

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

#[tokio::test]
async fn put_compose_text_only_preserves_selection() {
    let client = user_client().await;
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
    let client = user_client().await;
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
    assert_eq!(resp.status(), 200);

    let (state, _text, _images, _mode) = fetch_compose_row(&pool, id).await;
    assert_eq!(state, "discarded", "DELETE must flip state to discarded");
}

#[tokio::test]
async fn discard_then_late_put_returns_gone() {
    let client = user_client().await;
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
    let client = user_client().await;
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
    assert_eq!(r1.status(), 200);

    let r2 = client
        .delete(thread_url(&id))
        .send()
        .await
        .expect("second DELETE failed");
    assert_eq!(r2.status(), 204, "second DELETE must be idempotent (204)");
}

#[tokio::test]
async fn delete_unknown_thread_is_idempotent_no_op() {
    let client = user_client().await;
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

#[tokio::test]
async fn put_compose_on_archived_thread_is_accepted() {
    let client = user_client().await;
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
        200,
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

#[tokio::test]
async fn put_compose_on_archived_rejects_mode_change() {
    let client = user_client().await;
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
    let client = user_client().await;
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
    let client = user_client().await;
    let resp = client
        .post(threads_url())
        .json(&json!({ "id": Uuid::new_v4(), "mode": "bogus" }))
        .send()
        .await
        .expect("POST failed");
    assert_eq!(resp.status(), 400, "unknown mode must be 400 Bad Request");
}

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
    let client = user_client().await;
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

    let stale_body = json!({ "text": "You can have both", "compose_epoch": 0 });
    let resp = client
        .put(compose_url(&id))
        .json(&stale_body)
        .send()
        .await
        .expect("PUT compose failed");
    assert_eq!(resp.status(), 200, "the first write is at the live epoch");

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
        "a write composed before the submission must be refused, got {}",
        resp.status()
    );
    let body: serde_json::Value = resp.json().await.expect("412 body");
    assert_eq!(
        body["compose_epoch"].as_i64(),
        Some(1),
        "the refusal must hand back the current epoch so the client can re-issue"
    );
    assert!(
        body["summary_version"].as_i64().is_some_and(|v| v > 0) && body["text"] == "",
        "the refusal carries the current compose report: {body}"
    );

    let (_state, text, _images, _mode) = fetch_compose_row(&pool, id).await;
    assert_eq!(text, "", "the refused write must not have been applied");
}

#[tokio::test]
async fn put_compose_at_the_current_epoch_is_applied() {
    let client = user_client().await;
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
        .json(&json!({ "text": "a genuinely new follow-up", "compose_epoch": 1 }))
        .send()
        .await
        .expect("PUT compose failed");
    assert_eq!(resp.status(), 200);

    let (_state, text, _images, _mode) = fetch_compose_row(&pool, id).await;
    assert_eq!(text, "a genuinely new follow-up");
}

#[tokio::test]
async fn consecutive_compose_puts_at_one_epoch_are_all_accepted() {
    let client = user_client().await;
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
            200,
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

#[tokio::test]
async fn a_stale_write_carrying_a_mode_is_refused_as_stale_not_mode_locked() {
    let client = user_client().await;
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
    assert_eq!(resp.status(), 200, "the first write is at the live epoch");

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
    assert_eq!(state, "active", "the row must be post-send for this case");
    assert_eq!(text, "", "the refused write must not have been applied");
    assert_eq!(mode, None, "the refused write must not restore a mode");
}

#[tokio::test]
async fn a_current_write_carrying_a_mode_on_a_sent_thread_is_mode_locked() {
    let client = user_client().await;
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
    let client = user_client().await;
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
    assert_eq!(resp.status(), 200);

    let (_state, text, _images, _mode) = fetch_compose_row(&pool, id).await;
    assert_eq!(text, "from a client that predates the fence");
}

fn drafts_url() -> String {
    format!("{}/api/v1/threads/drafts", base_url())
}

#[tokio::test]
async fn a_composed_draft_is_listed_with_its_edit_time_and_thread_link() {
    let client = user_client().await;
    let id = Uuid::new_v4();
    client
        .post(threads_url())
        .json(&json!({ "id": id, "mode": "lucidos" }))
        .send()
        .await
        .expect("POST /threads failed");
    let text = format!("Draft for the drafts read {id}");
    let resp = client
        .put(compose_url(&id))
        .json(&json!({ "text": text }))
        .send()
        .await
        .expect("PUT compose failed");
    assert_eq!(resp.status(), 200);

    let drafts: serde_json::Value = client
        .get(format!("{}?limit=1000", drafts_url()))
        .send()
        .await
        .expect("GET drafts failed")
        .json()
        .await
        .expect("drafts JSON");
    let row = drafts
        .as_array()
        .expect("an array")
        .iter()
        .find(|d| d["thread_id"] == id.to_string())
        .expect("the composed draft is listed");
    assert_eq!(row["preview"], text);
    assert_eq!(row["state"], "composing");
    assert!(row["last_edited"].is_string(), "the PUT dates the draft");
    let link = row["link"].as_str().expect("a link");
    assert!(
        link.starts_with("thread:") && link.ends_with(&format!("/{id}")),
        "the link is the owning thread's: {link}"
    );
    assert!(row.get("text").is_none(), "the list carries previews only");

    let one: serde_json::Value = client
        .get(format!("{}?thread_id={id}", drafts_url()))
        .send()
        .await
        .expect("GET one draft failed")
        .json()
        .await
        .expect("draft JSON");
    assert_eq!(one["text"], text);

    client
        .delete(thread_url(&id))
        .send()
        .await
        .expect("DELETE failed");
    let gone = client
        .get(format!("{}?thread_id={id}", drafts_url()))
        .send()
        .await
        .expect("GET discarded draft failed");
    assert_eq!(gone.status(), 404, "a discarded thread holds no draft");
}

#[tokio::test]
async fn a_send_consumes_its_draft_at_accept_and_the_ack_reports_it() {
    let client = user_client().await;
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
    let text = format!("Say exactly: \"consumed {id}\"");
    let resp = client
        .put(compose_url(&id))
        .json(&json!({ "text": format!("{text}\n"), "compose_epoch": 0 }))
        .send()
        .await
        .expect("PUT compose failed");
    assert_eq!(resp.status(), 200);

    let ack: serde_json::Value = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&json!({ "message": text, "thread_id": id, "mode": "human" }))
        .send()
        .await
        .expect("chat request failed")
        .json()
        .await
        .expect("chat ack body");

    assert_eq!(
        ack["compose"]["text"], "",
        "the stored draft was the message: {ack}"
    );
    assert_eq!(ack["compose"]["compose_epoch"].as_i64(), Some(1));
    assert_eq!(
        fetch_compose_epoch(&pool, id).await,
        1,
        "the epoch moved once"
    );
}

#[tokio::test]
async fn recovered_draft_refuses_a_foreign_restore_generation() {
    let client = user_client().await;
    let id = Uuid::new_v4();
    let response = client
        .post(threads_url())
        .json(&json!({
            "id": id,
            "mode": "lucidos",
            "recovered_compose": {
                "generation": Uuid::new_v4(),
                "text": "preserved draft",
                "image_hashes": [],
                "selection": {"model": "example-model"}
            }
        }))
        .send()
        .await
        .expect("recovery response");
    assert_eq!(response.status(), 412);
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("test database");
    let rows: i64 =
        sqlx::query_scalar("SELECT count(*) FROM thread_summaries WHERE thread_id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM events WHERE aggregate_id = $1")
        .bind(id.to_string())
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((rows, events), (0, 0));
    pool.close().await;
}

#[tokio::test]
async fn stale_legacy_selection_preserves_the_pinned_agent_and_accepts_new_text() {
    let client = user_client().await;
    let id = Uuid::new_v4();
    let created = client
        .post(threads_url())
        .json(&json!({"id": id, "mode": "lucidos"}))
        .send()
        .await
        .unwrap();
    assert!(created.status().is_success());
    for (text, selection) in [
        (
            "original",
            json!({"codingAgent":"claude-code", "model":"old"}),
        ),
        ("new peer text", json!({"model":"new"})),
    ] {
        let response = client
            .put(compose_url(&id))
            .json(&json!({"text":text, "selection":selection}))
            .send()
            .await
            .unwrap();
        assert!(response.status().is_success());
    }
    let pool = sqlx::PgPool::connect(&db_url()).await.unwrap();
    assert_eq!(
        fetch_compose_selection(&pool, id).await.unwrap(),
        json!({"codingAgent":"claude-code", "model":"new"})
    );
    assert_eq!(fetch_compose_row(&pool, id).await.1, "new peer text");
    let response = client
        .put(compose_url(&id))
        .json(&json!({"text":"new peer text", "selection":{"codingAgent":"codex"}}))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    assert_eq!(
        fetch_compose_selection(&pool, id).await.unwrap(),
        json!({"codingAgent":"codex"})
    );
}
