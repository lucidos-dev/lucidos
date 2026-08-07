//! E2E coverage for `POST /api/v1/triggers/run`, the **off-schedule run**.
//!
//! Unit tests in `engine_impl::trigger_runs` pin the pure precondition checker;
//! this pins the wiring end to end over real HTTP, because the operation exists
//! precisely to replace a workaround that *looked* like it worked. The three
//! things worth proving at this seam:
//!
//! 1. A cron trigger actually fires and records `last_run`, so the run is real
//!    rather than an admission that goes nowhere.
//! 2. A paused trigger is refused up front. Submitting blind would be dropped
//!    by the queue executor with only a log line, and the caller would be told
//!    a run started, which is the exact class of lie this replaces (an agent
//!    once "started" a nightly job by resuming a paused trigger).
//! 3. An event-only trigger is refused and pointed at emitting its event. A
//!    payload-less fire is a shape it has never had.

use crate::support::{base_url, http_client, unique_marker, workspace_path};
use serde_json::json;
use std::time::{Duration, Instant};

/// A script trigger, so a fire needs no LLM provider to complete. The script
/// writes nothing and exits 0; `last_run` is what we assert on.
const PROBE_SCRIPT: &str = r#"#!/usr/bin/env python3
print("ok")
"#;

/// Cron pinned to a moment that never arrives inside a test run (03:00 on the
/// 1st of January). The point is that the trigger HAS a schedule, not that the
/// schedule fires: an off-schedule run must be the only thing that runs it, so
/// a passing assertion can't be a scheduled fire in disguise.
const NEVER_SOON_CRON: &str = "0 0 3 1 1 *";

/// Create a trigger and return its id.
///
/// **`run` is an intent unless the test actually fires the trigger.** A script
/// trigger needs a `.py` under `data/triggers/<slug>/`, and `data/` is
/// git-tracked, so the file leaves the e2e workspace's working tree dirty from
/// the moment it is written until the engine's post-run auto-commit picks it
/// up. A trigger that never fires never gets that auto-commit, so the dirt
/// lasts the whole test, and the concurrently-running apply tests fail with
/// "Cannot merge: the repository has uncommitted changes". The three refusal
/// tests below never reach execution, so they carry an intent they will never
/// run and touch no files at all.
async fn create_trigger(
    client: &reqwest::Client,
    name: &str,
    slug: &str,
    run: serde_json::Value,
    body_extra: serde_json::Value,
) -> String {
    let mut body = json!({ "name": name, "slug": slug, "run": run });
    for (k, v) in body_extra.as_object().expect("extra is an object") {
        body[k] = v.clone();
    }

    let created: serde_json::Value = client
        .post(format!("{}/api/v1/triggers", base_url()))
        .json(&body)
        .send()
        .await
        .expect("POST /triggers failed")
        .json()
        .await
        .expect("Invalid JSON");
    assert_eq!(created["success"], true, "create failed: {created}");

    find_trigger(client, name)
        .await
        .expect("created trigger is listed")["id"]
        .as_str()
        .expect("trigger id")
        .to_string()
}

/// A never-executed `run` for the refusal tests. Naming it makes the "this
/// trigger is not meant to fire" contract explicit at each call site.
fn unreachable_intent() -> serde_json::Value {
    json!({ "type": "intent", "intent": "never executed: this trigger's run is always refused" })
}

async fn find_trigger(client: &reqwest::Client, name: &str) -> Option<serde_json::Value> {
    let body: serde_json::Value = client
        .get(format!("{}/api/v1/triggers", base_url()))
        .send()
        .await
        .expect("GET /triggers failed")
        .json()
        .await
        .expect("Invalid JSON");
    body["triggers"]
        .as_array()?
        .iter()
        .find(|t| t["name"] == name)
        .cloned()
}

async fn run_trigger(client: &reqwest::Client, id: &str) -> serde_json::Value {
    try_run_trigger(client, id).await.expect("run_trigger")
}

