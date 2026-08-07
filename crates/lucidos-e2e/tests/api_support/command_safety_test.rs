//! E2E coverage for the **Lucidos Agent command safety** HTTP surface (ADR
//! 0002). Two contracts are exercised over real HTTP against the booted e2e
//! workspace:
//!
//! 1. The trigger **side-effect grant** (Phase 5) round-trips through
//!    `POST/PUT/GET /api/v1/triggers` as a typed `SideEffectCategory` set, and an
//!    unknown category is rejected at the serde boundary.
//! 2. The command-checkpoint **undo** and **diff** endpoints (Phase 4) fail
//!    closed on an unknown or malformed checkpoint id.
//! 3. The guard itself, end to end (Phase 4 and the 2026-08-06 addendum): a
//!    scripted `run_python` really reaches it, and a command whose destruction
//!    was gitignored leaves no undo card behind.
//!
//! The first two need no `command_guard` preference: the grant field is stored
//! on the trigger regardless of the guard, and the endpoints resolve the
//! checkpoint from the event store directly. The third turns the guard on for
//! its own duration and puts it back.

use crate::support::{base_url, http_client, unique_marker};
use serde_json::json;

/// Ensure a timezone preference exists so `create_trigger` doesn't have to fall
/// back to its UTC default. The `set_preference` handler reads the key from the
/// `?key=` query param (not the body), so pass it there with `{value}` in the
/// body — the body-only `{key, value}` form is silently a no-op.
async fn set_timezone_utc(client: &reqwest::Client) {
    let _ = client
        .put(format!("{}/api/v1/preferences?key=timezone", base_url()))
        .json(&json!({ "value": "UTC" }))
        .send()
        .await;
}

/// Create a schedule trigger carrying `grant` and assert the API accepted it.
async fn create_trigger_with_grant(client: &reqwest::Client, name: &str, grant: &[&str]) {
    let body = json!({
        "name": name,
        "run": { "type": "intent", "intent": "noop" },
        "cron_expressions": ["0 0 8 * * *"],
        "side_effect_grant": grant,
    });
    let resp = client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&body)
        .send()
        .await
        .expect("POST /triggers failed");
    let res: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert_eq!(
        res["success"], true,
        "Trigger create must succeed: {:?}",
        res
    );
}

/// Replace the side-effect grant of an existing trigger.
async fn update_trigger_grant(client: &reqwest::Client, trigger_id: &str, grant: &[&str]) {
    let resp = client
        .put(format!("{}/api/v1/triggers?id={}", base_url(), trigger_id))
        .json(&json!({ "side_effect_grant": grant }))
        .send()
        .await
        .expect("PUT /triggers failed");
    let res: serde_json::Value = resp.json().await.expect("Invalid JSON");
    assert_eq!(
        res["success"], true,
        "Trigger update must succeed: {:?}",
        res
    );
}

async fn delete_trigger(client: &reqwest::Client, trigger_id: &str) {
    let _ = client
        .delete(format!("{}/api/v1/triggers?id={}", base_url(), trigger_id))
        .send()
        .await;
}

