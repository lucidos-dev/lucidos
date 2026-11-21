//! Fetching from and pushing to `origin`, against a real bare remote.

use super::common::{git_ok, make_test_repo, OriginFixture};
use super::*;

#[test]
fn only_a_moved_origin_counts_as_a_rejected_push() {
    let porcelain = "To origin\n!\tabc:refs/heads/main\t[rejected] (non-fast-forward)\nDone";
    assert!(origin::push_was_rejected(porcelain, ""));
    assert!(origin::push_was_rejected(
        "",
        "! [rejected] main -> main (fetch first)"
    ));
    let hook = "!\tabc:refs/heads/main\t[remote rejected] (pre-receive hook declined)";
    assert!(
        !origin::push_was_rejected(hook, ""),
        "fetching again would not help a hook refusal"
    );
    assert!(!origin::push_was_rejected(
        "",
        "fatal: could not read from remote repository"
    ));
}

#[tokio::test]
async fn a_repo_without_origin_lands_on_local_main() {
    let (_tmp, repo) = make_test_repo().await;
    let local = git_ok(&repo, &["rev-parse", "main"]).await;
    let base = landing_base(&repo, &LandingOrder::OriginFirst)
        .await
        .unwrap();
    assert_eq!(base.base, local);
    assert_eq!(base.origin, OriginTip::NoOrigin);
    assert!(!base.origin.needs_push(&local));
}

/// Another gateway pushed first: the base is `origin/main`, ahead of local.
#[tokio::test]
async fn origin_ahead_of_local_main_is_the_base() {
    let f = OriginFixture::new().await;
    let local = f.local_main().await;
    let pushed = f.other_pushes("other.txt", "other").await;
    let base = landing_base(&f.repo, &LandingOrder::OriginFirst)
        .await
        .unwrap();
    assert_eq!(base.local, local);
    assert_eq!(base.base, pushed);
    assert_eq!(base.origin, OriginTip::At(pushed.clone()));
    assert!(!base.origin.needs_push(&pushed));
}

/// Local commits `origin` lacks are a base too. The next push carries them.
#[tokio::test]
async fn local_main_ahead_of_origin_is_the_base_and_needs_a_push() {
    let f = OriginFixture::new().await;
    let at_origin = f.origin_main().await;
    std::fs::write(f.repo.join("local.txt"), "local").unwrap();
    git_ok(&f.repo, &["add", "."]).await;
    git_ok(&f.repo, &["commit", "-m", "local only"]).await;
    let local = f.local_main().await;
    let base = landing_base(&f.repo, &LandingOrder::OriginFirst)
        .await
        .unwrap();
    assert_eq!(base.base, local);
    assert_eq!(base.origin, OriginTip::At(at_origin));
    assert!(base.origin.needs_push(&local));
}

