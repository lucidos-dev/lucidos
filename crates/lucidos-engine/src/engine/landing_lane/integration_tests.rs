//! The integration branch against real repositories: the lane thread's
//! landing merges, the completion check, the catch-up, and the landing.
//!
//! Each test plays the lane thread with plain git in the lane worktree.

use super::*;
use crate::engine::change_ops::revert_landing_merge;
use crate::engine::landing_lane::landing::{publish_clean_pass, LandingInput, LandingStep};
use std::path::PathBuf;

async fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_cmd(args, dir).await.unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A merge the test expects to stop on a conflict.
async fn git_conflicts(dir: &Path, args: &[&str]) {
    let out = git_cmd(args, dir).await.unwrap();
    assert!(!out.status.success(), "git {args:?} should conflict");
}

struct Fixture {
    _tmp: tempfile::TempDir,
    repo: PathBuf,
    workspace: PathBuf,
}

/// A repo whose `main` holds `a.txt`, `b.txt`, `c.txt`, `d.txt` and `e.txt`,
/// each reading `base`, and a workspace beside it for worktrees.
async fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::create_dir_all(&workspace).unwrap();
    git(&repo, &["init", "-b", "main"]).await;
    git(&repo, &["config", "user.email", "test@example.com"]).await;
    git(&repo, &["config", "user.name", "Test"]).await;
    for f in ["a.txt", "b.txt", "c.txt", "d.txt", "e.txt"] {
        std::fs::write(repo.join(f), "base\n").unwrap();
    }
    git(&repo, &["add", "."]).await;
    git(&repo, &["commit", "-m", "init"]).await;
    Fixture {
        _tmp: tmp,
        repo,
        workspace,
    }
}

impl Fixture {
    /// A member branch off `main` that writes `file`, returning its snapshot.
    async fn member(&self, branch: &str, file: &str, text: &str) -> String {
        git(&self.repo, &["branch", branch, "main"]).await;
        let wt = self.workspace.join(format!("src-{branch}"));
        git(
            &self.repo,
            &["worktree", "add", wt.to_str().unwrap(), branch],
        )
        .await;
        std::fs::write(wt.join(file), text).unwrap();
        git(&wt, &["commit", "-am", &format!("{branch}: write {file}")]).await;
        git(&wt, &["rev-parse", "HEAD"]).await
    }

    /// `main` writes `text` into each of `files`, in the repo-root tree.
    async fn main_writes(&self, files: &[&str], text: &str) -> String {
        for f in files {
            std::fs::write(self.repo.join(f), text).unwrap();
        }
        git(&self.repo, &["commit", "-am", "main moves on"]).await;
        git(&self.repo, &["rev-parse", "main"]).await
    }

    /// Open a lane worktree for a batch based at `main` now.
    async fn lane(&self) -> (LaneWorktree, String) {
        let base = git(&self.repo, &["rev-parse", "main"]).await;
        let lane = LaneWorktree::of(&self.workspace, Uuid::new_v4());
        ensure_lane_worktree(&self.repo, &self.workspace, &lane, &base)
            .await
            .expect("lane worktree");
        (lane, base)
    }
}

fn member(n: u8, snapshot: &str) -> LaneMember {
    LaneMember {
        change_id: Uuid::from_bytes([n; 16]),
        snapshot_sha: snapshot.to_string(),
    }
}

/// The lane thread's landing merge of one member whose file conflicts with
/// `main`: merge, resolve to `resolved`, commit.
async fn land_conflicting(lane: &Path, snapshot: &str, title: &str, file: &str, resolved: &str) {
    git_conflicts(
        lane,
        &[
            "merge",
            "--no-ff",
            snapshot,
            "-m",
            &format!("Land: {title}"),
        ],
    )
    .await;
    std::fs::write(lane.join(file), resolved).unwrap();
    git(lane, &["add", file]).await;
    git(lane, &["commit", "--no-edit"]).await;
}

