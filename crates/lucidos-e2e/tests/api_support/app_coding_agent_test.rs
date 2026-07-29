//! API e2e for the app coding-agent thread lifecycle and concurrent-apply
//! paths (docs/plans/2026-05-27-app-coding-agent-threads-design.md §9).
//!
//! Skips the live Claude Code subprocess and instead seeds the projection +
//! worktree state directly: faster, deterministic, and the apply path itself
//! is what we're exercising. The non-test flow (`run_claude` → spawn → CC
//! emits ChangeProposed → Apply) is covered by browser e2e.

use crate::support::{
    base_url, db_url, git, git_in, http_client, seed_app_cc_thread_summary, workspace_path,
};
use futures::StreamExt;
use serde_json::json;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::io::StreamReader;
use uuid::Uuid;

async fn seed_change(
    client: &reqwest::Client,
    change_id: Uuid,
    thread_id: Uuid,
    branch_name: &str,
    repo_root: &str,
    files: &[&str],
) {
    let url = format!("{}/api/v1/internal/seed-change-for-test", base_url());
    let resp = client
        .post(&url)
        .json(&json!({
            "change_id": change_id.to_string(),
            "thread_id": thread_id.to_string(),
            "branch_name": branch_name,
            "repo_root": repo_root,
            "description": "App coding-agent test change",
            "files": files,
            "requires_restart": false,
            // App threads skip the harden gate regardless — pass false to
            // confirm the gate is short-circuited, not just satisfied.
            "hardened": false,
        }))
        .send()
        .await
        .expect("seed-change-for-test request failed");
    assert!(
        resp.status().is_success(),
        "seed-change-for-test returned {}: {}",
        resp.status(),
        resp.text().await.unwrap_or_default()
    );
}

/// Serialises the workspace add/diff/commit sequence in
/// `ensure_app_committed`. Two `#[tokio::test]`s on the same workspace race
/// on a shared git index: T1's `git add` stages T1's files, T2's `git add`
/// stages T2's files into the SAME index, T1 commits BOTH, and T2's commit
/// then fails with "nothing to commit". A process-wide mutex around the
/// stage-and-commit critical section eliminates the race without slowing
/// the rest of the test (worktree add, branch ops, file edits) at all.
static WORKSPACE_INDEX_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Ensure the e2e workspace has an app folder named `app_id` with at least
/// one file committed on `main`. Idempotent: skips when the folder already
/// exists. The folder lives under `<ws>/data/apps/<app_id>/`.
fn ensure_app_committed(app_id: &str, marker: &str) -> PathBuf {
    let ws = workspace_path();
    let app_dir = ws.join("data/apps").join(app_id);
    if !app_dir.exists() {
        std::fs::create_dir_all(&app_dir).unwrap();
    }
    let manifest = app_dir.join("manifest.json");
    if !manifest.exists() {
        std::fs::write(
            &manifest,
            format!(
                "{{\"id\":\"{}\",\"name\":\"{} test app\",\"description\":\"E2E test\"}}",
                app_id, marker
            ),
        )
        .unwrap();
    }
    let index = app_dir.join("index.html");
    if !index.exists() {
        std::fs::write(
            &index,
            format!("<!doctype html><title>{} test app</title>", marker),
        )
        .unwrap();
    }
    // Stage + commit anything new. Quiet about empty-commit cases. The mutex
    // keeps two parallel test runs from interleaving add/diff/commit on the
    // shared workspace index.
    let _index_guard = WORKSPACE_INDEX_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let rel_app = format!("data/apps/{}", app_id);
    git(&["add", &rel_app]);
    let status = std::process::Command::new("git")
        .args(["diff", "--cached", "--quiet"])
        .current_dir(&ws)
        .status()
        .expect("git diff --cached");
    if !status.success() {
        git(&["commit", "-m", &format!("seed e2e test app {}", app_id)]);
    }
    app_dir
}

