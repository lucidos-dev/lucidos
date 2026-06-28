use super::*;

#[test]
fn parse_modified_file_diff() {
    let input = concat!(
        "diff --git a/src/main.rs b/src/main.rs\n",
        "--- a/src/main.rs\n",
        "+++ b/src/main.rs\n",
        "@@ -1,3 +1,4 @@\n",
        " fn main() {\n",
        "-    println!(\"old\");\n",
        "+    println!(\"new\");\n",
        "+    println!(\"extra\");\n",
        " }\n",
    );
    let files = parse_diff_output(input);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "src/main.rs");
    assert_eq!(files[0].status, "modified");
    assert_eq!(files[0].hunks.len(), 1);
    assert_eq!(files[0].hunks[0].old_start, 1);
    assert_eq!(files[0].hunks[0].old_count, 3);
    assert_eq!(files[0].hunks[0].new_start, 1);
    assert_eq!(files[0].hunks[0].new_count, 4);
    // 2 context + 1 deletion + 2 additions = 5
    assert_eq!(files[0].hunks[0].lines.len(), 5);
}

#[test]
fn parse_added_file_diff() {
    let input = concat!(
        "diff --git a/new.rs b/new.rs\n",
        "new file mode 100644\n",
        "--- /dev/null\n",
        "+++ b/new.rs\n",
        "@@ -0,0 +1,2 @@\n",
        "+fn new_fn() {\n",
        "+}\n",
    );
    let files = parse_diff_output(input);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].status, "added");
    assert_eq!(files[0].path, "new.rs");
}

#[test]
fn parse_deleted_file_diff() {
    let input = concat!(
        "diff --git a/old.rs b/old.rs\n",
        "deleted file mode 100644\n",
        "--- a/old.rs\n",
        "+++ /dev/null\n",
        "@@ -1,2 +0,0 @@\n",
        "-fn old_fn() {\n",
        "-}\n",
    );
    let files = parse_diff_output(input);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].status, "deleted");
    assert_eq!(files[0].path, "old.rs");
}

#[test]
fn parse_multiple_files() {
    let input = concat!(
        "diff --git a/a.rs b/a.rs\n",
        "--- a/a.rs\n",
        "+++ b/a.rs\n",
        "@@ -1 +1 @@\n",
        "-old\n",
        "+new\n",
        "diff --git a/b.rs b/b.rs\n",
        "--- a/b.rs\n",
        "+++ b/b.rs\n",
        "@@ -1 +1 @@\n",
        "-x\n",
        "+y\n",
    );
    let files = parse_diff_output(input);
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, "a.rs");
    assert_eq!(files[1].path, "b.rs");
}

#[test]
fn parse_empty_diff() {
    let files = parse_diff_output("");
    assert!(files.is_empty());
}

#[test]
fn parse_diff_output_filters_engine_injected_paths() {
    let input = concat!(
        "diff --git a/.lucidos-workspace b/.lucidos-workspace\n",
        "new file mode 100644\n",
        "--- /dev/null\n",
        "+++ b/.lucidos-workspace\n",
        "@@ -0,0 +1,2 @@\n",
        "+/Users/me/workspaces/dev\n",
        "+abc-uuid\n",
        "diff --git a/.lucidos/bin/lucidos b/.lucidos/bin/lucidos\n",
        "new file mode 120000\n",
        "--- /dev/null\n",
        "+++ b/.lucidos/bin/lucidos\n",
        "@@ -0,0 +1 @@\n",
        "+/usr/local/bin/lucidos\n",
        "diff --git a/.claude/skills/lucidos-cli/SKILL.md b/.claude/skills/lucidos-cli/SKILL.md\n",
        "new file mode 100644\n",
        "--- /dev/null\n",
        "+++ b/.claude/skills/lucidos-cli/SKILL.md\n",
        "@@ -0,0 +1,2 @@\n",
        "+# lucidos-cli skill\n",
        "+content\n",
        "diff --git a/src/real.rs b/src/real.rs\n",
        "--- a/src/real.rs\n",
        "+++ b/src/real.rs\n",
        "@@ -1 +1 @@\n",
        "-old\n",
        "+new\n",
    );
    let files = parse_diff_output(input);
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["src/real.rs"],
        "engine-injected paths must be filtered from parse_diff_output, got: {:?}",
        paths
    );
}

