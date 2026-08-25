use super::*;

/// Parity with the frontend's `appIdFromFolder` helper. The two derivations
/// MUST agree on what counts as a valid app folder — divergence would
/// silently scope the backend diff to a phantom path while the frontend
/// renders nothing.
#[test]
fn extract_app_id_matches_frontend_validation() {
    assert_eq!(
        super::extract_app_id("/Users/me/ws/data/apps/habit-tracker"),
        Some("habit-tracker".to_string())
    );
    assert_eq!(
        super::extract_app_id("/ws/data/apps/habit-tracker/"),
        Some("habit-tracker".to_string())
    );
    assert_eq!(
        super::extract_app_id("data/apps/habit-tracker"),
        Some("habit-tracker".to_string())
    );

    // Refusals: same shapes the TS helper rejects.
    assert_eq!(super::extract_app_id("/ws/data/artifacts/foo"), None);
    assert_eq!(super::extract_app_id("/ws/data/apps"), None);
    assert_eq!(super::extract_app_id("/ws/data/apps/"), None);
    assert_eq!(super::extract_app_id("/ws/data/apps/."), None);
    assert_eq!(super::extract_app_id("/ws/data/apps/.."), None);
    assert_eq!(super::extract_app_id("/ws/apps/foo"), None);
    assert_eq!(super::extract_app_id("/ws/projects/foo"), None);
    assert_eq!(super::extract_app_id(""), None);

    // Nested: last data/apps/ wins, same as the TS helper.
    assert_eq!(
        super::extract_app_id("/ws/data/apps/outer/data/apps/inner"),
        Some("inner".to_string())
    );
}

#[test]
fn browse_rejects_path_traversal() {
    assert!(super::is_dangerous_browse_path("../etc"));
    assert!(super::is_dangerous_browse_path("/foo/../bar"));
    assert!(super::is_dangerous_browse_path(""));
    assert!(!super::is_dangerous_browse_path("/Users/me/projects"));
    assert!(!super::is_dangerous_browse_path("/tmp"));
}

// Tilde expansion moved to `core::home_path` (shared with the
// `manage_repositories` tool and coding-agent `folder` resolution) and is
// covered by `core::home_path::tests`.

/// End-to-end of the worktree-diff helpers against a real git repo +
/// linked worktree on a feature branch with one extra commit. Asserts
/// the base ref resolves to a usable target and the diff range produces
/// the expected single-file change.
#[tokio::test]
async fn worktree_diff_helpers_against_real_repo() {
    async fn run(args: &[&str], cwd: &std::path::Path) -> std::process::Output {
        tokio::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .unwrap()
    }

    let main_repo = tempfile::tempdir().unwrap();
    let main_path = main_repo.path();

    // `-c init.defaultBranch=main` makes the test pass on hosts where the
    // user's git defaults differ (master vs main). Without it,
    // `git init` creates `master` and our subsequent `worktree add ... main`
    // fails with "invalid reference: main".
    run(&["-c", "init.defaultBranch=main", "init", "-q"], main_path).await;
    run(&["config", "user.email", "test@example.com"], main_path).await;
    run(&["config", "user.name", "Test"], main_path).await;
    tokio::fs::write(main_path.join("README.md"), "init\n")
        .await
        .unwrap();
    run(&["add", "."], main_path).await;
    run(&["commit", "-q", "-m", "init"], main_path).await;

    // Linked worktree on a new branch, one extra commit.
    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");
    run(
        &[
            "worktree",
            "add",
            "-b",
            "feature/diff-helpers",
            wt_path.to_str().unwrap(),
            "main",
        ],
        main_path,
    )
    .await;
    tokio::fs::write(wt_path.join("README.md"), "init\nadded line\n")
        .await
        .unwrap();
    run(&["add", "."], &wt_path).await;
    run(&["commit", "-q", "-m", "edit readme"], &wt_path).await;

    // Base ref: no `origin` remote, so origin/HEAD lookup fails — the
    // helper must fall back to local `main`.
    let base = crate::engine::git_ops::default_local_branch(&wt_path).await;
    assert_eq!(
        base, "main",
        "expected fallback to local main when no origin remote is set, got {base}"
    );

    // Repo root: must point back at the main worktree, not the linked one.
    let root = super::resolve_worktree_repo_root(&wt_path)
        .await
        .unwrap()
        .unwrap();
    let canonical_main = main_path.canonicalize().unwrap();
    let canonical_root = std::path::PathBuf::from(&root).canonicalize().unwrap();
    assert_eq!(
        canonical_root, canonical_main,
        "repo root should be the main worktree: got {root}"
    );

    // 3-dot diff against the resolved base: exactly one modified file.
    let range = format!("{}...HEAD", base);
    let out = run(&["diff", &range, "--no-color"], &wt_path).await;
    assert!(out.status.success(), "git diff failed: {:?}", out);
    let files = super::parse_diff_output(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "README.md");
    assert_eq!(files[0].status, "modified");
}

