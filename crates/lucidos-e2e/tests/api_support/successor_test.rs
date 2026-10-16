//! API e2e for *successor threads*: the context-size read and the composer's
//! handoff (`docs/plans/2026-10-13-auto-compact-trial-and-successor-threads.md`).

use crate::support::{
    base_url, count_events_of_type, db_url, git, git_in, insert_session_started,
    seed_cc_thread_summary, unique_marker, user_client, workspace_path,
};
use lucidos_engine::engine::successor::{HANDOFF_NUDGE_TOKENS, HANDOFF_OFFER_TOKENS};
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
    let nudge_size = HANDOFF_NUDGE_TOKENS + 1;
    insert_capture(
        &pool,
        thread_id,
        json!({"usage": {"input_tokens": offer_size, "output_tokens": 1}}),
    )
    .await;
    insert_capture(
        &pool,
        thread_id,
        json!({"usage": {"input_tokens": nudge_size}, "parent_tool_use_id": "toolu_e2e"}),
    )
    .await;
    let offered = get_context(&client, thread_id).await;

    insert_capture(
        &pool,
        thread_id,
        json!({"usage": {"input_tokens": nudge_size, "output_tokens": 1}}),
    )
    .await;
    let nudged = get_context(&client, thread_id).await;

    delete_thread_rows(&pool, thread_id).await;

    assert_eq!(
        empty,
        json!({"input_tokens": null, "can_hand_off": true, "offer": false, "nudge": false, "applied": false, "implementer": implementer}),
        "a thread with no captures has no size"
    );
    assert_eq!(
        offered,
        json!({"input_tokens": offer_size, "can_hand_off": true, "offer": true, "nudge": false, "applied": false, "implementer": implementer}),
        "the later sub-agent capture must not count"
    );
    assert_eq!(
        nudged,
        json!({"input_tokens": nudge_size, "can_hand_off": true, "offer": true, "nudge": true, "applied": false, "implementer": implementer}),
        "a newer main-session capture is the size"
    );
}

/// I19: the composer's handoff wakes nothing on the old thread. It records
/// the link on both rows, and the old thread's summary names its successor.
#[tokio::test]
async fn composer_successor_wakes_nothing_on_the_old_thread() {
    let client = user_client().await;
    let pool = connect().await;
    let old = WorktreeThread::seed(&pool, "successor-composer").await;
    insert_capture(
        &pool,
        old.thread_id,
        json!({"usage": {"input_tokens": HANDOFF_NUDGE_TOKENS + 1, "output_tokens": 1}}),
    )
    .await;
    let captures_before = count_events_of_type(&pool, old.thread_id, "ContextCaptured").await;

    let (status, body) = post_successor(
        &client,
        old.thread_id,
        json!({"message": "Say exactly: \"successor-ok\""}),
    )
    .await;
    assert_eq!(status, 200, "the handoff must succeed: {body}");
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
        count_events_of_type(&pool, old.thread_id, "SuccessorThreadSpawned").await,
        1,
        "the handoff records exactly one link on the old thread"
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

    let (status, body) = post_successor(
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
    let queued: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'data'->'request' FROM events \
         WHERE event_type = 'ThreadQueued' AND payload->'data'->>'thread_id' = $1 \
         ORDER BY sequence LIMIT 1",
    )
    .bind(successor_id.to_string())
    .fetch_optional(&pool)
    .await
    .expect("read the successor's queued request");
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

/// A model the backend does not offer refuses the handoff before anything is
/// recorded, and names the models it does offer.
#[tokio::test]
async fn successor_refuses_an_implementer_the_backend_lacks() {
    let client = user_client().await;
    let pool = connect().await;
    let old = WorktreeThread::seed(&pool, "successor-bad-implementer").await;

    let (status, body) = post_successor(
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

/// Uncommitted work refuses the handoff, so the successor never starts
/// without it. The refusal records no link.
#[tokio::test]
async fn successor_refuses_a_dirty_worktree() {
    let client = user_client().await;
    let pool = connect().await;
    let old = WorktreeThread::seed(&pool, "successor-dirty").await;
    std::fs::write(old.worktree.join("uncommitted.txt"), "not yet\n").unwrap();

    let (status, body) =
        post_successor(&client, old.thread_id, json!({"message": "carry on"})).await;
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
        post_successor(&client, old.thread_id, json!({"message": "carry on"})).await;
    let spawned = count_events_of_type(&pool, old.thread_id, "SuccessorThreadSpawned").await;
    delete_thread_rows(&pool, old.thread_id).await;

    assert_eq!(status, 409, "a working thread must refuse: {body}");
    assert_eq!(body["reason"], "thread-busy", "{body}");
    assert_eq!(spawned, 0, "a refused handoff records no link");
}

/// The request carries exactly one opening: a narrative or a message.
#[tokio::test]
async fn successor_refuses_neither_or_both_bodies() {
    let client = user_client().await;
    let thread_id = Uuid::new_v4();
    for body in [
        json!({}),
        json!({"message": "next", "narrative": "# Outcomes"}),
        json!({"message": "   "}),
    ] {
        let (status, reply) = post_successor(&client, thread_id, body.clone()).await;
        assert_eq!(status, 400, "{body} must be refused: {reply}");
    }
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