/// Read the `side_effect_grant` of one trigger as a sorted Vec. The TriggerInfo
/// wire shape omits the field when empty (`skip_serializing_if = "Vec::is_empty"`),
/// so an absent array reads back as the empty grant.
fn grant_of(trigger: &serde_json::Value) -> Vec<String> {
    let mut v: Vec<String> = trigger["side_effect_grant"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// Poll `GET /api/v1/triggers` until the trigger named `name` has the expected
/// (sorted) grant. POST/PUT return once the event is persisted, but the
/// in-memory scheduler projection the list endpoint reads updates
/// asynchronously — under the suite's parallel load a single GET races the
/// subscriber, so poll until it catches up. Returns the trigger's id.
async fn poll_trigger_grant(
    client: &reqwest::Client,
    name: &str,
    expected_sorted: &[&str],
) -> String {
    let mut want: Vec<String> = expected_sorted.iter().map(|s| s.to_string()).collect();
    want.sort();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let listed: serde_json::Value = client
            .get(format!("{}/api/v1/triggers", base_url()))
            .send()
            .await
            .expect("GET /triggers failed")
            .json()
            .await
            .expect("Invalid JSON");
        let found = listed["triggers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["name"].as_str() == Some(name))
            .cloned();
        if let Some(ref t) = found {
            if grant_of(t) == want {
                return t["id"].as_str().unwrap().to_string();
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "Trigger '{}' grant did not become {:?} within 5s (last saw {:?})",
            name,
            want,
            found.as_ref().map(grant_of),
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// Create → GET → update (replace) → update (clear) round-trip of the typed
/// side-effect grant set.
#[tokio::test]
async fn trigger_side_effect_grant_round_trips() {
    let client = http_client();
    set_timezone_utc(&client).await;
    let name = unique_marker("e2e-grant");

    // Create with two categories; GET must round-trip them.
    create_trigger_with_grant(&client, &name, &["email", "external_api"]).await;
    let trigger_id = poll_trigger_grant(&client, &name, &["email", "external_api"]).await;

    // Replace with a different single category.
    update_trigger_grant(&client, &trigger_id, &["cloud_cli"]).await;
    poll_trigger_grant(&client, &name, &["cloud_cli"]).await;

    // Clear it: an empty array drops the grant entirely.
    update_trigger_grant(&client, &trigger_id, &[]).await;
    poll_trigger_grant(&client, &name, &[]).await;

    delete_trigger(&client, &trigger_id).await;
}

/// `side_effect_grant` is a typed serde enum, so an unknown category fails to
/// deserialize at the `Json` extractor before the handler runs. Note: axum
/// 0.7's `Json` rejects a valid-JSON-but-wrong-shape body (a `JsonDataError`)
/// with **422 Unprocessable Entity** — 400 is reserved for malformed JSON
/// *syntax* (`JsonSyntaxError`). Either way the unknown category never reaches
/// the scheduler.
#[tokio::test]
async fn trigger_create_rejects_unknown_side_effect_category() {
    let client = http_client();
    set_timezone_utc(&client).await;

    let body = json!({
        "name": unique_marker("e2e-bad-grant"),
        "run": { "type": "intent", "intent": "noop" },
        "cron_expressions": ["0 0 8 * * *"],
        "side_effect_grant": ["yolo"],
    });
    let resp = client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&body)
        .send()
        .await
        .expect("POST /triggers failed");
    assert_eq!(
        resp.status().as_u16(),
        422,
        "Unknown side-effect category must be rejected at the serde boundary (422)"
    );
}

/// The same rejection applies to PUT: an unknown category can't sneak in via an
/// update either.
#[tokio::test]
async fn trigger_update_rejects_unknown_side_effect_category() {
    let client = http_client();
    set_timezone_utc(&client).await;
    let name = unique_marker("e2e-bad-update");

    // A real trigger to target.
    create_trigger_with_grant(&client, &name, &["email"]).await;
    let trigger_id = poll_trigger_grant(&client, &name, &["email"]).await;

    let resp = client
        .put(format!("{}/api/v1/triggers?id={}", base_url(), trigger_id))
        .json(&json!({ "side_effect_grant": ["not_a_category"] }))
        .send()
        .await
        .expect("PUT /triggers failed");
    assert_eq!(
        resp.status().as_u16(),
        422,
        "Unknown side-effect category on update must be rejected (422)"
    );

    // The bad update must not have mutated the stored grant.
    poll_trigger_grant(&client, &name, &["email"]).await;
    delete_trigger(&client, &trigger_id).await;
}

/// `POST /api/v1/command-checkpoint/undo` with an unknown id fails closed: the
/// engine can't find a matching `CommandCheckpointed` event, so the handler
/// returns 400 with an `[ERROR]`-prefixed body (mirrors the change
/// discard/revert endpoints).
#[tokio::test]
async fn command_checkpoint_undo_unknown_id_returns_error() {
    let client = http_client();
    let unknown_id = uuid::Uuid::new_v4().to_string();

    let resp = client
        .post(format!("{}/api/v1/command-checkpoint/undo", base_url()))
        .json(&json!({ "checkpoint_id": unknown_id }))
        .send()
        .await
        .expect("POST /command-checkpoint/undo failed");

    assert_eq!(
        resp.status().as_u16(),
        400,
        "Unknown checkpoint id must return 400 Bad Request"
    );
    let body = resp.text().await.expect("Failed to read body");
    assert!(
        body.starts_with("[ERROR]"),
        "Error body must use the [ERROR] prefix, got: {body:?}"
    );
    assert!(
        body.contains("no command checkpoint"),
        "Error body should name the missing checkpoint, got: {body:?}"
    );
}

/// A malformed body (missing the required `checkpoint_id`) is rejected before
/// the handler runs — a guard against the endpoint silently treating an empty
/// id as a valid no-op.
#[tokio::test]
async fn command_checkpoint_undo_missing_id_is_rejected() {
    let client = http_client();
    let resp = client
        .post(format!("{}/api/v1/command-checkpoint/undo", base_url()))
        .json(&json!({}))
        .send()
        .await
        .expect("POST /command-checkpoint/undo failed");
    let status = resp.status().as_u16();
    assert!(
        (400..500).contains(&status),
        "Missing checkpoint_id should return 4xx, got {status}"
    );
}

/// `GET /api/v1/command-checkpoint/diff` refuses an id that is not a UUID
/// before it can reach a ref name. The id is interpolated into
/// `refs/lucidos/command-checkpoints/<id>` and
/// `refs/lucidos/command-post-images/<id>`, so a path-shaped id is the thing
/// this parse exists to stop.
#[tokio::test]
async fn command_checkpoint_diff_rejects_a_non_uuid_id() {
    let client = http_client();
    for id in ["../../heads/main", "not-a-uuid", ""] {
        let resp = client
            .get(format!("{}/api/v1/command-checkpoint/diff", base_url()))
            .query(&[("checkpoint_id", id)])
            .send()
            .await
            .expect("GET /command-checkpoint/diff failed");
        assert_eq!(
            resp.status().as_u16(),
            400,
            "checkpoint_id {id:?} must be rejected as malformed"
        );
    }
}

/// A well-formed id with no `CommandCheckpointed` behind it is a 404, not an
/// empty diff: an empty diff means "this command changed nothing", which is a
/// different claim from "there is no such checkpoint".
#[tokio::test]
async fn command_checkpoint_diff_unknown_id_is_not_found() {
    let client = http_client();
    let resp = client
        .get(format!("{}/api/v1/command-checkpoint/diff", base_url()))
        .query(&[("checkpoint_id", uuid::Uuid::new_v4().to_string())])
        .send()
        .await
        .expect("GET /command-checkpoint/diff failed");
    assert_eq!(resp.status().as_u16(), 404);
}

// --- The guard, end to end (ADR 0002 Phase 4 + the 2026-08-06 addendum) ------

/// Read a preference so the test can put it back.
async fn get_preference(client: &reqwest::Client, key: &str) -> Option<String> {
    let resp = client
        .get(format!("{}/api/v1/preferences?key={key}", base_url()))
        .send()
        .await
        .ok()?;
    let body: serde_json::Value = resp.json().await.ok()?;
    body.get("value")?.as_str().map(str::to_string)
}

async fn set_preference(client: &reqwest::Client, key: &str, value: Option<&str>) {
    let body = match value {
        Some(v) => json!({ "value": v }),
        // Restoring an unset preference: the accessors treat "" as unset for
        // the guard (it compares against "true") and for the judge (anything
        // but "false" is on).
        None => json!({ "value": "" }),
    };
    let resp = client
        .put(format!("{}/api/v1/preferences?key={key}", base_url()))
        .json(&body)
        .send()
        .await
        .expect("preference write failed");
    assert!(
        resp.status().is_success(),
        "setting {key} should succeed, got {}",
        resp.status()
    );
}

fn checkpoint_ref_count() -> usize {
    let out = crate::support::git_in(
        &crate::support::workspace_path(),
        &["for-each-ref", "--format=%(refname)", "refs/lucidos/"],
    );
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
}

/// The reported 2026-08-06 bug, driven through the **real guard** instead of a
/// seeded event: a `run_python` step whose only destruction lands inside a
/// gitignored path must leave no `CommandCheckpointed` card behind, because the
/// snapshot never captured what it destroyed and the Undo it would offer could
/// neither restore nor remove anything.
///
/// This is the one seam nothing else reaches.
/// `command-checkpoint-card.spec.ts` seeds the event straight into the DB, and
/// the `git_ops::checkpoint` unit tests stop at the git layer; only this
/// exercises classify, snapshot, run, post image, and emit-or-suppress as one
/// chain. It is reachable at all because the mock provider gained a
/// `MOCK_RUN_PYTHON:` sentinel: without it nothing in e2e can make the agent
/// issue a command for the guard to gate.
///
/// **Deliberately the negative case only.** Asserting the positive one (a card
/// with counts, then an Undo that removes the created file) needs the command
/// to make a git-VISIBLE change to the workspace repo, and `workspace_path()`
/// is the very repo the apply tests merge into: a dirty tree fails their apply
/// with "Cannot merge: the repository has uncommitted changes". So the
/// created-file half is covered where it costs nothing, in the
/// `git_ops::checkpoint` unit tests. Do NOT "complete" this test by adding a
/// case that writes a tracked file.
#[tokio::test]
async fn a_command_destroying_only_ignored_content_leaves_no_undo_card() {
    let pool = sqlx::PgPool::connect(&crate::support::db_url())
        .await
        .expect("connect to the e2e workspace database");
    let client = crate::support::user_client().await;

    // Both preferences are workspace-wide, so they are shared state within a
    // run and get put back below, before the assertions, so a failing one still
    // restores. A panic in the polling between here and there would leak them,
    // which is tolerable and not worth a Drop guard: no other test reads either
    // key, and the e2e database is recreated per run rather than per test.
    let prior_guard = get_preference(&client, "command_guard").await;
    let prior_judge = get_preference(&client, "command_guard_judge").await;
    // Judge off, so classification is the deterministic static fallback rather
    // than a mock LLM reply that would not parse as a verdict.
    set_preference(&client, "command_guard", Some("true")).await;
    set_preference(&client, "command_guard_judge", Some("false")).await;

    // Scratch under `.lucidos/`, which the e2e workspace gitignores. Destroying
    // it is exactly the shape of the reported step, and it leaves the working
    // tree clean, so no concurrent apply test can see it.
    let scratch_rel = format!(".lucidos/tmp/e2e-ckpt-{}", uuid::Uuid::new_v4().simple());
    let scratch = crate::support::workspace_path().join(&scratch_rel);
    std::fs::create_dir_all(&scratch).expect("create scratch dir");
    std::fs::write(scratch.join("f.txt"), "scratch").expect("write scratch file");

    // The checkpoint pair images the WHOLE working tree, so anything another
    // test lands there between the two snapshots is read as this command's own
    // effect and produces the card this test proves absent. Hold the tree still
    // for the guarded turn; see `workspace_tree_lock`.
    let _tree_quiet = crate::support::workspace_tree_lock().write().await;

    let refs_before = checkpoint_ref_count();
    let marker = crate::support::unique_marker("api-command-guard");
    let code = format!("import shutil; shutil.rmtree('{scratch_rel}')");
    let resp = client
        .post(format!("{}/api/v1/chat/stream", base_url()))
        .json(&json!({
            "message": format!("{marker} MOCK_RUN_PYTHON:{code}"),
            "mode": "human",
        }))
        .send()
        .await
        .expect("chat request failed");
    assert_eq!(resp.status(), 200, "chat/stream should accept the message");

    let thread_id = crate::support::poll_thread_summary_by_marker(&pool, &marker, 25)
        .await
        .thread_id;

    // Wait for the turn to finish, so "no card" is a settled answer rather than
    // one the emit simply has not reached yet.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        let done: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM events WHERE aggregate = 'thread' AND aggregate_id = $1 \
               AND event_type IN ('ResponseGenerated', 'ResponseFailed', 'ResponseAborted')",
        )
        .bind(thread_id.to_string())
        .fetch_one(&pool)
        .await
        .expect("DB query failed");
        if done > 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the guarded turn never terminated"
        );
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }

    let tool_results: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE aggregate = 'thread' AND aggregate_id = $1 \
           AND event_type = 'ToolResult'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("DB query failed");
    let checkpoints: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM events WHERE aggregate = 'thread' AND aggregate_id = $1 \
           AND event_type = 'CommandCheckpointed'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .expect("DB query failed");

    set_preference(&client, "command_guard", prior_guard.as_deref()).await;
    set_preference(&client, "command_guard_judge", prior_judge.as_deref()).await;

    // The command really ran through the guard. Without this the "no card"
    // assertion below would also pass if the tool had never been called.
    assert_eq!(
        tool_results, 1,
        "the scripted run_python should have produced exactly one ToolResult"
    );
    assert!(
        !scratch.exists(),
        "the command should have destroyed its scratch dir at {scratch_rel}"
    );
    // The bug: this used to be 1, and its Undo restored nothing, removed
    // nothing, and then reported "Reverted".
    assert_eq!(
        checkpoints, 0,
        "a command whose destruction was gitignored must leave no undo card"
    );
    // And the pair it snapshotted was dropped rather than left pinning objects.
    assert_eq!(
        checkpoint_ref_count(),
        refs_before,
        "the empty-diff path should have deleted both checkpoint refs"
    );
}