/// The Diff button must base its diff on **local** `main`, not the
/// remote tracking ref `origin/main`. Otherwise, when local `main` is
/// ahead of `origin/main` (e.g. the engine has applied work but the
/// push to origin hasn't landed yet), every commit on local main that
/// isn't on origin shows up in the Diff viewer as if it belonged to the
/// current thread — even though Apply has already merged those commits.
/// This was the symptom users saw as "the Diff button shows lots of
/// code changes when the thread only touched markdown."
#[tokio::test]
async fn diff_base_is_local_main_when_origin_is_behind() {
    async fn run(args: &[&str], cwd: &std::path::Path) -> std::process::Output {
        tokio::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .unwrap()
    }

    let origin_dir = tempfile::tempdir().unwrap();
    let origin_path = origin_dir.path();
    run(
        &["-c", "init.defaultBranch=main", "init", "-q", "--bare"],
        origin_path,
    )
    .await;

    let main_repo = tempfile::tempdir().unwrap();
    let main_path = main_repo.path();
    run(&["-c", "init.defaultBranch=main", "init", "-q"], main_path).await;
    run(&["config", "user.email", "test@example.com"], main_path).await;
    run(&["config", "user.name", "Test"], main_path).await;
    tokio::fs::write(main_path.join("README.md"), "init\n")
        .await
        .unwrap();
    run(&["add", "."], main_path).await;
    run(&["commit", "-q", "-m", "init"], main_path).await;
    run(
        &["remote", "add", "origin", origin_path.to_str().unwrap()],
        main_path,
    )
    .await;
    run(&["push", "-q", "origin", "main"], main_path).await;
    // origin/HEAD must exist for `symbolic-ref refs/remotes/origin/HEAD`
    // to succeed — that path is what the buggy code used to return
    // `origin/main` instead of `main`.
    run(&["remote", "set-head", "origin", "main"], main_path).await;

    tokio::fs::write(main_path.join("already-applied.txt"), "shipped\n")
        .await
        .unwrap();
    run(&["add", "."], main_path).await;
    run(
        &["commit", "-q", "-m", "ship feature (not yet pushed)"],
        main_path,
    )
    .await;

    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");
    run(
        &[
            "worktree",
            "add",
            "-b",
            "cc/feature",
            wt_path.to_str().unwrap(),
            "main",
        ],
        main_path,
    )
    .await;
    tokio::fs::write(wt_path.join("cc-work.txt"), "from CC\n")
        .await
        .unwrap();
    run(&["add", "."], &wt_path).await;
    run(&["commit", "-q", "-m", "cc: add cc-work.txt"], &wt_path).await;

    // The cc-diff response must:
    //   - report `base_ref = "main"` (local), not `"origin/main"`
    //   - list ONLY `cc-work.txt` — the already-applied commit's file
    //     must not appear, because Apply has already merged it.
    let diff = super::diff_via_worktree(&wt_path, None)
        .await
        .expect("diff_via_worktree should succeed");

    assert_eq!(
        diff.base_ref, "main",
        "expected local `main` as base, got `{}` — Diff viewer would \
         show stale already-applied commits as if they were new",
        diff.base_ref
    );

    let paths: Vec<&str> = diff.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["cc-work.txt"],
        "expected only the CC worktree's own file, got {:?} — the \
         already-applied commit's file leaked into the diff because \
         the base was the stale `origin/main`",
        paths
    );
}

