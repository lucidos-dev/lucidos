use std::path::Path;
use std::process::Command;

/// The repo tracks `knowhow/` and nothing else. The directory also holds the
/// local token and the paired devices. A staged blob copies a file without its
/// permissions.
const USER_DIR_GITIGNORE: &str = "/*\n!/.gitignore\n!/knowhow/\n";

/// Ensure the user-level Lucidos directory is a git repo that tracks only
/// `knowhow/`, with `.git` readable by its owner alone. A no-op when the
/// directory does not exist.
pub fn ensure_git_init(user_dir: &Path) {
    if !user_dir.exists() {
        return;
    }
    if user_dir.join(".git").exists() {
        restrict_git_dir(user_dir);
        return;
    }
    log!("[UserDir] Initializing git repo at {}", user_dir.display());
    let output = Command::new("git")
        .args(["init"])
        .current_dir(user_dir)
        .output();
    match output {
        Ok(o) if o.status.success() => {
            log!("[UserDir] Git repo initialized");
            restrict_git_dir(user_dir);
            if let Err(e) = std::fs::write(user_dir.join(".gitignore"), USER_DIR_GITIGNORE) {
                log!("[UserDir] Could not write .gitignore, so nothing is staged: {e}");
                return;
            }
            let mut stage = vec!["add", "--", ".gitignore"];
            if user_dir.join("knowhow").is_dir() {
                stage.push("knowhow");
            }
            // Initial commit so there's a HEAD for subsequent auto_commit calls
            match Command::new("git")
                .args(&stage)
                .current_dir(user_dir)
                .output()
            {
                Ok(o) if !o.status.success() => log!(
                    "[UserDir] git add failed: {}",
                    String::from_utf8_lossy(&o.stderr)
                ),
                Err(e) => log!("[UserDir] git add failed: {}", e),
                _ => {}
            }
            match Command::new("git")
                .args(["commit", "-m", "initial commit", "--allow-empty"])
                .current_dir(user_dir)
                .output()
            {
                Ok(o) if !o.status.success() => log!(
                    "[UserDir] initial commit failed: {}",
                    String::from_utf8_lossy(&o.stderr)
                ),
                Err(e) => log!("[UserDir] initial commit failed: {}", e),
                _ => {}
            }
        }
        Ok(o) => log!(
            "[UserDir] git init failed: {}",
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) => log!("[UserDir] Failed to run git: {}", e),
    }
}

/// Make `.git` owner-only. Git writes its objects world-readable, so this is
/// what keeps a secret an older engine already staged away from other accounts.
#[cfg(unix)]
fn restrict_git_dir(user_dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let git_dir = user_dir.join(".git");
    if let Err(e) = std::fs::set_permissions(&git_dir, std::fs::Permissions::from_mode(0o700)) {
        log!(
            "[UserDir] Could not make {} owner-only: {e}",
            git_dir.display()
        );
    }
}

#[cfg(not(unix))]
fn restrict_git_dir(_user_dir: &Path) {}