fn read(dir: &Path, file: &str) -> String {
    std::fs::read_to_string(dir.join(file)).unwrap()
}

/// Three members that each conflict with `main` land as three landing
/// merges, in first-parent order. Each has its own first parent, and its
/// snapshot as second parent. Reverting the middle one undoes only it.
#[tokio::test]
async fn a_conflicted_batch_lands_as_one_landing_merge_per_member() {
    let fx = fixture().await;
    let s1 = fx.member("b1", "a.txt", "m1\n").await;
    let s2 = fx.member("b2", "b.txt", "m2\n").await;
    let s3 = fx.member("b3", "c.txt", "m3\n").await;
    fx.main_writes(&["a.txt", "b.txt", "c.txt"], "main\n").await;
    let (lane, base) = fx.lane().await;
    land_conflicting(&lane.path, &s1, "one", "a.txt", "main+m1\n").await;
    land_conflicting(&lane.path, &s2, "two", "b.txt", "main+m2\n").await;
    land_conflicting(&lane.path, &s3, "three", "c.txt", "main+m3\n").await;

    let members = [member(1, &s1), member(2, &s2), member(3, &s3)];
    let merges = verify_integration(&lane.path, &base, &members)
        .await
        .expect("a complete integration branch");
    let landed = land_integration(MainLocks::private_for_test(), &fx.repo, &lane.path)
        .await
        .unwrap();
    let head = git(&lane.path, &["rev-parse", "HEAD"]).await;
    assert_eq!(
        landed,
        IntegrationLanding::Landed {
            pre: base.clone(),
            post: head.clone()
        },
        "main had not moved, so no catch-up"
    );
    assert_eq!(git(&fx.repo, &["rev-parse", "main"]).await, head);

    let line = git(&fx.repo, &["log", "--first-parent", "--format=%H", "main"]).await;
    let line: Vec<&str> = line.lines().take(4).collect();
    let shas: Vec<&str> = merges.iter().map(|(_, m)| m.merge_sha.as_str()).collect();
    assert_eq!(line, vec![shas[2], shas[1], shas[0], base.as_str()]);
    assert_eq!(merges[0].1.first_parent, base);
    assert_eq!(merges[1].1.first_parent, merges[0].1.merge_sha);
    assert_eq!(merges[2].1.first_parent, merges[1].1.merge_sha);
    for ((change_id, merge), (snapshot, n)) in merges.iter().zip([(&s1, 1u8), (&s2, 2), (&s3, 3)]) {
        assert_eq!(*change_id, Uuid::from_bytes([n; 16]));
        let parents = git(&fx.repo, &["log", "-1", "--format=%P", &merge.merge_sha]).await;
        assert_eq!(parents, format!("{} {}", merge.first_parent, snapshot));
    }
    assert_eq!(
        git(
            &fx.repo,
            &["log", "-1", "--format=%s", &merges[1].1.merge_sha]
        )
        .await,
        "Land: two"
    );

    revert_landing_merge(&fx.repo, &merges[1].1.merge_sha)
        .await
        .unwrap();
    assert_eq!(read(&fx.repo, "a.txt"), "main+m1\n");
    assert_eq!(
        read(&fx.repo, "b.txt"),
        "main\n",
        "the middle change is undone"
    );
    assert_eq!(read(&fx.repo, "c.txt"), "main+m3\n");
}

