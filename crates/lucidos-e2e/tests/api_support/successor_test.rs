//! API e2e for *successor threads*: the context-size read and the composer's
//! handoff (`docs/plans/2026-10-13-auto-compact-trial-and-successor-threads.md`).

use crate::support::{
    base_url, count_events_of_type, db_url, git, git_in, insert_session_started,
    local_process_client, seed_cc_thread_summary, seed_chat_thread_summary, unique_marker,
    user_client, workspace_path,
};
use lucidos_engine::engine::successor::HANDOFF_OFFER_TOKENS;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;
use uuid::Uuid;

async fn connect() -> sqlx::PgPool {
    sqlx::PgPool::connect(&db_url())
        .await
        .expect("Failed to connect to E2E workspace database")
}

/// Record a `ContextCaptured` on `thread_id` straight into the events table.
async fn insert_capture(pool: &sqlx::PgPool, thread_id: Uuid, payload: serde_json::Value) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
         VALUES ($1, 'ContextCaptured', $2, $3, 'thread', $3::text)",
    )
    .bind(Uuid::new_v4())
    .bind(payload)
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("insert ContextCaptured failed");
}

async fn get_context(client: &reqwest::Client, thread_id: Uuid) -> serde_json::Value {
    let resp = client
        .get(format!(
            "{}/api/v1/threads/{}/context",
            base_url(),
            thread_id
        ))
        .send()
        .await
        .expect("context request failed");
    assert_eq!(resp.status().as_u16(), 200, "context route must answer 200");
    resp.json().await.expect("context response was not JSON")
}

async fn post_successor(
    client: &reqwest::Client,
    thread_id: Uuid,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let resp = client
        .post(format!(
            "{}/api/v1/threads/{}/successor",
            base_url(),
            thread_id
        ))
        .json(&body)
        .send()
        .await
        .expect("successor request failed");
    let status = resp.status().as_u16();
    let text = resp.text().await.expect("read body failed");
    let body = serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text));
    (status, body)
}

/// The *Successor card*'s Continue.
async fn post_continue(
    client: &reqwest::Client,
    thread_id: Uuid,
    body: serde_json::Value,
) -> (u16, serde_json::Value) {
    let resp = client
        .post(format!(
            "{}/api/v1/threads/{}/successor-offer/continue",
            base_url(),
            thread_id
        ))
        .json(&body)
        .send()
        .await
        .expect("continue request failed");
    let status = resp.status().as_u16();
    let text = resp.text().await.expect("read body failed");
    let body = serde_json::from_str(&text).unwrap_or(serde_json::Value::String(text));
    (status, body)
}

/// The first message the thread queue holds for `thread_id`.
async fn queued_request(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT payload->'data'->'request' FROM events \
         WHERE event_type = 'ThreadQueued' AND payload->'data'->>'thread_id' = $1 \
         ORDER BY sequence LIMIT 1",
    )
    .bind(thread_id.to_string())
    .fetch_optional(pool)
    .await
    .expect("read the successor's queued request")
}

async fn delete_thread_rows(pool: &sqlx::PgPool, thread_id: Uuid) {
    sqlx::query("DELETE FROM events WHERE thread_id = $1")
        .bind(thread_id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .execute(pool)
        .await
        .ok();
}

/// A top-level coding-agent thread whose last idle recorded a worktree of
/// the workspace repo, holding one commit on its own branch. The worktree
/// lives in a temp dir outside the workspace, so the workspace tree stays
/// untouched.
struct WorktreeThread {
    thread_id: Uuid,
    branch: String,
    worktree: PathBuf,
    _tmp: tempfile::TempDir,
}

impl WorktreeThread {
    async fn seed(pool: &sqlx::PgPool, label: &str) -> Self {
        let thread_id = Uuid::new_v4();
        let branch = format!("claude-code/{}", unique_marker(label));
        let tmp = tempfile::tempdir().expect("tempdir");
        let worktree = tmp.path().join("wt");
        git(&[
            "worktree",
            "add",
            worktree.to_str().unwrap(),
            "-b",
            &branch,
            "main",
        ]);
        std::fs::write(worktree.join("successor-e2e.txt"), "carried work\n").unwrap();
        git_in(&worktree, &["add", "successor-e2e.txt"]);
        git_in(&worktree, &["commit", "-q", "-m", "e2e: work to carry"]);

        seed_cc_thread_summary(pool, thread_id, "idle").await;
        insert_session_started(pool, thread_id, &branch).await;
        sqlx::query(
            "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
             VALUES ($1, 'CodingAgentIdled', $2, $3, 'thread', $3::text)",
        )
        .bind(Uuid::new_v4())
        .bind(json!({
            "worktree_path": worktree.to_str().unwrap(),
            "has_changes": false,
        }))
        .bind(thread_id)
        .execute(pool)
        .await
        .expect("insert CodingAgentIdled failed");

        Self {
            thread_id,
            branch,
            worktree,
            _tmp: tmp,
        }
    }
}

/// Removes the worktree and its branch even when an assertion fails.
impl Drop for WorktreeThread {
    fn drop(&mut self) {
        let ws = workspace_path();
        std::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(&self.worktree)
            .current_dir(&ws)
            .output()
            .ok();
        std::process::Command::new("git")
            .args(["branch", "-D", &self.branch])
            .current_dir(&ws)
            .output()
            .ok();
    }
}

/// Record the model and effort a coding-agent session runs on, as a session
/// start does.
async fn insert_settings(pool: &sqlx::PgPool, thread_id: Uuid, model: &str, effort: &str) {
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
         VALUES ($1, 'CodingAgentSettingsChanged', $2, $3, 'thread', $3::text)",
    )
    .bind(Uuid::new_v4())
    .bind(json!({"model": model, "reasoning_effort": effort, "coding_agent": "claude-code"}))
    .bind(thread_id)
    .execute(pool)
    .await
    .expect("insert CodingAgentSettingsChanged failed");
}

