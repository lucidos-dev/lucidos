use std::path::Path;
use std::process::Command;

/// Ensure the user-level Lucidos directory is a git repo. No-op if .git/ already exists or dir doesn't exist.
pub fn ensure_git_init(user_dir: &Path) {
    if !user_dir.exists() {
        return;
    }
    if user_dir.join(".git").exists() {
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
            // Initial commit so there's a HEAD for subsequent auto_commit calls
            match Command::new("git")
                .args(["add", "."])
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

/// Stage a file and commit in the user dir. Best-effort — logs errors but does not propagate.
pub fn auto_commit(user_dir: &Path, relative_path: &str, message: &str) {
    let add = Command::new("git")
        .args(["add", relative_path])
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
}