/// A clean change lands while the batch resolves, and the batch then lands
/// over it with a clean catch-up merge.
#[tokio::test]
async fn a_pass_through_lands_mid_resolution_and_the_batch_catches_up() {
    let fx = fixture().await;
    let s1 = fx.member("b1", "a.txt", "m1\n").await;
    fx.main_writes(&["a.txt"], "main\n").await;
    let (lane, base) = fx.lane().await;
    land_conflicting(&lane.path, &s1, "one", "a.txt", "main+m1\n").await;

    let pass = fx.member("p", "d.txt", "pass\n").await;
    let members = [member(1, &s1)];
    let published = publish_clean_pass(
        MainLocks::private_for_test(),
        &fx.repo,
        &[LandingInput {
            change_id: Uuid::new_v4(),
            snapshot_sha: pass.clone(),
            subject: "Land: pass".into(),
            body: String::new(),
        }],
    )
    .await
    .unwrap();
    let LandingStep::Landed(pass_merge) = &published.chain.steps[0].1 else {
        panic!("the clean change lands at once");
    };

    verify_integration(&lane.path, &base, &members)
        .await
        .unwrap();
    let landed = land_integration(MainLocks::private_for_test(), &fx.repo, &lane.path)
        .await
        .unwrap();
    let IntegrationLanding::Landed { pre, post } = landed else {
        panic!("a clean catch-up lands: {landed:?}");
    };
    assert_eq!(
        pre, pass_merge.merge_sha,
        "main as the pass-through left it"
    );
    assert_eq!(git(&fx.repo, &["rev-parse", "main"]).await, post);
    assert_eq!(
        git(&fx.repo, &["log", "-1", "--format=%s", &post]).await,
        CATCH_UP_SUBJECT
    );
    assert_eq!(
        git(&fx.repo, &["log", "-1", "--format=%P", &post]).await,
        format!(
            "{} {}",
            git(&lane.path, &["rev-parse", "HEAD^1"]).await,
            pass_merge.merge_sha
        )
    );
    assert_eq!(read(&fx.repo, "a.txt"), "main+m1\n");
    assert_eq!(read(&fx.repo, "d.txt"), "pass\n");
}

/// The resolver edits a file a pass-through change touched. The catch-up
/// conflicts, goes back to the lane thread with what main gained, and moves
/// nothing. Once the lane thread merges main itself, the batch lands.
#[tokio::test]
async fn a_catch_up_conflict_goes_back_to_the_lane_thread_and_never_fails() {
    let fx = fixture().await;
    let s1 = fx.member("b1", "a.txt", "m1\n").await;
    fx.main_writes(&["a.txt"], "main\n").await;
    let (lane, base) = fx.lane().await;
    land_conflicting(&lane.path, &s1, "one", "a.txt", "main+m1\n").await;
    let members = [member(1, &s1)];

    let pass = fx.member("p", "e.txt", "pass\n").await;
    publish_clean_pass(
        MainLocks::private_for_test(),
        &fx.repo,
        &[LandingInput {
            change_id: Uuid::new_v4(),
            snapshot_sha: pass.clone(),
            subject: "Land: pass".into(),
            body: String::new(),
        }],
    )
    .await
    .unwrap();
    let main_after_pass = git(&fx.repo, &["rev-parse", "main"]).await;

    std::fs::write(lane.path.join("e.txt"), "resolver\n").unwrap();
    git(&lane.path, &["commit", "-am", "harden: fix e"]).await;

    verify_integration(&lane.path, &base, &members)
        .await
        .unwrap();
    let head_before = git(&lane.path, &["rev-parse", "HEAD"]).await;
    let landed = land_integration(MainLocks::private_for_test(), &fx.repo, &lane.path)
        .await
        .unwrap();
    let IntegrationLanding::CatchUpConflict { main_sha, commits } = landed else {
        panic!("the catch-up must conflict: {landed:?}");
    };
    assert_eq!(main_sha, main_after_pass);
    assert!(
        commits.iter().any(|c| c.contains("Land: pass")),
        "the follow-up names what main gained: {commits:?}"
    );
    assert_eq!(git(&fx.repo, &["rev-parse", "main"]).await, main_after_pass);
    assert_eq!(git(&lane.path, &["rev-parse", "HEAD"]).await, head_before);
    assert_eq!(
        git_answer(
            &["rev-parse", "--verify", "--quiet", "MERGE_HEAD"],
            &lane.path
        )
        .await,
        GitAnswer::No,
        "the catch-up merge was aborted"
    );

    // The follow-up turn: the lane thread merges main and resolves.
    git_conflicts(
        &lane.path,
        &["merge", "--no-ff", &main_sha, "-m", CATCH_UP_SUBJECT],
    )
    .await;
    std::fs::write(lane.path.join("e.txt"), "resolver+pass\n").unwrap();
    git(&lane.path, &["add", "e.txt"]).await;
    git(&lane.path, &["commit", "--no-edit"]).await;
    verify_integration(&lane.path, &base, &members)
        .await
        .unwrap();
    let landed = land_integration(MainLocks::private_for_test(), &fx.repo, &lane.path)
        .await
        .unwrap();
    assert!(
        matches!(landed, IntegrationLanding::Landed { .. }),
        "{landed:?}"
    );
    assert_eq!(read(&fx.repo, "e.txt"), "resolver+pass\n");
    assert_eq!(read(&fx.repo, "a.txt"), "main+m1\n");
}