/// The context size is the latest capture of the thread's own turn. A
/// sub-agent's capture never counts, however large or recent it is. The
/// reading also names the implementer the thread runs on.
#[tokio::test]
async fn context_reports_the_latest_main_session_capture() {
    let client = user_client().await;
    let pool = connect().await;
    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "idle").await;
    insert_settings(&pool, thread_id, "claude-opus-5-5", "high").await;
    let implementer = json!({
        "coding_agent": "claude-code",
        "model": "claude-opus-5-5",
        "reasoning_effort": "high",
    });

    let empty = get_context(&client, thread_id).await;

    let offer_size = HANDOFF_OFFER_TOKENS + 1;
    let grown_size = offer_size + 1;
    insert_capture(
        &pool,
        thread_id,
        json!({"usage": {"input_tokens": offer_size, "output_tokens": 1}}),
    )
    .await;
    insert_capture(
        &pool,
        thread_id,
        json!({"usage": {"input_tokens": grown_size}, "parent_tool_use_id": "toolu_e2e"}),
    )
    .await;
    let offered = get_context(&client, thread_id).await;

    insert_capture(
        &pool,
        thread_id,
        json!({"usage": {"input_tokens": grown_size, "output_tokens": 1}}),
    )
    .await;
    let grown = get_context(&client, thread_id).await;

    delete_thread_rows(&pool, thread_id).await;

    assert_eq!(
        empty,
        json!({"input_tokens": null, "can_hand_off": true, "offer": false, "applied": false, "implementer": implementer}),
        "a thread with no captures has no size"
    );
    assert_eq!(
        offered,
        json!({"input_tokens": offer_size, "can_hand_off": true, "offer": true, "applied": false, "implementer": implementer}),
        "the later sub-agent capture must not count"
    );
    assert_eq!(
        grown,
        json!({"input_tokens": grown_size, "can_hand_off": true, "offer": true, "applied": false, "implementer": implementer}),
        "a newer main-session capture is the size"
    );
}

/// The implementer names the effort the thread's calls ran at. A later pick
/// no call ran at yet does not change it.
#[tokio::test]
async fn context_names_the_effort_in_force_not_a_pick() {
    let client = user_client().await;
    let pool = connect().await;
    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "idle").await;
    insert_settings(&pool, thread_id, "claude-opus-5-5", "medium").await;
    insert_capture(
        &pool,
        thread_id,
        json!({"usage": {"input_tokens": 1000, "output_tokens": 1}, "reasoning_effort": "medium"}),
    )
    .await;
    insert_settings(&pool, thread_id, "claude-opus-5-5", "xhigh").await;

    let context = get_context(&client, thread_id).await;
    delete_thread_rows(&pool, thread_id).await;

    assert_eq!(
        context["implementer"]["reasoning_effort"], "medium",
        "{context}"
    );
}

/// The switch estimate warns on a model change over a warm cache and stays
/// silent for the pair the thread already runs.
#[tokio::test]
async fn switch_estimate_warns_only_when_the_switch_rewrites_a_warm_cache() {
    let client = user_client().await;
    let pool = connect().await;
    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "idle").await;
    insert_settings(&pool, thread_id, "claude-opus-5-5", "medium").await;
    insert_capture(
        &pool,
        thread_id,
        json!({
            "model": "claude-opus-5-5",
            "usage": {"input_tokens": 200_000, "cache_creation_tokens": 1000, "cache_creation_1h_tokens": 0},
        }),
    )
    .await;
    let estimate = |query: &'static str| {
        let (client, thread) = (client.clone(), thread_id);
        async move {
            let resp = client
                .get(format!(
                    "{}/api/v1/threads/{}/switch-estimate?{query}",
                    base_url(),
                    thread
                ))
                .send()
                .await
                .expect("switch-estimate request failed");
            assert_eq!(resp.status().as_u16(), 200);
            resp.json::<serde_json::Value>().await.expect("not JSON")
        }
    };

    let switched = estimate("model=claude-fable-5-1&effort=medium").await;
    let same = estimate("model=claude-opus-5-5&effort=medium").await;
    let unknown = client
        .get(format!(
            "{}/api/v1/threads/{}/switch-estimate",
            base_url(),
            Uuid::new_v4()
        ))
        .send()
        .await
        .expect("switch-estimate request failed");
    delete_thread_rows(&pool, thread_id).await;

    assert_eq!(switched["context_tokens"], 200_000);
    assert_eq!(switched["rebuilds_cache"], true);
    assert!(switched["extra_cost_usd"].as_f64().unwrap() > 0.0);
    assert_eq!(same["rebuilds_cache"], false, "the same pair is a no-op");
    assert_eq!(unknown.status().as_u16(), 404);
}

