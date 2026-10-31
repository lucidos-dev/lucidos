use super::*;

/// Helper: create a temp git repo with an initial commit on main.
/// Returns (tempdir_guard, repo_path).
pub(crate) async fn make_test_repo() -> (tempfile::TempDir, std::path::PathBuf) {
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

/// Run git in `dir` and return its stdout. Panics when git fails.
pub(crate) async fn git_ok(dir: &std::path::Path, args: &[&str]) -> String {
    let out = git_cmd(args, dir).await.unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A bare `origin`, the checkout under test, and a second clone that plays
/// another gateway pushing to the same `origin`.
pub(crate) struct OriginFixture {
    _tmp: tempfile::TempDir,
    pub(crate) origin: std::path::PathBuf,
    pub(crate) repo: std::path::PathBuf,
    pub(crate) other: std::path::PathBuf,
}

impl OriginFixture {
    pub(crate) async fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let origin = tmp.path().join("origin.git");
        let repo = tmp.path().join("repo");
        let other = tmp.path().join("other");
        std::fs::create_dir(&repo).unwrap();
        git_ok(
            tmp.path(),
            &["init", "--bare", "-b", "main", origin.to_str().unwrap()],
        )
        .await;
        git_ok(&repo, &["init", "-b", "main"]).await;
        Self::identify(&repo).await;
        std::fs::write(repo.join("shared.txt"), "base\n").unwrap();
        git_ok(&repo, &["add", "."]).await;
        git_ok(&repo, &["commit", "-m", "init"]).await;
        git_ok(
            &repo,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        )
        .await;
        git_ok(&repo, &["push", "origin", "main"]).await;
        git_ok(
            tmp.path(),
            &["clone", origin.to_str().unwrap(), other.to_str().unwrap()],
        )
        .await;
        Self::identify(&other).await;
        Self {
            _tmp: tmp,
            origin,
            repo,
            other,
        }
    }

    async fn identify(dir: &std::path::Path) {
        git_ok(dir, &["config", "user.email", "test@example.com"]).await;
        git_ok(dir, &["config", "user.name", "Test"]).await;
    }

    /// The other clone commits `file` on its `main` and pushes it. Returns
    /// the new `origin/main`.
    pub(crate) async fn other_pushes(&self, file: &str, text: &str) -> String {
        git_ok(&self.other, &["pull", "--ff-only", "origin", "main"]).await;
        std::fs::write(self.other.join(file), text).unwrap();
        git_ok(&self.other, &["add", "."]).await;
        git_ok(
            &self.other,
            &["commit", "-m", &format!("other: write {file}")],
        )
        .await;
        git_ok(&self.other, &["push", "origin", "main"]).await;
        self.origin_main().await
    }

    pub(crate) async fn origin_main(&self) -> String {
        git_ok(&self.origin, &["rev-parse", "main"]).await
    }

    pub(crate) async fn local_main(&self) -> String {
        git_ok(&self.repo, &["rev-parse", "main"]).await
    }

    /// Point `origin` at a path that does not exist, as an unreachable host.
    pub(crate) async fn cut_origin(&self) {
        let gone = self.origin.with_file_name("gone.git");
        git_ok(
            &self.repo,
            &["remote", "set-url", "origin", gone.to_str().unwrap()],
        )
        .await;
    }

    /// Let `origin` refuse every push, as a server that rejects the update.
    pub(crate) fn refuse_pushes(&self) {
        let hook = self.origin.join("hooks").join("pre-receive");
        std::fs::write(&hook, "#!/bin/sh\necho refused by test >&2\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Refuse a push while another is in flight, as GitHub's ref lock does.
    pub(crate) fn refuse_overlapping_pushes(&self) {
        let hook = self.origin.join("hooks").join("pre-receive");
        std::fs::write(
            &hook,
            "#!/bin/sh\nmkdir inflight 2>/dev/null || { echo cannot lock ref >&2; exit 1; }\n\
             sleep 1\nrmdir inflight\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}
