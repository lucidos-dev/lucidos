use crate::support::{
    as_stamped_device, base_url, db_url, git, git_in, http_client, seed_app_cc_thread_summary,
    user_client, workspace_path, E2E_DEVICE_ID,
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

async fn ensure_app_committed(app_id: &str, marker: &str) -> PathBuf {
    let _tree = crate::support::workspace_tree_lock().write().await;
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

fn create_app_worktree(app_id: &str, branch: &str) -> PathBuf {
    let ws = workspace_path();
    let wt = ws
        .join(".lucidos/worktrees")
        .join(format!("e2e-app-{}", branch.replace('/', "-")));
    if wt.exists() {
        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force", wt.to_str().unwrap()])
            .current_dir(&ws)
            .output();
        let _ = std::fs::remove_dir_all(&wt);
    }
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
    git_in(
        &wt,
        &["sparse-checkout", "set", &format!("data/apps/{}", app_id)],
    );
    git_in(&wt, &["checkout", branch]);
    wt
}

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
        let byte_stream = resp
            .bytes_stream()
            .map(|r| r.map_err(std::io::Error::other));
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

#[tokio::test]
async fn app_coding_agent_lifecycle() {
    let client = user_client().await;
    let ws = workspace_path();
    let repo_root = ws.to_str().unwrap().to_string();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect e2e DB");

    let suffix = Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let app_id = format!("e2e-app-{}", suffix);
    let app_dir = ensure_app_committed(&app_id, &suffix).await;

    let thread_id = Uuid::new_v4();
    let change_id = Uuid::new_v4();
    let branch = format!(
        "claude-code/app/{}/{}-{}",
        app_id,
        suffix,
        change_id.as_simple()
    );

    seed_app_cc_thread_summary(&pool, thread_id, &app_id, "idle").await;

    let wt = create_app_worktree(&app_id, &branch);
    let edited_rel = format!("data/apps/{}/index.html", app_id);
    let edited_abs = wt.join(&edited_rel);
    std::fs::write(
        &edited_abs,
        format!("<!doctype html><title>edited {}</title>", suffix),
    )
    .expect("write edited file");
    git_in(&wt, &["add", &edited_rel]);
    git_in(&wt, &["commit", "-m", "e2e app coding-agent edit"]);

    let app_id_for_predicate = app_id.clone();
    let sse_handle = await_sse_line(
        move |line| {
            line.contains("\"type\":\"AppUiRefreshRequested\"")
                && line.contains(&format!("\"app_id\":\"{}\"", app_id_for_predicate))
        },
        Duration::from_secs(60),
    )
    .await;

    seed_change(
        &client,
        change_id,
        thread_id,
        &branch,
        &repo_root,
        &[&edited_rel],
    )
    .await;

    let apply_url = format!("{}/api/v1/changes/{}/apply", base_url(), change_id);
    let tree = crate::support::workspace_tree_lock().write().await;
    let resp = as_stamped_device(client.post(&apply_url), E2E_DEVICE_ID)
        .send()
        .await
        .expect("apply");
    drop(tree);
    let status = resp.status().as_u16();
    let body: serde_json::Value = resp.json().await.expect("apply JSON");
    assert_eq!(status, 200, "apply should succeed: {:?}", body);
    assert_eq!(body["status"], "applied", "expected applied: {:?}", body);
    assert_eq!(
        body["restart_required"], false,
        "app apply must never request restart: {:?}",
        body
    );

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

    cleanup(&pool, &[thread_id], &[change_id], &[&branch], &[wt]).await;
    let _ = &app_dir;

    pool.close().await;
}

#[tokio::test]
async fn app_coding_agent_concurrent_apply() {
    let client = user_client().await;
    let ws = workspace_path();
    let repo_root = ws.to_str().unwrap().to_string();
    let pool = sqlx::PgPool::connect(&db_url())
        .await
        .expect("connect e2e DB");

    let suffix = Uuid::new_v4().as_simple().to_string()[..8].to_string();
    let app_id = format!("e2e-app-conc-{}", suffix);
    let app_dir = ensure_app_committed(&app_id, &suffix).await;

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

    let _tree = crate::support::workspace_tree_lock().write().await;

    let req_a = as_stamped_device(client.post(&url_a), E2E_DEVICE_ID).send();
    let req_b = as_stamped_device(client.post(&url_b), E2E_DEVICE_ID).send();
    let (resp_a, resp_b) = tokio::join!(req_a, req_b);
    let body_a: serde_json::Value = resp_a.expect("apply A").json().await.expect("JSON A");
    let body_b: serde_json::Value = resp_b.expect("apply B").json().await.expect("JSON B");

    assert_eq!(body_a["status"], "applied", "A: {:?}", body_a);
    assert_eq!(body_b["status"], "applied", "B: {:?}", body_b);

    for f in [&edit_a, &edit_b] {
        let out = std::process::Command::new("git")
            .args(["cat-file", "-e", &format!("main:{}", f)])
            .current_dir(&ws)
            .status()
            .expect("git cat-file");
        assert!(
            out.success(),
            "{} should exist on main after both applies",
            f
        );
    }

    cleanup(
        &pool,
        &[tid_a, tid_b],
        &[cid_a, cid_b],
        &[&branch_a, &branch_b],
        &[wt_a, wt_b],
    )
    .await;
    let _ = &app_dir;

    pool.close().await;
}
