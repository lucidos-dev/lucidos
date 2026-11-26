use super::*;

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

    assert_eq!(super::extract_app_id("/ws/data/artifacts/foo"), None);
    assert_eq!(super::extract_app_id("/ws/data/apps"), None);
    assert_eq!(super::extract_app_id("/ws/data/apps/"), None);
    assert_eq!(super::extract_app_id("/ws/data/apps/."), None);
    assert_eq!(super::extract_app_id("/ws/data/apps/.."), None);
    assert_eq!(super::extract_app_id("/ws/apps/foo"), None);
    assert_eq!(super::extract_app_id("/ws/projects/foo"), None);
    assert_eq!(super::extract_app_id(""), None);

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

    run(&["-c", "init.defaultBranch=main", "init", "-q"], main_path).await;
    run(&["config", "user.email", "test@example.com"], main_path).await;
    run(&["config", "user.name", "Test"], main_path).await;
    tokio::fs::write(main_path.join("README.md"), "init\n")
        .await
        .unwrap();
    run(&["add", "."], main_path).await;
    run(&["commit", "-q", "-m", "init"], main_path).await;

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

    let base = crate::engine::git_ops::default_local_branch(&wt_path).await;
    assert_eq!(
        base, "main",
        "expected fallback to local main when no origin remote is set, got {base}"
    );

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

    let range = format!("{}...HEAD", base);
    let out = run(&["diff", &range, "--no-color"], &wt_path).await;
    assert!(out.status.success(), "git diff failed: {:?}", out);
    let files = super::parse_diff_output(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "README.md");
    assert_eq!(files[0].status, "modified");
}

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

    let diff = super::diff_via_worktree(&wt_path, &wt_path, None)
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

    run(
        &["remote", "add", "origin", origin_path.to_str().unwrap()],
        main_path,
    )
    .await;
    run(&["push", "-q", "origin", "main"], main_path).await;
    run(&["remote", "set-head", "origin", "main"], main_path).await;

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

    let diff = super::diff_via_worktree(&wt_path, &wt_path, None)
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

    let diff = super::diff_via_worktree(&wt_path, &wt_path, Some("data/apps/habit-tracker"))
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

    let unscoped = super::diff_via_worktree(&wt_path, &wt_path, None)
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

async fn seed_cc_thread(bus: &crate::engine::event_bus::EventBus, thread_id: Uuid) {
    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: crate::engine::thread_events::ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "implement the ticket".into(),
            user_image_hashes: vec![],
            device_id: None,
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
            sandbox: None,
            session_id: String::new(),
            branch: "lucidos-claude-code-repo-example-repo-implement-the-ticket".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: cc_meta(),
    })
    .await
    .unwrap();
}

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
            turn_end: None,
        },
        meta: cc_meta(),
    })
    .await
    .unwrap();
}

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

#[test]
fn live_thread_worktree_skips_a_conflict_merge_tree() {
    let own = std::path::PathBuf::from("/tmp/lucidos-test/thread-worktree");
    let merge = std::path::PathBuf::from("/tmp/lucidos-test/merge-worktree");
    let (mut session, _rx) = crate::engine::types::AgentSession::for_test();

    session.worktree_path = Some(own.clone());
    assert_eq!(super::live_thread_worktree(Some(&session)), Some(own));

    session.conflict = Some(crate::engine::types::ConflictBinding::Detached {
        change_id: Uuid::new_v4(),
    });
    session.worktree_path = Some(merge);
    assert_eq!(
        super::live_thread_worktree(Some(&session)),
        None,
        "the merge worktree belongs to the merge, not to the thread"
    );

    assert_eq!(super::live_thread_worktree(None), None);
}

#[test]
fn live_thread_worktree_keeps_the_own_tree_after_a_tier_1_binding() {
    let own = std::path::PathBuf::from("/tmp/lucidos-test/thread-worktree");
    let thread_id = Uuid::new_v4();
    let (mut session, _rx) = crate::engine::types::AgentSession::for_test();
    session.worktree_path = Some(own.clone());
    let mut sessions = std::collections::HashMap::from([(thread_id, session)]);

    crate::engine::bind_in_place_conflict_resolution(&mut sessions, thread_id, Uuid::new_v4());

    assert_eq!(
        super::live_thread_worktree(sessions.get(&thread_id)),
        Some(own),
        "an in-place merge runs in the thread's own worktree"
    );
}

#[tokio::test]
async fn resolve_diff_worktree_finds_the_deterministic_path_before_the_first_idle() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_cc_thread(&bus, thread_id).await;
    let ws = tempfile::tempdir().unwrap();
    let expected =
        crate::engine::session_queries::deterministic_worktree_path(ws.path(), thread_id);

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

