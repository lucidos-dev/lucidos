//! Tests for the command checkpoint pair (ADR 0002, Phase 4 and the 2026-08-06
//! addendum).
//!
//! The load-bearing ones are the four that keep undo from eating something it
//! must not: `undo_removes_only_what_the_command_created`,
//! `a_created_file_edited_since_the_command_is_kept` (and its symlink twin
//! `a_created_symlink_repointed_since_the_command_is_kept`),
//! `an_unanswerable_comparison_removes_nothing`, and
//! `safe_repo_path_refuses_anything_outside_the_workspace`.

use super::*;
use std::fs;

async fn git(args: &[&str], dir: &Path) {
    let out = git_cmd(args, dir).await.unwrap();
    assert!(
        out.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

async fn init_repo(root: &Path) {
    git(&["init", "-b", "main"], root).await;
    git(&["config", "user.email", "t@example.com"], root).await;
    git(&["config", "user.name", "Test"], root).await;
}

/// The whole undo, as the engine performs it: restore the pre image, then drop
/// what the command created.
async fn undo(root: &Path, id: &str) -> u32 {
    restore_command_checkpoint(root, id).await.unwrap();
    let effects = diff_checkpoint_effects(root, id).await.unwrap();
    match effects {
        Some(e) => remove_created_files(root, id, &e.created).await,
        None => 0,
    }
}

#[tokio::test]
async fn checkpoint_restores_deleted_overwritten_and_untracked() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root).await;
    fs::create_dir_all(root.join("data/artifacts")).unwrap();
    fs::write(root.join("data/artifacts/keep.txt"), "original").unwrap();
    fs::write(root.join("data/artifacts/del.txt"), "deleteme").unwrap();
    git(&["add", "-A"], root).await;
    git(&["commit", "-m", "init"], root).await;
    // An uncommitted (untracked) file present at checkpoint time must also be
    // captured: a reversible command could delete it.
    fs::write(root.join("data/artifacts/untracked.txt"), "untracked").unwrap();

    let id = "11111111-1111-1111-1111-111111111111";
    create_command_checkpoint(root, id).await.unwrap();

    // The "reversible" command: delete one, overwrite another, create a new.
    fs::remove_file(root.join("data/artifacts/del.txt")).unwrap();
    fs::write(root.join("data/artifacts/keep.txt"), "CLOBBERED").unwrap();
    fs::remove_file(root.join("data/artifacts/untracked.txt")).unwrap();
    fs::write(root.join("data/artifacts/new.txt"), "created-after").unwrap();
    create_command_post_image(root, id).await.unwrap();

    let effects = diff_checkpoint_effects(root, id).await.unwrap().unwrap();
    assert_eq!(effects.restores, 3, "two deletions and one overwrite");
    assert_eq!(effects.removes(), 1);
    assert_eq!(effects.created[0], "data/artifacts/new.txt");

    assert_eq!(undo(root, id).await, 1);

    // Deleted file restored, overwrite reverted, untracked file restored.
    assert_eq!(
        fs::read_to_string(root.join("data/artifacts/del.txt")).unwrap(),
        "deleteme"
    );
    assert_eq!(
        fs::read_to_string(root.join("data/artifacts/keep.txt")).unwrap(),
        "original"
    );
    assert_eq!(
        fs::read_to_string(root.join("data/artifacts/untracked.txt")).unwrap(),
        "untracked"
    );
    // The 2026-08-06 change: a file the command created is now removed too.
    assert!(!root.join("data/artifacts/new.txt").exists());

    // The snapshot left the repo's real HEAD untouched (commit still "init").
    let head_subject = git_cmd(&["log", "-1", "--format=%s"], root).await.unwrap();
    assert_eq!(
        String::from_utf8_lossy(&head_subject.stdout).trim(),
        "init",
        "checkpoint must not have moved HEAD"
    );

    // The refs survive the undo: the card's diff viewer still reads them.
    assert!(diff_checkpoint_effects(root, id).await.unwrap().is_some());
}

