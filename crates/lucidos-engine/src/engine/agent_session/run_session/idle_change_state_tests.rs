use super::*;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::git_ops::git_cmd;
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
use crate::test_support::{setup_test_db, teardown_test_db};

const TRACKED: &str = "lucidos-claude-code-repo-example-repo-implement-the-ticket";

/// A temp git repo with one commit on `main`, standing in for both the
/// external repository and the session's worktree (the resolver only ever
/// reads, so one directory serves as both).
async fn make_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    let _ = git_cmd(&["init"], &repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], &repo).await;
    let _ = git_cmd(&["config", "user.email", "test@test.test"], &repo).await;
    let _ = git_cmd(&["config", "user.name", "test"], &repo).await;
    tokio::fs::write(repo.join("a.txt"), "first").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "initial"], &repo).await;
    (tmp, repo)
}

async fn head_sha(repo: &std::path::Path) -> String {
    let out = git_cmd(&["rev-parse", "HEAD"], repo).await.unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Put the repo on the engine-named branch and commit a `.rs` file, the way a
/// coding-agent turn would. Returns the anchor SHA the session started from.
async fn run_a_turn_on_the_tracked_branch(repo: &std::path::Path) -> String {
    let anchor = head_sha(repo).await;
    let _ = git_cmd(&["checkout", "-b", TRACKED], repo).await;
    tokio::fs::write(repo.join("feature.rs"), "fn work() {}")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "agent work"], repo).await;
    anchor
}

/// Create the thread's `thread_summaries` row through the normal projection
/// path, so the test reads the same column production writes.
async fn seed_thread(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "implement the ticket".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

/// What the worktree's post-commit hook does mid-turn: reconcile
/// `coding_agent_has_diff` from the branch the worktree is really on.
async fn hook_recorded_a_diff(pool: &sqlx::PgPool, thread_id: Uuid) {
    sqlx::query("UPDATE thread_summaries SET coding_agent_has_diff = TRUE WHERE thread_id = $1")
        .bind(thread_id)
        .execute(pool)
        .await
        .unwrap();
}

fn input<'a>(
    pool: &'a sqlx::PgPool,
    thread_id: Uuid,
    repo: &'a std::path::Path,
    anchor: Option<&'a str>,
) -> IdleChangeStateInput<'a> {
    IdleChangeStateInput {
        pool,
        thread_id,
        repo_root: repo,
        worktree_path: Some(repo),
        tracked_branch: TRACKED,
        is_external_repo: true,
        anchor_sha: anchor,
    }
}

/// The reported bug, end to end: a repo skill renames the branch mid-session,
/// so the tracked ref is gone by the time the turn idles. The idle must still
/// report the diff, or the Diff button never lights.
#[tokio::test]
async fn renamed_branch_idle_still_reports_the_diff() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;

    let (_tmp, repo) = make_repo().await;
    let anchor = run_a_turn_on_the_tracked_branch(&repo).await;
    let _ = git_cmd(
        &["branch", "-m", TRACKED, "ticket-1234-drop-unused-tables"],
        &repo,
    )
    .await;

    let state = resolve_idle_change_state(input(&pool, thread_id, &repo, Some(&anchor))).await;

    assert_eq!(
        state.branch_name, "ticket-1234-drop-unused-tables",
        "the idle must follow the worktree onto the renamed branch"
    );
    assert!(
        state.has_changes,
        "the branch carries a real diff, so the Diff button must light"
    );
    assert!(state.requires_restart, "the diff contains a .rs file");
    assert_eq!(
        state.changed_files.as_deref(),
        Some(&["feature.rs".to_string()][..]),
        "the file list must come from the branch the work is actually on"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The second defect on its own: git cannot answer at all (here, the whole
/// repo is gone). That is UNKNOWN, and must not walk a thread the post-commit
/// hook already marked as having a diff back to "no changes".
#[tokio::test]
async fn unanswerable_probe_does_not_downgrade_a_thread_with_a_diff() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    hook_recorded_a_diff(&pool, thread_id).await;

    let missing = std::path::PathBuf::from("/nonexistent/repo-for-idle-probe");
    let state = resolve_idle_change_state(IdleChangeStateInput {
        pool: &pool,
        thread_id,
        repo_root: &missing,
        worktree_path: Some(&missing),
        tracked_branch: TRACKED,
        is_external_repo: true,
        anchor_sha: None,
    })
    .await;

    assert!(
        state.has_changes,
        "git could not answer, so the thread keeps the state the hook recorded"
    );
    assert!(
        state.changed_files.is_none(),
        "an unanswerable probe must be reported as unknown, never as an empty file list"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// An ANSWERED probe that finds nothing still clears the flag: a commit and a
/// revert genuinely leave no diff, and carrying `true` forward there is the
/// phantom-Apply card.
#[tokio::test]
async fn answered_empty_probe_clears_a_previously_recorded_diff() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;
    hook_recorded_a_diff(&pool, thread_id).await;

    let (_tmp, repo) = make_repo().await;
    let anchor = run_a_turn_on_the_tracked_branch(&repo).await;
    let _ = git_cmd(&["revert", "--no-edit", "HEAD"], &repo).await;

    let state = resolve_idle_change_state(input(&pool, thread_id, &repo, Some(&anchor))).await;

    assert!(
        !state.has_changes,
        "the commits cancelled out, so the answered-empty diff must clear the flag"
    );
    assert_eq!(state.changed_files.as_deref(), Some(&[][..]));

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A Lucidos-source thread keeps its engine-named branch even when the
/// worktree wandered: Apply depends on that name.
#[tokio::test]
async fn lucidos_source_thread_never_adopts() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    seed_thread(&bus, thread_id).await;

    let (_tmp, repo) = make_repo().await;
    let anchor = run_a_turn_on_the_tracked_branch(&repo).await;
    let _ = git_cmd(&["branch", "-m", TRACKED, "renamed-anyway"], &repo).await;

    let state = resolve_idle_change_state(IdleChangeStateInput {
        is_external_repo: false,
        ..input(&pool, thread_id, &repo, Some(&anchor))
    })
    .await;

    assert_eq!(
        state.branch_name, TRACKED,
        "a Lucidos-source thread must keep the engine-named branch"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
