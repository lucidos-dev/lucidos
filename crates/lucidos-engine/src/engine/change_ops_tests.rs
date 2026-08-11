// # macOS test-suite SIGABRT note (read this before blaming a `change_ops` test)
//
// Three earlier Claude Code sessions reported `apply_round_trip_branch_can_advance_again`
// and `discard_resets_even_when_worktree_is_dirty` as the trigger
// for a SIGABRT under parallel `cargo test -p lucidos-engine --lib`.
// They were wrong. Bisection in a fourth session pinned the actual
// cause: the **wasmtime-`Engine`-creating tests** in
// `api::proxy_wasm_*`, `api::proxy_pipeline_builder`, and
// `api::proxy::tests::reload_*`.
//
// The error on stderr was:
//   `mach_msg failed with 268451845 (10004005)`     (= MACH_RCV_INTERRUPTED)
// raised by libdispatch; exit 134 (SIGABRT). Mechanism: every
// `wasmtime::Engine::new()` allocates a JIT memory pool via macOS
// `MAP_JIT` `mmap`, which uses Mach IPC for permission negotiation.
// Many concurrent allocations from concurrent `#[tokio::test]`
// runtimes crossed a per-process kernel threshold and libdispatch
// panicked. The abort surfaced in whichever test happened to be
// running when the threshold was hit — `change_ops` tests are heavy
// git-subprocess users and were *common collateral*, never the cause.
//
// What was tried and ruled out before the right fix landed (each
// verified by patching + a 5-run re-bisection on the failing schedule):
//   - subprocess-concurrency throttle on `git_cmd` (4 and 32 permits)
//   - replacing `tokio::process::Command` with `std::process::Command`
//     wrapped in `spawn_blocking`
//   - throttling `setup_test_db` Postgres pool churn
//   - sharing `wasmtime::Engine` across tests via `OnceLock` (each
//     test still creates its own `Module` and `Store`, both of which
//     also `MAP_JIT`)
//   - per-call `reqwest::Client` instead of `LazyLock`
//   - `axum::serve(..).with_graceful_shutdown(..)` (kept as a
//     separate cleanup; doesn't fix abort)
//   - `MallocNanoZone=0`, `OS_ACTIVITY_MODE=disable`,
//     `MallocCheckHeapEach=1`
//   - `tokio::sync::Semaphore::new(1)` throttle on every
//     `#[tokio::test]` in `api::proxy_*` (proxy tokio tests fully
//     serialize; abort still fires in subsequent non-proxy tests
//     because they're CONCURRENT with proxy tests, not after)
//   - `serial_test::serial(group)` on every proxy `#[tokio::test]`
//     AND every proxy sync `#[test]`
//   - `--test-threads=2..=6` (still aborts; only `=1` passes
//     single-threaded — the standing testing policy forbids it as
//     the primary fix)
//
// The fix that actually worked: move the 13+2 Engine-creating cases
// to `crates/lucidos-engine/tests/proxy_wasm_engine.rs`. Each
// `[[test]]` binary runs in its OWN process, so it gets its own
// per-process Mach IPC budget and can't blow past the threshold.
// `cargo test -p lucidos-engine` runs both the lib's parallel test
// binary AND that integration binary; the lib stays parallel-safe
// and the wasm tests stay covered. Engine-touching internals are
// re-exported through the `__wasm_test_internals` `#[doc(hidden)]`
// module on the crate root, since `tests/` integration tests can
// only see the lib's public surface.
//
// The tests in this module are correct, isolated (each gets its own
// `tempfile::tempdir()`), and pass under the suite's normal parallel
// schedule. Don't add `#[ignore]` or `#[serial]` here.
use super::*;

async fn make_test_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().to_path_buf();
    let _ = git_cmd(&["init"], &repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], &repo).await;
    tokio::fs::write(repo.join("init.txt"), "initial")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "initial commit"], &repo).await;
    (tmp, repo)
}