#[tokio::test]
async fn undo_removes_only_what_the_command_created() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root).await;
    fs::write(root.join("seed.txt"), "seed").unwrap();
    git(&["add", "-A"], root).await;
    git(&["commit", "-m", "init"], root).await;

    let id = "22222222-2222-2222-2222-222222222222";
    create_command_checkpoint(root, id).await.unwrap();
    fs::write(root.join("by-command.txt"), "from the command").unwrap();
    create_command_post_image(root, id).await.unwrap();

    // Everything below happens AFTER the command returned, so none of it is in
    // the pre/post diff and none of it may be touched.
    fs::write(root.join("later.txt"), "written by something else").unwrap();
    fs::create_dir_all(root.join("later-dir")).unwrap();
    fs::write(root.join("later-dir/x.txt"), "also later").unwrap();

    assert_eq!(undo(root, id).await, 1);
    assert!(!root.join("by-command.txt").exists());
    assert!(root.join("later.txt").exists());
    assert!(root.join("later-dir/x.txt").exists());
    assert!(root.join("seed.txt").exists());
}

#[tokio::test]
async fn a_created_file_edited_since_the_command_is_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root).await;
    fs::write(root.join("seed.txt"), "seed").unwrap();
    git(&["add", "-A"], root).await;
    git(&["commit", "-m", "init"], root).await;

    let id = "33333333-3333-3333-3333-333333333333";
    create_command_checkpoint(root, id).await.unwrap();
    fs::write(root.join("report.md"), "generated").unwrap();
    create_command_post_image(root, id).await.unwrap();

    // The user edited the file the step produced. Undo must not eat that work.
    fs::write(root.join("report.md"), "generated, then edited by hand").unwrap();

    assert_eq!(undo(root, id).await, 0);
    assert_eq!(
        fs::read_to_string(root.join("report.md")).unwrap(),
        "generated, then edited by hand"
    );
}

#[tokio::test]
async fn removing_created_files_prunes_only_directories_it_emptied() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root).await;
    fs::create_dir_all(root.join("data/keep")).unwrap();
    fs::write(root.join("data/keep/existing.txt"), "was here first").unwrap();
    git(&["add", "-A"], root).await;
    git(&["commit", "-m", "init"], root).await;

    let id = "44444444-4444-4444-4444-444444444444";
    create_command_checkpoint(root, id).await.unwrap();
    // One brand-new directory tree, and one file dropped into an existing one.
    fs::create_dir_all(root.join("data/artifacts/plugins")).unwrap();
    fs::write(root.join("data/artifacts/plugins/app.zip"), "zip").unwrap();
    fs::write(root.join("data/keep/added.txt"), "added").unwrap();
    create_command_post_image(root, id).await.unwrap();

    assert_eq!(undo(root, id).await, 2);
    // The directories the command created are gone with their contents …
    assert!(!root.join("data/artifacts/plugins").exists());
    assert!(!root.join("data/artifacts").exists());
    // … and one that still holds a pre-existing file survives.
    assert!(root.join("data/keep/existing.txt").exists());
    assert!(root.join("data").exists());
}

#[tokio::test]
async fn a_command_touching_only_ignored_paths_has_no_effects() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root).await;
    fs::write(root.join(".gitignore"), ".lucidos/\n").unwrap();
    fs::create_dir_all(root.join(".lucidos/tmp/stage")).unwrap();
    fs::write(root.join(".lucidos/tmp/stage/f"), "scratch").unwrap();
    git(&["add", "-A"], root).await;
    git(&["commit", "-m", "init"], root).await;

    let id = "55555555-5555-5555-5555-555555555555";
    create_command_checkpoint(root, id).await.unwrap();
    // The reported case: an rmtree of a gitignored staging directory. The
    // snapshot never captured it, so undo could restore nothing.
    fs::remove_dir_all(root.join(".lucidos/tmp/stage")).unwrap();
    create_command_post_image(root, id).await.unwrap();

    let effects = diff_checkpoint_effects(root, id).await.unwrap().unwrap();
    assert!(
        effects.is_empty(),
        "an ignored-only change must produce no effects, got {effects:?}"
    );
}

/// The guard that stands between a path out of the checkpoint diff and a
/// `remove_file`. Tested directly rather than through `remove_created_files`,
/// because the comparison step there refuses first for any path git did not
/// report, which would make the whole thing pass without the guard existing.
#[test]
fn safe_repo_path_refuses_anything_outside_the_workspace() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("ws");
    fs::create_dir_all(root.join("data")).unwrap();
    fs::create_dir_all(tmp.path().join("elsewhere")).unwrap();
    // A symlink inside the workspace pointing out of it: a lexical prefix check
    // would let a path under it through.
    std::os::unix::fs::symlink(tmp.path().join("elsewhere"), root.join("link")).unwrap();

    assert!(safe_repo_path(&root, "data/ours.txt").is_some());
    for escape in ["../outside.txt", "/etc/hosts", "link/target.txt", ""] {
        assert!(
            safe_repo_path(&root, escape).is_none(),
            "{escape} must not resolve to a removable path"
        );
    }
}