/// The cache-warmth read answers the seconds left on a fresh cache write, and
/// null for a thread with no main-session call.
#[tokio::test]
async fn cache_warmth_counts_down_a_fresh_cache_write() {
    let client = user_client().await;
    let pool = connect().await;
    let warm = Uuid::new_v4();
    let silent = Uuid::new_v4();
    seed_cc_thread_summary(&pool, warm, "idle").await;
    seed_cc_thread_summary(&pool, silent, "idle").await;
    insert_capture(
        &pool,
        warm,
        json!({
            "model": "claude-opus-5-5",
            "usage": {"input_tokens": 1000, "cache_creation_tokens": 1000, "cache_creation_1h_tokens": 0},
        }),
    )
    .await;
    let warmth = |thread: Uuid| {
        let client = client.clone();
        async move {
            client
                .get(format!(
                    "{}/api/v1/threads/{thread}/cache-warmth",
                    base_url()
                ))
                .send()
                .await
                .expect("cache-warmth request failed")
        }
    };
    let fresh = warmth(warm).await;
    let fresh_status = fresh.status().as_u16();
    let fresh_body = fresh.json::<serde_json::Value>().await.expect("not JSON");
    let cold = warmth(silent)
        .await
        .json::<serde_json::Value>()
        .await
        .expect("not JSON");
    let unknown = warmth(Uuid::new_v4()).await;
    delete_thread_rows(&pool, warm).await;
    delete_thread_rows(&pool, silent).await;

    assert_eq!(fresh_status, 200);
    let left = fresh_body["remaining_secs"].as_i64().expect("a number");
    assert!(
        (200..=300).contains(&left),
        "a fresh 5-minute write: {left}"
    );
    assert!(cold["remaining_secs"].is_null(), "no call, no warmth");
    assert_eq!(unknown.status().as_u16(), 404);
}