/// Create a sparse-checkout worktree mirroring what
/// `git_ops::create_sparse_app_worktree` produces in production: branch from
/// `main` (the helper's fresh-branch arm; in production it reuses an
/// existing branch on resume), cone-mode sparse-checkout narrowed to
/// `data/apps/<app_id>/`.
fn create_app_worktree(app_id: &str, branch: &str) -> PathBuf {
    let ws = workspace_path();
    let wt = ws
        .join(".lucidos/worktrees")
        .join(format!("e2e-app-{}", branch.replace('/', "-")));
    // Clean any leftover from a prior run.
    if wt.exists() {
        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force", wt.to_str().unwrap()])
            .current_dir(&ws)
            .output();
        let _ = std::fs::remove_dir_all(&wt);
    }
    // `worktree add --no-checkout` + `sparse-checkout` mirrors the engine's
    // production path. Cone mode always materialises top-level files, so
    // `.gitignore` is visible at the worktree root.
    git(&[
        "worktree",
        "add",
        "--no-checkout",
        wt.to_str().unwrap(),
        "-b",
        branch,
        "main",
    ]);
    git_in(&wt, &["sparse-checkout", "init", "--cone"]);
    git_in(&wt, &["sparse-checkout", "set", &format!("data/apps/{}", app_id)]);
    git_in(&wt, &["checkout", branch]);
    wt
}

/// Subscribe to `/api/v1/events` and collect SSE lines until one matching
/// `predicate` arrives, or `max` elapses. Returns the matching line. Use for
/// transient events (`AppUiRefreshRequested`, `PresenceCheck`, …) that
/// `EventBus::is_persisted` deliberately keeps out of the events table —
/// polling the DB would never find them.
///
/// The connection is established (response headers awaited) BEFORE this
/// function returns. The engine's SSE handler calls `event_bus.subscribe()`
/// synchronously at handler entry, before it returns the response — so once
/// the headers have arrived, the broadcast subscription is guaranteed live.
/// The caller can then trigger the event-producing action knowing the
/// subscriber will see it; no head-start sleep is needed. (A blind sleep was
/// the prior source of flakiness: under heavy parallel load the connect
/// hadn't completed within the sleep window, the apply's emit fired before
/// the subscription registered, and the transient event was missed.)
async fn await_sse_line(
    predicate: impl Fn(&str) -> bool + Send + 'static,
    max: Duration,
) -> tokio::task::JoinHandle<Option<String>> {
    let resp = http_client()
        .get(format!("{}/api/v1/events", base_url()))
        .header("Accept", "text/event-stream")
        .timeout(max + Duration::from_secs(5))
        .send()
        .await
        .expect("SSE connect");
    tokio::spawn(async move {
        let byte_stream = resp.bytes_stream().map(|r| r.map_err(std::io::Error::other));
        let mut lines = BufReader::new(StreamReader::new(byte_stream)).lines();
        let deadline = tokio::time::sleep(max);
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                line = lines.next_line() => match line {
                    Ok(Some(line)) => {
                        if predicate(&line) {
                            return Some(line);
                        }
                    }
                    Ok(None) | Err(_) => return None,
                },
                _ = &mut deadline => return None,
            }
        }
    })
}

/// Cleanup helper — drops any state the tests stamped onto the e2e workspace
/// so re-runs start from a clean slate. Ignores errors (best-effort).
async fn cleanup(
    pool: &sqlx::PgPool,
    thread_ids: &[Uuid],
    change_ids: &[Uuid],
    branches: &[&str],
    worktrees: &[PathBuf],
) {
    let ws = workspace_path();
    for wt in worktrees {
        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force", wt.to_str().unwrap()])
            .current_dir(&ws)
            .output();
        let _ = std::fs::remove_dir_all(wt);
    }
    for branch in branches {
        let _ = std::process::Command::new("git")
            .args(["branch", "-D", branch])
            .current_dir(&ws)
            .output();
    }
    for cid in change_ids {
        let _ = sqlx::query("DELETE FROM changes WHERE id = $1")
            .bind(cid)
            .execute(pool)
            .await;
    }
    for tid in thread_ids {
        let _ = sqlx::query("DELETE FROM events WHERE aggregate_id = $1::text")
            .bind(tid)
            .execute(pool)
            .await;
        let _ = sqlx::query("DELETE FROM thread_summaries WHERE thread_id = $1")
            .bind(tid)
            .execute(pool)
            .await;
    }
}