#[test]
fn parse_binary_file_skipped() {
    // Binary files have no hunks — parser should produce a file with empty hunks
    let input = concat!(
        "diff --git a/image.png b/image.png\n",
        "new file mode 100644\n",
        "Binary files /dev/null and b/image.png differ\n",
    );
    let files = parse_diff_output(input);
    assert_eq!(files.len(), 1);
    assert!(files[0].hunks.is_empty());
}

#[test]
fn parse_rename_diff() {
    let input = concat!(
        "diff --git a/old_name.rs b/new_name.rs\n",
        "similarity index 100%\n",
        "rename from old_name.rs\n",
        "rename to new_name.rs\n",
    );
    let files = parse_diff_output(input);
    assert_eq!(files.len(), 1);
    // Path extracted from "diff --git a/old b/new" header
    assert_eq!(files[0].path, "new_name.rs");
    assert!(files[0].hunks.is_empty());
}

#[test]
fn parse_multiple_hunks() {
    let input = concat!(
        "diff --git a/lib.rs b/lib.rs\n",
        "--- a/lib.rs\n",
        "+++ b/lib.rs\n",
        "@@ -1,2 +1,2 @@\n",
        "-old1\n",
        "+new1\n",
        " same\n",
        "@@ -10,3 +10,4 @@\n",
        " ctx\n",
        "-old2\n",
        "+new2\n",
        "+extra\n",
        " ctx\n",
    );
    let files = parse_diff_output(input);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].hunks.len(), 2);
    assert_eq!(files[0].hunks[0].old_start, 1);
    assert_eq!(files[0].hunks[0].lines.len(), 3); // 1 del + 1 add + 1 ctx
    assert_eq!(files[0].hunks[1].old_start, 10);
    // 1 ctx + 1 del + 2 add + 1 ctx = 5
    assert_eq!(files[0].hunks[1].lines.len(), 5);
}

#[test]
fn parse_no_newline_at_eof() {
    // git diff outputs "\ No newline at end of file" — parser should skip it
    let input = concat!(
        "diff --git a/f.rs b/f.rs\n",
        "--- a/f.rs\n",
        "+++ b/f.rs\n",
        "@@ -1 +1 @@\n",
        "-old\n",
        "\\ No newline at end of file\n",
        "+new\n",
        "\\ No newline at end of file\n",
    );
    let files = parse_diff_output(input);
    assert_eq!(files.len(), 1);
    // Only the -old and +new lines, not the backslash lines
    assert_eq!(files[0].hunks[0].lines.len(), 2);
}

#[test]
fn parse_hunk_header_no_count() {
    // Single-line hunk: @@ -5 +5 @@ (no comma count)
    let hunk = parse_hunk_header("@@ -5 +5 @@").unwrap();
    assert_eq!(hunk.old_start, 5);
    assert_eq!(hunk.old_count, 1);
    assert_eq!(hunk.new_start, 5);
    assert_eq!(hunk.new_count, 1);
}

#[test]
fn parse_hunk_header_with_function_context() {
    // Real git often appends function name: @@ -10,3 +10,4 @@ fn main()
    let hunk = parse_hunk_header("@@ -10,3 +10,4 @@ fn main()").unwrap();
    assert_eq!(hunk.old_start, 10);
    assert_eq!(hunk.old_count, 3);
    assert_eq!(hunk.new_start, 10);
    assert_eq!(hunk.new_count, 4);
}

