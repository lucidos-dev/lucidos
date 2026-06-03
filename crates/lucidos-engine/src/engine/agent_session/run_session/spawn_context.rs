use crate::engine::agent_session::node_modules_setup::has_install_marker;
use crate::engine::agent_session::prompts::{
    app_worktree_recovery_system_prompt, app_worktree_system_prompt,
    conflict_resolution_system_prompt, external_repo_recovery_system_prompt,
    external_repo_system_prompt, recovery_system_prompt, worktree_system_prompt,
};
use crate::engine::agent_session::spawn::{resolve_branch_for_resume, BranchResolution};
use crate::engine::claude_code::{WORKTREE_EXCLUDE_PATHS, WORKTREE_WORKSPACE_MARKER};
use crate::engine::git_ops::{
    add_paths_to_worktree_exclude, catchup_with_main, resolve_worktree_base, worktree_add,
    worktree_current_branch,
};
use crate::engine::LucidosEngine;
use std::path::PathBuf;
use uuid::Uuid;

/// Resolved worktree / branch / system-prompt context for a `run_direct_agent`
/// spawn. Produced by [`LucidosEngine::resolve_run_worktree_context`], the
/// "start" lifecycle stage extracted from the driver: it picks recovery vs
/// conflict vs normal mode, creates/reuses the isolated worktree, links
/// node_modules, writes the workspace marker, and builds the system prompt.
pub(super) struct SpawnWorktreeContext {
    pub(super) cwd: PathBuf,
    pub(super) system_prompt: String,
    pub(super) branch_name: String,
    pub(super) worktree_path: Option<PathBuf>,
    pub(super) interactive_session: bool,
    pub(super) adoption_note: Option<String>,
    pub(super) resume_session_id: Option<String>,
}

