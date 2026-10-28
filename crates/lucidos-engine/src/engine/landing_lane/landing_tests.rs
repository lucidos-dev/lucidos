//! Landing merges against real repositories.

use super::*;
use crate::engine::change_ops::revert_landing_merge;
use std::path::PathBuf;

async fn git(repo: &Path, args: &[&str]) -> String {
    let out = git_cmd(args, repo).await.unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

async fn repo() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]).await;
    git(&repo, &["config", "user.email", "test@example.com"]).await;
    git(&repo, &["config", "user.name", "Test"]).await;
    std::fs::write(repo.join("shared.txt"), "base\n").unwrap();
    git(&repo, &["add", "."]).await;
    git(&repo, &["commit", "-m", "init"]).await;
    (tmp, repo)
}

/// A branch off `main` that writes `file`, returning its head.
async fn branch_writing(repo: &Path, branch: &str, file: &str, text: &str) -> String {
    git(repo, &["branch", branch, "main"]).await;
    let tree = tempfile::tempdir().unwrap();
    let wt = tree.path().join("wt");
    git(repo, &["worktree", "add", wt.to_str().unwrap(), branch]).await;
    std::fs::write(wt.join(file), text).unwrap();
    git(&wt, &["add", "."]).await;
    git(&wt, &["commit", "-m", &format!("{branch}: write {file}")]).await;
    let head = git(&wt, &["rev-parse", "HEAD"]).await;
    git(
        repo,
        &["worktree", "remove", "--force", wt.to_str().unwrap()],
    )
    .await;
    head
}

fn input(n: u8, snapshot: &str) -> LandingInput {
    LandingInput {
        change_id: Uuid::from_bytes([n; 16]),
        snapshot_sha: snapshot.to_string(),
        subject: format!("Land: change {n}"),
        body: format!("Branch: b{n}"),
    }
}

fn landed(chain: &LandingChain, n: usize) -> LandingMerge {
    match &chain.steps[n].1 {
        LandingStep::Landed(merge) => merge.clone(),
        other => panic!("step {n} did not land: {other:?}"),
    }
}

/// Three members, three landing merges, one line each on the first-parent
/// history, and the repo-root tree follows `main`.
#[tokio::test]
async fn three_members_land_as_three_merges_in_first_parent_order() {
    let (_tmp, repo) = repo().await;
    let a = branch_writing(&repo, "b1", "a.txt", "a").await;
    let b = branch_writing(&repo, "b2", "b.txt", "b").await;
    let c = branch_writing(&repo, "b3", "c.txt", "c").await;
    let base = git(&repo, &["rev-parse", "main"]).await;

    let published = publish_clean_pass(
        MainLocks::private_for_test(),
        &repo,
        &LandingOrder::OriginFirst,
        &[input(1, &a), input(2, &b), input(3, &c)],
    )
    .await
    .unwrap()
    .published();
    assert_eq!(published.failed, None);
    assert_eq!(published.base, base);

    let merges: Vec<LandingMerge> = (0..3).map(|n| landed(&published.chain, n)).collect();
    let first_parent_line = git(&repo, &["log", "--first-parent", "--format=%H", "main"]).await;
    let line: Vec<&str> = first_parent_line.lines().collect();
    assert_eq!(
        line,
        vec![
            merges[2].merge_sha.as_str(),
            merges[1].merge_sha.as_str(),
            merges[0].merge_sha.as_str(),
            base.as_str()
        ]
    );
    assert_eq!(merges[0].first_parent, base);
    assert_eq!(merges[1].first_parent, merges[0].merge_sha);
    for (merge, snapshot) in merges.iter().zip([&a, &b, &c]) {
        let parents = git(&repo, &["log", "-1", "--format=%P", &merge.merge_sha]).await;
        assert_eq!(parents, format!("{} {}", merge.first_parent, snapshot));
    }
    assert_eq!(
        git(&repo, &["log", "-1", "--format=%s", &merges[1].merge_sha]).await,
        "Land: change 2"
    );
    for f in ["a.txt", "b.txt", "c.txt"] {
        assert!(repo.join(f).exists(), "the repo-root tree holds {f}");
    }
}