/// I1: a cold Continue wakes nothing on the old thread. It records the link
/// on both rows. The successor opens with the handoff brief and the typed
/// request, and the old thread's summary names its successor.
#[tokio::test]
async fn composer_successor_wakes_nothing_on_the_old_thread() {
    let client = user_client().await;
    let pool = connect().await;
    let old = WorktreeThread::seed(&pool, "successor-composer").await;
    insert_capture(
        &pool,
        old.thread_id,
        json!({"usage": {"input_tokens": HANDOFF_OFFER_TOKENS + 1, "output_tokens": 1}}),
    )
    .await;
    let captures_before = count_events_of_type(&pool, old.thread_id, "ContextCaptured").await;
    let messages_before = count_events_of_type(&pool, old.thread_id, "MessageReceived").await;

    let (status, body) = post_continue(
        &client,
        old.thread_id,
        json!({"message": "Say exactly: \"successor-ok\""}),
    )
    .await;
    assert_eq!(status, 200, "the handoff must succeed: {body}");
    assert_eq!(body["path"], "brief", "an unwritten cache is cold: {body}");
    let successor_id = body["successor_thread_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or_else(|| panic!("successor_thread_id must be a uuid: {body}"));
    assert!(
        body["link"]
            .as_str()
            .is_some_and(|link| link.contains(&successor_id.to_string())),
        "the link must point at the successor: {body}"
    );

    assert_eq!(
        count_events_of_type(&pool, old.thread_id, "ContextCaptured").await,
        captures_before,
        "the handoff itself must add no capture to the old thread"
    );
    // A woken session would emit asynchronously, so look again later.
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        count_events_of_type(&pool, old.thread_id, "ContextCaptured").await,
        captures_before,
        "nothing may wake the old thread after the handoff"
    );
    assert_eq!(
        count_events_of_type(&pool, old.thread_id, "MessageReceived").await,
        messages_before,
        "a cold Continue sends nothing to the old thread"
    );
    assert_eq!(
        count_events_of_type(&pool, old.thread_id, "SuccessorThreadSpawned").await,
        1,
        "the handoff records exactly one link on the old thread"
    );
    let opening = queued_request(&pool, successor_id)
        .await
        .expect("the handoff queues the successor before it replies");
    let opening = opening["message"].as_str().expect("a message");
    assert!(
        opening.contains(&format!(
            "lucidos events query --thread-id {}",
            old.thread_id
        )) && opening.ends_with("Say exactly: \"successor-ok\""),
        "the successor opens with the brief and the request: {opening}"
    );

    let linked: Option<Uuid> =
        sqlx::query_scalar("SELECT successor_thread_id FROM thread_summaries WHERE thread_id = $1")
            .bind(old.thread_id)
            .fetch_one(&pool)
            .await
            .expect("read old row");
    assert_eq!(
        linked,
        Some(successor_id),
        "the old row names its successor"
    );

    // The thread queue creates the successor's row, so it may lag the reply.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let successor_of = loop {
        let row: Option<Option<Uuid>> =
            sqlx::query_scalar("SELECT successor_of FROM thread_summaries WHERE thread_id = $1")
                .bind(successor_id)
                .fetch_optional(&pool)
                .await
                .expect("read successor row");
        if let Some(Some(of)) = row {
            break Some(of);
        }
        if std::time::Instant::now() >= deadline {
            break None;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    assert_eq!(
        successor_of,
        Some(old.thread_id),
        "the successor row must name the thread it continues"
    );

    let summary: serde_json::Value = client
        .get(format!("{}/api/v1/threads/{}", base_url(), old.thread_id))
        .send()
        .await
        .expect("thread summary request failed")
        .json()
        .await
        .expect("thread summary was not JSON");
    assert_eq!(
        summary["successor"]["thread_id"].as_str(),
        Some(successor_id.to_string().as_str()),
        "the old thread's summary must carry its successor: {summary}"
    );

    // Stop whatever the successor's spawn started.
    client
        .post(format!("{}/api/v1/threads/archive", base_url()))
        .json(&json!({ "thread_id": successor_id.to_string() }))
        .send()
        .await
        .ok();
}

/// The implementer a handoff names is the one the successor's first session
/// runs on, not the old thread's.
#[tokio::test]
async fn successor_runs_on_the_implementer_it_was_handed() {
    let client = user_client().await;
    let pool = connect().await;
    let old = WorktreeThread::seed(&pool, "successor-implementer").await;
    insert_settings(&pool, old.thread_id, "claude-opus-5-5", "high").await;

    let (status, body) = post_continue(
        &client,
        old.thread_id,
        json!({
            "message": "Say exactly: \"successor-ok\"",
            "coding_agent_model": "sonnet",
            "reasoning_effort": "medium",
        }),
    )
    .await;
    assert_eq!(status, 200, "the handoff must succeed: {body}");
    assert_eq!(
        body["implementer"],
        json!({"coding_agent": "claude-code", "model": "sonnet", "reasoning_effort": "medium"}),
        "the reply names what the successor runs on: {body}"
    );
    let successor_id = body["successor_thread_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or_else(|| panic!("successor_thread_id must be a uuid: {body}"));

    // The thread queue records the request it was handed. Its first session
    // starts on exactly that request.
    let queued = queued_request(&pool, successor_id).await;
    client
        .post(format!("{}/api/v1/threads/archive", base_url()))
        .json(&json!({ "thread_id": successor_id.to_string() }))
        .send()
        .await
        .ok();
    let queued = queued.expect("the handoff queues the successor before it replies");
    assert_eq!(queued["coding_agent"], "claude-code", "{queued}");
    assert_eq!(queued["cc_model"], "sonnet", "{queued}");
    assert_eq!(queued["reasoning_effort"], "medium", "{queued}");
}

/// Naming the old thread's model by its bare id keeps the old `[1m]` variant.
/// The successor's first call can then read the prompt cache.
#[tokio::test]
async fn successor_keeps_the_context_variant_of_its_model() {
    let client = user_client().await;
    let pool = connect().await;
    let old = WorktreeThread::seed(&pool, "successor-context-variant").await;
    insert_settings(&pool, old.thread_id, "claude-opus-5-5[1m]", "high").await;

    let (status, body) = post_continue(
        &client,
        old.thread_id,
        json!({
            "message": "Say exactly: \"successor-ok\"",
            "coding_agent_model": "claude-opus-5-5",
            "reasoning_effort": "medium",
        }),
    )
    .await;
    assert_eq!(status, 200, "the handoff must succeed: {body}");
    assert_eq!(
        body["implementer"],
        json!({"coding_agent": "claude-code", "model": "claude-opus-5-5[1m]", "reasoning_effort": "medium"}),
        "the successor keeps the old model's exact id: {body}"
    );
    let successor_id = body["successor_thread_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or_else(|| panic!("successor_thread_id must be a uuid: {body}"));
    let queued_model = queued_request(&pool, successor_id)
        .await
        .and_then(|request| request["cc_model"].as_str().map(str::to_string));
    client
        .post(format!("{}/api/v1/threads/archive", base_url()))
        .json(&json!({ "thread_id": successor_id.to_string() }))
        .send()
        .await
        .ok();
    assert_eq!(queued_model.as_deref(), Some("claude-opus-5-5[1m]"));
}

/// A model the backend does not offer refuses the handoff before anything is
/// recorded, and names the models it does offer.
#[tokio::test]
async fn successor_refuses_an_implementer_the_backend_lacks() {
    let client = user_client().await;
    let pool = connect().await;
    let old = WorktreeThread::seed(&pool, "successor-bad-implementer").await;

    let (status, body) = post_continue(
        &client,
        old.thread_id,
        json!({"message": "carry on", "coding_agent": "codex", "coding_agent_model": "sonnet"}),
    )
    .await;
    let spawned = count_events_of_type(&pool, old.thread_id, "SuccessorThreadSpawned").await;
    delete_thread_rows(&pool, old.thread_id).await;

    assert_eq!(status, 400, "an unoffered model must refuse: {body}");
    assert_eq!(body["reason"], "implementer-not-offered", "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("gpt-5.6-sol")),
        "the refusal lists the offered models: {body}"
    );
    assert_eq!(spawned, 0, "a refused handoff records no link");
}

/// A warm Continue refuses an implementer the backend lacks before it asks
/// the old agent for a narrative. It records no request and sends no message.
#[tokio::test]
async fn a_warm_continue_refuses_an_unoffered_implementer_before_the_narrative_turn() {
    let client = user_client().await;
    let pool = connect().await;
    let old = WorktreeThread::seed(&pool, "successor-warm-bad-implementer").await;
    insert_capture(
        &pool,
        old.thread_id,
        json!({
            "model": "claude-opus-5-5",
            "usage": {
                "input_tokens": HANDOFF_OFFER_TOKENS + 1,
                "cache_creation_tokens": HANDOFF_OFFER_TOKENS + 1,
                "cache_creation_1h_tokens": 0,
            },
        }),
    )
    .await;
    let messages_before = count_events_of_type(&pool, old.thread_id, "MessageReceived").await;

    let (status, body) = post_continue(
        &client,
        old.thread_id,
        json!({"message": "carry on", "coding_agent": "codex", "coding_agent_model": "sonnet"}),
    )
    .await;
    let requested = count_events_of_type(&pool, old.thread_id, "SuccessorRequested").await;
    let messages = count_events_of_type(&pool, old.thread_id, "MessageReceived").await;
    delete_thread_rows(&pool, old.thread_id).await;

    assert_eq!(status, 400, "an unoffered model must refuse: {body}");
    assert_eq!(body["reason"], "implementer-not-offered", "{body}");
    assert_eq!(requested, 0, "a refused Continue records no request");
    assert_eq!(
        messages, messages_before,
        "a refused Continue wakes nothing"
    );
}

/// Stay here is a recorded decision at the context size the thread holds.
#[tokio::test]
async fn stay_here_records_the_size_it_declined_at() {
    let client = user_client().await;
    let pool = connect().await;
    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "idle").await;
    insert_capture(
        &pool,
        thread_id,
        json!({"usage": {"input_tokens": HANDOFF_OFFER_TOKENS + 1, "output_tokens": 1}}),
    )
    .await;

    let resp = client
        .post(format!(
            "{}/api/v1/threads/{thread_id}/successor-offer/decline",
            base_url()
        ))
        .send()
        .await
        .expect("decline request failed");
    let status = resp.status().as_u16();
    let declined_at: Option<i64> = sqlx::query_scalar(
        "SELECT (payload->>'input_tokens')::bigint FROM events \
         WHERE thread_id = $1 AND event_type = 'SuccessorOfferDeclined'",
    )
    .bind(thread_id)
    .fetch_optional(&pool)
    .await
    .expect("read the decline");
    delete_thread_rows(&pool, thread_id).await;

    assert_eq!(status, 204, "Stay here must succeed");
    assert_eq!(
        declined_at,
        Some((HANDOFF_OFFER_TOKENS + 1) as i64),
        "the decline carries the size it was made at"
    );
}

/// A Stay here that reaches a working thread records nothing. A stale tap
/// then cannot hide the card at the size the new turn reached.
#[tokio::test]
async fn stay_here_on_a_working_thread_records_nothing() {
    let client = user_client().await;
    let pool = connect().await;
    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "active").await;
    insert_capture(
        &pool,
        thread_id,
        json!({"usage": {"input_tokens": HANDOFF_OFFER_TOKENS + 1, "output_tokens": 1}}),
    )
    .await;

    let resp = client
        .post(format!(
            "{}/api/v1/threads/{thread_id}/successor-offer/decline",
            base_url()
        ))
        .send()
        .await
        .expect("decline request failed");
    let status = resp.status().as_u16();
    let declined = count_events_of_type(&pool, thread_id, "SuccessorOfferDeclined").await;
    delete_thread_rows(&pool, thread_id).await;

    assert_eq!(status, 409, "a working thread must refuse");
    assert_eq!(declined, 0, "a refused Stay here records no decision");
}

/// Uncommitted work refuses the handoff, so the successor never starts
/// without it. The refusal records no link.
#[tokio::test]
async fn successor_refuses_a_dirty_worktree() {
    let client = user_client().await;
    let pool = connect().await;
    let old = WorktreeThread::seed(&pool, "successor-dirty").await;
    std::fs::write(old.worktree.join("uncommitted.txt"), "not yet\n").unwrap();

    let (status, body) =
        post_continue(&client, old.thread_id, json!({"message": "carry on"})).await;
    let spawned = count_events_of_type(&pool, old.thread_id, "SuccessorThreadSpawned").await;
    delete_thread_rows(&pool, old.thread_id).await;

    assert_eq!(status, 409, "a dirty worktree must refuse: {body}");
    assert_eq!(body["reason"], "uncommitted-work", "{body}");
    assert_eq!(spawned, 0, "a refused handoff records no link");
}

/// A stale composer tap cannot hand off a thread whose turn has started.
#[tokio::test]
async fn composer_successor_refuses_a_working_thread() {
    let client = user_client().await;
    let pool = connect().await;
    let old = WorktreeThread::seed(&pool, "successor-busy").await;
    seed_cc_thread_summary(&pool, old.thread_id, "running").await;

    let (status, body) =
        post_continue(&client, old.thread_id, json!({"message": "carry on"})).await;
    let spawned = count_events_of_type(&pool, old.thread_id, "SuccessorThreadSpawned").await;
    delete_thread_rows(&pool, old.thread_id).await;

    assert_eq!(status, 409, "a working thread must refuse: {body}");
    assert_eq!(body["reason"], "thread-busy", "{body}");
    assert_eq!(spawned, 0, "a refused handoff records no link");
}

/// The agent's handoff carries a narrative. A message is the card's, and
/// the successor route ignores it.
#[tokio::test]
async fn successor_refuses_a_body_with_no_narrative() {
    let client = user_client().await;
    let thread_id = Uuid::new_v4();
    for body in [
        json!({}),
        json!({"message": "next"}),
        json!({"narrative": "   "}),
    ] {
        let (status, reply) = post_successor(&client, thread_id, body.clone()).await;
        assert_eq!(status, 400, "{body} must be refused: {reply}");
        assert_eq!(reply["reason"], "narrative-incomplete", "{reply}");
    }
}

/// I3, I4: a request recorded at the card while the cache was warm reaches the
/// successor the agent spawns. It carries the implementer the user picked,
/// though the agent names none.
#[tokio::test]
async fn the_narrative_spawn_carries_the_request_recorded_at_the_card() {
    let client = user_client().await;
    let pool = connect().await;
    let old = WorktreeThread::seed(&pool, "successor-pending-request").await;
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
         VALUES (gen_random_uuid(), 'SuccessorRequested', $1, $2, 'thread', $2::text)",
    )
    .bind(json!({"message": "Add a dark-mode toggle", "coding_agent_model": "sonnet"}))
    .bind(old.thread_id)
    .execute(&pool)
    .await
    .expect("record the request");

    let (status, body) =
        post_successor(&client, old.thread_id, json!({"narrative": NARRATIVE})).await;
    assert_eq!(status, 200, "the handoff must succeed: {body}");
    assert_eq!(body["implementer"]["model"], "sonnet", "{body}");
    let successor_id = successor_id_of(&body);
    let queued = queued_request(&pool, successor_id).await;
    archive(&client, successor_id).await;
    let queued = queued.expect("the handoff queues the successor before it replies");
    let message = queued["message"].as_str().expect("a message");
    assert!(
        message.ends_with("The user's first request for you:\n\nAdd a dark-mode toggle"),
        "{message}"
    );
    assert_eq!(queued["cc_model"], "sonnet", "{queued}");
}

