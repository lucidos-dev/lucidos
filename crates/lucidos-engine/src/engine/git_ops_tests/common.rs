use super::*;

pub(crate) use crate::core::git::test_support::make_test_repo;

pub(crate) async fn git_ok(dir: &std::path::Path, args: &[&str]) -> String {
    let out = git_cmd(args, dir).await.unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

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

    pub(crate) async fn cut_origin(&self) {
        let gone = self.origin.with_file_name("gone.git");
        git_ok(
            &self.repo,
            &["remote", "set-url", "origin", gone.to_str().unwrap()],
        )
        .await;
    }

    pub(crate) fn refuse_pushes(&self) {
        let hook = self.origin.join("hooks").join("pre-receive");
        std::fs::write(&hook, "#!/bin/sh\necho refused by test >&2\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

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
