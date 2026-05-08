//! Per-worktree post-commit hook installer.
//!
//! Phase 4.2 of the CC resume architecture: each CC commit emits a
//! `ChangeProposed` event in real time, instead of one aggregated emit at
//! end-of-turn. The mechanism is a `post-commit` git hook installed in the
//! worktree's per-worktree git hooks directory; the hook POSTs `{thread_id,
//! sha}` to the engine's `/api/internal/commit-made` endpoint.
//!
//! Design constraints:
//! - **Non-blocking commits.** The hook backgrounds the curl call so a slow
//!   or unavailable engine doesn't stall the developer's commit.
//! - **Engine-down tolerance.** `curl ... || true` swallows network errors;
//!   if the engine is down, we lose the per-commit event but the
//!   end-of-turn `changes`-table write still records the change for Apply.
//! - **Per-worktree, not shared.** The hook is written to
//!   `<worktree>/.git/info/../hooks/post-commit` resolved via `git
//!   rev-parse --git-path hooks/post-commit`, so each worktree gets its
//!   own thread-id-baked hook. (Linked worktrees have their own
//!   `.git/worktrees/<name>/hooks/` dir; main worktrees use
//!   `.git/hooks/`.)

use std::path::Path;

// `log!` is a macro re-exported at crate root via `#[macro_export]` — no
// `use` needed; calls resolve via `crate::log!(...)` style.

/// Install the `post-commit` git hook into the given CC worktree.
///
/// Uses a worktree-private hooks directory (set via `core.hooksPath` in the
/// worktree's per-worktree config) so the hook fires ONLY for commits made
/// inside this CC worktree — never for the developer's commits in the main
/// checkout or other worktrees on the same repo. Without this isolation,
/// the shared `.git/hooks/post-commit` (git's default) would call our
/// engine endpoint for every commit on every branch, including the user's
/// own work outside Lucidos.
///
/// The hook bakes in `thread_id` and `api_port` at install time so the
/// shell script itself stays minimal — no env-var lookup at fire time.
/// Idempotent: overwrites any existing hook (we own this file).
///
/// Failures are logged but never propagated — a missing hook only
/// degrades to "no per-commit events"; end-of-turn aggregation still
/// captures the change in the DB for Apply.
pub(super) async fn install_post_commit_hook(worktree_path: &Path, thread_id: uuid::Uuid) {
    let api_port =
        std::env::var("LUCIDOS_API_PORT").unwrap_or_else(|_| "3000".to_string());

    // Worktree-private hooks dir lives next to the worktree's gitdir
    // (`.git/worktrees/<name>/hooks/` for linked worktrees, `.git/hooks/`
    // for the main one). Resolve via `--git-path` which knows the right
    // gitdir for the calling worktree, then anchor to a `lucidos-hooks/`
    // subdir we own — avoids stomping any user-added hooks.
    let gitdir_path = match crate::engine::git_ops::git_cmd(
        &["rev-parse", "--git-path", "lucidos-hooks"],
        worktree_path,
    )
    .await
    {
        Ok(o) if o.status.success() => {
            let rel = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if rel.is_empty() {
                log!(
                    "[ClaudeCode] commit-hook install: empty lucidos-hooks path for {} — skipping",
                    worktree_path.display()
                );
                return;
            }
            if std::path::Path::new(&rel).is_absolute() {
                std::path::PathBuf::from(rel)
            } else {
                worktree_path.join(&rel)
            }
        }
        Ok(o) => {
            log!(
                "[ClaudeCode] commit-hook install: git rev-parse failed in {}: {}",
                worktree_path.display(),
                String::from_utf8_lossy(&o.stderr).trim()
            );
            return;
        }
        Err(e) => {
            log!(
                "[ClaudeCode] commit-hook install: git rev-parse errored in {}: {}",
                worktree_path.display(),
                e
            );
            return;
        }
    };

    let hook_path = gitdir_path.join("post-commit");
    if let Some(parent) = hook_path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            log!(
                "[ClaudeCode] commit-hook install: failed to create {} ({})",
                parent.display(),
                e
            );
            return;
        }
    }

    let script = render_hook_script(thread_id, &api_port);
    if let Err(e) = tokio::fs::write(&hook_path, script.as_bytes()).await {
        log!(
            "[ClaudeCode] commit-hook install: failed to write {} ({})",
            hook_path.display(),
            e
        );
        return;
    }

    // chmod 0o755 — without exec bit git silently skips the hook.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) =
            tokio::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).await
        {
            log!(
                "[ClaudeCode] commit-hook install: failed to chmod {} ({})",
                hook_path.display(),
                e
            );
            return;
        }
    }

    // Point THIS worktree's git at our private hooks dir. Requires
    // `extensions.worktreeConfig=true` so the setting is per-worktree
    // rather than shared via the main repo config.
    if let Err(e) = enable_worktree_config(worktree_path).await {
        log!(
            "[ClaudeCode] commit-hook install: enable_worktree_config failed for {} ({})",
            worktree_path.display(),
            e
        );
        return;
    }
    let hooks_dir_str = match gitdir_path.to_str() {
        Some(s) => s.to_string(),
        None => {
            log!(
                "[ClaudeCode] commit-hook install: hooks dir path is not UTF-8: {}",
                gitdir_path.display()
            );
            return;
        }
    };
    if let Err(e) = crate::engine::git_ops::git_cmd(
        &[
            "config",
            "--worktree",
            "core.hooksPath",
            &hooks_dir_str,
        ],
        worktree_path,
    )
    .await
    {
        log!(
            "[ClaudeCode] commit-hook install: failed to set core.hooksPath for {} ({})",
            worktree_path.display(),
            e
        );
        return;
    }

    log!(
        "[ClaudeCode] commit-hook installed at {} (thread {}, port {})",
        hook_path.display(),
        thread_id,
        api_port
    );
}