/// Stage a file and commit in the user dir. Best-effort — logs errors but does not propagate.
pub fn auto_commit(user_dir: &Path, relative_path: &str, message: &str) {
    // `--` for the same reason the commit below has one: `relative_path` comes
    // from a model-supplied file tool path, and a name opening with `-` is read
    // as an option. `git add -A` would stage the whole tree instead of the one
    // file the caller asked for.
    let add = Command::new("git")
        .args(["add", "--", relative_path])
        .current_dir(user_dir)
        .output();
    match add {
        Ok(o) if !o.status.success() => {
            log!(
                "[UserDir] git add failed: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            return;
        }
        Err(e) => {
            log!("[UserDir] git add failed: {}", e);
            return;
        }
        _ => {}
    }
    let commit = Command::new("git")
        .args(["commit", "-m", message, "--", relative_path])
        .current_dir(user_dir)
        .output();
    match commit {
        Ok(o) if o.status.success() => {
            log!("[UserDir] Committed: {}", message);
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            if !stderr.contains("nothing to commit") {
                log!("[UserDir] git commit warning: {}", stderr);
            }
        }
        Err(e) => log!("[UserDir] git commit failed: {}", e),
    }
}

/// Delete a file in the user dir and commit the removal. The delete fails
/// loudly, so a caller never reports a file gone that is still there. The
/// commit is best-effort, as in [`auto_commit`].
pub fn remove_and_commit(
    user_dir: &Path,
    relative_path: &str,
    message: &str,
) -> std::io::Result<()> {
    std::fs::remove_file(user_dir.join(relative_path))?;
    auto_commit(user_dir, relative_path, message);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_git_init_creates_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".lucidos");
        std::fs::create_dir_all(dir.join("knowhow")).unwrap();

        ensure_git_init(&dir);
        assert!(
            dir.join(".git").exists(),
            "should have initialized git repo"
        );
    }

    /// The first boot ran `git add .` over the whole directory, which copied
    /// the 0600 local token into a world-readable git object.
    #[test]
    #[cfg(unix)]
    fn ensure_git_init_never_stages_outside_knowhow() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".lucidos");
        std::fs::create_dir_all(dir.join("knowhow")).unwrap();
        std::fs::write(dir.join("knowhow/a.md"), "Content.").unwrap();
        std::fs::write(dir.join("local-token"), "secret-token-bytes").unwrap();
        std::fs::write(dir.join("paired-devices.json"), "{}").unwrap();

        ensure_git_init(&dir);

        let tracked = git_stdout(&dir, &["ls-files"]);
        assert_eq!(tracked, ".gitignore\nknowhow/a.md\n");
        let blob = git_stdout(&dir, &["hash-object", "local-token"]);
        let found = Command::new("git")
            .args(["cat-file", "-e", blob.trim()])
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(!found.status.success(), "no object holds the token");
        let mode = std::fs::metadata(dir.join(".git"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    /// An older engine already staged everything. Making `.git` owner-only is
    /// what closes those objects to other accounts.
    #[test]
    #[cfg(unix)]
    fn ensure_git_init_makes_an_existing_git_dir_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".lucidos");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::set_permissions(dir.join(".git"), std::fs::Permissions::from_mode(0o755)).unwrap();

        ensure_git_init(&dir);

        let mode = std::fs::metadata(dir.join(".git"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[test]
    fn ensure_git_init_skips_existing_repo() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".lucidos");
        std::fs::create_dir_all(&dir).unwrap();

        ensure_git_init(&dir);
        assert!(dir.join(".git").exists());

        // Second call should not fail
        ensure_git_init(&dir);
    }

    #[test]
    fn auto_commit_stages_and_commits() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".lucidos");
        std::fs::create_dir_all(dir.join("knowhow")).unwrap();
        ensure_git_init(&dir);

        // Write a file
        std::fs::write(
            dir.join("knowhow/test.md"),
            "---\nname: Test\n---\nContent.",
        )
        .unwrap();
        auto_commit(&dir, "knowhow/test.md", "update knowhow: test");

        // Check git log
        let output = std::process::Command::new("git")
            .args(["log", "--oneline", "-1"])
            .current_dir(&dir)
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&output.stdout);
        assert!(
            log.contains("update knowhow: test"),
            "commit message should match, got: {}",
            log
        );
    }

    fn git_stdout(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    #[test]
    fn remove_and_commit_deletes_the_file_and_records_the_removal() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".lucidos");
        std::fs::create_dir_all(dir.join("knowhow")).unwrap();
        ensure_git_init(&dir);
        std::fs::write(dir.join("knowhow/test.md"), "Content.").unwrap();
        auto_commit(&dir, "knowhow/test.md", "add knowhow: test");

        remove_and_commit(&dir, "knowhow/test.md", "delete knowhow: test").unwrap();

        assert!(!dir.join("knowhow/test.md").exists());
        let log = git_stdout(&dir, &["log", "--oneline", "-1"]);
        assert!(log.contains("delete knowhow: test"), "got: {log}");
        let status = git_stdout(&dir, &["status", "--porcelain"]);
        assert!(status.is_empty(), "the removal must be committed: {status}");
    }

    #[test]
    fn remove_and_commit_fails_when_the_file_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".lucidos");
        std::fs::create_dir_all(&dir).unwrap();
        ensure_git_init(&dir);

        assert!(remove_and_commit(&dir, "knowhow/absent.md", "delete").is_err());
    }
}