/// A comparison that cannot be made removes nothing. The pre image is still a
/// valid restore point, so undo's first half stands; its second half declines
/// rather than guessing, because guessing wrong here deletes user data.
#[tokio::test]
async fn an_unanswerable_comparison_removes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root).await;
    fs::write(root.join("kept.txt"), "still here").unwrap();

    let created = vec!["kept.txt".to_string()];
    assert_eq!(
        remove_created_files(root, "no-such-checkpoint", &created).await,
        0
    );
    assert!(root.join("kept.txt").exists());
}

/// Symlinks, which a blob-sha comparison gets wrong in both directions: git
/// stores the link target path as the blob, while hashing the path follows the
/// link and reads the target file, and a dangling link does not open at all.
/// Both used to survive an undo that was supposed to remove them.
#[tokio::test]
async fn undo_removes_a_created_symlink_dangling_or_not() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root).await;
    fs::write(root.join("real.txt"), "target content").unwrap();
    git(&["add", "-A"], root).await;
    git(&["commit", "-m", "init"], root).await;

    let id = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
    create_command_checkpoint(root, id).await.unwrap();
    std::os::unix::fs::symlink("real.txt", root.join("live-link")).unwrap();
    std::os::unix::fs::symlink("gone.txt", root.join("dangling-link")).unwrap();
    create_command_post_image(root, id).await.unwrap();

    let effects = diff_checkpoint_effects(root, id).await.unwrap().unwrap();
    assert_eq!(effects.removes(), 2, "both links are created entries");

    assert_eq!(undo(root, id).await, 2);
    assert!(std::fs::symlink_metadata(root.join("live-link")).is_err());
    assert!(std::fs::symlink_metadata(root.join("dangling-link")).is_err());
    // Removing the link must not touch what it pointed at.
    assert_eq!(
        fs::read_to_string(root.join("real.txt")).unwrap(),
        "target content"
    );
}

/// The other half of the symlink case: a link the user has since re-pointed is
/// their edit, and undo keeps it exactly as it keeps an edited regular file.
#[tokio::test]
async fn a_created_symlink_repointed_since_the_command_is_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root).await;
    fs::write(root.join("a.txt"), "a").unwrap();
    fs::write(root.join("b.txt"), "b").unwrap();
    git(&["add", "-A"], root).await;
    git(&["commit", "-m", "init"], root).await;

    let id = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
    create_command_checkpoint(root, id).await.unwrap();
    std::os::unix::fs::symlink("a.txt", root.join("link")).unwrap();
    create_command_post_image(root, id).await.unwrap();

    fs::remove_file(root.join("link")).unwrap();
    std::os::unix::fs::symlink("b.txt", root.join("link")).unwrap();

    assert_eq!(undo(root, id).await, 0);
    assert_eq!(
        std::fs::read_link(root.join("link")).unwrap(),
        Path::new("b.txt")
    );
}

#[tokio::test]
async fn effects_are_unavailable_without_a_post_image() {
    // Every checkpoint written before 2026-08-06 is this shape: a pre ref and
    // nothing else. Undo must degrade to restore-only rather than error.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root).await;
    fs::write(root.join("a.txt"), "v1").unwrap();
    git(&["add", "-A"], root).await;
    git(&["commit", "-m", "init"], root).await;

    let id = "66666666-6666-6666-6666-666666666666";
    create_command_checkpoint(root, id).await.unwrap();
    fs::write(root.join("a.txt"), "v2").unwrap();
    fs::write(root.join("b.txt"), "new").unwrap();

    assert!(diff_checkpoint_effects(root, id).await.unwrap().is_none());
    assert_eq!(undo(root, id).await, 0);
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "v1");
    assert!(root.join("b.txt").exists(), "restore-only leaves it");
}

#[tokio::test]
async fn restore_missing_checkpoint_errors() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root).await;
    fs::write(root.join("f.txt"), "x").unwrap();
    git(&["add", "-A"], root).await;
    git(&["commit", "-m", "init"], root).await;
    assert!(restore_command_checkpoint(root, "nope").await.is_err());
}

#[tokio::test]
async fn checkpoint_in_repo_without_head_is_parentless() {
    // A freshly-initialized repo with no commit still snapshots (parentless
    // commit): the guard shouldn't fail on a brand-new workspace.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root).await;
    fs::write(root.join("a.txt"), "v1").unwrap();
    let id = "77777777-7777-7777-7777-777777777777";
    create_command_checkpoint(root, id).await.unwrap();
    fs::write(root.join("a.txt"), "v2").unwrap();
    create_command_post_image(root, id).await.unwrap();
    assert_eq!(undo(root, id).await, 0);
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "v1");
    delete_command_checkpoint_pair(root, id).await;
    assert!(diff_checkpoint_effects(root, id).await.unwrap().is_none());
}

