use super::*;
use crate::engine::git_ops::git_cmd;

/// A clone with a sibling directory, and a worktree of it inside a workspace:
///
/// ```text
/// <tmp>/projects/repo          main checkout
/// <tmp>/projects/sibling       what `../sibling` means from the clone
/// <tmp>/ws/.lucidos/worktrees/thread-1
/// ```
struct Fixture {
    _tmp: tempfile::TempDir,
    main: PathBuf,
    worktree: PathBuf,
    workspace: PathBuf,
    sibling: PathBuf,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_git_dir(None).await
    }

    /// `git_dir` places the clone's `.git` elsewhere (`--separate-git-dir`),
    /// relative to the temp root.
    async fn with_git_dir(git_dir: Option<&str>) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(tmp.path()).unwrap();
        let main = root.join("projects/repo");
        let sibling = root.join("projects/sibling");
        let workspace = root.join("ws");
        let worktree = workspace.join(".lucidos/worktrees/thread-1");
        for dir in [&main, &sibling, &workspace.join("data")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        let git_dir = git_dir.map(|dir| {
            let dir = root.join(dir);
            std::fs::create_dir_all(dir.parent().unwrap()).unwrap();
            dir.to_string_lossy().into_owned()
        });
        let init: Vec<&str> = match &git_dir {
            Some(dir) => vec!["init", "-b", "main", "--separate-git-dir", dir],
            None => vec!["init", "-b", "main"],
        };
        for args in [
            &init[..],
            &["config", "user.email", "test@example.com"],
            &["config", "user.name", "Test"],
            &["commit", "--allow-empty", "-m", "init"],
        ] {
            git_cmd(args, &main).await.unwrap();
        }
        std::fs::create_dir_all(worktree.parent().unwrap()).unwrap();
        let out = git_cmd(
            &[
                "worktree",
                "add",
                worktree.to_str().unwrap(),
                "-b",
                "thread-1",
            ],
            &main,
        )
        .await
        .unwrap();
        assert!(out.status.success(), "git worktree add failed");
        Self {
            _tmp: tmp,
            main,
            worktree,
            workspace,
            sibling,
        }
    }

    fn settings(&self, dir: &Path, body: &str) {
        std::fs::create_dir_all(dir.join(".claude")).unwrap();
        std::fs::write(dir.join(REPO_CC_SETTINGS), body).unwrap();
    }

    fn entries(&self, entries: serde_json::Value) {
        let body = serde_json::json!({ "permissions": { "additionalDirectories": entries } });
        self.settings(&self.worktree, &body.to_string());
    }

    async fn grants(&self) -> Vec<PathBuf> {
        resolve(&self.worktree, &self.workspace).await
    }
}

/// The reported bug: `../sibling` from a worktree named nothing.
#[tokio::test]
async fn an_entry_leaving_the_repo_resolves_against_the_main_checkout() {
    let fx = Fixture::new().await;
    fx.entries(serde_json::json!(["../sibling/"]));
    assert_eq!(fx.grants().await, vec![fx.sibling.clone()]);
}

/// Git records no main checkout for a `--separate-git-dir` clone, and names its
/// git dir instead. Anchoring there would make `..` grant the git dir's parent.
#[tokio::test]
async fn a_clone_with_its_git_dir_elsewhere_grants_nothing() {
    let fx = Fixture::with_git_dir(Some("gitdirs/repo.git")).await;
    fx.entries(serde_json::json!(["../sibling", ".."]));
    assert!(fx.grants().await.is_empty());
}

/// A submodule keeps its shared `.git` outside its checkout, so the main
/// checkout rule alone would not cover it.
#[tokio::test]
async fn a_grant_never_overlaps_the_repos_git_dir() {
    let fx = Fixture::new().await;
    let protected = Protected::find(&fx.worktree, &fx.workspace).await.unwrap();
    assert_eq!(protected.git_dir, fx.main.join(".git"));
    let elsewhere = Protected {
        git_dir: fx.sibling.join("modules/repo"),
        ..protected
    };
    std::fs::create_dir_all(&elsewhere.git_dir).unwrap();
    assert!(reanchor("../sibling", &elsewhere).is_err());
    assert!(reanchor("../sibling/modules/repo", &elsewhere).is_err());
}

#[tokio::test]
async fn two_spellings_of_one_directory_grant_it_once() {
    let fx = Fixture::new().await;
    fx.entries(serde_json::json!([
        "../sibling",
        "../sibling/",
        "./../sibling"
    ]));
    assert_eq!(fx.grants().await, vec![fx.sibling.clone()]);
}