/// `run_trigger` that hands back a transport / parse failure instead of
/// panicking on it.
///
/// The capacity-policy caller below has to put the engine's original policy
/// back on EVERY path out. A panic between the two PUTs would skip the restore
/// and leave the shared engine on this test's widened limits for the rest of
/// the parallel suite, so that caller needs the failure as a value it can hold
/// until after it has restored.
async fn try_run_trigger(client: &reqwest::Client, id: &str) -> Result<serde_json::Value, String> {
    let resp = client
        .post(format!("{}/api/v1/triggers/run?id={}", base_url(), id))
        .send()
        .await
        .map_err(|e| format!("POST /triggers/run failed: {e}"))?;
    resp.json()
        .await
        .map_err(|e| format!("invalid JSON from /triggers/run: {e}"))
}

/// Delete a trigger that owns no files on disk. Best-effort: a failed cleanup
/// must not fail the assertion the test exists for.
async fn delete_trigger(client: &reqwest::Client, id: &str) {
    let _ = client
        .delete(format!("{}/api/v1/triggers?id={}", base_url(), id))
        .send()
        .await;
}

/// How long `cleanup_fired_trigger` keeps watching its path after removing it.
const CLEANUP_SETTLE: Duration = Duration::from_secs(15);

/// Delete a trigger that HAS FIRED AND remove its directory, committing the
/// removal so the e2e workspace's working tree is left clean (the engine
/// auto-commits dirty `data/` files after a run, so both the probe script and
/// the trigger's own `trigger.toml` are tracked by now). A dirty tree fails
/// every concurrent apply test with "Cannot merge: the repository has
/// uncommitted changes", so this is load-bearing, not tidiness. Best-effort
/// throughout.
///
/// Firing is what makes this necessary, not the trigger's kind: a trigger that
/// is only ever refused leaves its definition untracked and `delete_trigger`
/// alone suffices for it.
///
/// **One commit is not enough, and neither is "commit until it reads clean".**
/// The engine's auto-commit (`commit_dirty_logged` in `engine_impl/scripts.rs`)
/// is `commit_all_dirty`, which stages ALL of `data/` inside one blocking
/// closure and is fired by every script run, `run_python` and coding-agent turn
/// in this parallel suite. So a commit belonging to some OTHER test can have
/// staged this trigger's files BEFORE the removal below and commit them AFTER
/// it, writing the path back into HEAD while it is gone from the worktree. That
/// is an unstaged deletion, which is exactly the dirty tree this exists to
/// prevent, and it is what left `data/triggers/run-probe-*/scripts/run.py`
/// behind an engine "Script task output" commit on 2026-08-07. A clean read is
/// therefore not a terminal state: the path can go dirty again after it, so the
/// loop watches for the whole window rather than exiting on the first (or the
/// second) clean read.
///
/// The window is a ceiling, not a sleep: it costs nothing but polling, and each
/// test is its own async task, so it overlaps the rest of the suite.
async fn cleanup_fired_trigger(client: &reqwest::Client, id: &str, slug: &str) {
    delete_trigger(client, id).await;
    {
        // Removing the dir is a working-tree change, so it takes the same guard
        // the creation did; see `workspace_tree_lock`. Scoped to the removal
        // alone, deliberately: that guard's own contract is that only the moment
        // a file appears or disappears needs it, and a commit changes HEAD and
        // the index rather than the worktree the snapshot images. Holding it
        // across the poll below would pin every other tree writer behind a
        // fifteen-second read guard (tokio's RwLock is write-preferring, so one
        // queued snapshot would stall them all) to protect nothing.
        let _tree = crate::support::workspace_tree_lock().read().await;
        let _ = std::fs::remove_dir_all(workspace_path().join("data/triggers").join(slug));
    }
    let pathspec = format!("data/triggers/{}", slug);
    let deadline = Instant::now() + CLEANUP_SETTLE;
    while Instant::now() < deadline {
        let dirty = std::process::Command::new("git")
            .current_dir(workspace_path())
            .args(["status", "--porcelain", "--", &pathspec])
            .output()
            .map(|o| !o.stdout.is_empty())
            // A `git status` that would not run says nothing about the tree, so
            // treat it as dirty and keep trying rather than declaring victory.
            .unwrap_or(true);
        if dirty {
            // Pathspec form: commits the working-tree state of exactly this
            // path, so a concurrent test's staged changes are never swept in.
            // The failure this retries past is an unlucky one: the command
            // exits non-zero for a lost `index.lock` and for "nothing to
            // commit" alike, so the exit code cannot tell them apart and the
            // next poll is what settles it.
            let _ = std::process::Command::new("git")
                .current_dir(workspace_path())
                .args([
                    "commit",
                    "-q",
                    "-m",
                    "e2e: remove off-schedule-run probe trigger",
                    "--",
                    &pathspec,
                ])
                .output();
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The thread a trigger's most recent fire ran on, plus every terminator on it
/// paired with the turn each is anchored to.
type TurnTerminators = (Option<uuid::Uuid>, Vec<(String, Option<String>)>);

/// Read a fired trigger's turn out of the event log: its thread, and every
/// terminator on that thread with its `request_event_id`.
///
/// One function over one connection, returning a `Result` rather than
/// unwrapping: the caller still owns a trigger definition in the shared working
/// tree at this point, so a database hiccup must come back as a value it can
/// carry past its cleanup instead of a panic that strands the definition.
///
/// `origin.reason` is where the scheduler records which trigger fired, and a
/// null `request_event_id` is meaningful (it is what an unanchored terminator
/// looks like), so it is read as an `Option` rather than filtered out.
async fn read_turn_terminators(trigger_id: &str) -> Result<TurnTerminators, String> {
    let pool = sqlx::PgPool::connect(&crate::support::db_url())
        .await
        .map_err(|e| format!("connect to the e2e workspace database: {e}"))?;
    let result = async {
        let thread_id: Option<uuid::Uuid> = sqlx::query_scalar(
            "SELECT thread_id FROM events \
             WHERE event_type = 'TriggerStarted' \
               AND payload->'origin'->'reason'->>'trigger_id' = $1 \
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(trigger_id)
        .fetch_optional(&pool)
        .await
        .map_err(|e| format!("query TriggerStarted: {e}"))?;
        let Some(tid) = thread_id else {
            return Ok((None, vec![]));
        };
        let terminators = sqlx::query_as(
            "SELECT event_type, payload->>'request_event_id' FROM events \
             WHERE thread_id = $1 \
               AND event_type IN ('ResponseGenerated', 'ResponseCanceled', \
                                  'ResponseAborted', 'ResponseFailed') \
             ORDER BY sequence",
        )
        .bind(tid)
        .fetch_all(&pool)
        .await
        .map_err(|e| format!("query terminators: {e}"))?;
        Ok((Some(tid), terminators))
    }
    .await;
    pool.close().await;
    result
}

/// The capacity policy the engine is running with, read off the queue endpoint
/// so the restore below puts back exactly what was there rather than a guess at
/// the defaults.
async fn capacity_policy(client: &reqwest::Client) -> serde_json::Value {
    let body: serde_json::Value = client
        .get(format!("{}/api/v1/thread-queue", base_url()))
        .send()
        .await
        .expect("GET /thread-queue failed")
        .json()
        .await
        .expect("Invalid JSON");
    body["policy"].clone()
}

/// Replace the capacity policy. Sent whole: `PUT /thread-queue/policy` fills
/// absent fields with defaults rather than leaving them alone, so a partial
/// body would silently reset every limit this test did not name.
async fn set_capacity_policy(client: &reqwest::Client, policy: &serde_json::Value) {
    let resp = client
        .put(format!("{}/api/v1/thread-queue/policy", base_url()))
        .json(policy)
        .send()
        .await
        .expect("PUT /thread-queue/policy failed");
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    assert_eq!(status, 200, "policy PUT rejected: {body}");
}

/// Fire a trigger off-schedule with an admission slot guaranteed.
///
/// A free pool slot is the PREMISE of every test that fires, not part of what
/// any of them proves. Admission is decided against the SHARED capacity pool and
/// this suite runs its whole API surface in parallel: sampling
/// `GET /thread-queue` through a full run shows the pool pinned at its 32-slot
/// ceiling with other tests' in-flight chats, and a cron submitted right then is
/// honestly reported as `queued`, so the outcome under test never gets its
/// chance. So widen the two limits a cron submit can bind on (the global ceiling
/// and the per-kind cron cap) for the submit, then put the original policy back.
/// The window is one request wide: `submit` decides admission inline, so nothing
/// is gained by holding the wider policy across the fire itself.
///
/// Three things make this one helper rather than a block in each test:
///
/// - **The restore must happen on every path out.** Nothing between the two PUTs
///   may panic, or the shared engine keeps the widened limits for the rest of the
///   suite and every admission-refusal assertion silently stops testing anything.
///   Hence the `Result`: the run's failure is carried out as a value, and the
///   caller unwraps it after its own cleanup.
/// - **The widen is a read-modify-write over one shared setting**, so two of them
///   overlapping is not a slower test but a leak (the second reader saves the
///   first's widened policy as "the original"). `capacity_policy_lock` is what
///   makes them exclusive.
/// - Duplicating it per test is how one copy drifts out of step with the other.
async fn run_trigger_with_a_free_slot(
    client: &reqwest::Client,
    id: &str,
) -> Result<serde_json::Value, String> {
    let _cap = crate::support::capacity_policy_lock().lock().await;
    let original_policy = capacity_policy(client).await;
    let mut roomy_policy = original_policy.clone();
    roomy_policy["max_concurrent_total"] = json!(512);
    roomy_policy["max_concurrent_cron"] = json!(512);
    set_capacity_policy(client, &roomy_policy).await;
    let run = try_run_trigger(client, id).await;
    set_capacity_policy(client, &original_policy).await;
    run
}

async fn wait_for_last_run(
    client: &reqwest::Client,
    name: &str,
    timeout: Duration,
) -> Option<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(t) = find_trigger(client, name).await {
            if let Some(last_run) = t["last_run"].as_str() {
                return Some(last_run.to_string());
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    None
}

#[tokio::test]
async fn run_fires_a_cron_trigger_and_records_last_run() {
    let client = http_client();
    let slug = unique_marker("run-probe");
    let name = format!("Run probe {}", slug);
    // The one test that actually fires, so the one that needs a real script on
    // disk (a script run needs no LLM provider). The engine auto-commits it
    // after the run; `cleanup_script_trigger` commits its removal.
    let trigger_dir = workspace_path().join("data/triggers").join(&slug);
    {
        // The moment this file appears in the shared working tree is one the
        // command-checkpoint test must not be mid-snapshot for; see
        // `workspace_tree_lock`. Only the appearance needs the guard: a file
        // present across both of its images cancels out of the diff.
        let _tree = crate::support::workspace_tree_lock().read().await;
        std::fs::create_dir_all(trigger_dir.join("scripts")).expect("create scripts dir");
        std::fs::write(trigger_dir.join("scripts/run.py"), PROBE_SCRIPT).expect("write script");
    }
    let id = create_trigger(
        &client,
        &name,
        &slug,
        json!({ "type": "script", "path": format!("triggers/{}/scripts/run.py", slug) }),
        json!({ "cron_expressions": [NEVER_SOON_CRON] }),
    )
    .await;

    let before = find_trigger(&client, &name).await.expect("trigger listed");
    assert!(
        before["last_run"].is_null(),
        "fresh trigger must not have run yet: {before}"
    );

    // The assertions below are just as strong with an admission slot guaranteed:
    // a coalesced run, a refused run, or one that never records `last_run` still
    // fails. See `run_trigger_with_a_free_slot` for why the slot is established
    // rather than hoped for, and why the failure comes back as a value.
    let run = run_trigger_with_a_free_slot(&client, &id).await;

    let last_run = wait_for_last_run(&client, &name, Duration::from_secs(45)).await;
    cleanup_fired_trigger(&client, &id, &slug).await;

    // Unwrapped only after cleanup: a panic before it strands the probe script
    // and the trigger's definition in the shared working tree, which is the
    // dirty tree that fails every concurrent apply test.
    let resp = run.expect("off-schedule run request");
    assert_eq!(resp["success"], true, "run refused: {resp}");
    assert_eq!(
        resp["status"], "started",
        "an idle trigger's run must start, not queue or coalesce: {resp}"
    );
    assert!(
        last_run.is_some(),
        "the run never recorded last_run, so nothing actually fired"
    );
}

/// One turn, one terminator, asserted over a real trigger fire in the event log.
///
/// The scheduler used to broadcast its own empty `ResponseGenerated` after
/// every successful `process_trigger`, on top of the one the agentic loop had
/// already emitted for the turn. On a clean run the duplicate was invisible: the
/// real terminator carried the text, and the frontend's empty-completion note is
/// suppressed when the exchange has any. The turn it broke was the one the user
/// stopped. `ResponseCanceled` landed, the phantom landed ~10ms later carrying no
/// `request_event_id`, and the timeline read "Response canceled" followed by a
/// "Done ✓ / The model returned an empty response" panel for a turn that had been
/// canceled, not answered (2026-08-07, the nightly release-prep trigger).
///
/// This asserts on the ordinary clean fire rather than reproducing the cancel,
/// because the duplicate is present on BOTH and the clean path has no race to
/// win: the assertion is a count, not a timing. A cancel repro would have to land
/// its Stop inside the mock's streaming window to prove the same thing.
///
/// It also pins the anchor. A terminator with no `request_event_id` belongs to no
/// turn, which is exactly what let this one fold into a boundary it had nothing
/// to do with, so a replacement phantom that happened to be the only terminator
/// would still fail here.
#[tokio::test]
async fn a_fired_trigger_turn_emits_exactly_one_terminator() {
    let client = http_client();
    let slug = unique_marker("run-terminator");
    let name = format!("Run terminator {}", slug);
    // An intent trigger, so the fire goes through the agentic loop (the path
    // that owns the turn's terminator) rather than a script. Under the e2e
    // default `LUCIDOS_MODEL=mock` the reply is fixed text and no tool runs; the
    // wording keeps a real provider to one turn too, if someone runs the suite
    // against one.
    let id = {
        // Creating the trigger writes `data/triggers/<slug>/trigger.toml` into
        // the shared working tree, and this trigger FIRES, so the engine's
        // post-run auto-commit tracks it. See `workspace_tree_lock`.
        let _tree = crate::support::workspace_tree_lock().read().await;
        create_trigger(
            &client,
            &name,
            &slug,
            json!({
                "type": "intent",
                "intent": "Reply with one short sentence confirming you ran. Call no tools.",
            }),
            json!({ "cron_expressions": [NEVER_SOON_CRON] }),
        )
        .await
    };

    let run = run_trigger_with_a_free_slot(&client, &id).await;

    // `last_run` is recorded by `record_trigger_completed`, which the scheduler
    // calls AFTER `process_trigger` has returned, i.e. after the loop's
    // terminator and after the phantom this test forbids. So waiting on it is an
    // upper bound on both being visible, and the count below races nothing.
    let last_run = wait_for_last_run(&client, &name, Duration::from_secs(90)).await;
    let probe = read_turn_terminators(&id).await;

    // No tree guard here: `cleanup_fired_trigger` takes its own, and tokio's
    // RwLock is write-preferring, so re-entering `read()` while already holding
    // one deadlocks the moment the snapshot test is queued for `write()`.
    cleanup_fired_trigger(&client, &id, &slug).await;

    // Everything above hands its failure back as a value, and every unwrap waits
    // until here, because this trigger's definition is in the shared working
    // tree until `cleanup_fired_trigger` has run: a panic before it strands the
    // definition and fails every concurrent apply test with a dirty tree.
    let run = run.expect("off-schedule run request");
    let (thread_id, terminators) = probe.expect("read the fired turn's terminators");

    assert_eq!(run["success"], true, "run refused: {run}");
    assert!(
        last_run.is_some(),
        "the run never recorded last_run, so nothing actually fired"
    );
    assert!(
        thread_id.is_some(),
        "the fire recorded no TriggerStarted, so there is no turn to check"
    );
    assert_eq!(
        terminators.len(),
        1,
        "a trigger fire is one turn and must leave exactly one terminator; found: {terminators:?}"
    );
    assert!(
        terminators[0].1.is_some(),
        "the terminator must be anchored on the turn it ends via request_event_id, \
         or it folds into whatever exchange happens to be last; found: {terminators:?}"
    );
}

#[tokio::test]
async fn run_refuses_a_paused_trigger_instead_of_silently_dropping_it() {
    let client = http_client();
    let slug = unique_marker("run-paused");
    let name = format!("Run paused {}", slug);
    let id = create_trigger(
        &client,
        &name,
        &slug,
        unreachable_intent(),
        json!({ "cron_expressions": [NEVER_SOON_CRON] }),
    )
    .await;

    let paused: serde_json::Value = client
        .put(format!("{}/api/v1/triggers?id={}", base_url(), id))
        .json(&json!({ "paused": true }))
        .send()
        .await
        .expect("PUT /triggers failed")
        .json()
        .await
        .expect("Invalid JSON");
    assert_eq!(paused["success"], true, "pause failed: {paused}");

    let resp = run_trigger(&client, &id).await;
    let after = find_trigger(&client, &name).await.expect("trigger listed");
    delete_trigger(&client, &id).await;

    assert_eq!(
        resp["success"], false,
        "a paused trigger's run must be refused, not reported as started: {resp}"
    );
    let message = resp["message"].as_str().unwrap_or_default();
    assert!(
        message.contains("paused"),
        "the refusal must say the trigger is paused: {resp}"
    );
    assert!(
        after["last_run"].is_null(),
        "nothing may have run for a paused trigger: {after}"
    );
}

#[tokio::test]
async fn run_refuses_an_event_only_trigger_and_points_at_emitting_the_event() {
    let client = http_client();
    let slug = unique_marker("run-eventonly");
    let name = format!("Run event-only {}", slug);
    let event_type = format!("E2eRunProbe{}", slug.replace('-', ""));
    let id = create_trigger(
        &client,
        &name,
        &slug,
        unreachable_intent(),
        json!({ "on": [{ "event_type": event_type }] }),
    )
    .await;

    let resp = run_trigger(&client, &id).await;
    delete_trigger(&client, &id).await;

    assert_eq!(
        resp["success"], false,
        "an event-only trigger has no scheduled fire to reproduce: {resp}"
    );
    let message = resp["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(&event_type),
        "the refusal must name the event the trigger subscribes to: {resp}"
    );
    assert!(
        message.contains("Emit"),
        "the refusal must point at the route that does work: {resp}"
    );
}

#[tokio::test]
async fn run_refuses_an_unknown_trigger_id() {
    let client = http_client();
    let resp = run_trigger(&client, "00000000-0000-0000-0000-000000000000").await;
    assert_eq!(resp["success"], false, "unknown id must be refused: {resp}");
    assert!(
        resp["message"]
            .as_str()
            .unwrap_or_default()
            .contains("No trigger found"),
        "the refusal must say the id is unknown: {resp}"
    );
}