/// The control route refuses an effort the thread's backend does not offer,
/// before it reaches a session or records anything. A valid tier gets past
/// the check and stops only at the missing live session.
#[tokio::test]
async fn control_refuses_an_effort_the_backend_does_not_offer() {
    let client = user_client().await;
    let pool = connect().await;
    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "idle").await;
    insert_settings(&pool, thread_id, "claude-opus-5-5", "high").await;
    let control = |effort: &str| {
        client
            .post(format!("{}/api/v1/claude-code/control", base_url()))
            .json(&json!({
                "thread_id": thread_id,
                "request": {"subtype": "set_reasoning_effort", "effort": effort},
            }))
            .send()
    };

    for invalid in ["turbo", "  "] {
        let resp = control(invalid).await.expect("control request failed");
        assert_eq!(resp.status().as_u16(), 400, "{invalid:?} must be refused");
        let body: serde_json::Value = resp.json().await.expect("control body was not JSON");
        assert!(
            body["error"]
                .as_str()
                .is_some_and(|e| e.contains("reasoning_effort")),
            "{body}"
        );
    }
    assert_eq!(
        count_events_of_type(&pool, thread_id, "CodingAgentSettingsChanged").await,
        1,
        "a refused effort records nothing beyond the seeded setting"
    );

    let resp = control("low").await.expect("control request failed");
    assert_eq!(
        resp.status().as_u16(),
        404,
        "a valid tier passes the check and stops at the missing live session"
    );
    delete_thread_rows(&pool, thread_id).await;
}

