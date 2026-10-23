//! API e2e for *successor sessions*: the context-size read, and the handoff to
//! a fresh session in the same thread
//! (`docs/plans/2026-10-23-successor-session-in-the-same-thread.md`).

use crate::support::{
    base_url, count_events_of_type, db_url, git, git_in, http_client, insert_session_started,
    local_process_client, seed_cc_thread_summary, seed_chat_thread_summary, unique_marker,
    user_client, workspace_path,
};
use lucidos_engine::engine::successor::HANDOFF_OFFER_TOKENS;
use serde_json::{json, Value};
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

/// The payload of `thread_id`'s latest `SessionHandedOff`.
async fn handoff_payload(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<serde_json::Value> {
    sqlx::query_scalar(
        "SELECT payload FROM events \
         WHERE thread_id = $1 AND event_type = 'SessionHandedOff' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .expect("read the handoff")
}

/// The first input of the successor session the engine starts in
/// `thread_id`, once it lands. The engine starts the session after the
/// reply. The API suite has no coding agent to run it, so the input is the
/// proof: it lands in the same thread, and its continuity has Rust tests.
async fn successor_session_input(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<Value> {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let text: Option<Value> = sqlx::query_scalar(
            "SELECT payload FROM events \
             WHERE thread_id = $1 AND event_type = 'MessageReceived' \
               AND sequence > (SELECT MAX(sequence) FROM events \
                     WHERE thread_id = $1 AND event_type = 'SessionHandedOff') \
             ORDER BY sequence LIMIT 1",
        )
        .bind(thread_id)
        .fetch_optional(pool)
        .await
        .expect("read the successor session's first message");
        if text.is_some() || std::time::Instant::now() >= deadline {
            return text;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// How many threads name `thread_id` as the thread they continue.
async fn successor_threads_of(pool: &sqlx::PgPool, thread_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM thread_summaries WHERE successor_of = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .expect("count successor threads")
}

fn head_of(worktree: &std::path::Path) -> String {
    String::from_utf8(git_in(worktree, &["rev-parse", "HEAD"]).stdout)
        .expect("a sha is utf-8")
        .trim()
        .to_string()
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
    assert_eq!(
        fresh_body["keep_alive"],
        json!({"state": "not_kept", "reason": "nothing_pending"}),
        "an idle thread waits on no card"
    );
    assert!(cold["remaining_secs"].is_null(), "no call, no warmth");
    assert!(
        cold["keep_alive"].is_null(),
        "a cold cache has no keep-alive"
    );
    assert_eq!(unknown.status().as_u16(), 404);
}

/// The per-thread keep-alive switch (ADR 0554): an unidentified caller is
/// refused, and a device's call records the event under that device and
/// projects the column. "on" clears it.
#[tokio::test]
async fn the_keep_alive_switch_is_recorded_under_the_device() {
    let client = user_client().await;
    let pool = connect().await;
    let thread_id = Uuid::new_v4();
    seed_cc_thread_summary(&pool, thread_id, "waiting_for_user_answer").await;
    let switch = |client: reqwest::Client, state: &'static str| async move {
        client
            .post(format!(
                "{}/api/v1/threads/{thread_id}/cache-keep-alive",
                base_url()
            ))
            .json(&json!({ "state": state }))
            .send()
            .await
            .expect("cache-keep-alive request failed")
            .status()
            .as_u16()
    };
    let off_at = || async {
        sqlx::query_scalar::<_, bool>(
            "SELECT cache_keep_alive_off_at IS NOT NULL FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_one(&pool)
        .await
        .expect("read the switch")
    };

    let refused = switch(http_client(), "off").await;
    let after_refusal = count_events_of_type(&pool, thread_id, "CacheKeepAliveTurnedOff").await;
    let turned_off = switch(client.clone(), "off").await;
    let actor: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT payload->'actor' FROM events \
          WHERE thread_id = $1 AND event_type = 'CacheKeepAliveTurnedOff'",
    )
    .bind(thread_id)
    .fetch_optional(&pool)
    .await
    .expect("read the switch event");
    let off = off_at().await;
    let turned_on = switch(client.clone(), "on").await;
    let on = off_at().await;
    let bad = switch(client.clone(), "later").await;
    delete_thread_rows(&pool, thread_id).await;

    assert_eq!(refused, 401, "a caller with no actor");
    assert_eq!(after_refusal, 0, "a refusal records nothing");
    assert_eq!(turned_off, 204);
    assert!(
        actor.is_some_and(|a| !a.is_null()),
        "the switch names who turned it off"
    );
    assert!(off, "the column projects the event");
    assert_eq!(turned_on, 204);
    assert!(!on, "on clears it");
    assert_eq!(bad, 400);
}

/// A cold Continue starts a successor session in the same thread at once,
/// from a handoff brief and the typed request. It creates no thread and moves
/// no branch.
#[tokio::test]
async fn a_cold_continue_starts_a_successor_session_in_the_same_thread() {
    let client = user_client().await;
    let pool = connect().await;
    let old = WorktreeThread::seed(&pool, "successor-composer").await;
    insert_capture(
        &pool,
        old.thread_id,
        json!({"usage": {"input_tokens": HANDOFF_OFFER_TOKENS + 1, "output_tokens": 1}}),
    )
    .await;
    let head = head_of(&old.worktree);

    let (status, body) = post_continue(
        &client,
        old.thread_id,
        json!({"message": "Say exactly: \"successor-ok\""}),
    )
    .await;
    let handoff = handoff_payload(&pool, old.thread_id).await;
    let input = successor_session_input(&pool, old.thread_id).await;
    let spawned = count_events_of_type(&pool, old.thread_id, "SuccessorThreadSpawned").await;
    let threads = successor_threads_of(&pool, old.thread_id).await;
    let head_after = head_of(&old.worktree);
    archive(&client, old.thread_id).await;

    assert_eq!(status, 200, "the handoff must succeed: {body}");
    assert_eq!(body["path"], "brief", "an unwritten cache is cold: {body}");
    assert_eq!(body["starts"], "now", "{body}");
    assert!(body.get("successor_thread_id").is_none(), "{body}");
    assert_eq!((spawned, threads), (0, 0), "a handoff creates no thread");
    assert_eq!(head_after, head, "a handoff moves no branch");
    let handoff = handoff.expect("the handoff is recorded before the reply");
    assert_eq!(handoff["opening"], "brief", "{handoff}");
    let message = handoff["message"].as_str().expect("a message");
    assert!(
        message.contains(&format!(
            "lucidos events query --thread-id {}",
            old.thread_id
        )) && !message.contains("successor-ok"),
        "the brief leaves the request to the first message: {message}"
    );
    assert_eq!(
        handoff["request"]["message"], "Say exactly: \"successor-ok\"",
        "{handoff}"
    );
    let input = input.expect("the successor session starts in the same thread");
    assert_eq!(input["text"], "Say exactly: \"successor-ok\"", "{input}");
    assert_eq!(
        input["mode"], "human",
        "the request starts the session as the user's own message: {input}"
    );
}

/// The implementer a handoff names is the one the successor session runs on,
/// not the previous session's.
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
    // The handoff records the implementer its successor session starts on.
    let handoff = handoff_payload(&pool, old.thread_id).await;
    archive(&client, old.thread_id).await;
    let handoff = handoff.expect("the handoff is recorded before the reply");
    assert_eq!(handoff["coding_agent"], "claude-code", "{handoff}");
    assert_eq!(handoff["coding_agent_model"], "sonnet", "{handoff}");
    assert_eq!(handoff["reasoning_effort"], "medium", "{handoff}");
}

/// Naming the thread's model by its bare id keeps its `[1m]` variant. The
/// successor session's first call can then read the prompt cache.
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
    let handoff_model = handoff_payload(&pool, old.thread_id)
        .await
        .and_then(|handoff| handoff["coding_agent_model"].as_str().map(str::to_string));
    archive(&client, old.thread_id).await;
    assert_eq!(handoff_model.as_deref(), Some("claude-opus-5-5[1m]"));
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
    let handed_off = count_events_of_type(&pool, old.thread_id, "SessionHandedOff").await;
    delete_thread_rows(&pool, old.thread_id).await;

    assert_eq!(status, 400, "an unoffered model must refuse: {body}");
    assert_eq!(body["reason"], "implementer-not-offered", "{body}");
    assert!(
        body["error"]
            .as_str()
            .is_some_and(|e| e.contains("gpt-5.6-sol")),
        "the refusal lists the offered models: {body}"
    );
    assert_eq!(handed_off, 0, "a refused handoff records nothing");
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

/// A warm Continue asks the old agent for its narrative as a coding-agent
/// message. The thread lock must not read it as a switch to the Lucidos Agent.
#[tokio::test]
async fn a_warm_continue_asks_the_old_agent_for_its_narrative() {
    let client = user_client().await;
    let pool = connect().await;
    let old = WorktreeThread::seed(&pool, "successor-warm-narrative").await;
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

    let (status, body) =
        post_continue(&client, old.thread_id, json!({"message": "carry on"})).await;
    let requested = count_events_of_type(&pool, old.thread_id, "SuccessorRequested").await;
    // A coding-agent dispatch records its message after the reply.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let messages = loop {
        let messages = count_events_of_type(&pool, old.thread_id, "MessageReceived").await;
        if messages > messages_before || std::time::Instant::now() >= deadline {
            break messages;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    };
    archive(&client, old.thread_id).await;
    delete_thread_rows(&pool, old.thread_id).await;

    assert_eq!(status, 200, "a warm Continue must succeed: {body}");
    assert_eq!(
        body["path"], "narrative",
        "a fresh cache write is warm: {body}"
    );
    assert_eq!(requested, 1, "the card's request is recorded");
    assert_eq!(
        messages,
        messages_before + 1,
        "the old agent is asked for its narrative"
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

/// Uncommitted work stays in the worktree, so it never blocks a handoff: the
/// successor session finds it there.
#[tokio::test]
async fn a_dirty_worktree_hands_off_with_its_work() {
    let client = user_client().await;
    let pool = connect().await;
    let old = WorktreeThread::seed(&pool, "successor-dirty").await;
    let uncommitted = old.worktree.join("uncommitted.txt");
    std::fs::write(&uncommitted, "not yet\n").unwrap();

    let (status, body) =
        post_successor(&client, old.thread_id, json!({"narrative": NARRATIVE})).await;
    let handed_off = count_events_of_type(&pool, old.thread_id, "SessionHandedOff").await;
    archive(&client, old.thread_id).await;

    assert_eq!(status, 200, "a dirty worktree hands off: {body}");
    assert_eq!(handed_off, 1);
    assert!(uncommitted.exists(), "the handoff keeps uncommitted work");
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
    let handed_off = count_events_of_type(&pool, old.thread_id, "SessionHandedOff").await;
    delete_thread_rows(&pool, old.thread_id).await;

    assert_eq!(status, 409, "a working thread must refuse: {body}");
    assert_eq!(body["reason"], "thread-busy", "{body}");
    assert_eq!(handed_off, 0, "a refused handoff records nothing");
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
/// successor session the agent's handoff starts, as its typist's own message.
/// It carries the implementer the user picked, though the agent names none.
#[tokio::test]
async fn the_narrative_handoff_carries_the_request_recorded_at_the_card() {
    let client = user_client().await;
    let pool = connect().await;
    let old = WorktreeThread::seed(&pool, "successor-pending-request").await;
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
         VALUES (gen_random_uuid(), 'SuccessorRequested', $1, $2, 'thread', $2::text)",
    )
    .bind(json!({
        "message": "Add a dark-mode toggle",
        "coding_agent_model": "sonnet",
        "actor": {"kind": "device", "device_id": "e2e-device"},
    }))
    .bind(old.thread_id)
    .execute(&pool)
    .await
    .expect("record the request");

    let (status, body) =
        post_successor(&client, old.thread_id, json!({"narrative": NARRATIVE})).await;
    let handoff = handoff_payload(&pool, old.thread_id).await;
    archive(&client, old.thread_id).await;

    assert_eq!(status, 200, "the handoff must succeed: {body}");
    assert_eq!(body["implementer"]["model"], "sonnet", "{body}");
    let handoff = handoff.expect("the handoff is recorded before the reply");
    assert_eq!(handoff["opening"], "narrative", "{handoff}");
    let message = handoff["message"].as_str().expect("a message");
    assert!(!message.contains("Add a dark-mode toggle"), "{message}");
    assert_eq!(
        handoff["request"],
        json!({
            "message": "Add a dark-mode toggle",
            "by": {"kind": "device", "device_id": "e2e-device"},
        }),
        "the request starts the successor session as its typist's message: {handoff}"
    );
    assert_eq!(handoff["coding_agent_model"], "sonnet", "{handoff}");
}

/// The control route refuses an effort the thread's backend does not offer,
/// before it records anything. A valid tier between turns is recorded at
/// once, with the picking device as its actor (ADR 0542).
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
        200,
        "a pick between turns is recorded"
    );
    let pick: Option<(Option<String>, Option<serde_json::Value>)> = sqlx::query_as(
        "SELECT payload->>'reasoning_effort', payload->'actor' FROM events \
         WHERE thread_id = $1 AND event_type = 'CodingAgentSettingsChanged' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(&pool)
    .await
    .expect("read the recorded pick");
    // The picker reads the pair back from the commands read.
    let commands: serde_json::Value = client
        .get(format!(
            "{}/api/v1/claude-code/commands?thread_id={thread_id}",
            base_url()
        ))
        .send()
        .await
        .expect("commands request failed")
        .json()
        .await
        .expect("commands body was not JSON");
    delete_thread_rows(&pool, thread_id).await;
    let (effort, actor) = pick.expect("the pick is a settings event");
    assert_eq!(effort.as_deref(), Some("low"));
    assert!(
        actor.is_some_and(|a| !a.is_null()),
        "the pick names who made it"
    );
    assert_eq!(commands["current_reasoning_effort"], "low", "{commands}");
    assert_eq!(commands["current_model"], "claude-opus-5-5", "{commands}");
}

/// While a turn runs, the rung is locked: a pick is refused with a reason the
/// client can tell apart, and nothing is recorded (ADR 0542).
#[tokio::test]
async fn control_refuses_a_pick_while_a_turn_runs() {
    let client = user_client().await;
    let pool = connect().await;
    for status in ["running", "waiting_for_user_answer"] {
        let thread_id = Uuid::new_v4();
        seed_cc_thread_summary(&pool, thread_id, status).await;
        let resp = client
            .post(format!("{}/api/v1/claude-code/control", base_url()))
            .json(&json!({
                "thread_id": thread_id,
                "request": {"subtype": "set_model", "model": "claude-sonnet-5-5"},
            }))
            .send()
            .await
            .expect("control request failed");
        let code = resp.status().as_u16();
        let body: serde_json::Value = resp.json().await.expect("control body was not JSON");
        let recorded = count_events_of_type(&pool, thread_id, "CodingAgentSettingsChanged").await;
        delete_thread_rows(&pool, thread_id).await;

        assert_eq!(code, 409, "{status}: a pick mid-turn is refused: {body}");
        assert_eq!(body["reason"], "turn-running", "{status}: {body}");
        assert_eq!(recorded, 0, "{status}: a refused pick records nothing");
    }
}

/// A narrative with the six sections the route requires.
const NARRATIVE: &str = "# Outcomes\nPlan approved.\n# Decisions\nNone.\n# Gotchas\nNone.\n\
    # State\nPlan committed.\n# Next step\nSay exactly: \"successor-ok\"\n# Files\nNone.\n";

async fn archive(client: &reqwest::Client, thread_id: Uuid) {
    client
        .post(format!("{}/api/v1/threads/archive", base_url()))
        .json(&json!({ "thread_id": thread_id.to_string() }))
        .send()
        .await
        .ok();
}

/// A child's handoff keeps its place under the parent and asks nobody,
/// whichever caller hands off. A second handoff before the successor session
/// starts is refused.
#[tokio::test]
async fn a_child_handoff_keeps_its_parent_and_asks_nobody() {
    for client in [user_client().await, local_process_client()] {
        let pool = connect().await;
        let parent = Uuid::new_v4();
        seed_cc_thread_summary(&pool, parent, "idle").await;
        let child = WorktreeThread::seed(&pool, "successor-child").await;
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
        let (again, again_body) =
            post_successor(&client, child.thread_id, json!({"narrative": NARRATIVE})).await;
        let place: (Option<Uuid>, Option<String>) = sqlx::query_as(
            "SELECT parent_thread_id, wake_mode FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(child.thread_id)
        .fetch_one(&pool)
        .await
        .expect("read the child row");
        let approvals =
            count_events_of_type(&pool, child.thread_id, "OwnerApprovalRequested").await;
        let spawned = count_events_of_type(&pool, child.thread_id, "SuccessorThreadSpawned").await;
        archive(&user_client().await, child.thread_id).await;
        delete_thread_rows(&pool, parent).await;

        assert_eq!(status, 200, "the handoff must succeed: {body}");
        assert_eq!(place, (Some(parent), Some("wake".to_string())));
        assert_eq!((approvals, spawned), (0, 0));
        assert_eq!(again, 409, "{again_body}");
        assert_eq!(again_body["reason"], "handoff-pending", "{again_body}");
    }
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