/// Enable `extensions.worktreeConfig` on the repo (shared setting; idempotent).
/// Without this, `git config --worktree ...` errors out with
/// "must be supported by extensions.worktreeConfig".
async fn enable_worktree_config(
    worktree_path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let out = crate::engine::git_ops::git_cmd(
        &["config", "extensions.worktreeConfig", "true"],
        worktree_path,
    )
    .await
    .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;
    if !out.status.success() {
        return Err(format!(
            "git config extensions.worktreeConfig=true failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    Ok(())
}

/// Render the shell script content for the post-commit hook.
///
/// The hook MUST:
/// - Run fast and never block the user's commit (background the curl call).
/// - Tolerate the engine being down (`|| true`).
/// - Carry both the thread id (so the engine knows which thread owns this
///   commit) and the SHA being committed.
fn render_hook_script(thread_id: uuid::Uuid, api_port: &str) -> String {
    format!(
        "#!/bin/sh
# Auto-generated by Lucidos engine — do not edit.
# Phase 4.2 of CC resume architecture: emit ChangeProposed per commit.
SHA=\"$(git rev-parse HEAD 2>/dev/null)\"
[ -n \"$SHA\" ] || exit 0
(curl -fsS -m 5 -X POST \"http://127.0.0.1:{port}/api/internal/commit-made\" \\
  -H \"Content-Type: application/json\" \\
  -d \"{{\\\"thread_id\\\":\\\"{thread_id}\\\",\\\"sha\\\":\\\"$SHA\\\"}}\" \\
  >/dev/null 2>&1 || true) &
exit 0
",
        port = api_port,
        thread_id = thread_id,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_script_contains_thread_id_and_port() {
        let tid = uuid::Uuid::nil();
        let script = render_hook_script(tid, "8765");
        assert!(script.contains("8765"));
        assert!(script.contains(&tid.to_string()));
        // Hook must be backgrounded so it never blocks the commit.
        assert!(script.contains(") &"));
        // Must tolerate engine-down.
        assert!(script.contains("|| true"));
    }

    #[test]
    fn rendered_script_starts_with_sh_shebang() {
        let script = render_hook_script(uuid::Uuid::nil(), "8080");
        assert!(script.starts_with("#!/bin/sh"));
    }

    #[tokio::test]
    async fn installer_writes_executable_hook_in_worktree() {
        // Set up a tiny git repo + branch + worktree, then install the hook
        // and verify it lands in the per-worktree hooks dir, with exec bits,
        // and that core.hooksPath is set to that dir for this worktree only.
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();

        let _ = crate::engine::git_ops::git_cmd(&["init", "-b", "main"], &repo).await;
        let _ = crate::engine::git_ops::git_cmd(
            &["config", "user.email", "test@example.com"],
            &repo,
        )
        .await;
        let _ = crate::engine::git_ops::git_cmd(&["config", "user.name", "Test"], &repo).await;
        std::fs::write(repo.join("seed.txt"), "x").unwrap();
        let _ = crate::engine::git_ops::git_cmd(&["add", "."], &repo).await;
        let _ = crate::engine::git_ops::git_cmd(&["commit", "-m", "init"], &repo).await;

        let wt = tmp.path().join("wt");
        let _ = crate::engine::git_ops::git_cmd(
            &[
                "worktree",
                "add",
                wt.to_str().unwrap(),
                "-b",
                "claude-code/hook-test",
            ],
            &repo,
        )
        .await;

        let tid = uuid::Uuid::new_v4();
        install_post_commit_hook(&wt, tid).await;

        // Resolve the lucidos-private hooks dir the same way the installer did.
        let resolved = crate::engine::git_ops::git_cmd(
            &["rev-parse", "--git-path", "lucidos-hooks/post-commit"],
            &wt,
        )
        .await
        .expect("git rev-parse");
        assert!(resolved.status.success());
        let hook_rel = String::from_utf8_lossy(&resolved.stdout).trim().to_string();
        let hook_path = if std::path::Path::new(&hook_rel).is_absolute() {
            std::path::PathBuf::from(hook_rel)
        } else {
            wt.join(hook_rel)
        };

        assert!(
            tokio::fs::try_exists(&hook_path).await.unwrap_or(false),
            "expected hook at {}",
            hook_path.display()
        );
        let body = tokio::fs::read_to_string(&hook_path).await.unwrap();
        assert!(body.contains(&tid.to_string()));
        assert!(body.contains("/api/internal/commit-made"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = tokio::fs::metadata(&hook_path)
                .await
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o755, "hook must be 0755 to fire");
        }

        // The CC worktree should have core.hooksPath pointing at the
        // lucidos-hooks dir; the main repo must NOT have it set, so the
        // developer's commits in the main checkout never fire our hook.
        let wt_hp = crate::engine::git_ops::git_cmd(
            &["config", "--worktree", "core.hooksPath"],
            &wt,
        )
        .await
        .expect("read worktree hooksPath");
        assert!(
            wt_hp.status.success(),
            "expected core.hooksPath set on the CC worktree"
        );
        assert!(
            String::from_utf8_lossy(&wt_hp.stdout)
                .trim()
                .contains("lucidos-hooks"),
            "core.hooksPath should point at lucidos-hooks dir, got {}",
            String::from_utf8_lossy(&wt_hp.stdout).trim()
        );

        // Main repo should not have a worktree-scoped hooksPath set
        // (extensions.worktreeConfig isolation).
        let main_hp = crate::engine::git_ops::git_cmd(
            &["config", "--worktree", "core.hooksPath"],
            &repo,
        )
        .await
        .expect("read main hooksPath");
        // Either non-zero exit (key missing) or empty stdout — both mean
        // "main repo is unaffected".
        let main_set =
            main_hp.status.success() && !String::from_utf8_lossy(&main_hp.stdout).trim().is_empty();
        assert!(
            !main_set,
            "main repo must not have core.hooksPath set; got {:?}",
            String::from_utf8_lossy(&main_hp.stdout)
        );
    }

    #[tokio::test]
    async fn end_to_end_commit_in_worktree_invokes_hook() {
        // Confirm git actually fires the installed hook. We replace the
        // engine endpoint with a tiny TCP listener that accepts one line
        // and records what landed; then we run a real `git commit` inside
        // the worktree and assert the recorder sees a POST.
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port().to_string();
        std::env::set_var("LUCIDOS_API_PORT", &port);

        let (tx, rx) = std::sync::mpsc::channel::<String>();
        let server = std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut reader = BufReader::new(
                    sock.try_clone().expect("clone tcp"),
                );
                let mut request_line = String::new();
                let _ = reader.read_line(&mut request_line);
                let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
                let _ = tx.send(request_line);
            }
        });

        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let _ = crate::engine::git_ops::git_cmd(&["init", "-b", "main"], &repo).await;
        let _ = crate::engine::git_ops::git_cmd(
            &["config", "user.email", "test@example.com"],
            &repo,
        )
        .await;
        let _ = crate::engine::git_ops::git_cmd(&["config", "user.name", "Test"], &repo).await;
        std::fs::write(repo.join("seed.txt"), "x").unwrap();
        let _ = crate::engine::git_ops::git_cmd(&["add", "."], &repo).await;
        let _ = crate::engine::git_ops::git_cmd(&["commit", "-m", "init"], &repo).await;

        let wt = tmp.path().join("wt");
        let _ = crate::engine::git_ops::git_cmd(
            &[
                "worktree",
                "add",
                wt.to_str().unwrap(),
                "-b",
                "claude-code/hook-fires",
            ],
            &repo,
        )
        .await;
        let _ = crate::engine::git_ops::git_cmd(
            &["config", "user.email", "test@example.com"],
            &wt,
        )
        .await;
        let _ = crate::engine::git_ops::git_cmd(&["config", "user.name", "Test"], &wt).await;

        let tid = uuid::Uuid::new_v4();
        install_post_commit_hook(&wt, tid).await;

        // Make a real commit in the worktree.
        std::fs::write(wt.join("a.txt"), "hello").unwrap();
        let _ = crate::engine::git_ops::git_cmd(&["add", "a.txt"], &wt).await;
        let _ = crate::engine::git_ops::git_cmd(&["commit", "-m", "real commit"], &wt).await;

        // Wait briefly — the hook backgrounds the curl call.
        let request = tokio::task::spawn_blocking(move || {
            rx.recv_timeout(std::time::Duration::from_secs(5)).ok()
        })
        .await
        .ok()
        .flatten();
        let _ = server.join();

        let request = request.expect("hook should POST to /api/internal/commit-made");
        assert!(
            request.contains("/api/internal/commit-made"),
            "request line should target the engine endpoint: {:?}",
            request
        );
    }
}