/// The completion check refuses anything half done or out of shape.
#[tokio::test]
async fn the_completion_check_refuses_a_half_done_branch() {
    let fx = fixture().await;
    let s1 = fx.member("b1", "a.txt", "m1\n").await;
    let s2 = fx.member("b2", "b.txt", "m2\n").await;
    fx.main_writes(&["a.txt"], "main\n").await;
    let (lane, base) = fx.lane().await;
    let members = [member(1, &s1), member(2, &s2)];

    git_conflicts(&lane.path, &["merge", "--no-ff", &s1, "-m", "Land: one"]).await;
    let open = verify_integration(&lane.path, &base, &members).await;
    assert_eq!(open, Err("a merge is still in progress".into()));

    std::fs::write(lane.path.join("a.txt"), "main+m1\n").unwrap();
    git(&lane.path, &["add", "a.txt"]).await;
    git(&lane.path, &["commit", "--no-edit"]).await;
    let missing = verify_integration(&lane.path, &base, &members).await;
    assert!(
        missing
            .as_ref()
            .is_err_and(|e| e.contains("no landing merge")),
        "a member never merged: {missing:?}"
    );

    std::fs::write(lane.path.join("c.txt"), "stray\n").unwrap();
    git(&lane.path, &["merge", "--no-ff", &s2, "-m", "Land: two"]).await;
    let dirty = verify_integration(&lane.path, &base, &members).await;
    assert_eq!(
        dirty,
        Err("the integration branch has uncommitted edits".into())
    );
    git(&lane.path, &["checkout", "--", "c.txt"]).await;
    assert!(verify_integration(&lane.path, &base, &members)
        .await
        .is_ok());
}

/// A fast-forward is no landing merge: Revert would have nothing to undo.
#[tokio::test]
async fn a_fast_forwarded_member_has_no_landing_merge() {
    let fx = fixture().await;
    let s1 = fx.member("b1", "a.txt", "m1\n").await;
    let (lane, base) = fx.lane().await;
    git(&lane.path, &["merge", "--ff-only", &s1]).await;
    let ff = verify_integration(&lane.path, &base, &[member(1, &s1)]).await;
    assert!(ff.is_err_and(|e| e.contains("no landing merge")));
}