/// Two lines of history are never merged by the lane. It waits and says why.
#[tokio::test]
async fn diverged_main_and_origin_are_refused() {
    let f = OriginFixture::new().await;
    f.other_pushes("other.txt", "other").await;
    std::fs::write(f.repo.join("local.txt"), "local").unwrap();
    git_ok(&f.repo, &["add", "."]).await;
    git_ok(&f.repo, &["commit", "-m", "local only"]).await;
    match landing_base(&f.repo, &LandingOrder::OriginFirst).await {
        Err(BaseRefusal::Origin(why)) => assert!(why.contains("diverged"), "{why}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn an_unreachable_origin_is_refused_not_skipped() {
    let f = OriginFixture::new().await;
    f.cut_origin().await;
    match landing_base(&f.repo, &LandingOrder::OriginFirst).await {
        Err(BaseRefusal::Origin(why)) => assert!(why.contains("could not fetch"), "{why}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn an_origin_without_main_takes_the_first_push() {
    let f = OriginFixture::new().await;
    git_ok(&f.origin, &["update-ref", "-d", "refs/heads/main"]).await;
    let base = landing_base(&f.repo, &LandingOrder::OriginFirst)
        .await
        .unwrap();
    assert_eq!(base.origin, OriginTip::Empty);
    assert_eq!(
        push_to_origin_main(&f.repo, &base.base).await,
        PushOutcome::Pushed
    );
    assert_eq!(f.origin_main().await, base.base);
}

#[tokio::test]
async fn a_push_behind_origin_is_rejected_and_a_refused_one_fails() {
    let f = OriginFixture::new().await;
    std::fs::write(f.repo.join("local.txt"), "local").unwrap();
    git_ok(&f.repo, &["add", "."]).await;
    git_ok(&f.repo, &["commit", "-m", "local only"]).await;
    let local = f.local_main().await;
    f.other_pushes("other.txt", "other").await;
    assert!(matches!(
        push_to_origin_main(&f.repo, &local).await,
        PushOutcome::Rejected(_)
    ));
    f.refuse_pushes();
    let tip = f.origin_main().await;
    git_ok(&f.repo, &["fetch", "origin"]).await;
    git_ok(&f.repo, &["reset", "--hard", &tip]).await;
    std::fs::write(f.repo.join("again.txt"), "again").unwrap();
    git_ok(&f.repo, &["add", "."]).await;
    git_ok(&f.repo, &["commit", "-m", "again"]).await;
    let again = f.local_main().await;
    assert!(matches!(
        push_to_origin_main(&f.repo, &again).await,
        PushOutcome::Failed(_)
    ));
}

/// An Apply outside the lane pushes loudly: a rejection comes back as a
/// reason, which the caller emits as `MainPushFailed`.
#[tokio::test]
async fn a_push_outside_the_lane_reports_a_rejection() {
    let f = OriginFixture::new().await;
    assert_eq!(merge::push_main(&f.repo).await, None, "nothing to push");
    f.other_pushes("other.txt", "other").await;
    std::fs::write(f.repo.join("local.txt"), "local").unwrap();
    git_ok(&f.repo, &["add", "."]).await;
    git_ok(&f.repo, &["commit", "-m", "local only"]).await;
    let reason = merge::push_main(&f.repo)
        .await
        .expect("the push was refused");
    assert!(reason.contains("origin/main has moved"), "{reason}");

    let (_tmp, no_origin) = make_test_repo().await;
    assert_eq!(merge::push_main(&no_origin).await, None);
}

/// Two Applies that land together each push `main`. The remote refuses a
/// push that overlaps another, as GitHub does with "cannot lock ref", so
/// the pushes must queue.
#[tokio::test]
async fn concurrent_pushes_of_main_queue_instead_of_racing() {
    let f = OriginFixture::new().await;
    f.refuse_overlapping_pushes();
    let local = commit_locally(&f, "landed.txt").await;
    let (a, b) = tokio::join!(merge::push_main(&f.repo), merge::push_main(&f.repo));
    assert_eq!((a, b), (None, None));
    assert_eq!(f.origin_main().await, local);
}

/// A push that queued behind a newer one carries a tip `origin` already holds.
/// That is a success, not a rewind refused as "origin/main has moved".
#[tokio::test]
async fn a_push_of_a_tip_origin_already_holds_succeeds() {
    let f = OriginFixture::new().await;
    let older = commit_locally(&f, "first.txt").await;
    let newer = commit_locally(&f, "second.txt").await;
    assert_eq!(
        push_to_origin_main(&f.repo, &newer).await,
        PushOutcome::Pushed
    );
    assert_eq!(
        push_to_origin_main(&f.repo, &older).await,
        PushOutcome::Pushed
    );
    assert_eq!(f.origin_main().await, newer);
}

/// With `--porcelain` git prints the refused ref on stdout, and the remote's
/// own words on stderr. The reason must carry both.
#[tokio::test]
async fn a_failed_push_names_why_the_remote_refused_it() {
    let f = OriginFixture::new().await;
    f.refuse_pushes();
    commit_locally(&f, "landed.txt").await;
    let reason = merge::push_main(&f.repo)
        .await
        .expect("the push was refused");
    assert!(reason.contains("pre-receive hook declined"), "{reason}");
    assert!(reason.contains("refused by test"), "{reason}");
}

/// Commit `file` on the checkout's own `main`, a commit `origin` lacks.
async fn commit_locally(f: &OriginFixture, file: &str) -> String {
    std::fs::write(f.repo.join(file), "local").unwrap();
    git_ok(&f.repo, &["add", "."]).await;
    git_ok(&f.repo, &["commit", "-m", &format!("local: write {file}")]).await;
    f.local_main().await
}

async fn follow(f: &OriginFixture) -> FollowOutcome {
    follow_origin_main(MainLocks::private_for_test(), &f.repo).await
}

async fn status(dir: &std::path::Path) -> String {
    git_ok(dir, &["status", "--porcelain"]).await
}

/// Another gateway landed: following moves `main` to it as a fast-forward,
/// and the checkout stays clean on `main`.
#[tokio::test]
async fn following_fast_forwards_main_to_origin() {
    let f = OriginFixture::new().await;
    let before = f.local_main().await;
    let pushed = f.other_pushes("other.txt", "other").await;
    assert_eq!(
        follow(&f).await,
        FollowOutcome::Followed {
            from: before,
            to: pushed.clone()
        }
    );
    assert_eq!(f.local_main().await, pushed);
    assert_eq!(
        git_ok(&f.repo, &["symbolic-ref", "HEAD"]).await,
        "refs/heads/main"
    );
    assert_eq!(
        status(&f.repo).await,
        "",
        "following never dirties the tree"
    );
    assert!(f.repo.join("other.txt").exists(), "the tree follows main");
    let parents = git_ok(&f.repo, &["rev-list", "--parents", "-n1", "main"]).await;
    assert_eq!(parents.split(' ').count(), 2, "no merge commit: {parents}");
}

#[tokio::test]
async fn following_with_nothing_new_moves_nothing() {
    let f = OriginFixture::new().await;
    let before = f.local_main().await;
    assert_eq!(follow(&f).await, FollowOutcome::UpToDate);
    assert_eq!(f.local_main().await, before);
}

#[tokio::test]
async fn following_without_origin_does_nothing() {
    let (_tmp, repo) = make_test_repo().await;
    assert_eq!(
        follow_origin_main(MainLocks::private_for_test(), &repo).await,
        FollowOutcome::NoOrigin
    );
}

#[tokio::test]
async fn following_an_unreachable_origin_says_so() {
    let f = OriginFixture::new().await;
    f.cut_origin().await;
    assert!(matches!(follow(&f).await, FollowOutcome::Unreachable(_)));
}

/// Local commits a push has not carried yet stay. The next push takes them.
#[tokio::test]
async fn following_leaves_a_main_ahead_of_origin_alone() {
    let f = OriginFixture::new().await;
    let local = commit_locally(&f, "local.txt").await;
    assert_eq!(follow(&f).await, FollowOutcome::UpToDate);
    assert_eq!(f.local_main().await, local);
}

/// Two lines of history are refused, never merged.
#[tokio::test]
async fn following_refuses_a_diverged_main() {
    let f = OriginFixture::new().await;
    f.other_pushes("other.txt", "other").await;
    let local = commit_locally(&f, "local.txt").await;
    match follow(&f).await {
        FollowOutcome::Refused(why) => assert!(why.contains("diverged"), "{why}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(f.local_main().await, local);
}

/// An uncommitted edit elsewhere rides along. One the new commits touch
/// refuses the follow, and the edit survives.
#[tokio::test]
async fn following_keeps_uncommitted_edits() {
    let f = OriginFixture::new().await;
    std::fs::write(f.repo.join("shared.txt"), "my edit\n").unwrap();
    let pushed = f.other_pushes("other.txt", "other").await;
    assert!(matches!(follow(&f).await, FollowOutcome::Followed { .. }));
    assert_eq!(f.local_main().await, pushed);
    assert_eq!(
        std::fs::read_to_string(f.repo.join("shared.txt")).unwrap(),
        "my edit\n"
    );

    let before = f.local_main().await;
    f.other_pushes("shared.txt", "theirs\n").await;
    assert!(matches!(follow(&f).await, FollowOutcome::Refused(_)));
    assert_eq!(f.local_main().await, before);
    assert_eq!(
        std::fs::read_to_string(f.repo.join("shared.txt")).unwrap(),
        "my edit\n"
    );
}

/// A checkout on another branch keeps its branch and its tree. Only the
/// `main` ref moves, and a worktree stays where it was.
#[tokio::test]
async fn following_moves_only_main_when_head_is_elsewhere() {
    let f = OriginFixture::new().await;
    git_ok(&f.repo, &["checkout", "-b", "topic"]).await;
    let wt = f.repo.with_file_name("wt");
    git_ok(
        &f.repo,
        &["worktree", "add", "-b", "session", wt.to_str().unwrap()],
    )
    .await;
    let pushed = f.other_pushes("other.txt", "other").await;
    assert!(matches!(follow(&f).await, FollowOutcome::Followed { .. }));
    assert_eq!(f.local_main().await, pushed);
    assert_eq!(
        git_ok(&f.repo, &["symbolic-ref", "HEAD"]).await,
        "refs/heads/topic"
    );
    assert!(!f.repo.join("other.txt").exists());
    assert_eq!(status(&f.repo).await, "");
    assert_eq!(
        git_ok(&wt, &["symbolic-ref", "HEAD"]).await,
        "refs/heads/session"
    );
    assert_eq!(status(&wt).await, "");
}