/// A member that conflicts is set aside, and the members after it still land.
#[tokio::test]
async fn a_conflicting_member_is_set_aside_and_the_rest_land() {
    let (_tmp, repo) = repo().await;
    let clean = branch_writing(&repo, "b1", "a.txt", "a").await;
    let clash = branch_writing(&repo, "b2", "shared.txt", "theirs\n").await;
    std::fs::write(repo.join("shared.txt"), "ours\n").unwrap();
    git(&repo, &["commit", "-am", "main edits shared"]).await;
    let later = branch_writing(&repo, "b3", "c.txt", "c").await;

    let published = publish_clean_pass(
        MainLocks::private_for_test(),
        &repo,
        &LandingOrder::OriginFirst,
        &[input(1, &clean), input(2, &clash), input(3, &later)],
    )
    .await
    .unwrap()
    .published();
    assert_eq!(published.chain.steps[1].1, LandingStep::SetAside);
    let first = landed(&published.chain, 0);
    let third = landed(&published.chain, 2);
    assert_eq!(third.first_parent, first.merge_sha, "the chain skips it");
    assert_eq!(git(&repo, &["rev-parse", "main"]).await, third.merge_sha);
    assert_eq!(
        std::fs::read_to_string(repo.join("shared.txt")).unwrap(),
        "ours\n"
    );
}

/// A batch a pass opens starts on `main` as the pass left it. When the same
/// pass landed a member, that is its landing merge, not `main` as it read it.
/// Otherwise the integration branch would miss what the pass just landed.
#[tokio::test]
async fn a_batch_the_pass_opens_starts_on_main_as_the_pass_left_it() {
    let (_tmp, repo) = repo().await;
    let clean = branch_writing(&repo, "b1", "a.txt", "a").await;
    let clash = branch_writing(&repo, "b2", "shared.txt", "theirs\n").await;
    std::fs::write(repo.join("shared.txt"), "ours\n").unwrap();
    git(&repo, &["commit", "-am", "main edits shared"]).await;

    let published = publish_clean_pass(
        MainLocks::private_for_test(),
        &repo,
        &LandingOrder::OriginFirst,
        &[input(1, &clean), input(2, &clash)],
    )
    .await
    .unwrap()
    .published();
    assert_eq!(published.chain.steps[1].1, LandingStep::SetAside);
    let main = git(&repo, &["rev-parse", "main"]).await;
    assert_ne!(main, published.base, "the pass moved main");
    assert_eq!(published.main_now(), main);

    let again = publish_clean_pass(
        MainLocks::private_for_test(),
        &repo,
        &LandingOrder::OriginFirst,
        &[input(2, &clash)],
    )
    .await
    .unwrap()
    .published();
    assert_eq!(
        again.main_now(),
        main,
        "a pass that lands nothing keeps main"
    );
}

#[tokio::test]
async fn a_member_already_on_main_lands_nothing() {
    let (_tmp, repo) = repo().await;
    let base = git(&repo, &["rev-parse", "main"]).await;
    let published = publish_clean_pass(
        MainLocks::private_for_test(),
        &repo,
        &LandingOrder::OriginFirst,
        &[input(1, &base)],
    )
    .await
    .unwrap()
    .published();
    assert_eq!(published.chain.steps[0].1, LandingStep::AlreadyOnMain(None));
    assert_eq!(git(&repo, &["rev-parse", "main"]).await, base);
    assert_eq!(published.main_now(), base);
}

#[tokio::test]
async fn an_unresolvable_snapshot_is_set_aside() {
    let (_tmp, repo) = repo().await;
    let base = git(&repo, &["rev-parse", "main"]).await;
    let missing = "0123456789012345678901234567890123456789";
    let published = publish_clean_pass(
        MainLocks::private_for_test(),
        &repo,
        &LandingOrder::OriginFirst,
        &[input(1, missing)],
    )
    .await
    .unwrap()
    .published();
    assert_eq!(published.chain.steps[0].1, LandingStep::SetAside);
    assert_eq!(git(&repo, &["rev-parse", "main"]).await, base);
}

/// The lane lands exactly the snapshot it queued. A commit the source thread
/// makes meanwhile stays on its branch and off `main`.
#[tokio::test]
async fn the_lane_lands_the_snapshot_and_keeps_a_later_commit_on_the_branch() {
    let (_tmp, repo) = repo().await;
    let snapshot = branch_writing(&repo, "b1", "a.txt", "a").await;
    let wt_dir = tempfile::tempdir().unwrap();
    let wt = wt_dir.path().join("wt");
    git(&repo, &["worktree", "add", wt.to_str().unwrap(), "b1"]).await;
    std::fs::write(wt.join("later.txt"), "later").unwrap();
    git(&wt, &["add", "."]).await;
    git(&wt, &["commit", "-m", "later work"]).await;
    let later = git(&wt, &["rev-parse", "HEAD"]).await;

    let published = publish_clean_pass(
        MainLocks::private_for_test(),
        &repo,
        &LandingOrder::OriginFirst,
        &[input(1, &snapshot)],
    )
    .await
    .unwrap()
    .published();
    let merge = landed(&published.chain, 0);
    assert_eq!(git(&repo, &["rev-parse", "b1"]).await, later);
    let on_main = git_answer(&["merge-base", "--is-ancestor", &later, "main"], &repo).await;
    assert_eq!(on_main, GitAnswer::No, "the later commit did not land");
    assert!(!repo.join("later.txt").exists());
    assert_eq!(
        git(&repo, &["merge-base", "main", "b1"]).await,
        snapshot,
        "the branch's new work starts at the snapshot"
    );
    assert_eq!(
        find_landing_merge(&repo, &snapshot).await.unwrap(),
        Some(merge)
    );
}