/// The mirror of the test above: when the local default branch has been
/// *rewritten* so the remote-tracking ref no longer reaches it — a repo
/// migration, a force-pull, or a rebase landing on the user's local `main` —
/// the Diff button must fall back to `origin/<default>`, which still holds the
/// branch's true fork point. Otherwise the three-dot merge-base collapses to an
/// ancient common ancestor and the diff balloons with unrelated churn.
///
/// Regression for the `example-repo` migration report: the migration system
/// committed to the user's local `main` ("secrets transfer", "migration notice"
/// commits) and rewrote its history so the PR branch's fork point was no longer
/// an ancestor of local `main`. Lucidos diffed against local `main` and showed
/// 59 files; the GitHub PR (diffed against the untouched remote base) showed 12.
#[tokio::test]
async fn diff_base_falls_back_to_origin_when_local_main_diverged() {
    async fn run(args: &[&str], cwd: &std::path::Path) -> std::process::Output {
        tokio::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .unwrap()
    }

    let origin_dir = tempfile::tempdir().unwrap();
    let origin_path = origin_dir.path();
    run(
        &["-c", "init.defaultBranch=main", "init", "-q", "--bare"],
        origin_path,
    )
    .await;

    let main_repo = tempfile::tempdir().unwrap();
    let main_path = main_repo.path();
    run(&["-c", "init.defaultBranch=main", "init", "-q"], main_path).await;
    run(&["config", "user.email", "test@example.com"], main_path).await;
    run(&["config", "user.name", "Test"], main_path).await;

    // Root commit, then the fork point the PR branch builds on top of.
    tokio::fs::write(main_path.join("README.md"), "init\n")
        .await
        .unwrap();
    run(&["add", "."], main_path).await;
    run(&["commit", "-q", "-m", "init"], main_path).await;
    let c_root = String::from_utf8_lossy(&run(&["rev-parse", "HEAD"], main_path).await.stdout)
        .trim()
        .to_string();

    tokio::fs::write(main_path.join("fork-point.txt"), "shipped before the PR\n")
        .await
        .unwrap();
    run(&["add", "."], main_path).await;
    run(
        &["commit", "-q", "-m", "feature on main (PR forks here)"],
        main_path,
    )
    .await;

    // Publish to origin and record origin/HEAD → main. `origin/main` now holds
    // the fork point.
    run(
        &["remote", "add", "origin", origin_path.to_str().unwrap()],
        main_path,
    )
    .await;
    run(&["push", "-q", "origin", "main"], main_path).await;
    run(&["remote", "set-head", "origin", "main"], main_path).await;

    // CC worktree forks from the fork point, adds its own file.
    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");
    run(
        &[
            "worktree",
            "add",
            "-b",
            "cc/feature",
            wt_path.to_str().unwrap(),
            "main",
        ],
        main_path,
    )
    .await;
    tokio::fs::write(wt_path.join("cc-work.txt"), "from CC\n")
        .await
        .unwrap();
    run(&["add", "."], &wt_path).await;
    run(&["commit", "-q", "-m", "cc: add cc-work.txt"], &wt_path).await;

    // A migration tool rewrites local `main`: reset back to the root and commit
    // a migration notice. `origin/main` (the fork point) is no longer an
    // ancestor of local `main` — they diverge at the root.
    run(&["reset", "-q", "--hard", &c_root], main_path).await;
    tokio::fs::write(main_path.join("MIGRATION.md"), "moved to new-org\n")
        .await
        .unwrap();
    run(&["add", "."], main_path).await;
    run(
        &[
            "commit",
            "-q",
            "-m",
            "migration: secrets transfer (automated)",
        ],
        main_path,
    )
    .await;

    let diff = super::diff_via_worktree(&wt_path, None)
        .await
        .expect("diff_via_worktree should succeed");

    assert_eq!(
        diff.base_ref, "origin/main",
        "local `main` diverged from the remote — base must fall back to \
         `origin/main`, got `{}`",
        diff.base_ref,
    );

    let paths: Vec<&str> = diff.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["cc-work.txt"],
        "only the CC branch's own file belongs in the diff; the fork-point \
         file leaked because the base used the rewritten local `main`: {:?}",
        paths,
    );
}

