use crate::engine::agent_session::node_modules_setup::{
    has_install_marker, member_node_modules_links,
};
use crate::engine::agent_session::prompts::{
    app_worktree_recovery_system_prompt, app_worktree_system_prompt, append_backend_rules,
    conflict_resolution_system_prompt, external_repo_recovery_system_prompt,
    external_repo_system_prompt, recovery_system_prompt, worktree_system_prompt,
};
use crate::engine::agent_session::spawn::{resolve_branch_for_resume, BranchResolution};
use crate::engine::claude_code::{WORKTREE_EXCLUDE_PATHS, WORKTREE_WORKSPACE_MARKER};
use crate::engine::git_ops::{
    add_paths_to_worktree_exclude, catchup_with_main, install_coding_agent_diff_hook,
    resolve_worktree_base, worktree_add, worktree_current_branch,
};
use crate::engine::LucidosEngine;
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// Hardlink a `node_modules` tree from `src` into the worktree at `dst`.
/// Clears any partial `dst` first (a stale dir makes `cp` nest as `dst/src`),
/// then tries instant hardlinks (`cp -al`), falling back to a full copy
/// (`cp -a`, e.g. across filesystems). `label` names the tree in logs
/// (e.g. "node_modules", "crates/lucidos-app/node_modules"). Best-effort: a
/// failure is logged, not fatal — a missing frontend tree degrades tests, it
/// doesn't break the session.
async fn link_node_modules_tree(src: &Path, dst: &Path, label: &str) {
    match tokio::fs::remove_dir_all(dst).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            log!(
                "[AgentSession] Skipping {} link — failed to clear stale dir: {}",
                label,
                e
            );
            return;
        }
    }
    log!("[AgentSession] Linking {} to worktree...", label);
    // Try hardlinks first (instant, zero disk space); fall back to a full copy.
    let hl = tokio::process::Command::new("cp")
        .args(["-al", &src.to_string_lossy(), &dst.to_string_lossy()])
        .output()
        .await;
    if matches!(&hl, Ok(o) if o.status.success()) {
        log!("[AgentSession] {} hardlinked to worktree", label);
        return;
    }
    match tokio::process::Command::new("cp")
        .args(["-a", &src.to_string_lossy(), &dst.to_string_lossy()])
        .output()
        .await
    {
        Ok(o) if o.status.success() => log!("[AgentSession] {} copied to worktree", label),
        Ok(o) => log!(
            "[AgentSession] cp {} failed: {}",
            label,
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => log!("[AgentSession] Failed to copy {}: {}", label, e),
    }
}

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
    /// True when THIS call created the worktree directory (vs. reusing a
    /// recovery / conflict / resume worktree). The spawn-failure cleanup
    /// (`git_ops::cleanup_failed_spawn`) only removes what this attempt
    /// created — a pre-existing worktree holds the thread's work.
    pub(super) worktree_created: bool,
    /// True when THIS call created `branch_name` (fresh spawn, `-b`). Same
    /// cleanup contract: a reused branch may hold committed work and must
    /// survive a failed spawn.
    pub(super) branch_created: bool,
}