async fn rev_parse(repo: &Path, refname: &str) -> String {
    String::from_utf8_lossy(&git_cmd(&["rev-parse", refname], repo).await.unwrap().stdout)
        .trim()
        .to_string()
}

/// Reverting a fast-forwarded branch undoes its commits.
#[tokio::test]
async fn revert_fast_forward_commits() {
    let (_tmp, repo) = make_test_repo().await;

    // Create feature branch with a commit
    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
    tokio::fs::write(repo.join("feature.txt"), "feature work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "add feature"], &repo).await;

    let pre_sha = rev_parse(&repo, "main").await;
    let post_sha = rev_parse(&repo, "feature").await;

    // Fast-forward main to feature
    let _ = git_cmd(&["checkout", "main"], &repo).await;
    let _ = git_cmd(&["merge", "--ff-only", "feature"], &repo).await;

    // feature.txt should exist
    assert!(repo.join("feature.txt").exists());

    // Revert the fast-forward
    let result = revert_with_shas(&repo, &pre_sha, &post_sha, "feature").await;
    assert!(result.is_ok(), "revert should succeed: {:?}", result.err());

    // feature.txt should be gone
    assert!(
        !repo.join("feature.txt").exists(),
        "feature.txt should be removed after revert"
    );
}

/// Reverting a catchup merge (main merged INTO branch, then ff'd to main)
/// correctly uses -m 2 to undo the branch changes, not main's changes.
#[tokio::test]
async fn revert_catchup_merge_uses_correct_parent() {
    let (_tmp, repo) = make_test_repo().await;

    // Create feature branch with a commit
    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
    tokio::fs::write(repo.join("feature.txt"), "feature work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "add feature"], &repo).await;

    // Go back to main and make a commit (simulates other work landing on main)
    let _ = git_cmd(&["checkout", "main"], &repo).await;
    tokio::fs::write(repo.join("main-work.txt"), "main work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "main work"], &repo).await;

    let pre_sha = rev_parse(&repo, "main").await;

    // Catchup: merge main INTO feature (creates "Merge branch 'main' into feature")
    let _ = git_cmd(&["checkout", "feature"], &repo).await;
    let _ = git_cmd(&["merge", "main", "--no-edit"], &repo).await;
    let post_sha = rev_parse(&repo, "feature").await;

    // Fast-forward main to the merge commit
    let _ = git_cmd(&["checkout", "main"], &repo).await;
    let _ = git_cmd(&["merge", "--ff-only", "feature"], &repo).await;

    // Both files should exist
    assert!(repo.join("feature.txt").exists());
    assert!(repo.join("main-work.txt").exists());

    // Revert should undo feature changes but keep main's work
    let result = revert_with_shas(&repo, &pre_sha, &post_sha, "feature").await;
    assert!(result.is_ok(), "revert should succeed: {:?}", result.err());

    // feature.txt should be gone (branch changes reverted)
    assert!(
        !repo.join("feature.txt").exists(),
        "feature.txt should be removed — branch changes should be reverted"
    );
    // main-work.txt should still exist (main's changes preserved)
    assert!(
        repo.join("main-work.txt").exists(),
        "main-work.txt should remain — main's changes should be preserved"
    );
}

/// Reverting a regular merge ("Merge branch 'feature' into main")
/// correctly uses -m 1 to undo the branch changes.
#[tokio::test]
async fn revert_regular_merge_uses_correct_parent() {
    let (_tmp, repo) = make_test_repo().await;

    // Create feature branch with a commit
    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
    tokio::fs::write(repo.join("feature.txt"), "feature work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "add feature"], &repo).await;

    // Go back to main and make a commit so merge is non-ff
    let _ = git_cmd(&["checkout", "main"], &repo).await;
    tokio::fs::write(repo.join("main-work.txt"), "main work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "main work"], &repo).await;

    let pre_sha = rev_parse(&repo, "main").await;

    // Regular merge: merge feature INTO main
    let _ = git_cmd(&["merge", "feature", "--no-edit"], &repo).await;
    let post_sha = rev_parse(&repo, "main").await;

    // Both files should exist
    assert!(repo.join("feature.txt").exists());
    assert!(repo.join("main-work.txt").exists());

    // Revert should undo feature changes but keep main's work
    let result = revert_with_shas(&repo, &pre_sha, &post_sha, "feature").await;
    assert!(result.is_ok(), "revert should succeed: {:?}", result.err());

    assert!(
        !repo.join("feature.txt").exists(),
        "feature.txt should be removed — branch changes should be reverted"
    );
    assert!(
        repo.join("main-work.txt").exists(),
        "main-work.txt should remain — main's changes should be preserved"
    );
}

/// Multiple fast-forwarded commits are all reverted.
#[tokio::test]
async fn revert_multiple_fast_forward_commits() {
    let (_tmp, repo) = make_test_repo().await;

    let pre_sha = rev_parse(&repo, "main").await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
    tokio::fs::write(repo.join("a.txt"), "a").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "add a"], &repo).await;
    tokio::fs::write(repo.join("b.txt"), "b").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "add b"], &repo).await;

    let post_sha = rev_parse(&repo, "feature").await;

    let _ = git_cmd(&["checkout", "main"], &repo).await;
    let _ = git_cmd(&["merge", "--ff-only", "feature"], &repo).await;

    assert!(repo.join("a.txt").exists());
    assert!(repo.join("b.txt").exists());

    let result = revert_with_shas(&repo, &pre_sha, &post_sha, "feature").await;
    assert!(result.is_ok(), "revert should succeed: {:?}", result.err());

    assert!(
        !repo.join("a.txt").exists(),
        "a.txt should be removed after revert"
    );
    assert!(
        !repo.join("b.txt").exists(),
        "b.txt should be removed after revert"
    );
}

// ── Merge ownership during a conflict resolution ──

/// The incident this guard exists for (2026-08-11): a conflict-resolution
/// session was mid-turn on its 5-step merge prompt when a second
/// `apply_change` for the same change found that very session live, took the
/// Tier-1 path, and fast-forwarded `main` at step 2. Pairing open plus a live
/// session means the resolver owns the merge, and every other caller refuses.
#[test]
fn a_live_resolver_owns_its_change_merge() {
    assert_eq!(
        decide_merge_ownership(Some(true), true),
        MergeOwnership::ResolverOwnsIt
    );
}

/// The liveness half is what keeps the guard from wedging Apply, and it names
/// the RESOLVER rather than any live session. Both wedge shapes collapse to
/// `resolver_present = false` here: an engine restart empties `agent_sessions`
/// so a pairing stranded by a crash has nobody carrying it, and a later
/// unrelated turn on that same thread is not a resolver either (its session
/// carries no `conflict_change_id` for this change). Either way the apply falls
/// through to the ordinary tiers instead of being refused forever.
#[test]
fn a_stranded_pairing_with_no_resolver_does_not_block_apply() {
    assert_eq!(
        decide_merge_ownership(Some(true), false),
        MergeOwnership::CallerMayMerge
    );
}

/// No resolution in flight is the ordinary case: a live session (the thread
/// the user is working in) must not stop its own change from applying.
#[test]
fn a_closed_pairing_never_blocks_apply() {
    assert_eq!(
        decide_merge_ownership(Some(false), true),
        MergeOwnership::CallerMayMerge
    );
    assert_eq!(
        decide_merge_ownership(Some(false), false),
        MergeOwnership::CallerMayMerge
    );
}

/// A projection query that could not run is UNKNOWN, never a "no"
/// (`.claude/rules/rust.md`). Merging under a working resolver is the direction
/// that destroys something, so an unknown refuses while a resolver is present
/// and costs the caller a retry. With nobody resolving there is nobody to own
/// the merge whatever the query would have said, so it proceeds.
#[test]
fn an_unanswerable_pairing_probe_refuses_only_while_a_resolver_is_present() {
    assert_eq!(
        decide_merge_ownership(None, true),
        MergeOwnership::ResolverOwnsIt
    );
    assert_eq!(
        decide_merge_ownership(None, false),
        MergeOwnership::CallerMayMerge
    );
}

// ── Phase 0: Pre-refactor safety net ──

#[test]
fn apply_result_applied_without_restart() {
    let cid = Uuid::new_v4();
    let tid = Uuid::new_v4();
    let result = ApplyResult::applied(cid, Some(tid), false);
    assert_eq!(result.status, ApplyStatus::Applied);
    assert_eq!(result.change_id, cid);
    assert_eq!(result.thread_id, Some(tid));
    assert!(!result.restart_required);
    assert!(result.conflict_thread_id.is_none());
    assert!(result.review_thread_id.is_none());
    assert!(!result.message.contains("restart"));
}

#[test]
fn apply_result_applied_with_restart() {
    let cid = Uuid::new_v4();
    let result = ApplyResult::applied(cid, None, true);
    assert_eq!(result.status, ApplyStatus::Applied);
    assert!(result.restart_required);
    assert!(
        result.message.contains("restart"),
        "message should mention restart: {}",
        result.message
    );
    assert!(result.conflict_thread_id.is_none());
    assert!(result.review_thread_id.is_none());
}

#[test]
fn apply_result_applied_with_merge_carries_shas_and_counts() {
    let cid = Uuid::new_v4();
    let tid = Uuid::new_v4();
    let pre = "0".repeat(40);
    let post = "1".repeat(40);
    let commits = vec!["fix: a".to_string(), "fix: b".to_string()];
    let result = ApplyResult::applied_with_merge(
        cid,
        Some(tid),
        false,
        pre.clone(),
        post.clone(),
        &commits,
        5,
    );
    assert_eq!(result.status, ApplyStatus::Applied);
    assert_eq!(result.change_id, cid);
    assert_eq!(result.thread_id, Some(tid));
    assert_eq!(result.previous_commit.as_deref(), Some(pre.as_str()));
    assert_eq!(result.applied_commit.as_deref(), Some(post.as_str()));
    assert_eq!(result.commits_applied, 2);
    assert_eq!(result.files_changed, 5);
}

#[test]
fn now_epoch_millis_is_reasonable() {
    let ms = now_epoch_millis();
    // Should be after 2026-01-01 and before 2100-01-01
    assert!(
        ms > 1_767_225_600_000,
        "epoch millis {} is too small (before 2026)",
        ms
    );
    assert!(
        ms < 4_102_444_800_000,
        "epoch millis {} is too large (after 2100)",
        ms
    );
}

/// Reverting a merge with a mismatched pre_sha fails explicitly
/// instead of silently reverting the wrong side.
#[tokio::test]
async fn revert_merge_with_wrong_pre_sha_fails() {
    let (_tmp, repo) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
    tokio::fs::write(repo.join("feature.txt"), "feature")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "add feature"], &repo).await;

    let _ = git_cmd(&["checkout", "main"], &repo).await;
    tokio::fs::write(repo.join("main-work.txt"), "main")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "main work"], &repo).await;

    // Merge feature into main
    let _ = git_cmd(&["merge", "feature", "--no-edit"], &repo).await;
    let post_sha = rev_parse(&repo, "main").await;

    // Pass a bogus pre_sha that doesn't match either parent
    let result = revert_with_shas(
        &repo,
        "0000000000000000000000000000000000000000",
        &post_sha,
        "feature",
    )
    .await;
    assert!(
        result.is_err(),
        "should fail when pre_sha doesn't match any parent"
    );
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("does not match any parent"),
        "error should explain the mismatch: {}",
        err
    );
}