/// Full lifecycle (sans live CC subprocess): seed an app thread + worktree,
/// emit ChangeProposed, POST Apply, assert ff-merge into workspace main AND
/// `AppUiRefreshRequested` emitted on the app aggregate when an iframe-bundled
/// file changed.
#[tokio::test]
async fn app_coding_agent_lifecycle() {
    let client = http_client();
    let ws = workspace_path();
    let repo_root = ws.to_str().unwrap().to_string();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect e2e DB");

    let suffix = Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let app_id = format!("e2e-app-{}", suffix);
    let app_dir = ensure_app_committed(&app_id, &suffix);

    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    let branch = format!("claude-code/app/{}/{}-{}", app_id, suffix, change_id.as_simple());

    seed_app_cc_thread_summary(&pool, thread_id, &app_id, "idle").await;

    let wt = create_app_worktree(&app_id, &branch);
    // Modify an iframe-bundled file so the apply path is expected to emit
    // AppUiRefreshRequested (per any_iframe_bundled_file_changed: HTML/CSS/JS
    // + manifest under the app folder).
    let edited_rel = format!("data/apps/{}/index.html", app_id);
    let edited_abs = wt.join(&edited_rel);
    std::fs::write(&edited_abs, format!("<!doctype html><title>edited {}</title>", suffix))
        .expect("write edited file");
    git_in(&wt, &["add", &edited_rel]);
    git_in(&wt, &["commit", "-m", "e2e app coding-agent edit"]);

    // Subscribe to SSE BEFORE seeding the change so we don't miss the
    // transient `AppUiRefreshRequested` (it's not persisted to the events
    // table — see `SystemEvent::is_persisted`). The handle resolves to the
    // matching SSE line, or None if the 10s window elapses.
    let app_id_for_predicate = app_id.clone();
    let sse_handle = await_sse_line(
        move |line| {
            line.contains("\"type\":\"AppUiRefreshRequested\"")
                && line.contains(&format!("\"app_id\":\"{}\"", app_id_for_predicate))
        },
        Duration::from_secs(10),
    )
    .await;

    // No head-start sleep needed: `await_sse_line` only returns once the SSE
    // response headers have arrived, and the engine subscribes to the event
    // broadcast before sending them — so the subscription is already live.

    seed_change(&client, change_id, thread_id, &branch, &repo_root, &[&edited_rel]).await;

    let apply_url = format!("{}/api/v1/changes/{}/apply", base_url(), change_id);
    let resp = client.post(&apply_url).send().await.expect("apply");
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.expect("apply JSON");
    assert_eq!(status, 200, "apply should succeed: {:?}", body);
    assert_eq!(body["status"], "applied", "expected applied: {:?}", body);
    // App apply never restarts — explicit gate per design §4.3, not
    // path-pattern coincidence.
    assert_eq!(
        body["restart_required"], false,
        "app apply must never request restart: {:?}",
        body
    );

    // Workspace main now carries the edit.
    let head_sha = String::from_utf8(git(&["rev-parse", "main"]).stdout)
        .expect("utf8")
        .trim()
        .to_string();
    let on_main = std::process::Command::new("git")
        .args(["show", &format!("{}:{}", head_sha, edited_rel)])
        .current_dir(&ws)
        .output()
        .expect("git show");
    let blob = String::from_utf8_lossy(&on_main.stdout).into_owned();
    assert!(
        blob.contains(&format!("edited {}", suffix)),
        "main HEAD should carry the edit, got: {:?}",
        blob
    );

    // AppUiRefreshRequested fires once an iframe-bundled file lands.
    // Transient SSE event (`is_persisted` returns false) carrying the
    // merged app's id.
    let sse_line = sse_handle
        .await
        .expect("SSE task panicked")
        .unwrap_or_else(|| {
            panic!(
                "Timed out waiting for AppUiRefreshRequested SSE event for app {}",
                app_id
            )
        });
    assert!(
        sse_line.contains(&format!("\"app_id\":\"{}\"", app_id)),
        "AppUiRefreshRequested should name the merged app: {}",
        sse_line
    );

    // Clean up — leaving rows would skew later runs.
    cleanup(&pool, &[thread_id], &[change_id], &[&branch], &[wt]).await;
    // Intentionally leave `app_dir` on disk + committed. Removing it would
    // briefly dirty the workspace tree between `rm -rf` and the cleanup
    // commit, and any parallel `changes_test` apply landing in that window
    // gets "Cannot merge: uncommitted changes" from `auto_commit_safe_files_if_dirty`
    // (the engine's safety check). The seed file is tiny and uniquely
    // named per UUID suffix, so leftovers don't conflict with future runs.
    let _ = &app_dir;

    pool.close().await;
}

