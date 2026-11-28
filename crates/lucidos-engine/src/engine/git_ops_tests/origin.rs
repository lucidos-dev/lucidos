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

#[tokio::test]
async fn landing_queue_probe_leaves_main_and_the_queue_unchanged() {
    let f = OriginFixture::new().await;
    let origin_main = f.origin_main().await;
    probe_origin_landing_queue(&f.repo).await.unwrap();
    let queue = git_ok(&f.origin, &["for-each-ref", "refs/lucidos/queue/"]).await;
    assert!(queue.is_empty(), "{queue}");
    assert_eq!(f.origin_main().await, origin_main);
}

#[tokio::test]
async fn landing_queue_probe_refuses_an_origin_that_hides_queue_refs() {
    let f = OriginFixture::new().await;
    git_ok(
        &f.origin,
        &["config", "receive.hideRefs", "refs/lucidos/queue/"],
    )
    .await;
    let error = probe_origin_landing_queue(&f.repo).await.unwrap_err();
    assert!(error.contains("hidden ref"), "{error}");
}

#[tokio::test]
async fn landing_queue_probe_refuses_a_write_only_queue_and_cleans_up() {
    let f = OriginFixture::new().await;
    git_ok(
        &f.origin,
        &["config", "uploadpack.hideRefs", "refs/lucidos/queue/"],
    )
    .await;
    let error = probe_origin_landing_queue(&f.repo).await.unwrap_err();
    assert!(error.contains("not visible"), "{error}");
    let queue = git_ok(&f.origin, &["for-each-ref", "refs/lucidos/queue/"]).await;
    assert!(queue.is_empty(), "{queue}");
}

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

#[tokio::test]
async fn concurrent_pushes_of_main_queue_instead_of_racing() {
    let f = OriginFixture::new().await;
    f.refuse_overlapping_pushes();
    let local = commit_locally(&f, "landed.txt").await;
    let (a, b) = tokio::join!(merge::push_main(&f.repo), merge::push_main(&f.repo));
    assert_eq!((a, b), (None, None));
    assert_eq!(f.origin_main().await, local);
}

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

#[tokio::test]
async fn following_leaves_a_main_ahead_of_origin_alone() {
    let f = OriginFixture::new().await;
    let local = commit_locally(&f, "local.txt").await;
    assert_eq!(follow(&f).await, FollowOutcome::UpToDate);
    assert_eq!(f.local_main().await, local);
}

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