#[tokio::test]
async fn resolve_diff_worktree_refuses_a_thread_id_it_does_not_know() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
    let real = Uuid::new_v4();
    seed_cc_thread(&bus, real).await;
    let ws = tempfile::tempdir().unwrap();
    let worktree = crate::engine::session_queries::deterministic_worktree_path(ws.path(), real);
    tokio::fs::create_dir_all(&worktree).await.unwrap();

    let mut bytes = real.into_bytes();
    bytes[15] ^= 0xff;
    let colliding = Uuid::from_bytes(bytes);
    assert_eq!(
        crate::engine::session_queries::deterministic_worktree_path(ws.path(), colliding),
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

#[tokio::test]
async fn an_unanswerable_app_pathspec_lookup_fails_the_request() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let app_thread = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, coding_agent_kind, coding_agent_folder) \
         VALUES ($1, 'app', '/ws/data/apps/habit-tracker')",
    )
    .bind(app_thread)
    .execute(&pool)
    .await
    .expect("seed an app coding-agent thread");

    assert_eq!(
        super::lookup_app_pathspec(&pool, app_thread).await,
        Ok(Some("data/apps/habit-tracker".to_string())),
        "an app thread scopes its diff to its own folder"
    );

    let other = Uuid::new_v4();
    sqlx::query("INSERT INTO thread_summaries (thread_id) VALUES ($1)")
        .bind(other)
        .execute(&pool)
        .await
        .expect("seed a plain thread");
    assert_eq!(super::lookup_app_pathspec(&pool, other).await, Ok(None));

    pool.close().await;
    let (status, message) = super::lookup_app_pathspec(&pool, app_thread)
        .await
        .expect_err("a lookup that could not run is not an answer");
    assert_eq!(
        status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "an unscoped diff is worse than no diff: {message}"
    );

    crate::test_support::teardown_test_db(&db_name).await;
}

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

async fn repo_with_files(files: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
    use crate::engine::git_ops::git_cmd;
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    let _ = git_cmd(&["init"], &repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], &repo).await;
    let _ = git_cmd(&["config", "user.email", "test@test.test"], &repo).await;
    let _ = git_cmd(&["config", "user.name", "test"], &repo).await;
    for file in files {
        tokio::fs::write(repo.join(file), "body").await.unwrap();
    }
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "initial"], &repo).await;
    (tmp, repo)
}

fn header_of(response: &Response, name: header::HeaderName) -> Option<String> {
    response
        .headers()
        .get(name)
        .map(|v| v.to_str().unwrap().to_string())
}

#[tokio::test]
async fn git_show_file_sandboxes_documents_that_can_run_script() {
    let active = [
        "page.html",
        "page.htm",
        "page.xhtml",
        "logo.svg",
        "feed.xml",
    ];
    let (_tmp, repo) = repo_with_files(&active).await;

    for path in active {
        let response = super::git_show_file(&repo, "main", path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(
            header_of(&response, header::CONTENT_SECURITY_POLICY).as_deref(),
            Some("sandbox"),
            "{path} must render at an opaque origin with script off"
        );
        assert_eq!(
            header_of(&response, header::X_CONTENT_TYPE_OPTIONS).as_deref(),
            Some("nosniff"),
            "{path}"
        );
    }
}

#[tokio::test]
async fn git_show_file_sends_nosniff_on_every_type() {
    let inert = ["notes.txt", "Makefile", "image.png", "spec.pdf", "main.rs"];
    let (_tmp, repo) = repo_with_files(&inert).await;

    for path in inert {
        let response = super::git_show_file(&repo, "main", path).await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        assert_eq!(
            header_of(&response, header::X_CONTENT_TYPE_OPTIONS).as_deref(),
            Some("nosniff"),
            "{path}"
        );
        assert_eq!(
            header_of(&response, header::CONTENT_SECURITY_POLICY),
            None,
            "{path} renders inline as before, so a PDF preview keeps working"
        );
    }
}

#[tokio::test]
async fn adding_a_repository_announces_it_exactly_once() {
    if !crate::test_support::in_engine_child(
        "api::repositories::tests::adding_a_repository_announces_it_exactly_once",
    )
    .await
    {
        return;
    }
    let (_pool, db) = crate::test_support::setup_test_db().await;
    let (engine, workspace) = crate::test_support::boot_mock_engine(&db).await;
    let state = crate::api::permission_requests::tests::permission_test_state(
        engine,
        workspace.path().to_path_buf(),
    )
    .await;
    let (_tmp, repo) = repo_with_files(&["a.txt"]).await;
    let pool = state.pool.clone();
    let count_added = || async {
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM events WHERE event_type = 'RepositoryAdded'",
        )
        .fetch_one(&pool)
        .await
        .unwrap()
    };
    let before = count_added().await;

    let result = add_repository(
        State(state),
        crate::api::gateway_stamp::test_support::stamped_device_headers("owner-phone"),
        Json(AddRepositoryRequest {
            name: "announced".to_string(),
            path: repo.to_string_lossy().into_owned(),
            description: None,
        }),
    )
    .await;
    match result {
        Ok((status, _)) => assert_eq!(status, StatusCode::CREATED),
        Err((status, message)) => panic!("the add must succeed: {status} {message}"),
    }

    assert_eq!(count_added().await, before + 1);
    crate::test_support::teardown_test_db(&db).await;
}

#[tokio::test]
async fn resolve_recorded_branch_keeps_the_name_when_git_cannot_be_asked() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let gone = std::path::Path::new("/definitely/not/a/directory/on/this/host");

    let resolved = super::resolve_recorded_branch(
        &pool,
        Uuid::new_v4(),
        gone,
        "recorded-branch".to_string(),
        "main",
    )
    .await;
    assert_eq!(resolved, Ok("recorded-branch".to_string()));

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}