/// Two app coding-agent threads on the same app, both Apply concurrently:
/// `MERGE_MUTEX` serialises the merges, and both land cleanly via the
/// rebase-catchup path (the second branch fast-forwards over the first's
/// commit). Mirrors the design's risk-list item 9 (concurrent CC on Lucidos
/// source already works; app threads reuse the same mutex).
#[tokio::test]
async fn app_coding_agent_concurrent_apply() {
    let client = http_client();
    let ws = workspace_path();
    let repo_root = ws.to_str().unwrap().to_string();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect e2e DB");

    let suffix = Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let app_id = format!("e2e-app-conc-{}", suffix);
    let app_dir = ensure_app_committed(&app_id, &suffix);

    // Two threads, two worktrees, two branches — each edits a different file
    // so the rebase is trivially clean (the test's job is to prove
    // serialisation + idempotency, not conflict resolution).
    let tid_a = Uuid::new_v4();
    let tid_b = Uuid::new_v4();
    let cid_a = Uuid::new_v4();
    let cid_b = Uuid::new_v4();
    let branch_a = format!("claude-code/app/{}/A-{}", app_id, cid_a.as_simple());
    let branch_b = format!("claude-code/app/{}/B-{}", app_id, cid_b.as_simple());

    seed_app_cc_thread_summary(&pool, tid_a, &app_id, "idle").await;
    seed_app_cc_thread_summary(&pool, tid_b, &app_id, "idle").await;

    let wt_a = create_app_worktree(&app_id, &branch_a);
    let wt_b = create_app_worktree(&app_id, &branch_b);

    let edit_a = format!("data/apps/{}/style-a.css", app_id);
    let edit_b = format!("data/apps/{}/style-b.css", app_id);
    std::fs::write(wt_a.join(&edit_a), "/* A */").unwrap();
    std::fs::write(wt_b.join(&edit_b), "/* B */").unwrap();
    git_in(&wt_a, &["add", &edit_a]);
    git_in(&wt_a, &["commit", "-m", "e2e conc apply A"]);
    git_in(&wt_b, &["add", &edit_b]);
    git_in(&wt_b, &["commit", "-m", "e2e conc apply B"]);

    seed_change(&client, cid_a, tid_a, &branch_a, &repo_root, &[&edit_a]).await;
    seed_change(&client, cid_b, tid_b, &branch_b, &repo_root, &[&edit_b]).await;

    let url_a = format!("{}/api/v1/changes/{}/apply", base_url(), cid_a);
    let url_b = format!("{}/api/v1/changes/{}/apply", base_url(), cid_b);

    // Fire concurrently. MERGE_MUTEX inside change_ops serialises the actual
    // ff-merge, but both requests are alive in the engine at the same time.
    let req_a = client.post(&url_a).send();
    let req_b = client.post(&url_b).send();
    let (resp_a, resp_b) = tokio::join!(req_a, req_b);
    let body_a: serde_json::Value = resp_a.expect("apply A").json().await.expect("JSON A");
    let body_b: serde_json::Value = resp_b.expect("apply B").json().await.expect("JSON B");

    // Both must land. The plan's risk-list #9 is that the rebase-catchup
    // retry handles the second one — assert both reach status=applied.
    assert_eq!(body_a["status"], "applied", "A: {:?}", body_a);
    assert_eq!(body_b["status"], "applied", "B: {:?}", body_b);

    // Both files end up in workspace main.
    for f in [&edit_a, &edit_b] {
        let out = std::process::Command::new("git")
            .args(["cat-file", "-e", &format!("main:{}", f)])
            .current_dir(&ws)
            .status()
            .expect("git cat-file");
        assert!(out.success(), "{} should exist on main after both applies", f);
    }

    cleanup(
        &pool,
        &[tid_a, tid_b],
        &[cid_a, cid_b],
        &[&branch_a, &branch_b],
        &[wt_a, wt_b],
    )
    .await;
    // Intentionally leave `app_dir` on disk + committed — see the matching
    // comment in `app_coding_agent_lifecycle`. The added style-a.css /
    // style-b.css files are also committed to main by Apply, so the
    // workspace stays clean.
    let _ = &app_dir;

    pool.close().await;
}