#[test]
fn parse_hunk_header_malformed() {
    // Too few tokens — need at least @@ -x +y @@
    assert!(parse_hunk_header("@@").is_none());
    assert!(parse_hunk_header("@@ -1").is_none());
}

#[test]
fn parse_range_basic() {
    assert_eq!(parse_range("5,3"), (5, 3));
    assert_eq!(parse_range("1"), (1, 1));
    assert_eq!(parse_range("0,0"), (0, 0));
}

/// Parity with the frontend's `appIdFromFolder` helper. The two derivations
/// MUST agree on what counts as a valid app folder — divergence would
/// silently scope the backend diff to a phantom path while the frontend
/// renders nothing.
#[test]
fn extract_app_id_matches_frontend_validation() {
    assert_eq!(
        super::extract_app_id("/Users/me/ws/data/apps/habit-tracker"),
        Some("habit-tracker".to_string())
    );
    assert_eq!(
        super::extract_app_id("/ws/data/apps/habit-tracker/"),
        Some("habit-tracker".to_string())
    );
    assert_eq!(
        super::extract_app_id("data/apps/habit-tracker"),
            Some("habit-tracker".to_string())
        );

        // Refusals: same shapes the TS helper rejects.
        assert_eq!(super::extract_app_id("/ws/data/artifacts/foo"), None);
    assert_eq!(super::extract_app_id("/ws/data/apps"), None);
    assert_eq!(super::extract_app_id("/ws/data/apps/"), None);
    assert_eq!(super::extract_app_id("/ws/data/apps/."), None);
    assert_eq!(super::extract_app_id("/ws/data/apps/.."), None);
    assert_eq!(super::extract_app_id("/ws/apps/foo"), None);
    assert_eq!(super::extract_app_id("/ws/projects/foo"), None);
    assert_eq!(super::extract_app_id(""), None);

    // Nested: last data/apps/ wins, same as the TS helper.
    assert_eq!(
        super::extract_app_id("/ws/data/apps/outer/data/apps/inner"),
            Some("inner".to_string())
        );
    }

    // Defense-in-depth: even after we set `-c core.quotepath=false` on the
    // git invocations, the parser must never let the raw `diff --git` header
    // line bleed into the user-facing filename if a quoted path ever slips
    // through (e.g. paths with literal quotes, control bytes, or a future
    // git version that quotes for some other reason).
    #[test]
    fn parse_added_file_with_quoted_unicode_path() {
        let input = concat!(
            r#"diff --git "a/dir/7_\360\237\247\256_HLL.py" "b/dir/7_\360\237\247\256_HLL.py""#,
        "\n",
        "new file mode 100644\n",
        "--- /dev/null\n",
        r#"+++ "b/dir/7_\360\237\247\256_HLL.py""#,
        "\n",
        "@@ -0,0 +1 @@\n",
        "+content\n",
    );
    let files = parse_diff_output(input);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "dir/7_🧮_HLL.py");
    assert_eq!(files[0].status, "added");
    assert!(
        !files[0].path.contains("diff --git"),
        "raw diff header must never become the displayed filename, got: {:?}",
        files[0].path
    );
}

#[test]
fn parse_diff_git_path_handles_quoted_form() {
    let line =
        r#"diff --git "a/dir/7_\360\237\247\256_HLL.py" "b/dir/7_\360\237\247\256_HLL.py""#;
    assert_eq!(parse_diff_git_path(line), "dir/7_🧮_HLL.py");
}

#[test]
fn parse_diff_git_path_handles_unquoted_form() {
    assert_eq!(
        parse_diff_git_path("diff --git a/src/main.rs b/src/main.rs"),
        "src/main.rs"
    );
}

#[test]
fn parse_diff_git_path_handles_rename_unquoted() {
    // Renames have different a/ and b/ paths — we want the b/ side.
    assert_eq!(
        parse_diff_git_path("diff --git a/old_name.rs b/new_name.rs"),
        "new_name.rs"
    );
}

