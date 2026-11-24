use super::*;

pub(crate) async fn git(dir: &Path, args: &[&str]) -> String {
    let out = git_cmd(args, dir).await.unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

async fn commit(repo: &Path, file: &str) -> String {
    std::fs::write(repo.join(file), file).unwrap();
    git(repo, &["add", "."]).await;
    git(repo, &["commit", "-m", file]).await;
    git(repo, &["rev-parse", "HEAD"]).await
}

/// A repository on `main` with one commit.
pub(crate) async fn repo_at(path: &Path) -> PathBuf {
    std::fs::create_dir_all(path).unwrap();
    git(path, &["init", "-b", "main"]).await;
    git(path, &["config", "user.email", "test@example.com"]).await;
    git(path, &["config", "user.name", "Test"]).await;
    commit(path, "seed.txt").await;
    path.to_path_buf()
}

/// Make `branch` in `repo` with one commit beyond `main`, and return its tip.
pub(crate) async fn pending_branch(repo: &Path, branch: &str) -> String {
    git(repo, &["checkout", "-q", "-b", branch]).await;
    let tip = commit(repo, &format!("{branch}.txt")).await;
    git(repo, &["checkout", "-q", "main"]).await;
    tip
}

fn row(repo: &Path, branch: &str) -> (String, String) {
    (repo.to_string_lossy().into_owned(), branch.to_string())
}

async fn branch_tip(repo: &Path, branch: &str) -> Option<String> {
    rev_parse(repo, &format!("refs/heads/{branch}"))
        .await
        .unwrap()
}

#[tokio::test]
async fn bundles_bring_back_the_pending_branches_of_every_repository() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let source = repo_at(&tmp.path().join("lucidos")).await;
    let external = repo_at(&tmp.path().join("external")).await;
    let one = pending_branch(&source, "change-one").await;
    let two = pending_branch(&source, "change-two").await;
    let three = pending_branch(&external, "change-three").await;

    let out = tmp.path().join("staging").join(ARCHIVE_DIR);
    let rows = [
        row(&source, "change-one"),
        row(&source, "change-two"),
        row(&external, "change-three"),
    ];
    assert!(write_bundles(&workspace, &rows, &out).await.unwrap());

    for (repo, branch) in [
        (&source, "change-one"),
        (&source, "change-two"),
        (&external, "change-three"),
    ] {
        git(repo, &["branch", "-D", branch]).await;
    }

    assert_eq!(restore_bundles(&out).await.unwrap(), Vec::<String>::new());
    assert_eq!(branch_tip(&source, "change-one").await, Some(one));
    assert_eq!(branch_tip(&source, "change-two").await, Some(two));
    assert_eq!(branch_tip(&external, "change-three").await, Some(three));
}

#[tokio::test]
async fn nothing_is_bundled_that_the_tar_or_main_already_holds() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    let inside = repo_at(&workspace.join("data")).await;
    pending_branch(&inside, "workspace-change").await;
    let outside = repo_at(&tmp.path().join("outside")).await;
    git(&outside, &["branch", "merged"]).await;
    let plain = tmp.path().join("no-longer-a-checkout");
    std::fs::create_dir_all(&plain).unwrap();

    let out = tmp.path().join(ARCHIVE_DIR);
    let rows = [
        row(&inside, "workspace-change"),
        row(&outside, "merged"),
        row(&outside, "deleted-long-ago"),
        row(&tmp.path().join("gone"), "lost"),
        row(&plain, "lost-too"),
    ];
    assert!(!write_bundles(&workspace, &rows, &out).await.unwrap());
    assert!(!out.exists(), "an empty run writes no directory");
}

#[tokio::test]
async fn a_missing_repository_is_reported_and_the_others_still_restore() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let kept = repo_at(&tmp.path().join("kept")).await;
    let lost = repo_at(&tmp.path().join("lost")).await;
    let tip = pending_branch(&kept, "change").await;
    pending_branch(&lost, "change").await;

    let out = tmp.path().join(ARCHIVE_DIR);
    let rows = [row(&kept, "change"), row(&lost, "change")];
    assert!(write_bundles(&workspace, &rows, &out).await.unwrap());
    git(&kept, &["branch", "-D", "change"]).await;
    std::fs::remove_dir_all(&lost).unwrap();

    let problems = restore_bundles(&out).await.unwrap();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(
        problems[0].contains(&*lost.to_string_lossy()),
        "{problems:?}"
    );
    assert_eq!(branch_tip(&kept, "change").await, Some(tip));
}

#[tokio::test]
async fn a_restore_never_overwrites_newer_work_on_the_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let repo = repo_at(&tmp.path().join("repo")).await;
    pending_branch(&repo, "change").await;

    let out = tmp.path().join(ARCHIVE_DIR);
    assert!(write_bundles(&workspace, &[row(&repo, "change")], &out)
        .await
        .unwrap());
    git(&repo, &["checkout", "-q", "change"]).await;
    git(&repo, &["reset", "-q", "--hard", "main"]).await;
    let diverged = commit(&repo, "other.txt").await;
    git(&repo, &["checkout", "-q", "main"]).await;

    let problems = restore_bundles(&out).await.unwrap();
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(branch_tip(&repo, "change").await, Some(diverged));
}

/// The target clone's `main` lags the source, so it lacks the bundle's
/// prerequisite. The restore fetches `origin` and tries again.
#[tokio::test]
async fn a_lagging_clone_fetches_origin_for_the_prerequisites() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(&workspace).unwrap();
    let origin = tmp.path().join("origin.git");
    let source = repo_at(&tmp.path().join("source")).await;
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            "--bare",
            &source.to_string_lossy(),
            &origin.to_string_lossy(),
        ],
    )
    .await;
    let target = tmp.path().join("target");
    git(
        tmp.path(),
        &[
            "clone",
            "-q",
            &origin.to_string_lossy(),
            &target.to_string_lossy(),
        ],
    )
    .await;
    git(
        &source,
        &["remote", "add", "origin", &origin.to_string_lossy()],
    )
    .await;
    commit(&source, "newer-main.txt").await;
    git(&source, &["push", "-q", "origin", "main"]).await;
    let tip = pending_branch(&source, "change").await;

    let out = tmp.path().join(ARCHIVE_DIR);
    assert!(write_bundles(&workspace, &[row(&source, "change")], &out)
        .await
        .unwrap());
    let mut manifest: Manifest =
        serde_json::from_slice(&std::fs::read(out.join(MANIFEST)).unwrap()).unwrap();
    manifest.repos[0].repo_root = target.to_string_lossy().into_owned();
    std::fs::write(out.join(MANIFEST), serde_json::to_vec(&manifest).unwrap()).unwrap();

    assert_eq!(restore_bundles(&out).await.unwrap(), Vec::<String>::new());
    assert_eq!(branch_tip(&target, "change").await, Some(tip));
}