/// The lane worktree sits at `lane-<short id>` on `lane/<batch id>`, starts
/// at the base, survives a second open, and goes with its branch.
#[tokio::test]
async fn the_lane_worktree_opens_once_and_goes_with_its_branch() {
    let fx = fixture().await;
    let base = git(&fx.repo, &["rev-parse", "main"]).await;
    let batch_id = Uuid::new_v4();
    let lane = LaneWorktree::of(&fx.workspace, batch_id);
    assert!(lane
        .path
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with("lane-"));
    assert_eq!(lane.branch, format!("lane/{batch_id}"));

    create_lane_branch(&fx.repo, &lane.branch, &base)
        .await
        .unwrap();
    create_lane_branch(&fx.repo, &lane.branch, &base)
        .await
        .unwrap();
    ensure_lane_worktree(&fx.repo, &fx.workspace, &lane, &base)
        .await
        .unwrap();
    std::fs::write(lane.path.join("a.txt"), "work\n").unwrap();
    git(&lane.path, &["commit", "-am", "work"]).await;
    let head = git(&lane.path, &["rev-parse", "HEAD"]).await;
    ensure_lane_worktree(&fx.repo, &fx.workspace, &lane, &base)
        .await
        .unwrap();
    assert_eq!(
        git(&lane.path, &["rev-parse", "HEAD"]).await,
        head,
        "a second open keeps the work"
    );
    assert_eq!(
        read(
            &lane.path,
            crate::engine::claude_code::WORKTREE_WORKSPACE_MARKER
        ),
        fx.workspace.to_string_lossy(),
        "tagged for orphan recovery"
    );

    let listed = lane_worktrees(&fx.repo, &fx.workspace).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].0, batch_id);
    remove_lane_worktree(&fx.repo, &listed[0].1).await;
    assert!(!lane.path.exists());
    assert!(lane_worktrees(&fx.repo, &fx.workspace)
        .await
        .unwrap()
        .is_empty());
}

/// A lane thread mid-rebase has a detached HEAD, and a timed-out git read
/// names no branch either. Neither re-adds the tree over the lane thread's
/// work. Only a branch git names that is not the lane's is refused.
#[tokio::test]
async fn a_live_lane_worktree_on_no_branch_is_kept_with_its_work() {
    let fx = fixture().await;
    let (lane, base) = fx.lane().await;
    std::fs::write(lane.path.join("a.txt"), "resolved\n").unwrap();
    git(&lane.path, &["commit", "-am", "resolve"]).await;
    git(&lane.path, &["checkout", "--detach"]).await;
    let head = git(&lane.path, &["rev-parse", "HEAD"]).await;
    std::fs::write(lane.path.join("b.txt"), "in progress\n").unwrap();

    ensure_lane_worktree(&fx.repo, &fx.workspace, &lane, &base)
        .await
        .expect("a live tree on no branch is kept");
    assert_eq!(git(&lane.path, &["rev-parse", "HEAD"]).await, head);
    assert_eq!(read(&lane.path, "b.txt"), "in progress\n");

    git(&lane.path, &["checkout", "-b", "elsewhere"]).await;
    let refused = ensure_lane_worktree(&fx.repo, &fx.workspace, &lane, &base).await;
    assert!(
        refused.is_err_and(|e| e.contains("elsewhere")),
        "another branch is not the lane's"
    );
    assert_eq!(
        read(&lane.path, "b.txt"),
        "in progress\n",
        "the refusal touches nothing"
    );
}

/// The lane worktree gets the cached generated files, as every Lucidos
/// worktree does, so `/harden`'s `vite build` finds them.
#[tokio::test]
async fn the_lane_worktree_gets_the_cached_generated_files() {
    let fx = fixture().await;
    std::fs::create_dir_all(fx.repo.join("scripts")).unwrap();
    std::fs::write(
        fx.repo.join("scripts/codegen.sh"),
        "printf '%s' \"$1\" > generated.out\n",
    )
    .unwrap();
    git(&fx.repo, &["add", "scripts/codegen.sh"]).await;
    git(&fx.repo, &["commit", "-m", "codegen"]).await;
    let (lane, _) = fx.lane().await;
    assert_eq!(read(&lane.path, "generated.out"), "--cached-only");
}