#[test]
fn parse_diff_git_path_returns_empty_when_unparseable() {
    // Garbage in → empty out, never the cryptic raw line.
    assert_eq!(parse_diff_git_path("diff --git malformed"), "");
    assert_eq!(parse_diff_git_path(""), "");
}

#[test]
fn unquote_git_path_decodes_octal_to_utf8() {
    assert_eq!(
        unquote_git_path(r#""7_\360\237\247\256_x.py""#),
        "7_🧮_x.py"
    );
}

#[test]
fn unquote_git_path_decodes_standard_escapes() {
    assert_eq!(
        unquote_git_path(r#""a\tb\nc\\d\"e""#),
        "a\tb\nc\\d\"e"
    );
}

#[test]
fn unquote_git_path_passthrough_when_unquoted() {
    assert_eq!(unquote_git_path("plain/path.txt"), "plain/path.txt");
    assert_eq!(unquote_git_path(""), "");
    assert_eq!(unquote_git_path("\""), "\"");
}

#[test]
fn browse_rejects_path_traversal() {
    assert!(super::is_dangerous_browse_path("../etc"));
    assert!(super::is_dangerous_browse_path("/foo/../bar"));
        assert!(super::is_dangerous_browse_path(""));
    assert!(!super::is_dangerous_browse_path("/Users/me/projects"));
    assert!(!super::is_dangerous_browse_path("/tmp"));
}

#[test]
fn expand_tilde_handles_all_cases() {
    let home = std::env::var("HOME").unwrap();
    assert_eq!(super::expand_tilde("~"), home);
    assert!(super::expand_tilde("~/projects").starts_with(&home));
    assert_eq!(
        super::expand_tilde("~/projects"),
        format!("{}/projects", home)
    );
    assert_eq!(super::expand_tilde("/absolute/path"), "/absolute/path");
    assert_eq!(super::expand_tilde("relative"), "relative");
}

/// End-to-end of the worktree-diff helpers against a real git repo +
/// linked worktree on a feature branch with one extra commit. Asserts
/// the base ref resolves to a usable target and the diff range produces
/// the expected single-file change.
#[tokio::test]
async fn worktree_diff_helpers_against_real_repo() {
    async fn run(args: &[&str], cwd: &std::path::Path) -> std::process::Output {
        tokio::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .unwrap()
    }

    let main_repo = tempfile::tempdir().unwrap();
    let main_path = main_repo.path();

    // `-c init.defaultBranch=main` makes the test pass on hosts where the
    // user's git defaults differ (master vs main). Without it,
    // `git init` creates `master` and our subsequent `worktree add ... main`
    // fails with "invalid reference: main".
    run(
        &["-c", "init.defaultBranch=main", "init", "-q"],
        main_path,
    )
    .await;
    run(&["config", "user.email", "test@example.com"], main_path).await;
    run(&["config", "user.name", "Test"], main_path).await;
    tokio::fs::write(main_path.join("README.md"), "init\n")
        .await
        .unwrap();
    run(&["add", "."], main_path).await;
    run(&["commit", "-q", "-m", "init"], main_path).await;

    // Linked worktree on a new branch, one extra commit.
    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");
    run(
        &[
            "worktree",
            "add",
            "-b",
            "feature/diff-helpers",
            wt_path.to_str().unwrap(),
            "main",
        ],
        main_path,
    )
    .await;
    tokio::fs::write(wt_path.join("README.md"), "init\nadded line\n")
        .await
        .unwrap();
    run(&["add", "."], &wt_path).await;
    run(&["commit", "-q", "-m", "edit readme"], &wt_path).await;

    // Base ref: no `origin` remote, so origin/HEAD lookup fails — the
    // helper must fall back to local `main`.
    let base = crate::engine::git_ops::default_local_branch(&wt_path).await;
    assert_eq!(
        base, "main",
        "expected fallback to local main when no origin remote is set, got {base}"
    );

    // Repo root: must point back at the main worktree, not the linked one.
    let root = super::resolve_worktree_repo_root(&wt_path)
        .await
        .unwrap()
        .unwrap();
    let canonical_main = main_path.canonicalize().unwrap();
    let canonical_root = std::path::PathBuf::from(&root).canonicalize().unwrap();
    assert_eq!(
        canonical_root, canonical_main,
        "repo root should be the main worktree: got {root}"
    );

    // 3-dot diff against the resolved base: exactly one modified file.
    let range = format!("{}...HEAD", base);
    let out = run(&["diff", &range, "--no-color"], &wt_path).await;
        assert!(out.status.success(), "git diff failed: {:?}", out);
    let files = super::parse_diff_output(&String::from_utf8_lossy(&out.stdout));
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "README.md");
    assert_eq!(files[0].status, "modified");
}

/// The Diff button must base its diff on **local** `main`, not the
/// remote tracking ref `origin/main`. Otherwise, when local `main` is
/// ahead of `origin/main` (e.g. the engine has applied work but the
/// push to origin hasn't landed yet), every commit on local main that
/// isn't on origin shows up in the Diff viewer as if it belonged to the
/// current thread — even though Apply has already merged those commits.
/// This was the symptom users saw as "the Diff button shows lots of
/// code changes when the thread only touched markdown."
#[tokio::test]
async fn diff_base_is_local_main_when_origin_is_behind() {
    async fn run(args: &[&str], cwd: &std::path::Path) -> std::process::Output {
        tokio::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .unwrap()
    }

    let origin_dir = tempfile::tempdir().unwrap();
    let origin_path = origin_dir.path();
    run(
        &["-c", "init.defaultBranch=main", "init", "-q", "--bare"],
        origin_path,
    )
    .await;

    let main_repo = tempfile::tempdir().unwrap();
    let main_path = main_repo.path();
    run(
        &["-c", "init.defaultBranch=main", "init", "-q"],
        main_path,
    )
    .await;
    run(&["config", "user.email", "test@example.com"], main_path).await;
    run(&["config", "user.name", "Test"], main_path).await;
    tokio::fs::write(main_path.join("README.md"), "init\n")
        .await
        .unwrap();
    run(&["add", "."], main_path).await;
    run(&["commit", "-q", "-m", "init"], main_path).await;
    run(
        &["remote", "add", "origin", origin_path.to_str().unwrap()],
        main_path,
    )
    .await;
    run(&["push", "-q", "origin", "main"], main_path).await;
    // origin/HEAD must exist for `symbolic-ref refs/remotes/origin/HEAD`
    // to succeed — that path is what the buggy code used to return
    // `origin/main` instead of `main`.
    run(&["remote", "set-head", "origin", "main"], main_path).await;

    tokio::fs::write(main_path.join("already-applied.txt"), "shipped\n")
        .await
        .unwrap();
    run(&["add", "."], main_path).await;
    run(&["commit", "-q", "-m", "ship feature (not yet pushed)"], main_path).await;

    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");
    run(
        &[
            "worktree",
            "add",
            "-b",
            "cc/feature",
            wt_path.to_str().unwrap(),
            "main",
        ],
        main_path,
    )
    .await;
    tokio::fs::write(wt_path.join("cc-work.txt"), "from CC\n")
        .await
        .unwrap();
    run(&["add", "."], &wt_path).await;
    run(&["commit", "-q", "-m", "cc: add cc-work.txt"], &wt_path).await;

    // The cc-diff response must:
    //   - report `base_ref = "main"` (local), not `"origin/main"`
    //   - list ONLY `cc-work.txt` — the already-applied commit's file
    //     must not appear, because Apply has already merged it.
    let diff = super::diff_via_worktree(&wt_path, None)
        .await
        .expect("diff_via_worktree should succeed");

    assert_eq!(
        diff.base_ref, "main",
        "expected local `main` as base, got `{}` — Diff viewer would \
         show stale already-applied commits as if they were new",
        diff.base_ref
    );

    let paths: Vec<&str> = diff.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["cc-work.txt"],
        "expected only the CC worktree's own file, got {:?} — the \
         already-applied commit's file leaked into the diff because \
         the base was the stale `origin/main`",
        paths
    );
}