/// A narrative with the six sections the route requires.
const NARRATIVE: &str = "# Outcomes\nPlan approved.\n# Decisions\nNone.\n# Gotchas\nNone.\n\
    # State\nPlan committed.\n# Next step\nSay exactly: \"successor-ok\"\n# Files\nNone.\n";

/// The successor's projected row, once the thread queue has created it.
async fn successor_row(
    pool: &sqlx::PgPool,
    successor_id: Uuid,
) -> Option<(Option<Uuid>, Option<Uuid>, Option<String>)> {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    loop {
        let row: Option<(Option<Uuid>, Option<Uuid>, Option<String>)> = sqlx::query_as(
            "SELECT successor_of, parent_thread_id, wake_mode FROM thread_summaries \
             WHERE thread_id = $1 AND successor_of IS NOT NULL",
        )
        .bind(successor_id)
        .fetch_optional(pool)
        .await
        .expect("read successor row");
        if row.is_some() || std::time::Instant::now() >= deadline {
            return row;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

async fn archive(client: &reqwest::Client, thread_id: Uuid) {
    client
        .post(format!("{}/api/v1/threads/archive", base_url()))
        .json(&json!({ "thread_id": thread_id.to_string() }))
        .send()
        .await
        .ok();
}

fn successor_id_of(body: &serde_json::Value) -> Uuid {
    body["successor_thread_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .unwrap_or_else(|| panic!("successor_thread_id must be a uuid: {body}"))
}

/// I5: a top-level plan card's handoff starts a build chain. The builder is a
/// `no-wake` child of the planner, and its branch starts at the planner's
/// head, which the link records.
#[tokio::test]
async fn a_top_level_plan_card_handoff_starts_a_build_chain() {
    let client = user_client().await;
    let pool = connect().await;
    let planner = WorktreeThread::seed(&pool, "successor-build-chain").await;
    let head = String::from_utf8(git_in(&planner.worktree, &["rev-parse", "HEAD"]).stdout)
        .expect("a sha is utf-8");

    let (status, body) = post_successor(
        &client,
        planner.thread_id,
        json!({"narrative": NARRATIVE, "build_chain": true}),
    )
    .await;
    assert_eq!(status, 200, "the handoff must succeed: {body}");
    assert_eq!(body["build_chain"], true, "{body}");
    let builder = successor_id_of(&body);
    let row = successor_row(&pool, builder).await;
    let link: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload FROM events \
         WHERE thread_id = $1 AND event_type = 'SuccessorThreadSpawned'",
    )
    .bind(planner.thread_id)
    .fetch_optional(&pool)
    .await
    .expect("read the link");
    archive(&client, builder).await;

    assert_eq!(
        row,
        Some((
            Some(planner.thread_id),
            Some(planner.thread_id),
            Some("no-wake".to_string())
        )),
        "the builder is a no-wake child of its planner"
    );
    let link = link.expect("the handoff records its link");
    assert_eq!(link["starts_build_chain"], true, "{link}");
    assert_eq!(link["base_sha"].as_str(), Some(head.trim()), "{link}");
    assert!(
        link.get("handoff_consent").is_none(),
        "a builder under its own planner records no consent: {link}"
    );
}

/// ADR 0507, I3: a child's handoff through the real route records the
/// consent that let it through. A device caller is the owner. The
/// machine-local token names nobody, so it records none.
#[tokio::test]
async fn a_child_handoff_records_the_consent_that_let_it_through() {
    for (client, expected) in [
        (user_client().await, Some("owner")),
        (local_process_client(), None),
    ] {
        let pool = connect().await;
        let parent = Uuid::new_v4();
        seed_cc_thread_summary(&pool, parent, "idle").await;
        let child = WorktreeThread::seed(&pool, "successor-child-consent").await;
        sqlx::query(
            "UPDATE thread_summaries SET parent_thread_id = $2, wake_mode = 'wake' \
             WHERE thread_id = $1",
        )
        .bind(child.thread_id)
        .bind(parent)
        .execute(&pool)
        .await
        .expect("make the thread a child");

        let (status, body) =
            post_successor(&client, child.thread_id, json!({"narrative": NARRATIVE})).await;
        assert_eq!(status, 200, "the handoff must succeed: {body}");
        let successor = successor_id_of(&body);
        let link: serde_json::Value = sqlx::query_scalar(
            "SELECT payload FROM events \
             WHERE thread_id = $1 AND event_type = 'SuccessorThreadSpawned'",
        )
        .bind(child.thread_id)
        .fetch_one(&pool)
        .await
        .expect("the handoff records its link");
        successor_row(&pool, successor).await;
        archive(&user_client().await, successor).await;
        delete_thread_rows(&pool, parent).await;

        assert_eq!(
            link.get("handoff_consent").and_then(|v| v.as_str()),
            expected,
            "{link}"
        );
    }
}

/// I9: a child planner's handoff keeps the successor in its place, so the
/// parent still gets the report.
#[tokio::test]
async fn a_child_plan_card_handoff_keeps_its_place() {
    let client = user_client().await;
    let pool = connect().await;
    let parent = Uuid::new_v4();
    seed_cc_thread_summary(&pool, parent, "idle").await;
    let planner = WorktreeThread::seed(&pool, "successor-child-planner").await;
    sqlx::query(
        "UPDATE thread_summaries SET parent_thread_id = $2, wake_mode = 'wake' \
         WHERE thread_id = $1",
    )
    .bind(planner.thread_id)
    .bind(parent)
    .execute(&pool)
    .await
    .expect("make the planner a child");

    let (status, body) = post_successor(
        &client,
        planner.thread_id,
        json!({"narrative": NARRATIVE, "build_chain": true}),
    )
    .await;
    assert_eq!(status, 200, "the handoff must succeed: {body}");
    let successor = successor_id_of(&body);
    let row = successor_row(&pool, successor).await;
    archive(&client, successor).await;
    delete_thread_rows(&pool, parent).await;

    assert_eq!(body["build_chain"], false, "{body}");
    assert_eq!(
        row,
        Some((
            Some(planner.thread_id),
            Some(parent),
            Some("wake".to_string())
        )),
        "the successor takes the child planner's place"
    );
}

/// I2: the checkpoint route records any well-formed name a flow passes,
/// answers with the handoff estimate's verdict, and refuses a malformed name.
#[tokio::test]
async fn a_checkpoint_run_is_recorded_under_any_name() {
    let client = user_client().await;
    let pool = connect().await;
    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "idle").await;
    insert_capture(
        &pool,
        thread_id,
        json!({"model": "claude-opus-5-5", "usage": {"input_tokens": 900_000}}),
    )
    .await;
    let post = |body: serde_json::Value| {
        client
            .post(format!(
                "{}/api/v1/threads/{thread_id}/handoff-checkpoints",
                base_url()
            ))
            .json(&body)
            .send()
    };
    let verdict: serde_json::Value = post(json!({"name": "any-flow-step", "progress": "1/3"}))
        .await
        .expect("checkpoint request failed")
        .json()
        .await
        .expect("checkpoint body was not JSON");
    let bad = post(json!({"name": "Mid Build"}))
        .await
        .expect("checkpoint request failed");
    let recorded = count_events_of_type(&pool, thread_id, "HandoffCheckpointReached").await;
    let plain = get_context(&client, thread_id).await;
    delete_thread_rows(&pool, thread_id).await;

    let handoff = &verdict["handoff"];
    assert_eq!(handoff["name"], "any-flow-step", "{verdict}");
    assert_eq!(
        handoff["progress"],
        json!({"done": 1, "total": 3}),
        "{verdict}"
    );
    assert_eq!(handoff["pays"], true, "a 900k context pays: {verdict}");
    assert_eq!(
        handoff["without_card"], true,
        "a top-level thread needs no card"
    );
    assert_eq!(handoff["estimate"]["units_left"], 2, "{verdict}");
    assert!(handoff["estimate"]["saving_usd"].is_number(), "{verdict}");
    assert!(verdict["implementer"].is_object(), "{verdict}");
    assert_eq!(bad.status().as_u16(), 400);
    assert_eq!(recorded, 1, "the malformed run records nothing");
    assert!(
        plain.get("handoff").is_none(),
        "the context read records no run: {plain}"
    );
}

/// A chat thread has no session, so the estimate reads its pair from its latest
/// turn: a different model on a warm cache warns, the same pair stays silent.
#[tokio::test]
async fn switch_estimate_prices_a_chat_threads_model_change() {
    let client = user_client().await;
    let pool = connect().await;
    let thread_id = Uuid::new_v4();
    seed_chat_thread_summary(&pool, thread_id, "idle").await;
    insert_capture(
        &pool,
        thread_id,
        json!({
            "model": "claude-opus-5-5",
            "reasoning_effort": "medium",
            "usage": {"input_tokens": 90_000, "cache_creation_tokens": 1000, "cache_creation_1h_tokens": 0},
        }),
    )
    .await;
    let estimate = |query: &'static str| {
        let (client, thread) = (client.clone(), thread_id);
        async move {
            let resp = client
                .get(format!(
                    "{}/api/v1/threads/{}/switch-estimate?{query}",
                    base_url(),
                    thread
                ))
                .send()
                .await
                .expect("switch-estimate request failed");
            assert_eq!(resp.status().as_u16(), 200);
            resp.json::<serde_json::Value>().await.expect("not JSON")
        }
    };

    let switched = estimate("model=claude-fable-5-1&effort=medium").await;
    let same = estimate("model=claude-opus-5-5&effort=medium").await;
    delete_thread_rows(&pool, thread_id).await;

    assert_eq!(switched["rebuilds_cache"], true);
    assert!(switched["extra_cost_usd"].as_f64().unwrap() > 0.0);
    assert_eq!(same["rebuilds_cache"], false, "the same pair is a no-op");
}