impl LucidosEngine {
    /// Resolves which worktree + branch this spawn runs in, returning the
    /// [`SpawnWorktreeContext`] whose `*_created` flags the failure-path cleanup
    /// depends on. The parameter list mirrors the caller's already-destructured
    /// spawn request one-to-one — bundling it into a struct here would only move
    /// the same fields, so the arity lint is allowed rather than worked around.
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
        // Backend already resolved by `run_direct_agent` (stored value beats
        // the request) — threaded through so `append_backend_rules` can pick
        // the Codex teaching section without re-querying thread_summaries.
        coding_agent: crate::runtime::CodingAgent,
    ) -> Result<SpawnWorktreeContext, Box<dyn std::error::Error + Send + Sync>> {
        let mut adoption_note: Option<String> = None;
        let (
            cwd,
            system_prompt,
            branch_name,
            worktree_path,
            interactive_session,
            worktree_created,
            branch_created,
        ) = if let Some((wt_path, branch)) = recovery_worktree {
            // Recovery mode: reuse an orphaned worktree from a previous session
            log!(
                "[AgentSession] Starting recovery session on orphaned worktree: {} (branch {})",
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
            // Reused worktree + branch — nothing here is ours to delete on
            // a failed spawn.
            (
                wt_path.clone(),
                system_prompt,
                branch,
                Some(wt_path),
                true,
                false,
                false,
            )
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
            // The merge worktree + temp branch belong to the in-progress
            // merge, not to this spawn — keep them on failure.
            (
                cwd.clone(),
                system_prompt,
                temp_branch.clone(),
                Some(cwd),
                false,
                false,
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
            } = resolve_branch_for_resume(repo_root, resume_session_id, resume_branch.as_deref())
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
            // see the `resume_worktree_path` parameter doc. The validity check
            // guards downstream code that would otherwise treat a missing or
            // stranded path as "existing worktree" and skip both the lookup
            // and the create-worktree fallback. `is_live_worktree_at` (not a
            // bare `.git` existence test) ensures we never reuse a torn-down
            // tree that git resolves to the enclosing data repo.
            let caller_worktree = match resume_worktree_path.as_ref() {
                Some(p) if crate::engine::git_ops::is_live_worktree_at(p).await => Some(p.clone()),
                _ => None,
            };
            if let Some(ref caller) = caller_worktree {
                log!(
                    "[AgentSession] Using caller-supplied worktree path for resume: {}",
                    caller.display()
                );
            } else if let Some(ref skipped) = resume_worktree_path {
                log!(
                    "[AgentSession] Caller-supplied worktree path {} is missing or not a live linked worktree — falling back to branch lookup",
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
                    if reusing_branch {
                        Some(&branch_name)
                    } else {
                        None
                    },
                )
                .await
            };

            // Treat the path as "existing" only if a *live linked worktree*
            // lives there — toplevel resolves to the path itself. A bare
            // `.git`-exists test passes for a STRANDED tree (admin dir pruned
            // by a cleanup that raced this resume, or a half-finished prior
            // add), where git instead resolves upward to the enclosing data
            // repo; reusing it would run Claude Code against that repo. If the
            // recorded path is missing or stranded, fall through to the create
            // branch below, which clears any dead residue first.
            let existing_worktree = if crate::engine::git_ops::is_live_worktree_at(&resolved_path)
                .await
            {
                Some(resolved_path.clone())
            } else {
                if resolved_path.exists() {
                    log!(
                            "[AgentSession] worktree path {} exists but is not a live linked worktree (stranded) — recreating instead of running against the enclosing repo",
                            resolved_path.display()
                        );
                }
                None
            };

            let wt_path = if let Some(ref existing) = existing_worktree {
                log!(
                    "[AgentSession] Reusing existing worktree at {} for branch {}",
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
                    match crate::engine::agent_session::external_edits::verify_branch(
                        existing,
                        &branch_name,
                    )
                    .await
                    {
                        Ok(()) => branch_name,
                        Err(mismatch) => {
                            // External repos: a skill may legitimately have
                            // created a feature branch off our tracked one
                            // (e.g. `git checkout -b feature-1234`). Adopt it
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
                                        "[AgentSession] Adopting renegade branch '{}' (was expecting '{}') for thread {}",
                                        new_branch,
                                        branch_name,
                                        thread_id
                                    );
                                    adoption_note = Some(note);
                                    new_branch
                                }
                                None => {
                                    log!(
                                        "[AgentSession] Refusing spawn for thread {}: {}",
                                        thread_id,
                                        mismatch
                                    );
                                    return Err(format!(
                                        "Refusing to spawn the coding agent: {}",
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
                                "[AgentSession] Worktree {} is on branch {} (expected {}) — overriding",
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

            // Create worktree if we're not reusing an existing one. A *stranded*
            // directory may still occupy the target path — present on disk but
            // not a live linked worktree (`existing_worktree` is None above):
            // e.g. a Tier-0 cleanup that raced this resume, or a half-finished
            // prior `git worktree add`. `git worktree add` would then fail with
            // "already exists", and worse, leaving it in place is what let a
            // resumed session run against the enclosing data repo. Clear the
            // dead residue first so the spawn self-heals. This is safe precisely
            // because `is_live_worktree_at` proved there is no live git state to
            // lose — a worktree with real uncommitted work resolves as live and
            // is reused upstream, never reaching here. Confined to the workspace
            // worktrees dir as a blast-radius guard. (The inverse residue — git
            // has the path registered but the directory is GONE — is auto-healed
            // inside `worktree_add` via `git worktree prune`.)
            // Git-truth from the sparse app helper: did it `-b`-create the
            // branch, or adopt an existing one? Feeds `branch_created` below
            // so failure cleanup can never delete a branch this spawn merely
            // reused. `None` = the app create path didn't run.
            let mut app_branch_created: Option<bool> = None;
            if existing_worktree.is_none() {
                if wt_path.exists()
                    && wt_path
                        .starts_with(crate::engine::git_ops::worktrees_dir(self.workspace_path()))
                {
                    crate::engine::git_ops::clear_stranded_worktree_dir(repo_root, &wt_path).await;
                }
                if is_app_spawn {
                    // App spawn — sparse-checkout worktree narrowed to
                    // `data/apps/<id>/`. The standard `worktree_add` would
                    // materialise the whole workspace tree, which is
                    // unnecessary and would expose other apps' folders to CC.
                    let Some(ref app_id) = app_spawn_id else {
                        return Err("Internal: is_app_spawn set without app_spawn_id".into());
                    };
                    match crate::engine::git_ops::create_sparse_app_worktree(
                        repo_root,
                        app_id,
                        &branch_name,
                        &wt_path,
                    )
                    .await
                    {
                        Err(e) => {
                            log!("[AgentSession] Failed to create sparse app worktree: {}", e);
                            return Err(format!(
                                "Failed to create sparse-checkout app worktree: {e}"
                            )
                            .into());
                        }
                        Ok(created) => {
                            app_branch_created = Some(created);
                            log!(
                                "[AgentSession] {} sparse app worktree at {} on {} branch {} (app={})",
                                if created { "Created" } else { "Resumed" },
                                wt_path.display(),
                                if created { "new" } else { "existing" },
                                branch_name,
                                app_id
                            );
                        }
                    }
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
                                    "[AgentSession] Resumed worktree at {} on existing branch {}",
                                    wt_path.display(),
                                    branch_name
                                );
                            } else {
                                log!(
                                    "[AgentSession] Created worktree at {} on branch {}{}",
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
                            log!("[AgentSession] Failed to create worktree: {}", stderr);
                            return Err(format!(
                                "Failed to create git worktree for coding-agent isolation: {}",
                                stderr
                            )
                            .into());
                        }
                        Err(e) => {
                            log!("[AgentSession] Failed to run git worktree: {}", e);
                            return Err(format!(
                                "Failed to create git worktree for coding-agent isolation: {}",
                                e
                            )
                            .into());
                        }
                    }
                } // end non-app spawn branch
            }

            log!(
                "[AgentSession] [TIMING] worktree created: {:?}",
                cc_start.elapsed()
            );

            // Belt-and-suspenders: the agent's cwd MUST exist before we spawn
            // it. The branches above create a missing worktree
            // (`existing_worktree` is None) or reuse a validated live one — but
            // if the directory is STILL absent here (a reclaim or external
            // removal that beat `is_live_worktree_at`, e.g. an auto-recovery
            // resume firing right as the worktree was cleaned out from under a
            // signal-killed session), spawning hands the agent a missing cwd
            // and it dies with a cryptic `Path "<dir>" does not exist`
            // ResponseFailed instead of recovering. Recreate from the branch
            // one more time so the resume self-heals; only fail — with an
            // actionable message — if even that cannot produce the directory.
            if !wt_path.exists() {
                log!(
                    "[AgentSession] worktree {} still missing after setup — recreating from branch {} before spawn",
                    wt_path.display(),
                    branch_name
                );
                crate::engine::git_ops::clear_stranded_worktree_dir(repo_root, &wt_path).await;
                let recreated = if is_app_spawn {
                    match app_spawn_id.as_deref() {
                        Some(app_id) => crate::engine::git_ops::create_sparse_app_worktree(
                            repo_root,
                            app_id,
                            &branch_name,
                            &wt_path,
                        )
                        .await
                        .is_ok(),
                        None => false,
                    }
                } else {
                    matches!(
                        worktree_add(repo_root, &wt_path, &[branch_name.as_str()]).await,
                        Ok(o) if o.status.success()
                    )
                };
                if !recreated || !wt_path.exists() {
                    return Err(format!(
                        "Coding-agent worktree {} is missing and could not be recreated from branch {} — resend the message to retry.",
                        wt_path.display(),
                        branch_name
                    )
                    .into());
                }
                log!(
                    "[AgentSession] recreated missing worktree {} from branch {}",
                    wt_path.display(),
                    branch_name
                );
            }

            // Resumed/reused worktrees may be behind main. Catch them up so they
            // run the latest scripts/, configs, and source rather than stale
            // copies that pre-date recent fixes. New worktrees branched from
            // origin/main are already up to date and skip this.
            // App spawns skip catchup — the workspace git has no `origin` by
            // default, and catchup_with_main would noisily fail.
            if !is_external_repo && !is_app_spawn && (reusing_branch || existing_worktree.is_some())
            {
                if let Err(e) = catchup_with_main(&wt_path).await {
                    log!(
                        "[AgentSession] catchup_with_main failed for {} ({}) -- worktree is behind main; surfacing to the session instead of running stale",
                        wt_path.display(),
                        e
                    );
                    // Don't SILENTLY run a stale worktree. `catchup_with_main`
                    // aborted the merge (it can't auto-resolve conflicts), so
                    // the branch is now behind main with conflicting changes —
                    // the resumed session would otherwise build on pre-merge
                    // scripts/config with no signal. Surface it via the
                    // adoption note, which is injected into the session's next
                    // prompt AND recorded in the timeline, so CC resolves the
                    // merge with main before relying on shared tooling.
                    let note = build_catchup_conflict_note(&e.to_string());
                    adoption_note = Some(match adoption_note.take() {
                        Some(existing) => format!("{existing}\n{note}"),
                        None => note,
                    });
                }
            }

            // Copy node_modules from main repo to worktree so frontend tests work.
            // Much faster than `npm ci` (~1-2s hardlink vs 2-10min install).
            //
            // The repo is an npm WORKSPACE (root package.json `workspaces:
            // [crates/lucidos-app, packages/lucidos-sdk]`), so node_modules is
            // hoisted to the REPO ROOT (`<repo>/node_modules`); there is NO
            // `crates/lucidos-app/node_modules`. Provision the root tree — its
            // workspace-member symlinks (e.g. `lucidos-app -> ../crates/lucidos-app`)
            // are relative and resolve inside the worktree, and Node resolution
            // walks up to `<wt>/node_modules`, so the members need no own copy.
            // (Pointing at crates/lucidos-app here is what silently killed the
            // fast path after the workspace migration: the marker was never
            // found, so every CC worktree fell back to a cold `npm ci` that
            // saturated disk I/O and timed out the concurrent git checkout —
            // see docs/plans/2026-06-27-mobile-webkit-shard-contention.md.)
            // Can't symlink the tree itself: npm install in the worktree would
            // follow it and corrupt/delete the main repo's real node_modules.
            // Apps don't share Lucidos's node_modules — they're standalone
            // static folders. Skip the copy.
            if !is_external_repo && !is_app_spawn {
                let wt_node_modules = wt_path.join("node_modules");
                if !has_install_marker(&wt_node_modules) {
                    let src_node_modules = repo_root.join("node_modules");
                    if has_install_marker(&src_node_modules) {
                        // Fast path (~1-2s vs a 2-10min npm ci): hardlink main's
                        // hoisted ROOT node_modules into the worktree. Workspace
                        // MEMBER trees (un-hoistable deps like vitest) are linked
                        // separately below, independent of this root-marker gate.
                        link_node_modules_tree(&src_node_modules, &wt_node_modules, "node_modules")
                            .await;
                    } else {
                        // npm ci wipes node_modules itself, so any stale Vite
                        // cache here is fine to leave. Run it at the WORKSPACE
                        // ROOT so every member installs correctly — the repo is
                        // an npm workspace, so `npm ci` inside a member is wrong.
                        log!(
                            "[AgentSession] No node_modules in main repo to copy, running npm ci..."
                        );
                        match tokio::process::Command::new("npm")
                            .args(["ci", "--prefer-offline"])
                            .current_dir(&wt_path)
                            .output()
                            .await
                        {
                            Ok(o) if o.status.success() => {
                                log!("[AgentSession] node_modules installed in worktree");
                            }
                            Ok(o) => log!(
                                "[AgentSession] npm ci failed in worktree: {}",
                                String::from_utf8_lossy(&o.stderr).trim()
                            ),
                            Err(e) => {
                                log!("[AgentSession] Failed to run npm ci in worktree: {}", e)
                            }
                        }
                    }
                }

                // Workspace MEMBER node_modules (un-hoistable deps like vitest,
                // which npm nests in crates/lucidos-app/node_modules rather than
                // the hoisted root) are the trees the ROOT link above misses.
                // Provision them here — independent of the root marker — so a
                // worktree created by the old root-only provisioning (root
                // marker present, member tree absent) still gets its member tree
                // on the next spawn/resume. `member_node_modules_links` returns
                // only members present in main AND not already installed here, so
                // this is a no-op on the npm-ci path (which installs members
                // itself) and on a resume that already has them.
                for (member_src, member_dst) in member_node_modules_links(repo_root, &wt_path) {
                    let label = member_dst
                        .strip_prefix(&wt_path)
                        .unwrap_or(member_dst.as_path())
                        .to_string_lossy()
                        .into_owned();
                    link_node_modules_tree(&member_src, &member_dst, &label).await;
                }
            }

            log!(
                "[AgentSession] [TIMING] node_modules setup done: {:?}",
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
                log!("[AgentSession] Failed to write workspace marker: {}", e);
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
            // What did THIS call create? `existing_worktree.is_none()` means
            // the create path ran (worktree_add / sparse app worktree);
            // `!reusing_branch` on top means the branch was born here too
            // (`-b`). The sparse app helper reports the split from git truth
            // (it adopts an existing branch even when the caller expected a
            // fresh one), so prefer its answer when it ran. A reused branch
            // (resume) or reused worktree must survive a failed spawn — see
            // `cleanup_failed_spawn`.
            let worktree_created = existing_worktree.is_none();
            let branch_created = worktree_created && app_branch_created.unwrap_or(!reusing_branch);
            (
                cwd,
                system_prompt,
                branch_name,
                Some(wt_path),
                true,
                worktree_created,
                branch_created,
            )
        };
        if conflict_change.is_none() {
            if let Some(ref wt_path) = worktree_path {
                if let Err(e) = install_coding_agent_diff_hook(repo_root, wt_path).await {
                    log!(
                        "[AgentSession] Failed to install coding-agent diff hook for {}: {}",
                        wt_path.display(),
                        e
                    );
                }
            }
        }
        // Backend-specific teaching rides every prompt flavor (normal,
        // recovery, conflict, override) — a recovered or conflict-resolving
        // Codex session needs the CLI + question teaching just as much as a
        // fresh one.
        let system_prompt = append_backend_rules(system_prompt, coding_agent);
        Ok(SpawnWorktreeContext {
            cwd,
            system_prompt,
            branch_name,
            worktree_path,
            interactive_session,
            adoption_note,
            resume_session_id,
            worktree_created,
            branch_created,
        })
    }
}

/// Engine-injected resume note for when `catchup_with_main` aborted on a
/// resumed/reused worktree because `main` advanced and the auto-merge hit
/// conflicts. Prepended to the session's next prompt (and recorded in the
/// timeline) so the worktree is never *silently* run stale — CC resolves the
/// merge with `main` before relying on shared scripts/config. `err` is the
/// underlying `catchup_with_main` error for context.
fn build_catchup_conflict_note(err: &str) -> String {
    format!(
        "[Note from engine: `main` advanced while this thread was idle, and \
         auto-merging it into your worktree's branch hit conflicts ({err}). \
         Your worktree is now BEHIND main — its scripts/, configs, and source \
         may be stale. Before relying on shared tooling, run `git merge main`, \
         resolve the conflicts, and commit. Don't assume the worktree is \
         up to date.]"
    )
}

#[cfg(test)]
mod tests {
    use super::build_catchup_conflict_note;

    #[test]
    fn catchup_conflict_note_surfaces_staleness_and_error() {
        let note = build_catchup_conflict_note("New conflicts from concurrent main changes: x");
        // Must name the situation so CC (and the user, via the timeline) acts.
        assert!(note.contains("main"), "note must mention main");
        assert!(
            note.to_lowercase().contains("conflict"),
            "note must mention the conflict"
        );
        assert!(
            note.contains("git merge main"),
            "note must tell CC how to recover"
        );
        // The underlying error is carried through for context.
        assert!(note.contains("concurrent main changes"));
    }
}