/// The removal moves the tree aside like every worktree removal, so no
/// half-deleted tree is left at the lane path. A lane thread that checked out
/// another branch keeps that branch, and the lane branch still goes.
#[tokio::test]
async fn a_lane_worktree_goes_aside_and_keeps_a_branch_that_is_not_the_lanes() {
    let fx = fixture().await;
    let (lane, _) = fx.lane().await;
    git(&lane.path, &["checkout", "-b", "elsewhere"]).await;
    std::fs::write(lane.path.join("a.txt"), "work\n").unwrap();
    git(&lane.path, &["commit", "-am", "work elsewhere"]).await;

    assert!(
        remove_lane_worktree(&fx.repo, &lane).await,
        "the lane branch is gone"
    );
    assert!(!lane.path.exists());
    let aside = lane.path.with_file_name(format!(
        "{}{}",
        crate::engine::worktree_cleanup::RECLAIMING_PREFIX,
        lane.path.file_name().unwrap().to_string_lossy()
    ));
    assert!(!aside.exists(), "the moved-aside tree was deleted");
    let refname = format!("refs/heads/{}", lane.branch);
    assert_eq!(
        git_answer(&["rev-parse", "--verify", "--quiet", &refname], &fx.repo).await,
        GitAnswer::No
    );
    assert_eq!(
        git_answer(
            &["rev-parse", "--verify", "--quiet", "refs/heads/elsewhere"],
            &fx.repo
        )
        .await,
        GitAnswer::Yes,
        "a branch the lane did not make, with work on it, stays"
    );
    assert!(
        remove_lane_worktree(&fx.repo, &lane).await,
        "removing it again finds nothing and says gone"
    );
}

/// Several workspaces can share one checkout. A restart retires only lane
/// worktrees of closed batches its own workspace opened: never the open
/// batch's, and never a lane branch another workspace made.
#[tokio::test]
async fn a_restart_retires_only_this_workspaces_closed_lane_worktrees() {
    let fx = fixture().await;
    let (closed, _) = fx.lane().await;
    let (open, _) = fx.lane().await;
    let (foreign, _) = fx.lane().await;
    let batch = |lane: &LaneWorktree| lane_branch_batch(&lane.branch).unwrap();
    let opened_here = std::collections::HashSet::from([batch(&closed), batch(&open)]);

    let listed = lane_worktrees(&fx.repo, &fx.workspace).await.unwrap();
    assert_eq!(listed.len(), 3);
    let retired: Vec<&LaneWorktree> =
        closed_lanes_opened_here(&listed, &opened_here, Some(batch(&open))).collect();
    let retired_branches: Vec<&str> = retired.iter().map(|l| l.branch.as_str()).collect();
    assert_eq!(retired_branches, vec![closed.branch.as_str()]);

    for lane in retired {
        assert!(remove_lane_worktree(&fx.repo, lane).await);
    }
    let left: Vec<Uuid> = lane_worktrees(&fx.repo, &fx.workspace)
        .await
        .unwrap()
        .into_iter()
        .map(|(id, _)| id)
        .collect();
    assert_eq!(left.len(), 2);
    assert!(left.contains(&batch(&open)) && left.contains(&batch(&foreign)));
    assert!(
        foreign.path.exists(),
        "another workspace's lane worktree stays"
    );
}

/// Recovery reports a landing a crash cut off with `main` before the move.
#[tokio::test]
async fn main_before_the_last_move_comes_from_the_reflog() {
    let fx = fixture().await;
    let before = git(&fx.repo, &["rev-parse", "main"]).await;
    fx.main_writes(&["a.txt"], "moved\n").await;
    assert_eq!(main_before_last_move(&fx.repo).await, Some(before));
}

/// No Lucidos-source change can fail because `main` moved: the lane never
/// produces the concurrent-main error the app path uses for that.
#[test]
fn the_lane_never_reports_a_concurrent_main_conflict() {
    use crate::test_support::source_scan::production_sources;
    let marker = "CONCURRENT_MAIN_CONFLICT";
    let lane_files: Vec<String> = production_sources()
        .into_iter()
        .filter(|(rel, source)| rel.starts_with("engine/landing_lane/") && source.contains(marker))
        .map(|(rel, _)| rel)
        .collect();
    assert!(lane_files.is_empty(), "{lane_files:?}");
}