/// The mirror of the test above: when the local default branch has been
/// *rewritten* so the remote-tracking ref no longer reaches it — a repo
/// migration, a force-pull, or a rebase landing on the user's local `main` —
/// the Diff button must fall back to `origin/<default>`, which still holds the
/// branch's true fork point. Otherwise the three-dot merge-base collapses to an
/// ancient common ancestor and the diff balloons with unrelated churn.
///
/// Regression for the `example-repo` migration report: the migration system
/// committed to the user's local `main` ("secrets transfer", "migration notice"
/// commits) and rewrote its history so the PR branch's fork point was no longer
/// an ancestor of local `main`. Lucidos diffed against local `main` and showed
/// 59 files; the GitHub PR (diffed against the untouched remote base) showed 12.
#[tokio::test]
async fn diff_base_falls_back_to_origin_when_local_main_diverged() {
    async fn run(args: &[&str], cwd: &std::path::Path) -> std::process::Output {
        tokio::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .unwrap()
    }

    let origin_dir = tempfile::tempdir().unwrap();
    let origin_path = origin_dir.path();
    run(
        &["-c", "init.defaultBranch=main", "init", "-q", "--bare"],
        origin_path,
    )
    .await;

    let main_repo = tempfile::tempdir().unwrap();
    let main_path = main_repo.path();
    run(&["-c", "init.defaultBranch=main", "init", "-q"], main_path).await;
    run(&["config", "user.email", "test@example.com"], main_path).await;
    run(&["config", "user.name", "Test"], main_path).await;

    // Root commit, then the fork point the PR branch builds on top of.
    tokio::fs::write(main_path.join("README.md"), "init\n")
        .await
        .unwrap();
    run(&["add", "."], main_path).await;
    run(&["commit", "-q", "-m", "init"], main_path).await;
    let c_root = String::from_utf8_lossy(&run(&["rev-parse", "HEAD"], main_path).await.stdout)
        .trim()
        .to_string();

    tokio::fs::write(main_path.join("fork-point.txt"), "shipped before the PR\n")
        .await
        .unwrap();
    run(&["add", "."], main_path).await;
    run(&["commit", "-q", "-m", "feature on main (PR forks here)"], main_path).await;

    // Publish to origin and record origin/HEAD → main. `origin/main` now holds
    // the fork point.
    run(
        &["remote", "add", "origin", origin_path.to_str().unwrap()],
        main_path,
    )
    .await;
    run(&["push", "-q", "origin", "main"], main_path).await;
    run(&["remote", "set-head", "origin", "main"], main_path).await;

    // CC worktree forks from the fork point, adds its own file.
    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");
    run(
        &[
            "worktree",
            "add",
            "-b",
            "cc/feature",
            wt_path.to_str().unwrap(),
            "main",
        ],
        main_path,
    )
    .await;
    tokio::fs::write(wt_path.join("cc-work.txt"), "from CC\n")
        .await
        .unwrap();
    run(&["add", "."], &wt_path).await;
    run(&["commit", "-q", "-m", "cc: add cc-work.txt"], &wt_path).await;

    // A migration tool rewrites local `main`: reset back to the root and commit
    // a migration notice. `origin/main` (the fork point) is no longer an
    // ancestor of local `main` — they diverge at the root.
    run(&["reset", "-q", "--hard", &c_root], main_path).await;
    tokio::fs::write(main_path.join("MIGRATION.md"), "moved to new-org\n")
        .await
        .unwrap();
    run(&["add", "."], main_path).await;
    run(
        &["commit", "-q", "-m", "migration: secrets transfer (automated)"],
        main_path,
    )
    .await;

    let diff = super::diff_via_worktree(&wt_path, None)
        .await
        .expect("diff_via_worktree should succeed");

    assert_eq!(
        diff.base_ref, "origin/main",
        "local `main` diverged from the remote — base must fall back to \
         `origin/main`, got `{}`",
        diff.base_ref,
    );

    let paths: Vec<&str> = diff.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["cc-work.txt"],
        "only the CC branch's own file belongs in the diff; the fork-point \
         file leaked because the base used the rewritten local `main`: {:?}",
        paths,
    );
}