/// A crash between moving `main` and announcing the landing heals: recovery
/// finds the merge whose second parent is the snapshot.
#[tokio::test]
async fn recovery_finds_the_landing_merge_of_a_snapshot() {
    let (_tmp, repo) = repo().await;
    let a = branch_writing(&repo, "b1", "a.txt", "a").await;
    let b = branch_writing(&repo, "b2", "b.txt", "b").await;
    let published = publish_clean_pass(
        MainLocks::private_for_test(),
        &repo,
        &LandingOrder::OriginFirst,
        &[input(1, &a), input(2, &b)],
    )
    .await
    .unwrap()
    .published();
    assert_eq!(
        find_landing_merge(&repo, &b).await.unwrap(),
        Some(landed(&published.chain, 1))
    );
    let unlanded = branch_writing(&repo, "b3", "c.txt", "c").await;
    assert_eq!(find_landing_merge(&repo, &unlanded).await.unwrap(), None);
}

#[test]
fn a_rev_list_line_names_the_merge_and_its_parents() {
    let line = "m1 p1 snap";
    assert_eq!(
        landing_merge_of(line, "snap"),
        Some(LandingMerge {
            first_parent: "p1".into(),
            merge_sha: "m1".into()
        })
    );
    assert_eq!(landing_merge_of(line, "other"), None);
    assert_eq!(
        landing_merge_of("m1 p1", "p1"),
        None,
        "a first parent is not a snapshot"
    );
}

/// Every mover of `main` takes the merge lock first. A Revert pressed while a
/// pass holds it waits, so the pass's guarded move still lands. The revert
/// then follows on the new `main`.
#[tokio::test]
async fn a_revert_waits_for_a_landing_pass_and_cannot_fail_it() {
    use crate::engine::change_ops::{revert_on_main, RevertTarget};
    let (_tmp, repo) = repo().await;
    let locks = MainLocks::private_for_test();
    let first = branch_writing(&repo, "b1", "a.txt", "a").await;
    let published = publish_clean_pass(
        locks,
        &repo,
        &LandingOrder::OriginFirst,
        &[input(1, &first)],
    )
    .await
    .unwrap()
    .published();
    let first_merge = landed(&published.chain, 0);
    let second = branch_writing(&repo, "b2", "b.txt", "b").await;

    // A pass between its read of `main` and its move.
    let pass = locks.lock_merge().await;
    let base = git(&repo, &["rev-parse", "main"]).await;
    let chain = build_landing_chain(&repo, &base, &[input(2, &second)]).await;

    let revert_repo = repo.clone();
    let mut revert = tokio::spawn(async move {
        revert_on_main(
            locks,
            &revert_repo,
            RevertTarget::LandingMerge(&first_merge.merge_sha),
        )
        .await
        .map_err(|e| e.to_string())
    });
    let finished = tokio::time::timeout(std::time::Duration::from_millis(500), &mut revert).await;
    assert!(finished.is_err(), "the revert waits for the pass");
    assert_eq!(
        git(&repo, &["rev-parse", "main"]).await,
        base,
        "main held still"
    );

    let held = locks.lock_repo().await;
    ff_main_to_locked(&held, &repo, &chain.tip, &base)
        .await
        .expect("the pass's guarded move lands");
    drop(held);
    drop(pass);
    revert
        .await
        .unwrap()
        .expect("the revert runs once the pass is done");
    assert!(!repo.join("a.txt").exists(), "the first change is undone");
    assert!(repo.join("b.txt").exists(), "the pass's change stays");
}

