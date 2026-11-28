use super::*;

#[tokio::test]
async fn read_only_probes_leave_a_stale_index_alone() {
    let (_tmp, repo) = common::make_test_repo().await;
    let file = repo.join("tracked.txt");
    std::fs::write(&file, "same\n").unwrap();
    common::git_ok(&repo, &["add", "tracked.txt"]).await;
    common::git_ok(&repo, &["commit", "-q", "-m", "track"]).await;
    std::fs::File::options()
        .write(true)
        .open(&file)
        .unwrap()
        .set_modified(std::time::SystemTime::now() + Duration::from_secs(60))
        .unwrap();
    let index = repo.join(".git/index");
    let before = std::fs::read(&index).unwrap();

    git_cmd(&["status", "--porcelain"], &repo).await.unwrap();
    assert_eq!(has_unmerged_paths(&repo).await, GitAnswer::No);

    assert_eq!(
        std::fs::read(&index).unwrap(),
        before,
        "the probe rewrote the index, so it took index.lock"
    );
}
