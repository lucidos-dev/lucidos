use std::path::{Path, PathBuf};

const REPO_MARKER: &str = "scripts/web-dev.sh";

pub(crate) fn build_id_commit(id: &str) -> Option<&str> {
    if id.is_empty() || id.starts_with("src") {
        return None;
    }
    let commit = id.split('-').next().unwrap_or("");
    (!commit.is_empty()).then_some(commit)
}

pub(crate) fn disk_upgrade_verdict(
    disk_id: &str,
    running_id: &str,
    disk_is_strict_ancestor: Option<bool>,
) -> bool {
    !disk_id.is_empty() && disk_id != running_id && disk_is_strict_ancestor != Some(true)
}

pub(crate) fn repo_root_above(exe: &Path) -> Option<PathBuf> {
    exe.ancestors()
        .take_while(|a| a.extension().is_none_or(|ext| ext != "app"))
        .find(|a| a.join(REPO_MARKER).exists())
        .map(Path::to_path_buf)
}

pub(crate) async fn commit_is_strict_ancestor(
    root: &Path,
    ancestor: &str,
    descendant: &str,
) -> Option<bool> {
    if ancestor.starts_with(descendant) || descendant.starts_with(ancestor) {
        return Some(false);
    }
    let out = tokio::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .current_dir(root)
        .output()
        .await
        .ok()?;
    match out.status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

pub(crate) async fn disk_id_is_upgrade(exe: &Path, disk_id: &str, running_id: &str) -> bool {
    if disk_id.is_empty() || disk_id == running_id {
        return false;
    }
    let is_older = match (build_id_commit(disk_id), build_id_commit(running_id)) {
        (Some(disk_commit), Some(running_commit)) if disk_commit != running_commit => {
            match repo_root_above(exe) {
                Some(root) => commit_is_strict_ancestor(&root, disk_commit, running_commit).await,
                None => None,
            }
        }
        (Some(_), Some(_)) => Some(false),
        _ => None,
    };
    if is_older == Some(true) {
        crate::log_legacy!(
            "[Gateway] on-disk gateway binary ({}) is OLDER than the running one ({}) — \
             not offering a downgrade as a new version. Rebuild it forward with \
             `web-dev.sh -w <ws> -b`.",
            disk_id,
            running_id
        );
    }
    disk_upgrade_verdict(disk_id, running_id, is_older)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_id_commit_takes_the_sha_and_rejects_the_no_git_forms() {
        assert_eq!(build_id_commit("aa7075ee2"), Some("aa7075ee2"));
        assert_eq!(
            build_id_commit("aa7075ee2-0badc0ffee123456"),
            Some("aa7075ee2")
        );
        assert_eq!(build_id_commit("src-0123456789abcdef"), None);
        assert_eq!(build_id_commit(""), None);
        assert_eq!(build_id_commit("-abc"), None);
    }

    #[test]
    fn disk_upgrade_verdict_offers_only_a_step_forward() {
        assert!(!disk_upgrade_verdict("71c8d39b1", "aa7075ee2", Some(true)));
        assert!(disk_upgrade_verdict("bb1122334", "aa7075ee2", Some(false)));
        assert!(!disk_upgrade_verdict("aa7075ee2", "aa7075ee2", Some(false)));
        assert!(!disk_upgrade_verdict("", "aa7075ee2", None));
        assert!(disk_upgrade_verdict("cc9988776", "aa7075ee2", None));
        assert!(disk_upgrade_verdict(
            "aa7075ee2-0badc0ffee123456",
            "aa7075ee2",
            Some(false)
        ));
    }

    #[test]
    fn repo_root_above_finds_the_scripts_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        std::fs::create_dir_all(dir.join("scripts")).unwrap();
        std::fs::write(dir.join(REPO_MARKER), "#!/bin/bash\n").unwrap();
        let exe = dir.join(".launch/debug/plain/lucidos-gateway");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();

        assert_eq!(repo_root_above(&exe).as_deref(), Some(dir));
        assert_eq!(
            repo_root_above(
                &dir.join(
                    "target/release/bundle/macos/Demo.app/Contents/Resources/lucidos-gateway"
                )
            ),
            None
        );
        assert_eq!(
            repo_root_above(Path::new("/tmp/nowhere/lucidos-gateway")),
            None
        );
    }

    #[tokio::test]
    async fn commit_is_strict_ancestor_reads_history_direction() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .expect("git runs in the test environment")
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "test@example.com"]);
        git(&["config", "user.name", "Test"]);
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "first"]);
        let first = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_string();
        std::fs::write(dir.join("a.txt"), "two").unwrap();
        git(&["add", "."]);
        git(&["commit", "-qm", "second"]);
        let second = String::from_utf8(git(&["rev-parse", "HEAD"]).stdout)
            .unwrap()
            .trim()
            .to_string();

        assert_eq!(
            commit_is_strict_ancestor(dir, &first, &second).await,
            Some(true),
            "the earlier commit IS a strict ancestor of the later one"
        );
        assert_eq!(
            commit_is_strict_ancestor(dir, &second, &first).await,
            Some(false),
            "the later commit is NOT an ancestor of the earlier one"
        );
        assert_eq!(
            commit_is_strict_ancestor(dir, &second[..9], &second).await,
            Some(false),
            "the same commit abbreviated is still not a strict ancestor of itself"
        );
        assert_eq!(
            commit_is_strict_ancestor(dir, "0000000000000000000000000000000000000000", &second)
                .await,
            None,
            "an unknown object is indeterminate, never 'provably older'"
        );
    }
}