/// App coding-agent threads operate on `data/apps/<id>/` inside the
/// workspace git. The cc-diff response must scope to that pathspec so
/// the Diff button doesn't surface stray edits the agent made outside
/// its scope (the user picked "diff scoped to the app folder" — files
/// in `data/artifacts/`, `data/knowhow/`, etc. must not appear).
#[tokio::test]
async fn diff_via_worktree_scopes_to_app_pathspec() {
    async fn run(args: &[&str], cwd: &std::path::Path) -> std::process::Output {
        tokio::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .unwrap()
    }

    // Simulate a workspace as a real git repo with main committed.
    let ws_dir = tempfile::tempdir().unwrap();
    let ws_path = ws_dir.path();
    run(&["-c", "init.defaultBranch=main", "init", "-q"], ws_path).await;
    run(&["config", "user.email", "test@example.com"], ws_path).await;
    run(&["config", "user.name", "Test"], ws_path).await;
    tokio::fs::write(ws_path.join("README.md"), "ws\n")
        .await
        .unwrap();
    run(&["add", "."], ws_path).await;
    run(&["commit", "-q", "-m", "init"], ws_path).await;

    // App CC worktree on a branch — touches the app folder AND files
    // outside scope. We expect only the in-scope file in the response.
    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");
    run(
        &[
            "worktree",
            "add",
            "-b",
            "apps/habit-tracker",
            wt_path.to_str().unwrap(),
            "main",
        ],
        ws_path,
    )
    .await;

    let app_dir = wt_path.join("data/apps/habit-tracker");
    tokio::fs::create_dir_all(&app_dir).await.unwrap();
    tokio::fs::write(app_dir.join("manifest.json"), "{}\n")
        .await
        .unwrap();

    let artifacts_dir = wt_path.join("data/artifacts/habit-tracker");
    tokio::fs::create_dir_all(&artifacts_dir).await.unwrap();
    tokio::fs::write(artifacts_dir.join("notes.md"), "out of scope\n")
        .await
        .unwrap();

    run(&["add", "."], &wt_path).await;
    run(&["commit", "-q", "-m", "app + stray artifacts"], &wt_path).await;

    let diff = super::diff_via_worktree(&wt_path, Some("data/apps/habit-tracker"))
        .await
        .expect("diff_via_worktree should succeed with app pathspec");

    let paths: Vec<&str> = diff.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["data/apps/habit-tracker/manifest.json"],
        "expected only in-scope app file, got {:?} — out-of-scope edits \
         leaked into the response",
        paths
    );

    // Sanity: without the pathspec, the out-of-scope file is visible.
    // Asserts the scoping is what's filtering it out, not test setup.
    let unscoped = super::diff_via_worktree(&wt_path, None)
        .await
        .expect("diff_via_worktree should succeed without pathspec");
    let unscoped_paths: Vec<&str> = unscoped.files.iter().map(|f| f.path.as_str()).collect();
    assert!(
        unscoped_paths.contains(&"data/artifacts/habit-tracker/notes.md"),
        "control: unscoped diff should include the out-of-scope file, \
         got {:?}",
        unscoped_paths
    );
}

// -------------------- resolve_recorded_branch --------------------

/// A repo on `main` with one commit, plus a coding-agent branch carrying one
/// more. Returns the repo path and the SHA of the branch's work.
async fn repo_with_agent_branch(tracked: &str) -> (tempfile::TempDir, std::path::PathBuf, String) {
    use crate::engine::git_ops::git_cmd;
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    let _ = git_cmd(&["init"], &repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], &repo).await;
    let _ = git_cmd(&["config", "user.email", "test@test.test"], &repo).await;
    let _ = git_cmd(&["config", "user.name", "test"], &repo).await;
    tokio::fs::write(repo.join("a.txt"), "first").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "initial"], &repo).await;
    let _ = git_cmd(&["checkout", "-b", tracked], &repo).await;
    tokio::fs::write(repo.join("work.txt"), "agent work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "agent work"], &repo).await;
    let out = git_cmd(&["rev-parse", "HEAD"], &repo).await.unwrap();
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let _ = git_cmd(&["checkout", "main"], &repo).await;
    (tmp, repo, sha)
}