/// Revert undoes exactly one change: the middle member's lines go, and the
/// members on either side keep theirs.
#[tokio::test]
async fn reverting_a_landing_merge_undoes_that_change_only() {
    let (_tmp, repo) = repo().await;
    let a = branch_writing(&repo, "b1", "a.txt", "a").await;
    let b = branch_writing(&repo, "b2", "b.txt", "b").await;
    let c = branch_writing(&repo, "b3", "c.txt", "c").await;
    let published = publish_clean_pass(
        MainLocks::private_for_test(),
        &repo,
        &LandingOrder::OriginFirst,
        &[input(1, &a), input(2, &b), input(3, &c)],
    )
    .await
    .unwrap()
    .published();
    let middle = landed(&published.chain, 1);

    revert_landing_merge(&repo, &middle.merge_sha)
        .await
        .unwrap();

    assert!(repo.join("a.txt").exists());
    assert!(!repo.join("b.txt").exists(), "the middle change is undone");
    assert!(repo.join("c.txt").exists());
}

mod with_origin {
    //! A pass against a bare `origin` (ADR 0589).

    use super::*;
    use crate::engine::git_ops::common::{git_ok, OriginFixture};

    async fn pass(f: &OriginFixture, inputs: &[LandingInput]) -> CleanPass {
        pass_in_order(f, &LandingOrder::OriginFirst, inputs).await
    }

    async fn pass_in_order(
        f: &OriginFixture,
        order: &LandingOrder,
        inputs: &[LandingInput],
    ) -> CleanPass {
        publish_clean_pass(MainLocks::private_for_test(), &f.repo, order, inputs)
            .await
            .unwrap()
    }

    /// Unpaired, a pass lands locally and never reads `origin`, so an
    /// unreachable one stops nothing. The driver pushes after.
    #[tokio::test]
    async fn a_local_first_pass_lands_without_origin() {
        let f = OriginFixture::new().await;
        let before = f.origin_main().await;
        let a = branch_writing(&f.repo, "b1", "a.txt", "a").await;
        f.cut_origin().await;
        let published = pass_in_order(&f, &LandingOrder::LocalFirst, &[input(1, &a)])
            .await
            .published();
        assert!(published.moved_main());
        assert_eq!(f.local_main().await, landed(&published.chain, 0).merge_sha);
        assert_eq!(f.origin_main().await, before);
    }

    /// The gateway did not say whether it is paired: the pass waits, as an
    /// origin-first one would, and says why.
    #[tokio::test]
    async fn an_unknown_order_waits_and_moves_nothing() {
        let f = OriginFixture::new().await;
        let before = f.local_main().await;
        let a = branch_writing(&f.repo, "b1", "a.txt", "a").await;
        let order = LandingOrder::Unknown("the gateway is down".into());
        let CleanPass::WaitingForOrigin(why) = pass_in_order(&f, &order, &[input(1, &a)]).await
        else {
            panic!("the pass must wait");
        };
        assert_eq!(why, "the gateway is down");
        assert_eq!(f.local_main().await, before);
    }

    /// A landing counts once `origin/main` holds it, and local `main` is the
    /// same commit.
    #[tokio::test]
    async fn a_landing_is_on_origin_before_main_moves() {
        let f = OriginFixture::new().await;
        let a = branch_writing(&f.repo, "b1", "a.txt", "a").await;
        let published = pass(&f, &[input(1, &a)]).await.published();
        let merge = landed(&published.chain, 0);
        assert_eq!(f.origin_main().await, merge.merge_sha);
        assert_eq!(f.local_main().await, merge.merge_sha);
    }

    /// Another gateway pushed first. The pass builds on its tip, so local
    /// `main` stays an ancestor of `origin/main` and nothing diverges.
    #[tokio::test]
    async fn a_pass_builds_on_what_another_gateway_pushed() {
        let f = OriginFixture::new().await;
        let a = branch_writing(&f.repo, "b1", "a.txt", "a").await;
        let theirs = f.other_pushes("other.txt", "other").await;
        let published = pass(&f, &[input(1, &a)]).await.published();
        let merge = landed(&published.chain, 0);
        assert_eq!(published.base, theirs);
        assert_eq!(merge.first_parent, theirs);
        assert_eq!(f.origin_main().await, merge.merge_sha);
        assert_eq!(f.local_main().await, merge.merge_sha);
        assert!(f.repo.join("other.txt").exists());
    }

    /// An outside push that conflicts sets the member aside for a batch.
    /// Local `main` follows `origin`, and nothing is pushed.
    #[tokio::test]
    async fn an_outside_push_that_conflicts_sets_the_member_aside() {
        let f = OriginFixture::new().await;
        let clash = branch_writing(&f.repo, "b1", "shared.txt", "mine\n").await;
        let theirs = f.other_pushes("shared.txt", "theirs\n").await;
        let published = pass(&f, &[input(1, &clash)]).await.published();
        assert_eq!(published.chain.steps[0].1, LandingStep::SetAside);
        assert_eq!(published.main_now(), theirs, "a batch starts on origin");
        assert_eq!(f.origin_main().await, theirs);
        assert_eq!(f.local_main().await, theirs);
    }