// ── Phase 6.2: Apply preserves worktree + branch ──

/// Build a repo with a worktree on `feature` that has one commit ahead of
/// main, ready for the `catchup_and_ff_to_main` fast path. Returns
/// (tempdir guard for repo, repo path, tempdir guard for worktree, worktree path).
async fn make_repo_with_worktree() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    tempfile::TempDir,
    std::path::PathBuf,
) {
    let (tmp, repo) = make_test_repo().await;

    // Use a separate tempdir for the worktree so paths don't collide.
    let wt_tmp = tempfile::tempdir().unwrap();
    let wt_dir = wt_tmp.path().join("wt");

    let _ = git_cmd(
        &[
            "worktree",
            "add",
            "-b",
            "feature",
            wt_dir.to_str().unwrap(),
            "main",
        ],
        &repo,
    )
    .await;
    // Commit in the worktree so the branch advances.
    tokio::fs::write(wt_dir.join("feature.txt"), "feature work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &wt_dir).await;
    let _ = git_cmd(&["commit", "-m", "add feature"], &wt_dir).await;
    // Repo's HEAD lands wherever `worktree add` left it; pin to main so
    // post-merge assertions on the main worktree don't see the feature
    // branch checked out there.
    let _ = git_cmd(&["checkout", "main"], &repo).await;

    (tmp, repo, wt_tmp, wt_dir)
}

/// Apply path: after a successful ff-merge to main, the worktree dir and
/// its branch must still exist (Phase 6.2). The worktree's branch is at
/// main HEAD, so a follow-up `git diff main` is empty.
#[tokio::test]
async fn apply_keeps_worktree_and_resets_branch_to_main() {
    let (_tmp, repo, _wt_tmp, wt_dir) = make_repo_with_worktree().await;

    // Drive the same sequence Apply runs in change_ops:
    //   1. catchup_and_ff_to_main (advances main, tries branch -D — fails
    //      silently because the worktree owns the branch).
    //   2. reset_worktree_to_main_after_apply (resets the worktree to main).
    let merge = catchup_and_ff_to_main(&repo, &wt_dir, "feature")
        .await
        .expect("ff merge should succeed for a fast-forwardable branch");
    let (pre_sha, post_sha) = merge;
    assert_ne!(pre_sha, post_sha, "main should advance");

    let reset = reset_worktree_to_main_after_apply(&wt_dir).await;
    assert!(reset.is_ok(), "reset should succeed: {:?}", reset.err());

    // Phase 6.2 assertions:
    // (a) worktree dir still exists on disk
    assert!(
        wt_dir.exists(),
        "worktree dir should persist after Apply: {}",
        wt_dir.display()
    );
    assert!(
        wt_dir.join(".git").exists(),
        "worktree's .git pointer should still be valid"
    );

    // (b) branch ref still exists in the repo (catchup_and_ff_to_main
    //     can't delete it — the worktree owns it).
    let branch_check = git_cmd(&["rev-parse", "--verify", "feature"], &repo)
        .await
        .unwrap();
    assert!(
        branch_check.status.success(),
        "branch 'feature' should still exist: {}",
        String::from_utf8_lossy(&branch_check.stderr)
    );

    // (c) branch HEAD == main HEAD (no commits ahead of main).
    let branch_sha = rev_parse(&repo, "feature").await;
    let main_sha = rev_parse(&repo, "main").await;
    assert_eq!(
        branch_sha, main_sha,
        "branch should be at main HEAD after apply"
    );
}

/// `reset_worktree_to_main_after_apply` refuses to reset a dirty worktree
/// — silent reset would discard user work.
#[tokio::test]
async fn reset_worktree_to_main_refuses_when_dirty() {
    let (_tmp, repo, _wt_tmp, wt_dir) = make_repo_with_worktree().await;

    // ff-merge so main equals the branch tip.
    let _ = catchup_and_ff_to_main(&repo, &wt_dir, "feature")
        .await
        .unwrap();

    // Simulate the user editing a file between merge and reset.
    tokio::fs::write(wt_dir.join("feature.txt"), "user edit after merge")
        .await
        .unwrap();

    let reset = reset_worktree_to_main_after_apply(&wt_dir).await;
    assert!(reset.is_err(), "reset must refuse when worktree is dirty");
    let err = reset.unwrap_err().to_string();
    assert!(
        err.contains("dirty"),
        "error should mention dirty state: {}",
        err
    );
    // The user edit must still be on disk — reset must not have run.
    let contents = tokio::fs::read_to_string(wt_dir.join("feature.txt"))
        .await
        .unwrap();
    assert_eq!(
        contents, "user edit after merge",
        "user edit must be preserved when reset refuses"
    );
}

/// `reset_worktree_to_main_after_apply` is a no-op for a clean worktree
/// already at main HEAD (the typical Apply post-condition).
#[tokio::test]
async fn reset_worktree_to_main_is_noop_when_at_main_head() {
    let (_tmp, repo, _wt_tmp, wt_dir) = make_repo_with_worktree().await;

    let _ = catchup_and_ff_to_main(&repo, &wt_dir, "feature")
        .await
        .unwrap();

    // Worktree is now clean and the branch ref equals main.
    let pre = rev_parse(&wt_dir, "HEAD").await;
    let reset = reset_worktree_to_main_after_apply(&wt_dir).await;
    assert!(reset.is_ok());
    let post = rev_parse(&wt_dir, "HEAD").await;
    assert_eq!(pre, post, "no SHA movement on a no-op reset");
}

/// After Apply via the new path, a fresh `catchup_and_ff_to_main` round-
/// trip is possible: a new commit on the (preserved) branch can be
/// fast-forwarded into main again. This is the round-trip the spawn
/// dispatcher relies on when the user sends a follow-up message in the
/// same thread.
#[tokio::test]
async fn apply_round_trip_branch_can_advance_again() {
    let (_tmp, repo, _wt_tmp, wt_dir) = make_repo_with_worktree().await;

    // First Apply.
    let _ = catchup_and_ff_to_main(&repo, &wt_dir, "feature")
        .await
        .unwrap();
    reset_worktree_to_main_after_apply(&wt_dir).await.unwrap();

    // Simulate the next CC turn: make a new commit on the same branch.
    tokio::fs::write(wt_dir.join("turn2.txt"), "second turn work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &wt_dir).await;
    let commit = git_cmd(&["commit", "-m", "turn 2"], &wt_dir).await.unwrap();
    assert!(
        commit.status.success(),
        "commit on preserved branch must succeed: {}",
        String::from_utf8_lossy(&commit.stderr)
    );

    // Second Apply round-trip.
    let merge = catchup_and_ff_to_main(&repo, &wt_dir, "feature").await;
    assert!(
        merge.is_ok(),
        "second ff-merge round-trip should succeed: {:?}",
        merge.err()
    );
    reset_worktree_to_main_after_apply(&wt_dir).await.unwrap();

    // Worktree and branch still alive after two Applies.
    assert!(wt_dir.exists());
    let branch_check = git_cmd(&["rev-parse", "--verify", "feature"], &repo)
        .await
        .unwrap();
    assert!(
        branch_check.status.success(),
        "branch must persist across multiple applies"
    );
    let branch_sha = rev_parse(&repo, "feature").await;
    let main_sha = rev_parse(&repo, "main").await;
    assert_eq!(branch_sha, main_sha);
}

// ── Phase 6.3: Discard preserves worktree + branch ──

/// Discard's reset helper resets a worktree on a branch ahead of main back
/// to main HEAD, leaves the worktree directory and the branch ref on disk,
/// and wipes any uncommitted state (the user explicitly chose to discard).
#[tokio::test]
async fn discard_resets_worktree_and_preserves_branch() {
    let (_tmp, repo, _wt_tmp, wt_dir) = make_repo_with_worktree().await;

    // Worktree is on `feature` with one commit ahead of main.
    let main_sha_before = rev_parse(&repo, "main").await;
    let branch_sha_before = rev_parse(&repo, "feature").await;
    assert_ne!(
        main_sha_before, branch_sha_before,
        "precondition: branch must be ahead of main"
    );

    let reset = reset_worktree_to_main_after_discard(&wt_dir).await;
    assert!(
        reset.is_ok(),
        "discard reset should succeed: {:?}",
        reset.err()
    );

    // (a) worktree dir still exists on disk — discard must NOT remove it.
    assert!(
        wt_dir.exists(),
        "worktree dir must persist after discard: {}",
        wt_dir.display()
    );
    assert!(
        wt_dir.join(".git").exists(),
        "worktree's .git pointer must still be valid"
    );

    // (b) branch ref still exists — discard must NOT delete it.
    let branch_check = git_cmd(&["rev-parse", "--verify", "feature"], &repo)
        .await
        .unwrap();
    assert!(
        branch_check.status.success(),
        "branch 'feature' must still exist: {}",
        String::from_utf8_lossy(&branch_check.stderr)
    );

    // (c) branch HEAD == main HEAD — the discarded commits are gone from
    //     the branch tip.
    let branch_sha_after = rev_parse(&repo, "feature").await;
    let main_sha_after = rev_parse(&repo, "main").await;
    assert_eq!(
        branch_sha_after, main_sha_after,
        "branch must be reset to main HEAD after discard"
    );

    // (d) the file from the discarded commit is gone from the worktree.
    assert!(
        !wt_dir.join("feature.txt").exists(),
        "discarded commit's file must be removed from the worktree"
    );
}

/// Discard wipes a dirty worktree — unlike Apply, the user explicitly
/// asked to throw work away.
#[tokio::test]
async fn discard_resets_even_when_worktree_is_dirty() {
    let (_tmp, _repo, _wt_tmp, wt_dir) = make_repo_with_worktree().await;

    // Add an uncommitted edit on top of the committed branch state.
    tokio::fs::write(wt_dir.join("scratch.txt"), "throw me away")
        .await
        .unwrap();
    tokio::fs::write(wt_dir.join("feature.txt"), "dirty edit")
        .await
        .unwrap();

    let reset = reset_worktree_to_main_after_discard(&wt_dir).await;
    assert!(
        reset.is_ok(),
        "discard reset must succeed even when dirty: {:?}",
        reset.err()
    );

    // Both the committed file and the untracked file must be gone.
    assert!(
        !wt_dir.join("feature.txt").exists(),
        "tracked file from discarded commits must be removed"
    );
    assert!(
        !wt_dir.join("scratch.txt").exists(),
        "untracked file must be removed by clean -fd"
    );
}

/// After a discard, the same branch can be advanced again with a new
/// commit and resets cleanly a second time — round-trip the dispatcher
/// relies on when the user keeps working in the same thread post-discard.
#[tokio::test]
async fn discard_round_trip_branch_can_advance_again() {
    let (_tmp, repo, _wt_tmp, wt_dir) = make_repo_with_worktree().await;

    // First discard.
    reset_worktree_to_main_after_discard(&wt_dir).await.unwrap();

    // Simulate next CC turn on the same (preserved) branch.
    tokio::fs::write(wt_dir.join("turn2.txt"), "second turn work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &wt_dir).await;
    let commit = git_cmd(&["commit", "-m", "turn 2"], &wt_dir).await.unwrap();
    assert!(
        commit.status.success(),
        "commit on preserved branch must succeed: {}",
        String::from_utf8_lossy(&commit.stderr)
    );

    // Second discard.
    reset_worktree_to_main_after_discard(&wt_dir).await.unwrap();

    // Worktree, branch still alive; branch back at main.
    assert!(wt_dir.exists());
    let branch_check = git_cmd(&["rev-parse", "--verify", "feature"], &repo)
        .await
        .unwrap();
    assert!(
        branch_check.status.success(),
        "branch must persist across multiple discards"
    );
    let branch_sha = rev_parse(&repo, "feature").await;
    let main_sha = rev_parse(&repo, "main").await;
    assert_eq!(branch_sha, main_sha);
}

/// Regression guard for the 2026-07-28 Apply-over-mobile incident.
///
/// `apply_change_inner`'s Tier 2 used to `await` a whole coding-agent merge
/// session inline, so the session's lifetime was the *caller's* lifetime. For
/// the HTTP callers that meant an axum handler: when iOS Safari dropped the
/// connection 72 s into a conflict resolution on thread `293f96d5`, hyper
/// dropped the handler future, the subprocess died mid-tool
/// (`interruptedByShutdown` in its own transcript), and the `agent_sessions`
/// entry it left behind wedged the thread.
///
/// Every other CC-assisted merge path already detaches — Tier 1 via
/// `spawn_in_place_conflict_recovery`, Tier 3 via `spawn_merge_session`. This
/// pins Tier 2 to the same rule.
///
/// Source-text assertion for the reason given on
/// `run_merge_session_tier2_routes_through_start_merge_and_get_prompt`: nothing
/// in this crate constructs a live `LucidosEngine` outside `main.rs`, so the
/// spawn boundary can't be observed at runtime here. Brittle to formatting,
/// resilient to the regression that matters.
#[test]
fn tier2_merge_session_is_detached_from_the_caller_future() {
    let source = include_str!("change_ops/apply.rs");

    let call = source
        .find("run_merge_session_tier2(")
        .expect("Tier 2 must still drive a merge session");
    let spawn = source
        .find("spawn_cc_task_guarded(")
        .expect("the Tier-2 merge must be handed to a spawned task");

    assert!(
        spawn < call,
        "the Tier-2 merge session must be spawned, not awaited inline — an HTTP \
         handler's future must never own a coding-agent session (the caller \
         disconnecting would kill the merge and leave a phantom session behind)"
    );

    // The immediate return is the other half: having spawned, Tier 2 must hand
    // the caller a Conflict result rather than waiting for the merge. The
    // outcome reaches the user through ChangeApplied / ChangeApplyFailed.
    let tail = &source[call..];
    let window_end = tail.find("// Tier 3: No worktree").unwrap_or(tail.len());
    let after_spawn = &tail[..window_end];
    assert!(
        after_spawn.contains("ApplyResult::conflict("),
        "after spawning the merge, Tier 2 must return ApplyResult::conflict so \
         the caller stops waiting. Body was:\n{}",
        after_spawn
    );

    // Detaching moved the merge past `apply_change`'s `ApplyStatus::Applied`
    // gate, so the spawned task has to run the orphan-sibling reconcile itself
    // — and gate it on the row actually reaching `applied`, or a failed merge
    // would discard a newer sibling's work. Caught in review after the first
    // cut of the detach silently dropped it.
    assert!(
        after_spawn.contains("discard_orphaned_pending_siblings"),
        "the detached Tier-2 task must reconcile orphaned sibling pending changes — \
         apply_change's gate can't, because this path returns Conflict before the \
         merge lands. Body was:\n{}",
        after_spawn
    );
    assert!(
        after_spawn.contains(r#"c.status == "applied""#),
        "the detached reconcile must be gated on the change really applying, not on \
         the merge task merely finishing. Body was:\n{}",
        after_spawn
    );
}

/// The other half of the same invariant, checked at the API boundary: no axum
/// handler may drive a coding-agent session itself.
///
/// A handler's future dies with its client. Sessions therefore belong to
/// spawned tasks that report through events; handlers may only *start* them.
/// The 2026-07-28 incident reached a session from a handler transitively
/// (`claude_code_apply_now` → `apply_now` → `apply_change` → Tier 2), which no
/// single-file check can see — but a direct call in `api/` is the shape that
/// would make the transitive case easy to reintroduce, so it stays banned.
#[test]
fn api_handlers_never_drive_a_coding_agent_session_directly() {
    // Every `api/*.rs` that could plausibly reach a session.
    for (name, source) in [
        ("api/chat.rs", include_str!("../api/chat.rs")),
        ("api/claude_code.rs", include_str!("../api/claude_code.rs")),
        ("api/changes.rs", include_str!("../api/changes.rs")),
        ("api/history.rs", include_str!("../api/history.rs")),
    ] {
        assert!(
            !source.contains("run_direct_agent("),
            "{name} calls run_direct_agent directly — an HTTP handler must never own a \
             coding-agent session; hand it to a spawned task and answer the caller \
             immediately (see the Tier-2 detach in change_ops/apply.rs)"
        );
    }
}