fn cc_meta() -> crate::engine::thread_events::EventMeta {
    crate::engine::thread_events::EventMeta {
        channel: Some(crate::engine::thread_events::EventChannel::ClaudeCode),
        ..crate::engine::thread_events::EventMeta::NONE
    }
}

/// Open a coding-agent thread the way a real first turn does: the user's
/// message, then the spawn's `SessionStarted`. Both are load-bearing. The
/// lifecycle rejects `CodingAgentIdled` on a thread it has not classified as
/// CC, and `SessionStarted` is the only event that sets
/// `thread_summaries.is_coding_agent`, which
/// `thread_owns_a_coding_agent_worktree` reads.
async fn seed_cc_thread(bus: &crate::engine::event_bus::EventBus, thread_id: Uuid) {
    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: crate::engine::thread_events::ThreadEvent::MessageReceived {
            text: "implement the ticket".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: crate::engine::thread_events::ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: cc_meta(),
    })
    .await
    .unwrap();
    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: crate::engine::thread_events::ThreadEvent::SessionStarted {
            session_id: String::new(),
            branch: "lucidos-claude-code-repo-example-repo-implement-the-ticket".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        },
        meta: cc_meta(),
    })
    .await
    .unwrap();
}

/// Record a turn boundary the way a real idle does.
async fn seed_idle(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    head_sha: Option<&str>,
    worktree_path: Option<&str>,
) {
    seed_cc_thread(bus, thread_id).await;
    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: true,
            requires_restart: false,
            cc_session_id: Some("sess-1".into()),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: worktree_path.map(str::to_string),
            worktree_head_sha: head_sha.map(str::to_string),
            bg_bash_pending: false,
        },
        meta: cc_meta(),
    })
    .await
    .unwrap();
}

/// The reported bug at the API layer: the worktree has been reclaimed, so the
/// Diff falls back to the branch recorded at spawn, which a rename inside the
/// repo has since retired. It must find the work rather than 400 on a dead ref.
#[tokio::test]
async fn resolve_recorded_branch_follows_a_rename_once_the_worktree_is_gone() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let tracked = "lucidos-claude-code-repo-example-repo-implement-the-ticket";
    let (_tmp, repo, work_sha) = repo_with_agent_branch(tracked).await;
    seed_idle(&bus, thread_id, Some(&work_sha), None).await;
    let _ = crate::engine::git_ops::git_cmd(
        &["branch", "-m", tracked, "ticket-1234-drop-unused-tables"],
        &repo,
    )
    .await;

    let resolved =
        super::resolve_recorded_branch(&pool, thread_id, &repo, tracked.to_string(), "main")
            .await
            .expect("the work is still on the renamed branch");
    assert_eq!(resolved, "ticket-1234-drop-unused-tables");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn resolve_recorded_branch_keeps_a_branch_that_still_exists() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let thread_id = Uuid::new_v4();
    let tracked = "lucidos-claude-code-repo-example-repo-implement-the-ticket";
    let (_tmp, repo, _sha) = repo_with_agent_branch(tracked).await;

    let resolved =
        super::resolve_recorded_branch(&pool, thread_id, &repo, tracked.to_string(), "main")
            .await
            .expect("an existing branch needs no searching");
    assert_eq!(resolved, tracked);

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