    /// A push refused because `origin` moved rebuilds the pass. The hook
    /// refuses the first push only, the way a race with another lane would.
    #[tokio::test]
    async fn a_rejected_push_rebuilds_and_lands() {
        let f = OriginFixture::new().await;
        let a = branch_writing(&f.repo, "b1", "a.txt", "a").await;
        let hook = f.origin.join("hooks").join("pre-receive");
        let flag = f.origin.join("refused-once");
        let script = format!(
            "#!/bin/sh\nif [ ! -e '{flag}' ]; then touch '{flag}'; echo 'non-fast-forward' >&2; exit 1; fi\n",
            flag = flag.display()
        );
        std::fs::write(&hook, script).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

        let published = pass(&f, &[input(1, &a)]).await.published();
        let merge = landed(&published.chain, 0);
        assert!(flag.exists(), "the first push was refused");
        assert_eq!(f.origin_main().await, merge.merge_sha);
    }

    /// GitHub out of reach: the pass waits, says why, and moves nothing.
    #[tokio::test]
    async fn an_unreachable_origin_moves_nothing_and_says_why() {
        let f = OriginFixture::new().await;
        let before = f.local_main().await;
        let a = branch_writing(&f.repo, "b1", "a.txt", "a").await;
        f.cut_origin().await;
        let CleanPass::WaitingForOrigin(why) = pass(&f, &[input(1, &a)]).await else {
            panic!("the pass must wait");
        };
        assert!(why.contains("could not fetch"), "{why}");
        assert_eq!(f.local_main().await, before);
    }

    /// `origin` refuses the push outright: the pass waits and local `main`
    /// never gets a merge `origin` lacks.
    #[tokio::test]
    async fn a_refused_push_leaves_main_alone() {
        let f = OriginFixture::new().await;
        let before = f.local_main().await;
        let a = branch_writing(&f.repo, "b1", "a.txt", "a").await;
        f.refuse_pushes();
        let CleanPass::WaitingForOrigin(why) = pass(&f, &[input(1, &a)]).await else {
            panic!("the pass must wait");
        };
        assert!(why.contains("could not push"), "{why}");
        assert_eq!(f.local_main().await, before);
        assert_eq!(f.origin_main().await, before);
    }

    /// A push landed on `origin` but reported a failure, so the pass waited.
    /// The next pass finds the member on `main` and announces the merge it
    /// already has, so the restart flag and the merge survive.
    #[tokio::test]
    async fn a_push_whose_answer_was_lost_lands_with_its_merge() {
        use crate::engine::landing_lane::driver::PassOutcome;
        let f = OriginFixture::new().await;
        let a = branch_writing(&f.repo, "b1", "a.txt", "a").await;
        let base = f.local_main().await;
        let lost = build_landing_chain(&f.repo, &base, &[input(1, &a)]).await;
        let merge = landed(&lost, 0);
        assert_eq!(
            push_to_origin_main(&f.repo, &lost.tip).await,
            PushOutcome::Pushed
        );
        assert_eq!(f.local_main().await, base, "the pass read a failure");

        let published = pass(&f, &[input(1, &a)]).await.published();
        let step = &published.chain.steps[0].1;
        assert_eq!(*step, LandingStep::AlreadyOnMain(Some(merge.clone())));
        assert_eq!(PassOutcome::of(step, None), PassOutcome::Lands(&merge));
        assert_eq!(f.local_main().await, merge.merge_sha);
    }

    /// A crash between the push and the local move: recovery finds the
    /// landing merge on `origin/main`.
    #[tokio::test]
    async fn recovery_finds_a_landing_merge_only_origin_holds() {
        let f = OriginFixture::new().await;
        let a = branch_writing(&f.repo, "b1", "a.txt", "a").await;
        let base = f.local_main().await;
        let chain = build_landing_chain(&f.repo, &base, &[input(1, &a)]).await;
        let merge = landed(&chain, 0);
        assert_eq!(
            push_to_origin_main(&f.repo, &chain.tip).await,
            PushOutcome::Pushed
        );
        assert_eq!(f.local_main().await, base, "the crash: main did not move");
        git_ok(&f.repo, &["fetch", "origin"]).await;
        assert_eq!(find_landing_merge(&f.repo, &a).await.unwrap(), Some(merge));
    }
}