impl LucidosEngine {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn resolve_run_worktree_context(
        &self,
        recovery_worktree: Option<(PathBuf, String)>,
        conflict_change: &Option<crate::core::changes::Change>,
        system_prompt_override: Option<String>,
        app_spawn_id: &Option<String>,
        is_app_spawn: bool,
        is_external_repo: bool,
        external_repo_name: Option<String>,
        workspace_name: &str,
        repo_root: &std::path::Path,
        repo_id: &Option<String>,
        last_idle_sha: &Option<String>,
        resume_worktree_path: Option<PathBuf>,
        resume_branch: Option<String>,
        mut resume_session_id: Option<String>,
        thread_id: Uuid,
        cc_start: std::time::Instant,
    ) -> Result<SpawnWorktreeContext, Box<dyn std::error::Error + Send + Sync>> {
        let mut adoption_note: Option<String> = None;
        let (cwd, system_prompt, branch_name, worktree_path, interactive_session) = if let Some(
            (wt_path, branch),
        ) =
            recovery_worktree
        {
            // Recovery mode: reuse an orphaned worktree from a previous session
            log!(
                "[ClaudeCode] Starting recovery session on orphaned worktree: {} (branch {})",
                wt_path.display(),
                branch
            );
            let system_prompt = if let Some(sp) = system_prompt_override {
                sp
            } else if let Some(ref app_id) = app_spawn_id {
                app_worktree_recovery_system_prompt(&branch, workspace_name, app_id)
            } else if let Some(ref name) = external_repo_name {
                external_repo_recovery_system_prompt(name, &branch)
            } else {
                recovery_system_prompt(&branch, workspace_name)
            };
            (wt_path.clone(), system_prompt, branch, Some(wt_path), true)
        } else if let Some(ref change) = conflict_change {
            // Conflict mode: run in the merge worktree where the merge is in progress
            let wt_path_str = change
                .merge_worktree_path
                .as_ref()
                .ok_or("Conflict change has no merge worktree path — was the merge started?")?;
            let temp_branch = change
                .merge_temp_branch
                .as_ref()
                .ok_or("Conflict change has no merge temp branch")?;
            let cwd = PathBuf::from(wt_path_str);
            let system_prompt = conflict_resolution_system_prompt().to_string();
            (
                cwd.clone(),
                system_prompt,
                temp_branch.clone(),
                Some(cwd),
                false,
            )
        } else {
            // Normal mode: create an isolated worktree.
            // If resuming an idle session, reuse the previous branch (preserving its changes)
            // instead of creating a fresh branch that starts empty.
            let BranchResolution {
                mut branch_name,
                reusing_branch,
                resume_session_id: validated_sid,
            } = resolve_branch_for_resume(
                repo_root,
                resume_session_id,
                resume_branch.as_deref(),
            )
            .await;
            resume_session_id = validated_sid;
            // App coding-agent threads use a different branch shape so a
            // `git branch -a` on the workspace git is greppable by app id.
            // Only override fresh branches; resumed branches keep their
            // existing name (which already encodes the app id from the
            // first spawn).
            if is_app_spawn && !reusing_branch {
                if let Some(ref app_id) = app_spawn_id {
                    branch_name = crate::engine::git_ops::generate_app_branch_name(app_id);
                }
            }

            // Caller-supplied worktree path wins over branch-based lookup —
            // see the `resume_worktree_path` parameter doc. The existence
            // check guards downstream code that would otherwise treat a
            // missing path as "existing worktree" and skip both the lookup
            // and the create-worktree fallback.
            let caller_worktree = resume_worktree_path
                .as_ref()
                .filter(|p| p.exists() && p.join(".git").exists())
                .cloned();
            if let Some(ref caller) = caller_worktree {
                log!(
                    "[ClaudeCode] Using caller-supplied worktree path for resume: {}",
                    caller.display()
                );
            } else if let Some(ref skipped) = resume_worktree_path {
                log!(
                    "[ClaudeCode] Caller-supplied worktree path {} no longer exists — falling back to branch lookup",
                    skipped.display()
                );
            }

            // Phase 6.1: Resolve a persistent per-thread worktree path via
            // the central resolver, which checks (in order):
            //   1. The most recent `CodingAgentIdled` event's payload
            //   2. `git worktree list` filtered to the reused branch
            //   3. New deterministic `thread-<short>` path
            //
            // The caller-supplied path (set on resume routes) overrides the
            // resolver — its existence was already validated upstream.
            let resolved_path = if let Some(ref p) = caller_worktree {
                p.clone()
            } else {
                crate::engine::agent_session::resume::resolve_worktree_path(
                    self.pool(),
                    thread_id,
                    self.workspace_path(),
                    repo_root,
                    if reusing_branch { Some(&branch_name) } else { None },
                )
                .await
            };

            // Treat the path as "existing" only if a real worktree (with a
            // `.git` entry) lives there. If the recorded path no longer
            // exists, fall through to the create branch below — calling
            // `git worktree add` against an extinct location is fine; calling
            // it against a stale directory would just error and surface to
            // the user.
            let existing_worktree = if resolved_path.exists()
                && resolved_path.join(".git").exists()
            {
                Some(resolved_path.clone())
            } else {
                None
            };

            let wt_path = if let Some(ref existing) = existing_worktree {
                log!(
                    "[ClaudeCode] Reusing existing worktree at {} for branch {}",
                    existing.display(),
                    branch_name
                );
                existing.clone()
            } else {
                resolved_path
            };

            // When reusing an existing worktree, the actual checked-out branch may
            // differ from `branch_name`. Two reasons for the divergence:
            //
            // 1. CC itself ran `git checkout -b foo` mid-turn — legitimate; the
            //    new branch is the source of truth and we override.
            // 2. The user externally ran `git checkout other-branch` between
            //    turns. Phase 8.3: refuse the spawn when we have a recorded
            //    resume branch (`reusing_branch`) and the worktree no longer
            //    sits on it. Continuing would silently commit CC's work to the
            //    wrong ref. Discriminating (1) from (2) reliably is hard, so
            //    the trip-wire is "we had an expected branch coming in and
            //    the worktree disagrees." First spawns (no resume context)
            //    fall through to the override path because there's no
            //    expectation to violate.
            let branch_name = if let Some(ref existing) = existing_worktree {
                if reusing_branch {
                    match crate::engine::agent_session::external_edits::verify_branch(existing, &branch_name).await {
                        Ok(()) => branch_name,
                        Err(mismatch) => {
                            // External repos: a skill may legitimately have
                            // created a feature branch off our tracked one
                            // (e.g. `git checkout -b UA-1234`). Adopt it
                            // when its history contains our last commit.
                            // Internal threads keep the strict refusal so
                            // Apply has a stable claude-code/<id> branch.
                            let adopted = if is_external_repo {
                                crate::engine::agent_session::external_edits::try_adopt_renegade_branch(
                                    existing,
                                    last_idle_sha.as_deref(),
                                )
                                .await
                            } else {
                                None
                            };
                            match adopted {
                                Some((new_branch, note)) => {
                                    log!(
                                        "[ClaudeCode] Adopting renegade branch '{}' (was expecting '{}') for thread {}",
                                        new_branch,
                                        branch_name,
                                        thread_id
                                    );
                                    adoption_note = Some(note);
                                    new_branch
                                }
                                None => {
                                    log!(
                                        "[ClaudeCode] Refusing spawn for thread {}: {}",
                                        thread_id,
                                        mismatch
                                    );
                                    return Err(format!(
                                        "Refusing to spawn Claude Code: {}",
                                        mismatch
                                    )
                                    .into());
                                }
                            }
                        }
                    }
                } else {
                    match worktree_current_branch(existing).await {
                        Some(a) if a != branch_name => {
                            log!(
                                "[ClaudeCode] Worktree {} is on branch {} (expected {}) — overriding",
                                wt_path.display(),
                                a,
                                branch_name
                            );
                            a
                        }
                        _ => branch_name,
                    }
                }
            } else {
                branch_name
            };

            // Resolve the base ref the new worktree branches from. NEVER `HEAD`
            // for a Lucidos-source spawn: a shared checkout parked on an
            // unrelated in-flight `claude-code/*` branch would otherwise leak
            // that branch's commits into this thread as phantom "pending
            // changes". External repos keep the origin-or-HEAD contract.
            let worktree_base_branch = resolve_worktree_base(repo_root, is_external_repo).await;

            // Create worktree if we're not reusing an existing one. If the
            // deterministic path already exists ON DISK but git doesn't know
            // about it (e.g. residue from an interrupted previous spawn), do
            // NOT blow it away — we'd risk destroying user work. Surface the
            // collision via the standard `git worktree add` error path so the
            // caller can investigate. (The inverse residue — git still has the
            // path registered but the directory is GONE — is auto-healed inside
            // `worktree_add` via `git worktree prune`; that case has no user
            // work to lose since the directory is already absent.)
            if existing_worktree.is_none() {
                if is_app_spawn {
                    // App spawn — sparse-checkout worktree narrowed to
                    // `data/apps/<id>/`. The standard `worktree_add` would
                    // materialise the whole workspace tree, which is
                    // unnecessary and would expose other apps' folders to CC.
                    let Some(ref app_id) = app_spawn_id else {
                        return Err(
                            "Internal: is_app_spawn set without app_spawn_id".into()
                        );
                    };
                    if let Err(e) = crate::engine::git_ops::create_sparse_app_worktree(
                        repo_root,
                        app_id,
                        &branch_name,
                        &wt_path,
                    )
                    .await
                    {
                        log!("[ClaudeCode] Failed to create sparse app worktree: {}", e);
                        return Err(format!(
                            "Failed to create sparse-checkout app worktree: {e}"
                        )
                        .into());
                    }
                    log!(
                        "[ClaudeCode] Created sparse app worktree at {} on branch {} (app={})",
                        wt_path.display(),
                        branch_name,
                        app_id
                    );
                } else {
                let wt_extra: Vec<&str> = if reusing_branch {
                    vec![&branch_name]
                } else {
                    let mut args = vec!["-b", &branch_name];
                    if let Some(ref base_ref) = worktree_base_branch {
                        args.push(base_ref);
                    }
                    args
                };
                match worktree_add(repo_root, &wt_path, &wt_extra).await {
                    Ok(o) if o.status.success() => {
                        if reusing_branch {
                            log!(
                                "[ClaudeCode] Resumed worktree at {} on existing branch {}",
                                wt_path.display(),
                                branch_name
                            );
                        } else {
                            log!(
                                "[ClaudeCode] Created worktree at {} on branch {}{}",
                                wt_path.display(),
                                branch_name,
                                worktree_base_branch
                                    .as_ref()
                                    .map(|b| format!(" (from {})", b))
                                    .unwrap_or_default()
                            );
                        }
                    }
                    Ok(o) => {
                        let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
                        log!("[ClaudeCode] Failed to create worktree: {}", stderr);
                        return Err(format!(
                            "Failed to create git worktree for Claude Code isolation: {}",
                            stderr
                        )
                        .into());
                    }
                    Err(e) => {
                        log!("[ClaudeCode] Failed to run git worktree: {}", e);
                        return Err(format!(
                            "Failed to create git worktree for Claude Code isolation: {}",
                            e
                        )
                        .into());
                    }
                }
                } // end non-app spawn branch
            }

            log!(
                "[ClaudeCode] [TIMING] worktree created: {:?}",
                cc_start.elapsed()
            );

            // Resumed/reused worktrees may be behind main. Catch them up so they
            // run the latest scripts/, configs, and source rather than stale
            // copies that pre-date recent fixes. New worktrees branched from
            // origin/main are already up to date and skip this.
            // App spawns skip catchup — the workspace git has no `origin` by
            // default, and catchup_with_main would noisily fail.
            if !is_external_repo
                && !is_app_spawn
                && (reusing_branch || existing_worktree.is_some())
            {
                if let Err(e) = catchup_with_main(&wt_path).await {
                    log!(
                        "[ClaudeCode] catchup_with_main failed for {} ({}) -- worktree may be running stale scripts",
                        wt_path.display(),
                        e
                    );
                }
            }

            // Copy node_modules from main repo to worktree so frontend tests work.
            // Much faster than `npm ci` (~2s copy vs 2-10min install).
            // Can't symlink: npm install in the worktree follows the symlink
            // and corrupts/deletes the main repo's real node_modules.
            // Apps don't share Lucidos's node_modules — they're standalone
            // static folders. Skip the copy.
            if !is_external_repo && !is_app_spawn {
                let wt_node_modules = wt_path.join("crates/lucidos-app/node_modules");
                if !has_install_marker(&wt_node_modules) {
                    let src_node_modules = repo_root.join("crates/lucidos-app/node_modules");
                    if has_install_marker(&src_node_modules) {
                        // `cp src dst` nests as `dst/src` when dst already
                        // exists, so any partial dir (e.g. Vite's `.vite/`
                        // cache) must go before linking. Skip the link if
                        // clearing fails — proceeding would silently nest
                        // node_modules and break every subsequent run.
                        let cleared = match tokio::fs::remove_dir_all(&wt_node_modules).await {
                            Ok(()) => true,
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
                            Err(e) => {
                                log!(
                                    "[ClaudeCode] Skipping node_modules link — failed to clear stale dir: {}",
                                    e
                                );
                                false
                            }
                        };
                        if cleared {
                            log!("[ClaudeCode] Linking node_modules to worktree...");
                            // Try hardlinks first (instant, zero disk space).
                            // Falls back to full copy if hardlinks fail (e.g. cross-filesystem).
                            let hl = tokio::process::Command::new("cp")
                                .args([
                                    "-al",
                                    &src_node_modules.to_string_lossy(),
                                    &wt_node_modules.to_string_lossy(),
                                ])
                                .output()
                                .await;
                            let ok = matches!(&hl, Ok(o) if o.status.success());
                            if ok {
                                log!("[ClaudeCode] node_modules hardlinked to worktree");
                            } else {
                                match tokio::process::Command::new("cp")
                                    .args([
                                        "-a",
                                        &src_node_modules.to_string_lossy(),
                                        &wt_node_modules.to_string_lossy(),
                                    ])
                                    .output()
                                    .await
                                {
                                    Ok(o) if o.status.success() => {
                                        log!("[ClaudeCode] node_modules copied to worktree");
                                    }
                                    Ok(o) => log!(
                                        "[ClaudeCode] cp node_modules failed: {}",
                                        String::from_utf8_lossy(&o.stderr).trim()
                                    ),
                                    Err(e) => {
                                        log!("[ClaudeCode] Failed to copy node_modules: {}", e)
                                    }
                                }
                            }
                        }
                    } else {
                        // npm ci wipes node_modules itself, so any stale Vite
                        // cache here is fine to leave.
                        log!(
                            "[ClaudeCode] No node_modules in main repo to copy, running npm ci..."
                        );
                        match tokio::process::Command::new("npm")
                            .args(["ci", "--prefer-offline"])
                            .current_dir(wt_path.join("crates/lucidos-app"))
                            .output()
                            .await
                        {
                            Ok(o) if o.status.success() => {
                                log!("[ClaudeCode] node_modules installed in worktree");
                            }
                            Ok(o) => log!(
                                "[ClaudeCode] npm ci failed in worktree: {}",
                                String::from_utf8_lossy(&o.stderr).trim()
                            ),
                            Err(e) => log!("[ClaudeCode] Failed to run npm ci in worktree: {}", e),
                        }
                    }
                }
            }

            log!(
                "[ClaudeCode] [TIMING] node_modules setup done: {:?}",
                cc_start.elapsed()
            );

            // Tag the worktree with the owning workspace so orphan recovery
            // only cleans up worktrees belonging to this workspace. The
            // optional second line is the external repo's UUID — written only
            // when the session targets a registered external repo, so the
            // marker stays interpretable as "second line ⇒ external".
            let marker = wt_path.join(WORKTREE_WORKSPACE_MARKER);
            let ws_id = self.workspace_path.to_string_lossy().to_string();
            let marker_content = match (is_external_repo, repo_id.as_ref()) {
                (true, Some(rid)) => format!("{}\n{}", ws_id, rid),
                _ => ws_id,
            };
            if let Err(e) = tokio::fs::write(&marker, &marker_content).await {
                log!("[ClaudeCode] Failed to write workspace marker: {}", e);
            }

            // Add engine-injected paths to the worktree's git exclude so external
            // repos don't see them as untracked or accidentally commit them.
            add_paths_to_worktree_exclude(&wt_path, WORKTREE_EXCLUDE_PATHS).await;

            // App spawns run CC inside the app folder (per §1 decision:
            // deep cwd). Other kinds run at the worktree root.
            let cwd = if let Some(ref app_id) = app_spawn_id {
                wt_path.join("data").join("apps").join(app_id)
            } else {
                wt_path.clone()
            };
            let system_prompt = if let Some(ref app_id) = app_spawn_id {
                // Inline the manifest so CC has the app's display name +
                // icon + intent without needing to Read first. Fall back
                // to a placeholder when the manifest is missing on disk
                // (fresh worktree race).
                let manifest_path = wt_path
                    .join("data")
                    .join("apps")
                    .join(app_id)
                    .join("manifest.json");
                let manifest_json = match tokio::fs::read_to_string(&manifest_path).await {
                    Ok(s) => s,
                    Err(_) => format!(
                        "(manifest.json not yet on disk at {}; Read it to see the full app spec)",
                        manifest_path.display()
                    ),
                };
                app_worktree_system_prompt(&branch_name, workspace_name, app_id, &manifest_json)
            } else if let Some(ref name) = external_repo_name {
                let base = worktree_base_branch.as_deref().unwrap_or("origin/main");
                external_repo_system_prompt(name, &branch_name, base)
            } else {
                worktree_system_prompt(&branch_name, workspace_name)
            };
            (cwd, system_prompt, branch_name, Some(wt_path), true)
        };
        Ok(SpawnWorktreeContext {
            cwd,
            system_prompt,
            branch_name,
            worktree_path,
            interactive_session,
            adoption_note,
            resume_session_id,
        })
    }
}