/// Deleted rather than renamed: there is nothing to show, and the caller gets a
/// message naming the branch instead of git's raw "ambiguous argument".
#[tokio::test]
async fn resolve_recorded_branch_reports_a_deleted_branch_by_name() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let tracked = "lucidos-claude-code-repo-example-repo-implement-the-ticket";
    let (_tmp, repo, work_sha) = repo_with_agent_branch(tracked).await;
    seed_idle(&bus, thread_id, Some(&work_sha), None).await;
    let _ = crate::engine::git_ops::git_cmd(&["branch", "-D", tracked], &repo).await;

    let (status, message) =
        super::resolve_recorded_branch(&pool, thread_id, &repo, tracked.to_string(), "main")
            .await
            .expect_err("a deleted branch has no diff to show");
    assert_eq!(status, axum::http::StatusCode::NOT_FOUND);
    assert!(
        message.contains(tracked),
        "the error must name the branch it looked for: {message}"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

// -------------------- resolve_diff_worktree --------------------

/// A conflict-resolution session works in the merge worktree, on a temp branch
/// that is not the thread's. Diffing it would answer the Diff button with the
/// merge in progress instead of the thread's own work.
#[test]
fn live_thread_worktree_skips_a_conflict_merge_tree() {
    let own = std::path::PathBuf::from("/tmp/lucidos-test/thread-worktree");
    let merge = std::path::PathBuf::from("/tmp/lucidos-test/merge-worktree");
    let (mut session, _rx) = crate::engine::types::AgentSession::for_test();

    session.worktree_path = Some(own.clone());
    assert_eq!(super::live_thread_worktree(Some(&session)), Some(own));

    session.conflict_change_id = Some(Uuid::new_v4());
    session.worktree_path = Some(merge);
    assert_eq!(
        super::live_thread_worktree(Some(&session)),
        None,
        "the merge worktree belongs to the merge, not to the thread"
    );

    assert_eq!(super::live_thread_worktree(None), None);
}

/// The reported bug: an app coding-agent thread commits mid-turn, the Diff
/// button lights up, and the click answers 404. No `CodingAgentIdled` has been
/// emitted yet on a first turn, and an app thread records no
/// `SessionStarted.repo_id` for the branch-ref fallback to use. The worktree is
/// sitting at its deterministic path the whole time.
#[tokio::test]
async fn resolve_diff_worktree_finds_the_deterministic_path_before_the_first_idle() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let ws = tempfile::tempdir().unwrap();
    let expected =
        crate::engine::agent_session::resume::deterministic_worktree_path(ws.path(), thread_id);

    // Nothing on disk yet: the caller must fall through to the branch ref.
    assert_eq!(
        super::resolve_diff_worktree(&pool, thread_id, None, ws.path()).await,
        None,
        "a path that does not exist is not a worktree to diff"
    );

    tokio::fs::create_dir_all(&expected).await.unwrap();
    assert_eq!(
        super::resolve_diff_worktree(&pool, thread_id, None, ws.path()).await,
        Some(expected),
        "the first turn's worktree is findable from the thread id alone"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

/// The deterministic path reads the thread id's first 8 hex characters and
/// nothing else. An id naming no thread must not reach the worktree of a real
/// thread sharing that prefix. Nothing would scope that answer, so it would
/// carry the real thread's whole diff.
#[tokio::test]
async fn resolve_diff_worktree_refuses_a_thread_id_it_does_not_know() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let real = Uuid::new_v4();
    seed_cc_thread(&bus, real).await;
    let ws = tempfile::tempdir().unwrap();
    let worktree =
        crate::engine::agent_session::resume::deterministic_worktree_path(ws.path(), real);
    tokio::fs::create_dir_all(&worktree).await.unwrap();

    // Same 8-char prefix, different uuid, no thread of its own.
    let mut bytes = real.into_bytes();
    bytes[15] ^= 0xff;
    let colliding = Uuid::from_bytes(bytes);
    assert_eq!(
        crate::engine::agent_session::resume::deterministic_worktree_path(ws.path(), colliding),
        worktree,
        "test setup: the two ids must share a deterministic path"
    );

    assert_eq!(
        super::resolve_diff_worktree(&pool, colliding, None, ws.path()).await,
        None,
        "an unknown thread id must not reach a real thread's worktree"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

/// A running turn owns the answer. The last idle describes where the PREVIOUS
/// turn worked, so the live session's own worktree wins whenever both are on
/// disk.
#[tokio::test]
async fn resolve_diff_worktree_prefers_the_running_session_over_the_last_idle() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let ws = tempfile::tempdir().unwrap();

    let idled = ws.path().join("idled-worktree");
    let running = ws.path().join("running-worktree");
    tokio::fs::create_dir_all(&idled).await.unwrap();
    tokio::fs::create_dir_all(&running).await.unwrap();
    seed_idle(&bus, thread_id, None, idled.to_str()).await;

    assert_eq!(
        super::resolve_diff_worktree(&pool, thread_id, Some(running.clone()), ws.path()).await,
        Some(running),
        "the live session's worktree is the tree the agent is writing to"
    );
    assert_eq!(
        super::resolve_diff_worktree(&pool, thread_id, None, ws.path()).await,
        Some(idled),
        "with no session running, the last idle's recorded worktree stands"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}