#[tokio::test]
async fn retention_reclaims_only_pairs_past_the_window() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    init_repo(root).await;
    fs::write(root.join("f.txt"), "x").unwrap();
    git(&["add", "-A"], root).await;
    git(&["commit", "-m", "init"], root).await;

    let old = "88888888-8888-8888-8888-888888888888";
    let fresh = "99999999-9999-9999-9999-999999999999";
    for id in [old, fresh] {
        create_command_checkpoint(root, id).await.unwrap();
        create_command_post_image(root, id).await.unwrap();
    }
    // Backdate the old pair's pre image by rewriting its commit with an
    // explicit committer date; the sweep reads exactly that field.
    let tree = git_cmd(
        &[
            "rev-parse",
            &format!("{}^{{tree}}", command_checkpoint_ref(old)),
        ],
        root,
    )
    .await
    .unwrap();
    let tree = String::from_utf8_lossy(&tree.stdout).trim().to_string();
    let backdated = git_cmd_env(
        &["commit-tree", &tree, "-m", "old"],
        root,
        &[
            ("GIT_COMMITTER_DATE", OsStr::new("1000000000 +0000")),
            ("GIT_AUTHOR_DATE", OsStr::new("1000000000 +0000")),
            ("GIT_COMMITTER_NAME", OsStr::new("Lucidos")),
            ("GIT_COMMITTER_EMAIL", OsStr::new("lucidos@localhost")),
            ("GIT_AUTHOR_NAME", OsStr::new("Lucidos")),
            ("GIT_AUTHOR_EMAIL", OsStr::new("lucidos@localhost")),
        ],
    )
    .await
    .unwrap();
    let backdated = String::from_utf8_lossy(&backdated.stdout)
        .trim()
        .to_string();
    git(
        &["update-ref", &command_checkpoint_ref(old), &backdated],
        root,
    )
    .await;

    prune_expired_checkpoints(root, 1_100_000_000, CHECKPOINT_RETENTION_SECS).await;

    assert!(
        diff_checkpoint_effects(root, old).await.unwrap().is_none(),
        "the expired pair is reclaimed, both refs"
    );
    assert!(
        diff_checkpoint_effects(root, fresh)
            .await
            .unwrap()
            .is_some(),
        "a pair inside the window survives"
    );
    // The sweep is scoped to the checkpoint namespaces; ordinary refs stand.
    let head = git_cmd(&["rev-parse", "--verify", "refs/heads/main"], root)
        .await
        .unwrap();
    assert!(head.status.success(), "refs/heads/main must survive");
}

#[test]
fn parse_diff_tree_classifies_each_status() {
    let blob = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";
    let zeros = "0".repeat(40);
    let mut out: Vec<u8> = Vec::new();
    for (status, path) in [("A", "new.txt"), ("M", "changed.txt"), ("D", "gone.txt")] {
        out.extend_from_slice(format!(":100644 100644 {zeros} {blob} {status}").as_bytes());
        out.push(0);
        out.extend_from_slice(path.as_bytes());
        out.push(0);
    }
    let effects = parse_diff_tree_z(&out);
    assert_eq!(effects.restores, 2);
    assert_eq!(effects.created, vec!["new.txt".to_string()]);
}

#[test]
fn parse_diff_tree_skips_malformed_records() {
    // A truncated record, and a created path that is not valid UTF-8. Neither
    // may take the parse down or produce a path that would not resolve.
    let blob = "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391";
    let zeros = "0".repeat(40);
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b":100644 100644 short");
    out.push(0);
    out.extend_from_slice(b"truncated.txt");
    out.push(0);
    out.extend_from_slice(format!(":000000 100644 {zeros} {blob} A").as_bytes());
    out.push(0);
    out.extend_from_slice(&[0xff, 0xfe]);
    out.push(0);
    let effects = parse_diff_tree_z(&out);
    assert!(effects.is_empty());
}

#[test]
fn empty_effects_mean_nothing_to_show_and_nothing_to_undo() {
    assert!(CheckpointEffects::default().is_empty());
    assert!(!CheckpointEffects {
        restores: 1,
        created: vec![],
    }
    .is_empty());
}