/// App coding-agent threads operate on `data/apps/<id>/` inside the
/// workspace git. The cc-diff response must scope to that pathspec so
/// the Diff button doesn't surface stray edits the agent made outside
/// its scope (the user picked "diff scoped to the app folder" — files
    /// in `data/artifacts/`, `data/knowhow/`, etc. must not appear).
    #[tokio::test]
    async fn diff_via_worktree_scopes_to_app_pathspec() {
        async fn run(args: &[&str], cwd: &std::path::Path) -> std::process::Output {
            tokio::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .await
            .unwrap()
    }

    // Simulate a workspace as a real git repo with main committed.
    let ws_dir = tempfile::tempdir().unwrap();
    let ws_path = ws_dir.path();
    run(&["-c", "init.defaultBranch=main", "init", "-q"], ws_path).await;
    run(&["config", "user.email", "test@example.com"], ws_path).await;
    run(&["config", "user.name", "Test"], ws_path).await;
    tokio::fs::write(ws_path.join("README.md"), "ws\n").await.unwrap();
    run(&["add", "."], ws_path).await;
    run(&["commit", "-q", "-m", "init"], ws_path).await;

    // App CC worktree on a branch — touches the app folder AND files
    // outside scope. We expect only the in-scope file in the response.
    let wt_dir = tempfile::tempdir().unwrap();
    let wt_path = wt_dir.path().join("wt");
    run(
        &["worktree", "add", "-b", "apps/habit-tracker", wt_path.to_str().unwrap(), "main"],
        ws_path,
    )
    .await;

    let app_dir = wt_path.join("data/apps/habit-tracker");
    tokio::fs::create_dir_all(&app_dir).await.unwrap();
    tokio::fs::write(app_dir.join("manifest.json"), "{}\n")
        .await
        .unwrap();

    let artifacts_dir = wt_path.join("data/artifacts/habit-tracker");
    tokio::fs::create_dir_all(&artifacts_dir).await.unwrap();
    tokio::fs::write(artifacts_dir.join("notes.md"), "out of scope\n")
        .await
        .unwrap();

    run(&["add", "."], &wt_path).await;
    run(&["commit", "-q", "-m", "app + stray artifacts"], &wt_path).await;

    let diff = super::diff_via_worktree(&wt_path, Some("data/apps/habit-tracker"))
        .await
        .expect("diff_via_worktree should succeed with app pathspec");

    let paths: Vec<&str> = diff.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["data/apps/habit-tracker/manifest.json"],
        "expected only in-scope app file, got {:?} — out-of-scope edits \
         leaked into the response",
        paths
    );

    // Sanity: without the pathspec, the out-of-scope file is visible.
    // Asserts the scoping is what's filtering it out, not test setup.
    let unscoped = super::diff_via_worktree(&wt_path, None)
        .await
        .expect("diff_via_worktree should succeed without pathspec");
    let unscoped_paths: Vec<&str> =
        unscoped.files.iter().map(|f| f.path.as_str()).collect();
    assert!(
        unscoped_paths.contains(&"data/artifacts/habit-tracker/notes.md"),
        "control: unscoped diff should include the out-of-scope file, \
         got {:?}",
        unscoped_paths
    );
}
