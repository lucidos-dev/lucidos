use super::*;
use crate::engine::git_ops::common::make_test_repo;

async fn commit_file(repo: &Path, name: &str, body: &str, msg: &str) {
    tokio::fs::write(repo.join(name), body).await.unwrap();
    git_cmd(&["add", "."], repo).await.unwrap();
    git_cmd(&["commit", "-m", msg], repo).await.unwrap();
}

/// A repo whose `clash` branch and `main` both rewrite `init.txt`, and whose
/// `fresh` branch only adds a file.
async fn diverged_repo() -> (tempfile::TempDir, PathBuf) {
    let (tmp, repo) = make_test_repo().await;
    git_cmd(&["checkout", "-b", "clash"], &repo).await.unwrap();
    commit_file(&repo, "init.txt", "from the branch", "branch edit").await;
    git_cmd(&["checkout", "main"], &repo).await.unwrap();
    git_cmd(&["checkout", "-b", "fresh"], &repo).await.unwrap();
    commit_file(&repo, "new.txt", "new", "add a file").await;
    git_cmd(&["checkout", "main"], &repo).await.unwrap();
    commit_file(&repo, "init.txt", "from main", "main edit").await;
    (tmp, repo)
}

async fn refs_and_status(repo: &Path) -> (String, String) {
    let refs = git_cmd(&["for-each-ref"], repo).await.unwrap();
    let status = git_cmd(&["status", "--porcelain"], repo).await.unwrap();
    (
        String::from_utf8_lossy(&refs.stdout).to_string(),
        String::from_utf8_lossy(&status.stdout).to_string(),
    )
}

#[test]
fn merge_tree_output_is_read_by_exit_code() {
    assert_eq!(classify_merge_tree(Some(0), "abc123\n"), MergeProbe::Clean);
    assert_eq!(
        classify_merge_tree(Some(1), "abc123\na.rs\nsrc/b.rs\n"),
        MergeProbe::Conflicts(vec!["a.rs".into(), "src/b.rs".into()])
    );
    // A failure is not a clean merge, whatever stdout holds.
    assert_eq!(classify_merge_tree(Some(128), ""), MergeProbe::Unknown);
    assert_eq!(classify_merge_tree(None, ""), MergeProbe::Unknown);
}

#[tokio::test]
async fn a_branch_editing_what_main_edited_is_predicted_to_conflict() {
    let (_tmp, repo) = diverged_repo().await;
    assert_eq!(
        predict_merge_into_main(&repo, "clash").await,
        MergeProbe::Conflicts(vec!["init.txt".into()])
    );
    assert_eq!(
        predict_merge_into_main(&repo, "fresh").await,
        MergeProbe::Clean
    );
}

#[tokio::test]
async fn a_missing_branch_or_repo_is_unknown_never_clean() {
    let (_tmp, repo) = diverged_repo().await;
    assert_eq!(
        predict_merge_into_main(&repo, "no-such-branch").await,
        MergeProbe::Unknown
    );
    let not_a_repo = tempfile::tempdir().unwrap();
    assert_eq!(
        predict_merge_into_main(not_a_repo.path(), "clash").await,
        MergeProbe::Unknown
    );
}

#[tokio::test]
async fn the_probe_changes_no_ref_index_or_worktree() {
    let (_tmp, repo) = diverged_repo().await;
    let before = refs_and_status(&repo).await;
    let _ = predict_merge_into_main(&repo, "clash").await;
    let _ = predict_merge_into_main(&repo, "fresh").await;
    assert_eq!(refs_and_status(&repo).await, before);
}

/// The answer is reused until a tip moves, and asked again once one does.
#[tokio::test]
async fn a_moved_tip_is_probed_again() {
    let (_tmp, repo) = diverged_repo().await;
    assert_eq!(
        predict_merge_into_main(&repo, "fresh").await,
        MergeProbe::Clean
    );
    git_cmd(&["checkout", "fresh"], &repo).await.unwrap();
    commit_file(
        &repo,
        "init.txt",
        "now the branch edits it too",
        "clash later",
    )
    .await;
    git_cmd(&["checkout", "main"], &repo).await.unwrap();
    assert_eq!(
        predict_merge_into_main(&repo, "fresh").await,
        MergeProbe::Conflicts(vec!["init.txt".into()])
    );
}

fn cached_for(repo: &Path) -> Vec<MergeProbe> {
    let repo = repo.to_string_lossy().to_string();
    PROBE_CACHE
        .lock()
        .expect("probe cache")
        .iter()
        .filter(|((r, _, _), _)| *r == repo)
        .map(|(_, probe)| probe.clone())
        .collect()
}

/// A repeat read at the same tips is a cache hit, so a changes broadcast costs
/// one `rev-parse` per change. An unknown is never cached.
#[tokio::test]
async fn an_answer_is_cached_and_an_unknown_is_not() {
    let (_tmp, repo) = diverged_repo().await;
    let _ = predict_merge_into_main(&repo, "no-such-branch").await;
    assert!(
        cached_for(&repo).is_empty(),
        "an unknown must be asked again"
    );
    let _ = predict_merge_into_main(&repo, "fresh").await;
    assert_eq!(cached_for(&repo), vec![MergeProbe::Clean]);
}