/// An entry inside the repo already means the worktree's own copy. Moving it
/// would point the agent at the user's working tree.
#[tokio::test]
async fn an_entry_inside_the_repo_is_left_to_claude_code() {
    let fx = Fixture::new().await;
    std::fs::create_dir_all(fx.main.join("docs")).unwrap();
    std::fs::create_dir_all(fx.main.join("b")).unwrap();
    fx.entries(serde_json::json!(["docs", "./a/../b", "."]));
    assert!(fx.grants().await.is_empty());
}

#[tokio::test]
async fn absolute_and_home_entries_are_left_to_claude_code() {
    let fx = Fixture::new().await;
    fx.entries(serde_json::json!([
        fx.sibling.to_str().unwrap(),
        "~/projects"
    ]));
    assert!(fx.grants().await.is_empty());
}

/// No entry may widen the grant onto the clone, the workspace, or anything
/// that contains them.
#[tokio::test]
async fn a_grant_never_covers_the_repo_the_workspace_or_an_ancestor() {
    let fx = Fixture::new().await;
    std::fs::create_dir_all(fx.main.join("sub")).unwrap();
    fx.entries(serde_json::json!([
        "..",
        "../..",
        "../../../../../../../../../../..",
        "../repo",
        "../repo/sub",
        "../../ws",
        "../../ws/data",
        "../../ws/.lucidos",
    ]));
    assert!(fx.grants().await.is_empty());
}

#[tokio::test]
async fn a_symlink_cannot_smuggle_in_a_refused_directory() {
    let fx = Fixture::new().await;
    std::os::unix::fs::symlink(&fx.workspace, fx.sibling.join("ws-link")).unwrap();
    fx.entries(serde_json::json!(["../sibling/ws-link"]));
    assert!(fx.grants().await.is_empty());
}

#[tokio::test]
async fn a_missing_directory_or_a_file_grants_nothing() {
    let fx = Fixture::new().await;
    std::fs::write(fx.main.parent().unwrap().join("a-file"), "x").unwrap();
    fx.entries(serde_json::json!(["../nope", "../a-file"]));
    assert!(fx.grants().await.is_empty());
}

/// Refused entries do not take the good ones down with them.
#[tokio::test]
async fn one_refused_entry_leaves_the_others_granted() {
    let fx = Fixture::new().await;
    fx.entries(serde_json::json!(["..", 7, "../nope", "../sibling"]));
    assert_eq!(fx.grants().await, vec![fx.sibling.clone()]);
}

#[tokio::test]
async fn unreadable_settings_grant_nothing() {
    let fx = Fixture::new().await;
    assert!(fx.grants().await.is_empty(), "no settings file");
    fx.settings(&fx.worktree, "{ not json");
    assert!(fx.grants().await.is_empty(), "unparsable");
    fx.settings(
        &fx.worktree,
        r#"{"permissions":{"additionalDirectories":"../sibling"}}"#,
    );
    assert!(fx.grants().await.is_empty(), "not a list");
    fx.settings(&fx.worktree, r#"{"permissions":{}}"#);
    assert!(fx.grants().await.is_empty(), "no entry");
}

/// In the clone itself, Claude Code resolves the entry correctly already.
#[tokio::test]
async fn a_session_in_the_main_checkout_gets_nothing_extra() {
    let fx = Fixture::new().await;
    fx.settings(
        &fx.main,
        r#"{"permissions":{"additionalDirectories":["../sibling"]}}"#,
    );
    assert!(resolve(&fx.main, &fx.workspace).await.is_empty());
}

/// An unknown main checkout is never guessed at.
#[tokio::test]
async fn a_directory_outside_any_repo_grants_nothing() {
    let fx = Fixture::new().await;
    let plain = fx.workspace.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    fx.settings(
        &plain,
        r#"{"permissions":{"additionalDirectories":["../../projects/sibling"]}}"#,
    );
    assert!(resolve(&plain, &fx.workspace).await.is_empty());
}

#[test]
fn fold_dots_folds_lexically_and_refuses_climbing_past_the_root() {
    assert_eq!(
        fold_dots(Path::new("/a/b/./../c")),
        Some(PathBuf::from("/a/c"))
    );
    assert_eq!(fold_dots(Path::new("/a/../..")), None);
}
