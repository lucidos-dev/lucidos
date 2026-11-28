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

#[tokio::test]
async fn revert_fast_forward_commits() {
    let (_tmp, repo) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
    tokio::fs::write(repo.join("feature.txt"), "feature work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "add feature"], &repo).await;

    let pre_sha = rev_parse(&repo, "main").await;
    let post_sha = rev_parse(&repo, "feature").await;

    let _ = git_cmd(&["checkout", "main"], &repo).await;
    let _ = git_cmd(&["merge", "--ff-only", "feature"], &repo).await;

    assert!(repo.join("feature.txt").exists());

    let result = revert_with_shas(&repo, &pre_sha, &post_sha, "feature").await;
    assert!(result.is_ok(), "revert should succeed: {:?}", result.err());

    assert!(
        !repo.join("feature.txt").exists(),
        "feature.txt should be removed after revert"
    );
}

#[tokio::test]
async fn revert_catchup_merge_uses_correct_parent() {
    let (_tmp, repo) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
    tokio::fs::write(repo.join("feature.txt"), "feature work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "add feature"], &repo).await;

    let _ = git_cmd(&["checkout", "main"], &repo).await;
    tokio::fs::write(repo.join("main-work.txt"), "main work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "main work"], &repo).await;

    let pre_sha = rev_parse(&repo, "main").await;

    let _ = git_cmd(&["checkout", "feature"], &repo).await;
    let _ = git_cmd(&["merge", "main", "--no-edit"], &repo).await;
    let post_sha = rev_parse(&repo, "feature").await;

    let _ = git_cmd(&["checkout", "main"], &repo).await;
    let _ = git_cmd(&["merge", "--ff-only", "feature"], &repo).await;

    assert!(repo.join("feature.txt").exists());
    assert!(repo.join("main-work.txt").exists());

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

#[tokio::test]
async fn revert_regular_merge_uses_correct_parent() {
    let (_tmp, repo) = make_test_repo().await;

    let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
    tokio::fs::write(repo.join("feature.txt"), "feature work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "add feature"], &repo).await;

    let _ = git_cmd(&["checkout", "main"], &repo).await;
    tokio::fs::write(repo.join("main-work.txt"), "main work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "main work"], &repo).await;

    let pre_sha = rev_parse(&repo, "main").await;

    let _ = git_cmd(&["merge", "feature", "--no-edit"], &repo).await;
    let post_sha = rev_parse(&repo, "main").await;

    assert!(repo.join("feature.txt").exists());
    assert!(repo.join("main-work.txt").exists());

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

#[tokio::test]
async fn revert_fast_forward_range_containing_a_merge_of_main() {
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
    let pre_sha = rev_parse(&repo, "main").await;

    let _ = git_cmd(&["checkout", "feature"], &repo).await;
    let _ = git_cmd(&["merge", "main", "--no-edit"], &repo).await;
    tokio::fs::write(repo.join("fix.txt"), "fix").await.unwrap();
    let _ = git_cmd(&["add", "."], &repo).await;
    let _ = git_cmd(&["commit", "-m", "harden fix"], &repo).await;
    let post_sha = rev_parse(&repo, "feature").await;

    let _ = git_cmd(&["checkout", "main"], &repo).await;
    let _ = git_cmd(&["merge", "--ff-only", "feature"], &repo).await;

    let result = revert_with_shas(&repo, &pre_sha, &post_sha, "feature").await;
    assert!(result.is_ok(), "revert should succeed: {:?}", result.err());

    assert!(!repo.join("feature.txt").exists(), "branch work reverted");
    assert!(!repo.join("fix.txt").exists(), "harden fix reverted");
    assert!(repo.join("main-work.txt").exists(), "main's work kept");
}

#[test]
fn a_live_resolver_owns_its_change_merge() {
    assert_eq!(
        decide_merge_ownership(Some(true), true),
        MergeOwnership::ResolverOwnsIt
    );
}

#[test]
fn a_stranded_pairing_with_no_resolver_does_not_block_apply() {
    assert_eq!(
        decide_merge_ownership(Some(true), false),
        MergeOwnership::CallerMayMerge
    );
}

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

    let _ = git_cmd(&["merge", "feature", "--no-edit"], &repo).await;
    let post_sha = rev_parse(&repo, "main").await;

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

async fn make_repo_with_worktree() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    tempfile::TempDir,
    std::path::PathBuf,
) {
    let (tmp, repo) = make_test_repo().await;

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
    tokio::fs::write(wt_dir.join("feature.txt"), "feature work")
        .await
        .unwrap();
    let _ = git_cmd(&["add", "."], &wt_dir).await;
    let _ = git_cmd(&["commit", "-m", "add feature"], &wt_dir).await;
    let _ = git_cmd(&["checkout", "main"], &repo).await;

    (tmp, repo, wt_tmp, wt_dir)
}

#[tokio::test]
async fn apply_keeps_worktree_and_resets_branch_to_main() {
    let (_tmp, repo, _wt_tmp, wt_dir) = make_repo_with_worktree().await;

    let merge = catchup_and_ff_to_main(&repo, &wt_dir, "feature")
        .await
        .expect("ff merge should succeed for a fast-forwardable branch");
    let (pre_sha, post_sha) = merge;
    assert_ne!(pre_sha, post_sha, "main should advance");

    let reset = reset_worktree_to_main_after_apply(&wt_dir).await;
    assert!(reset.is_ok(), "reset should succeed: {:?}", reset.err());

    assert!(
        wt_dir.exists(),
        "worktree dir should persist after Apply: {}",
        wt_dir.display()
    );
    assert!(
        wt_dir.join(".git").exists(),
        "worktree's .git pointer should still be valid"
    );

    let branch_check = git_cmd(&["rev-parse", "--verify", "feature"], &repo)
        .await
        .unwrap();
    assert!(
        branch_check.status.success(),
        "branch 'feature' should still exist: {}",
        String::from_utf8_lossy(&branch_check.stderr)
    );

    let branch_sha = rev_parse(&repo, "feature").await;
    let main_sha = rev_parse(&repo, "main").await;
    assert_eq!(
        branch_sha, main_sha,
        "branch should be at main HEAD after apply"
    );
}

#[tokio::test]
async fn reset_worktree_to_main_refuses_when_dirty() {
    let (_tmp, repo, _wt_tmp, wt_dir) = make_repo_with_worktree().await;

    let _ = catchup_and_ff_to_main(&repo, &wt_dir, "feature")
        .await
        .unwrap();

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
    let contents = tokio::fs::read_to_string(wt_dir.join("feature.txt"))
        .await
        .unwrap();
    assert_eq!(
        contents, "user edit after merge",
        "user edit must be preserved when reset refuses"
    );
}

#[tokio::test]
async fn reset_worktree_to_main_is_noop_when_at_main_head() {
    let (_tmp, repo, _wt_tmp, wt_dir) = make_repo_with_worktree().await;

    let _ = catchup_and_ff_to_main(&repo, &wt_dir, "feature")
        .await
        .unwrap();

    let pre = rev_parse(&wt_dir, "HEAD").await;
    let reset = reset_worktree_to_main_after_apply(&wt_dir).await;
    assert!(reset.is_ok());
    let post = rev_parse(&wt_dir, "HEAD").await;
    assert_eq!(pre, post, "no SHA movement on a no-op reset");
}

#[tokio::test]
async fn apply_round_trip_branch_can_advance_again() {
    let (_tmp, repo, _wt_tmp, wt_dir) = make_repo_with_worktree().await;

    let _ = catchup_and_ff_to_main(&repo, &wt_dir, "feature")
        .await
        .unwrap();
    reset_worktree_to_main_after_apply(&wt_dir).await.unwrap();

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

    let merge = catchup_and_ff_to_main(&repo, &wt_dir, "feature").await;
    assert!(
        merge.is_ok(),
        "second ff-merge round-trip should succeed: {:?}",
        merge.err()
    );
    reset_worktree_to_main_after_apply(&wt_dir).await.unwrap();

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

#[tokio::test]
async fn discard_resets_worktree_and_preserves_branch() {
    let (_tmp, repo, _wt_tmp, wt_dir) = make_repo_with_worktree().await;

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

    assert!(
        wt_dir.exists(),
        "worktree dir must persist after discard: {}",
        wt_dir.display()
    );
    assert!(
        wt_dir.join(".git").exists(),
        "worktree's .git pointer must still be valid"
    );

    let branch_check = git_cmd(&["rev-parse", "--verify", "feature"], &repo)
        .await
        .unwrap();
    assert!(
        branch_check.status.success(),
        "branch 'feature' must still exist: {}",
        String::from_utf8_lossy(&branch_check.stderr)
    );

    let branch_sha_after = rev_parse(&repo, "feature").await;
    let main_sha_after = rev_parse(&repo, "main").await;
    assert_eq!(
        branch_sha_after, main_sha_after,
        "branch must be reset to main HEAD after discard"
    );

    assert!(
        !wt_dir.join("feature.txt").exists(),
        "discarded commit's file must be removed from the worktree"
    );
}

#[tokio::test]
async fn discard_resets_even_when_worktree_is_dirty() {
    let (_tmp, _repo, _wt_tmp, wt_dir) = make_repo_with_worktree().await;

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

    assert!(
        !wt_dir.join("feature.txt").exists(),
        "tracked file from discarded commits must be removed"
    );
    assert!(
        !wt_dir.join("scratch.txt").exists(),
        "untracked file must be removed by clean -fd"
    );
}

#[tokio::test]
async fn discard_round_trip_branch_can_advance_again() {
    let (_tmp, repo, _wt_tmp, wt_dir) = make_repo_with_worktree().await;

    reset_worktree_to_main_after_discard(&wt_dir).await.unwrap();

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

    reset_worktree_to_main_after_discard(&wt_dir).await.unwrap();

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

    let tail = &source[call..];
    let window_end = tail.find("// Tier 3: No worktree").unwrap_or(tail.len());
    let after_spawn = &tail[..window_end];
    assert!(
        after_spawn.contains("ApplyResult::conflict("),
        "after spawning the merge, Tier 2 must return ApplyResult::conflict so \
         the caller stops waiting. Body was:\n{}",
        after_spawn
    );

    assert!(
        after_spawn.contains("discard_orphaned_pending_siblings"),
        "the detached Tier-2 task must reconcile orphaned sibling pending changes — \
         apply_change's gate can't, because this path returns Conflict before the \
         merge lands. Body was:\n{}",
        after_spawn
    );
    assert!(
        after_spawn.contains("c.status() == ChangeStatus::Applied"),
        "the detached reconcile must be gated on the change really applying, not on \
         the merge task merely finishing. Body was:\n{}",
        after_spawn
    );
}

#[test]
fn a_live_session_harden_re_enters_the_apply() {
    let apply = include_str!("change_ops/apply.rs");
    let send = apply
        .find(".request_hardening_in_session(thread_id, &session.msg_tx)")
        .expect("the live-session arm still routes hardening through the session");
    let arm = &apply[send..];
    let arm = &arm[..arm
        .find("ApplyResult::hardening(")
        .expect("the arm reports hardening")];
    assert!(
        arm.contains("Ok(answered) =>") && arm.contains("spawn_apply_after_session_harden("),
        "the live-session harden must hand its receiver to the follow-up apply. Arm was:\n{arm}"
    );

    let spawn = include_str!("../../../lucidos-agents-api/src/engine/claude_code/spawn.rs");
    let production = &spawn[..spawn.find("\n#[cfg(test)]").unwrap()];
    let follow_up = &production[production
        .find("fn spawn_apply_after_session_harden(")
        .unwrap()..];
    let follow_up = &follow_up[..follow_up.find("\n    }\n").unwrap()];
    let turn = follow_up.find("wait_for_prompt_turn(").unwrap();
    let settle = follow_up.find("wait_until_settled(").unwrap();
    let apply_again = follow_up.find(".apply_after_hardening(").unwrap();
    assert!(
        turn < settle && settle < apply_again,
        "the follow-up waits for the turn, then its settle, then applies"
    );
    assert!(
        follow_up.contains("HardeningRun::Unsettled"),
        "a turn that never settles must not apply: its proposal may still write the branch"
    );

    let after = &production[production.find("async fn apply_after_hardening(").unwrap()..];
    let after = &after[..after.find("\n    }\n").unwrap()];
    assert!(
        !after.contains("ThreadEvent::ChangeApplyFailed"),
        "every post-hardening failure goes through an emitter that notifies Apply All"
    );
    let unsettled = &after[after.find("HardeningRun::Unsettled =>").unwrap()..];
    let unsettled = &unsettled[..unsettled.find("return;").unwrap()];
    assert!(
        unsettled.contains("emit_apply_failed(") && !unsettled.contains("apply_change("),
        "an unsettled hardening turn reports the apply failed. Arm was:\n{unsettled}"
    );
}

#[test]
fn api_handlers_never_drive_a_coding_agent_session_directly() {
    for (name, source) in [
        (
            "api/chat.rs",
            include_str!("../../../lucidos-agents-api/src/api/chat.rs"),
        ),
        (
            "api/claude_code.rs",
            include_str!("../../../lucidos-agents-api/src/api/claude_code.rs"),
        ),
        (
            "api/changes.rs",
            include_str!("../../../lucidos-agents-api/src/api/changes.rs"),
        ),
        (
            "api/history.rs",
            include_str!("../../../lucidos-agents-api/src/api/history.rs"),
        ),
    ] {
        assert!(
            !source.contains("run_direct_agent("),
            "{name} calls run_direct_agent directly — an HTTP handler must never own a \
             coding-agent session; hand it to a spawned task and answer the caller \
             immediately (see the Tier-2 detach in change_ops/apply.rs)"
        );
    }
}

fn apply_change_call_offsets(source: &str) -> Vec<usize> {
    let mut calls = Vec::new();
    let mut cursor = 0;
    while let Some(hit) = source[cursor..].find(".apply_change(") {
        let call = cursor + hit;
        cursor = call + 1;
        let line_start = source[..call].rfind('\n').map_or(0, |i| i + 1);
        if source[line_start..call].contains("//") {
            continue;
        }
        calls.push(call);
    }
    calls
}

const APPLY_CHANGE_CALLERS: &[(&str, &str)] = &[
    (
        "api/changes.rs",
        "requesting: the Apply button, via guard_change_action",
    ),
    (
        "api/chat.rs",
        "internal: auto-apply of the change a chat turn just produced",
    ),
    (
        "engine/agent_session/apply_now.rs",
        "internal: the session's own Apply Now, mid-turn by design",
    ),
    (
        "engine/apply_all_driver.rs",
        "internal: the batch driver, which drops unsettled members up front",
    ),
    (
        "engine/claude_code/spawn.rs",
        "internal: auto-apply once hardening finishes",
    ),
    (
        "engine/standing_apply.rs",
        "internal: the resolver, which fires BECAUSE the thread settled",
    ),
    (
        "engine/tools/mod.rs",
        "requesting: the `changes` LLM tool, via change_action_refusal",
    ),
];

fn apply_change_result_blocks(source: &str) -> Vec<&str> {
    let mut blocks = Vec::new();
    for call in apply_change_call_offsets(source) {
        let Some(open) = source[call..].find('{').map(|i| call + i) else {
            continue;
        };
        let mut depth = 0usize;
        for (offset, ch) in source[open..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        blocks.push(&source[open..=open + offset]);
                        break;
                    }
                }
                _ => {}
            }
        }
    }
    blocks
}

mod unknown_worktree_lookup_is_not_a_no {

    use super::super::apply::resolve_harden_worktree;
    use super::super::discard::settle_discarded_branch;
    use super::{git_cmd, rev_parse};
    use crate::engine::git_ops::WorktreeLookup;
    use crate::test_support::make_repo_and_worktree;
    use std::path::PathBuf;
    use uuid::Uuid;

    const BRANCH: &str = "lucidos-claude-code-repo-add-streaks-1c0ffee0";
    const UNHELD: &str = "lucidos-claude-code-repo-no-tree-2d0ffee0";

    async fn repo_with_live_harden_worktree() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let (tmp, repo, _wt) = make_repo_and_worktree(BRANCH).await;
        git_cmd(&["branch", UNHELD, "main"], &repo).await.unwrap();

        let harden = tmp.path().join("harden-1c0ffee0");
        git_cmd(
            &[
                "worktree",
                "add",
                harden.to_str().unwrap(),
                "-b",
                "harden-work",
            ],
            &repo,
        )
        .await
        .unwrap();
        std::fs::write(harden.join("uncommitted.txt"), "hardening in progress").unwrap();
        (tmp, repo, harden)
    }

    #[tokio::test]
    async fn an_unknown_lookup_never_clears_the_hardening_worktree() {
        let (_tmp, repo, harden) = repo_with_live_harden_worktree().await;

        let err = resolve_harden_worktree(&repo, BRANCH, &harden, WorktreeLookup::Unknown)
            .await
            .expect_err("an unanswerable lookup must refuse, not clear the tree");
        assert!(
            err.contains("Try Apply again"),
            "the refusal must say the click is retryable; got: {err}"
        );
        assert_eq!(
            std::fs::read_to_string(harden.join("uncommitted.txt")).unwrap(),
            "hardening in progress",
            "a live hardening session's uncommitted work must survive the refusal"
        );
    }

    #[tokio::test]
    async fn a_not_found_lookup_clears_and_recreates_the_hardening_worktree() {
        let (_tmp, repo, harden) = repo_with_live_harden_worktree().await;

        let path = resolve_harden_worktree(&repo, UNHELD, &harden, WorktreeLookup::NotFound)
            .await
            .expect("no worktree holds the branch, so a fresh one is created");

        assert_eq!(path, harden, "the fresh tree lands at the path Apply chose");
        assert!(
            !harden.join("uncommitted.txt").exists(),
            "NotFound clears the tree, which is what Unknown must never do"
        );
    }

    async fn repo_with_orphan_branch_ahead_of_main() -> (tempfile::TempDir, PathBuf) {
        let (tmp, repo, wt) = make_repo_and_worktree(BRANCH).await;
        std::fs::write(wt.join("work.txt"), "the only copy").unwrap();
        git_cmd(&["add", "."], &wt).await.unwrap();
        git_cmd(&["commit", "-m", "feat: work"], &wt).await.unwrap();
        git_cmd(
            &["worktree", "remove", "--force", wt.to_str().unwrap()],
            &repo,
        )
        .await
        .unwrap();
        (tmp, repo)
    }

    #[tokio::test]
    async fn an_unknown_lookup_never_moves_the_discarded_branch_ref() {
        let (_tmp, repo) = repo_with_orphan_branch_ahead_of_main().await;

        let before = rev_parse(&repo, BRANCH).await;
        assert_ne!(
            before,
            rev_parse(&repo, "main").await,
            "setup: the branch holds a commit main does not"
        );

        settle_discarded_branch(&repo, BRANCH, Uuid::new_v4(), WorktreeLookup::Unknown)
            .await
            .expect("the change is still discarded; only the ref move is skipped");

        assert_eq!(
            rev_parse(&repo, BRANCH).await,
            before,
            "git branch -f moves a ref with no old-value guard, so an unanswerable \
             lookup must not run it over the only copy of the work"
        );
    }

    #[tokio::test]
    async fn a_not_found_lookup_still_resets_the_discarded_branch_ref() {
        let (_tmp, repo) = repo_with_orphan_branch_ahead_of_main().await;

        settle_discarded_branch(&repo, BRANCH, Uuid::new_v4(), WorktreeLookup::NotFound)
            .await
            .expect("git branch -f runs when git said no worktree holds the branch");

        assert_eq!(
            rev_parse(&repo, BRANCH).await,
            rev_parse(&repo, "main").await,
            "the discarded branch is back at main"
        );
    }

    #[tokio::test]
    async fn a_branch_checked_out_in_the_primary_checkout_is_never_reset() {
        let (_tmp, repo, _wt) = make_repo_and_worktree(BRANCH).await;
        git_cmd(&["checkout", "-b", UNHELD], &repo).await.unwrap();
        std::fs::write(repo.join("tracked-work.txt"), "committed on the branch").unwrap();
        git_cmd(&["add", "tracked-work.txt"], &repo).await.unwrap();
        git_cmd(&["commit", "-m", "feat: work"], &repo)
            .await
            .unwrap();
        std::fs::write(repo.join("scratch.txt"), "the user's untracked notes").unwrap();
        let before = rev_parse(&repo, UNHELD).await;

        settle_discarded_branch(
            &repo,
            UNHELD,
            Uuid::new_v4(),
            WorktreeLookup::Found(repo.clone()),
        )
        .await
        .expect("the change is still discarded; the primary checkout is left alone");

        assert_eq!(
            std::fs::read_to_string(repo.join("scratch.txt")).unwrap(),
            "the user's untracked notes",
            "git clean -fd must not run in the primary checkout"
        );
        assert_eq!(rev_parse(&repo, UNHELD).await, before);
    }
}

mod guard_tests {
    #[test]
    fn every_merging_apply_result_keeps_the_requested_build_outcome() {
        for source in [
            include_str!("change_ops/apply.rs"),
            include_str!("landing_lane/driver.rs"),
        ] {
            for block in source.split("ApplyResult::applied_with_merge(").skip(1) {
                let end = block.find(".with_version_build(build_request)");
                assert!(
                    end.is_some_and(|end| end < 500),
                    "Apply dropped its build outcome"
                );
            }
        }
        let api = include_str!("../../../lucidos-agents-api/src/api/history.rs");
        assert!(api.contains("join_or_start_background_rebuild"));
        assert!(api.contains("restart_background_rebuild"));
    }

    use super::*;
    use crate::test_support::source_scan::production_sources;

    #[test]
    fn apply_change_callers_do_not_re_announce_the_failure() {
        for (path, source) in production_sources() {
            for block in apply_change_result_blocks(&source) {
                for banned in ["emit_apply_failed(", "ThreadEvent::ChangeApplyFailed"] {
                    assert!(
                        !block.contains(banned),
                        "{path} handles an apply_change result with `{banned}`. The apply path \
                     already emitted that failure, so this draws the same card twice. Log it \
                     and let the event stand, as apply_all_driver.rs does. Block was:\n{block}"
                    );
                }
            }
        }
    }

    #[test]
    fn every_apply_change_caller_is_classified() {
        let mut found: Vec<String> = production_sources()
            .into_iter()
            .filter(|(_, source)| !apply_change_call_offsets(source).is_empty())
            .map(|(path, _)| path)
            .collect();
        found.sort();
        let mut enrolled: Vec<String> = APPLY_CHANGE_CALLERS
            .iter()
            .map(|(path, _)| (*path).to_string())
            .collect();
        enrolled.sort();
        assert_eq!(
            found, enrolled,
            "the set of apply_change callers moved. Add the file to APPLY_CHANGE_CALLERS with its \
         reason. A surface a person or an agent drives must call \
         api::changes::change_action_refusal first, or it will merge a branch whose coding agent \
         is still writing to it. An engine-internal path must not."
        );
    }
}

#[test]
fn apply_result_wire_explains_the_same_deferred_request() {
    let result = ApplyResult::applied(Uuid::new_v4(), None, true)
        .with_version_build(crate::engine::build_deferral::BuildRequest::Deferred(2));
    let wire = serde_json::to_value(result).unwrap();
    assert_eq!(wire["status"], "applied");
    assert_eq!(wire["version_build"], "deferred");
    assert_eq!(wire["deferred_coding_threads"], 2);
    assert_eq!(
        wire["message"],
        "Applied. The new version waits for 2 coding threads to finish."
    );
}

#[test]
fn apply_result_wire_distinguishes_started_and_no_build() {
    for (request, expected) in [
        (
            crate::engine::build_deferral::BuildRequest::Started,
            "started",
        ),
        (crate::engine::build_deferral::BuildRequest::None, "none"),
    ] {
        let result = ApplyResult::applied(Uuid::new_v4(), None, true).with_version_build(request);
        let wire = serde_json::to_value(result).unwrap();
        assert_eq!(wire["version_build"], expected);
        assert_eq!(wire["deferred_coding_threads"], 0);
    }
}

async fn thread_rows(pool: &sqlx::PgPool, thread_id: Uuid, event_type: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM events WHERE thread_id = $1 AND event_type = $2")
        .bind(thread_id)
        .bind(event_type)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_refusing_hook_stops_an_apply_before_its_request_is_recorded() {
    use crate::test_support::{
        hook_refusals, install_refusing_hook, seed_pending_change, setup_test_db, teardown_test_db,
    };
    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = crate::engine_test_support::boot_mock_engine(&db).await;
    let thread_id = Uuid::new_v4();
    let change_id =
        seed_pending_change(&engine.event_bus, thread_id, "claude-code/gated-apply").await;
    install_refusing_hook(
        &engine.event_bus,
        "require-a-plan",
        "ChangeApplyRequested",
        "No plan approved.",
    )
    .await;

    let error = engine.apply_change(change_id, None).await.unwrap_err();

    assert!(error.to_string().contains("No plan approved."), "{error}");
    assert_eq!(
        thread_rows(&pool, thread_id, "ChangeApplyRequested").await,
        0
    );
    let announced: Vec<String> = sqlx::query_scalar(
        "SELECT payload->>'error' FROM events \
         WHERE thread_id = $1 AND event_type = 'ChangeApplyFailed'",
    )
    .bind(thread_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(announced.len(), 1, "{announced:?}");
    assert!(announced[0].contains("No plan approved."), "{announced:?}");
    assert_eq!(hook_refusals(&pool, "require-a-plan").await, 1);
    let change = engine
        .changes()
        .get_by_id(change_id)
        .await
        .unwrap()
        .unwrap();
    assert!(change.is_pending(), "{:?}", change.state);

    teardown_test_db(&db).await;
}
