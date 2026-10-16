use crate::engine::agent_session::node_modules_setup::{
    has_install_marker, member_node_modules_links,
};
use crate::engine::agent_session::prompts::{
    app_worktree_recovery_system_prompt, app_worktree_system_prompt,
    conflict_resolution_system_prompt, external_repo_recovery_system_prompt,
    external_repo_system_prompt, lane_thread_system_prompt, recovery_system_prompt, spawn_prompts,
    wake_decision_section, worktree_system_prompt, SessionContext, SpawnPromptInputs,
};
use crate::engine::agent_session::spawn::{
    resolve_branch_for_resume, BranchResolution, FreshBranch,
};
use crate::engine::claude_code::{WORKTREE_EXCLUDE_PATHS, WORKTREE_WORKSPACE_MARKER};
use crate::engine::git_ops::{
    add_paths_to_worktree_exclude, install_coding_agent_diff_hook, resolve_worktree_base,
    worktree_add, worktree_current_branch, BranchScope,
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

/// Copy this worktree's generated files from the codegen cache. They are build
/// outputs, so a fresh worktree may have none or stale ones. `--cached-only`
/// never compiles: on a miss it leaves the session to render them itself.
/// Best-effort like the `node_modules` link: a failure is logged, not fatal.
pub(crate) async fn provision_generated_files(wt_path: &Path) {
    let script = wt_path.join("scripts/codegen.sh");
    if !script.is_file() {
        return;
    }
    match tokio::process::Command::new("bash")
        .arg(&script)
        .arg("--cached-only")
        .current_dir(wt_path)
        .output()
        .await
    {
        Ok(o) if o.status.success() => log!(
            "[AgentSession] generated files provisioned: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Ok(o) => log!(
            "[AgentSession] codegen --cached-only failed in worktree: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => log!("[AgentSession] Failed to run codegen in worktree: {}", e),
    }
}

/// Give a Lucidos-source worktree the main checkout's `node_modules`, so its
/// frontend tests run. A hardlink takes 1 to 2 s, against minutes for `npm ci`.
/// A main checkout with none installed gets `npm ci` in the worktree instead.
async fn provision_node_modules(repo_root: &Path, wt_path: &Path) {
    let wt_node_modules = wt_path.join("node_modules");
    if !has_install_marker(&wt_node_modules) && !has_install_marker(&repo_root.join("node_modules"))
    {
        // At the workspace root, so every member installs. `npm ci` wipes
        // node_modules itself, so a stale Vite cache here does no harm.
        log!("[AgentSession] No node_modules in main repo to copy, running npm ci...");
        match tokio::process::Command::new("npm")
            .args(["ci", "--prefer-offline"])
            .current_dir(wt_path)
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
            Err(e) => log!("[AgentSession] Failed to run npm ci in worktree: {}", e),
        }
    }
    link_node_modules(repo_root, wt_path).await;
}

/// Hardlink the main checkout's installed `node_modules` into a worktree.
/// Links nothing the main checkout lacks or the worktree already has.
///
/// The repo is an npm workspace, so dependencies hoist to the repo root. The
/// root tree goes first: its member symlinks are relative, and Node resolution
/// walks up to `<wt>/node_modules`. Pointing the root link at a member once
/// sent every worktree to a cold `npm ci` that saturated the disk
/// (docs/plans/2026-06-27-mobile-webkit-shard-contention.md). A member tree
/// holds only what npm could not hoist. It links independently of the root
/// marker, so an older root-only worktree still gets it.
///
/// Never a symlink to the tree itself: an `npm install` in the worktree would
/// follow it and corrupt the main checkout's real `node_modules`.
pub(crate) async fn link_node_modules(repo_root: &Path, wt_path: &Path) {
    let wt_node_modules = wt_path.join("node_modules");
    let src_node_modules = repo_root.join("node_modules");
    if !has_install_marker(&wt_node_modules) && has_install_marker(&src_node_modules) {
        link_node_modules_tree(&src_node_modules, &wt_node_modules, "node_modules").await;
    }
    for (member_src, member_dst) in member_node_modules_links(repo_root, wt_path) {
        let label = member_dst
            .strip_prefix(wt_path)
            .unwrap_or(member_dst.as_path())
            .to_string_lossy()
            .into_owned();
        link_node_modules_tree(&member_src, &member_dst, &label).await;
    }
}

/// Resolved worktree / branch / system-prompt context for a `run_direct_agent`
/// spawn. Produced by [`LucidosEngine::resolve_run_worktree_context`], the
/// "start" lifecycle stage extracted from the driver: it picks recovery vs
/// conflict vs normal mode, creates/reuses the isolated worktree, links
/// node_modules, writes the workspace marker, and builds the system prompt
/// and the session context block (ADR 0491).
pub(super) struct SpawnWorktreeContext {
    pub(super) cwd: PathBuf,
    pub(super) system_prompt: String,
    pub(super) session_context: SessionContext,
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
        lane_duty: Option<&crate::engine::landing_lane::LaneDuty>,
        system_prompt_override: Option<String>,
        app_spawn_id: &Option<String>,
        is_app_spawn: bool,
        is_external_repo: bool,
        external_repo_name: Option<String>,
        // The registered repository's name, for the `repo-<name>` branch scope.
        // `None` for an app spawn (apps aren't in the registry) and for an
        // unregistered Lucidos source checkout.
        repo_name: Option<String>,
        workspace_name: &str,
        repo_root: &std::path::Path,
        repo_id: &Option<String>,
        idle_anchor: &crate::engine::agent_session::resume::IdleAnchor,
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
        // An external repo's base ref, for the session context. Known only
        // when this spawn resolved where a new worktree branches from.
        let mut base_ref: Option<String> = None;
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
                app_worktree_recovery_system_prompt(workspace_name, app_id)
            } else if let Some(ref name) = external_repo_name {
                external_repo_recovery_system_prompt(name)
            } else {
                recovery_system_prompt(workspace_name)
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
        } else if let Some(duty) = lane_duty {
            // Lane mode: the lane thread resolves its batch on the integration
            // branch. The lane owns the worktree and its cleanup.
            duty.ensure_worktree(repo_root, self.workspace_path())
                .await
                .map_err(|e| format!("The lane worktree could not be prepared: {e}"))?;
            let cwd = duty.worktree.path.clone();
            (
                cwd.clone(),
                lane_thread_system_prompt().to_string(),
                duty.worktree.branch.clone(),
                Some(cwd),
                false,
                false,
                false,
            )
        } else if let Some(ref change) = conflict_change {
            // Conflict mode: run in the merge worktree where the merge is in progress
            let merge = change
                .merge_worktree()
                .ok_or("Conflict change has no merge worktree. Was the merge started?")?;
            let cwd = PathBuf::from(&merge.path);
            let system_prompt = conflict_resolution_system_prompt().to_string();
            // The merge worktree + temp branch belong to the in-progress
            // merge, not to this spawn — keep them on failure.
            (
                cwd.clone(),
                system_prompt,
                merge.temp_branch.clone(),
                Some(cwd),
                false,
                false,
                false,
            )
        } else {
            // Normal mode: create an isolated worktree.
            // If resuming an idle session, reuse the previous branch (preserving its changes)
            // instead of creating a fresh branch that starts empty.
            //
            // A fresh branch is named for its thread:
            // `lucidos-<agent>-<app|repo>-<name>-<slug>`. The scope segment is
            // the app id for an app coding-agent thread and the registered
            // repository's name otherwise, so `git branch -a` is greppable by
            // what the thread works on. A resumed branch keeps whatever name it
            // was born with, including the legacy `claude-code/*` shape.
            let thread_name = self
                .event_store
                .thread_display_title(thread_id)
                .await
                .unwrap_or_default();
            let scope = match app_spawn_id {
                Some(ref app_id) => BranchScope::App(app_id.clone()),
                None => BranchScope::Repo(
                    repo_name.unwrap_or_else(|| BranchScope::LUCIDOS_REPO.to_string()),
                ),
            };
            // Kept alive past the resolution: the create below re-derives
            // through it when a concurrent spawn takes the allocated name.
            let fresh = FreshBranch {
                repo_root,
                agent: coding_agent,
                scope,
                thread_name: &thread_name,
                thread_id,
            };
            let BranchResolution {
                branch_name,
                reusing_branch,
                resume_session_id: validated_sid,
            } = resolve_branch_for_resume(&fresh, resume_session_id, resume_branch.as_deref())
                .await;
            resume_session_id = validated_sid;

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
            let mut branch_name = if let Some(ref existing) = existing_worktree {
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
                            // when its history contains our last commit, or
                            // when it provably continues the tracked branch.
                            // Internal threads keep the strict refusal so
                            // Apply has a stable engine-named branch.
                            //
                            // Adoption needs git to have NAMED a different
                            // branch. `Unanswered` means the probe timed out or
                            // could not run, so nothing is known. Sending it
                            // here only re-runs the same `rev-parse` to guess.
                            // The helper refuses a re-read that reports the
                            // branch we already expected, so the cost is a
                            // wasted probe rather than a false adoption.
                            let probe_named_a_branch = matches!(
                                mismatch,
                                crate::engine::agent_session::external_edits::BranchMismatch::OnOtherBranch { .. }
                                    | crate::engine::agent_session::external_edits::BranchMismatch::Detached { .. }
                            );
                            let adopted = if is_external_repo && probe_named_a_branch {
                                crate::engine::agent_session::external_edits::try_adopt_renegade_branch(
                                    repo_root,
                                    existing,
                                    &branch_name,
                                    idle_anchor,
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
            // unrelated in-flight `lucidos-*` branch would otherwise leak
            // that branch's commits into this thread as phantom "pending
            // changes". External repos keep the origin-or-HEAD contract.
            //
            // A successor thread starts at its old thread's last commit
            // instead, so its branch carries all of that thread's work. An
            // unreadable base fails the spawn: branching from `main` would
            // split the work across two branches.
            let carried_base = match crate::engine::successor::carried_base(self.pool(), thread_id)
                .await
            {
                Ok(base) => base,
                Err(e) => {
                    return Err(format!(
                        "Could not read where this successor thread's branch starts: {e}. Resend the message to retry."
                    )
                    .into())
                }
            };
            let worktree_base_branch = match carried_base.clone() {
                Some(base) => Some(base),
                None => resolve_worktree_base(repo_root, is_external_repo).await,
            };

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
                    match crate::engine::git_ops::create_sparse_app_worktree_from(
                        repo_root,
                        app_id,
                        &branch_name,
                        &wt_path,
                        carried_base.as_deref(),
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
                    // Resuming gets one attempt: the branch is this thread's
                    // own, so a failure is never a name race. A fresh branch
                    // lets the create decide the name, retrying past a sibling
                    // spawn that took the allocated one first.
                    let created = if reusing_branch {
                        match worktree_add(repo_root, &wt_path, &[branch_name.as_str()]).await {
                            Ok(o) if o.status.success() => Ok(branch_name.clone()),
                            Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
                            Err(e) => Err(e),
                        }
                    } else {
                        fresh
                            .create_worktree(
                                &wt_path,
                                worktree_base_branch.as_deref(),
                                branch_name.clone(),
                            )
                            .await
                    };
                    match created {
                        Ok(name) => {
                            branch_name = name;
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
                        Err(e) => {
                            log!("[AgentSession] Failed to create worktree: {}", e);
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
            // A stale git registration at that path is pruned by `worktree_add`
            // itself, so there is nothing to clear first.
            if !wt_path.exists() {
                log!(
                    "[AgentSession] worktree {} still missing after setup — recreating from branch {} before spawn",
                    wt_path.display(),
                    branch_name
                );
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

            // A reused worktree keeps exactly the tree its session left. The
            // engine never merges `main` in here: `main` moves only at Apply,
            // and Apply catches the branch up itself because a fast-forward
            // needs `main` as an ancestor. See
            // docs/adr/0241-session-spawn-does-not-merge-main.md.

            // Apps don't share Lucidos's node_modules: they are standalone
            // static folders.
            if !is_external_repo && !is_app_spawn {
                provision_node_modules(repo_root, &wt_path).await;

                // After node_modules: the SDK bundles build with its esbuild.
                provision_generated_files(&wt_path).await;
            }

            log!(
                "[AgentSession] [TIMING] node_modules and generated files done: {:?}",
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
                app_worktree_system_prompt(workspace_name, app_id, &manifest_json)
            } else if let Some(ref name) = external_repo_name {
                base_ref = Some(
                    worktree_base_branch
                        .clone()
                        .unwrap_or_else(|| "origin/main".to_string()),
                );
                external_repo_system_prompt(name)
            } else {
                worktree_system_prompt(workspace_name)
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
            if branch_created && carried_base.is_some() {
                crate::engine::successor::carry_plan_marker(
                    self.pool(),
                    repo_root,
                    thread_id,
                    &branch_name,
                )
                .await;
            }
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
        if conflict_change.is_none() && lane_duty.is_none() {
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
        // Every flavor talks to the same user. Read at spawn, so a change
        // reaches the next session rather than a running one.
        let literacy = crate::core::technical_literacy::coding_agent_section(
            crate::core::technical_literacy::read(&self.pool).await,
        );
        // ADR 0362: on the Tree memory module the session starts from the
        // workspace memory view. Only this appended prompt changes (I15).
        let memory = self.coding_agent_memory().await;
        // The same widgets section the chat agent reads, so a coding agent
        // embeds widgets in its options and replies the same way (ADR 0415).
        let widgets = self.coding_agent_widgets();
        let (system_prompt, session_context) = spawn_prompts(SpawnPromptInputs {
            flavor: system_prompt,
            coding_agent,
            workspace_sections: &[&literacy, &memory, &widgets],
            branch: &branch_name,
            base_ref: base_ref.as_deref(),
            repo_id: repo_id.as_deref(),
            app_id: app_spawn_id.as_deref(),
            // Only a sub-thread reports to a parent, so only it decides a wake.
            wake_section: wake_decision_section(&self.pool, thread_id).await,
        });
        Ok(SpawnWorktreeContext {
            cwd,
            system_prompt,
            session_context,
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

impl LucidosEngine {
    /// The widgets section for a coding agent's appended prompt: the embed
    /// rule and every built-in and reusable widget. A failed read leaves the
    /// section without its list rather than failing the spawn.
    fn coding_agent_widgets(&self) -> String {
        let widgets = self
            .app_manager
            .list_reusable_widgets()
            .unwrap_or_else(|e| {
                log!(
                    "[AgentSession] The reusable widgets could not be read: {}",
                    e
                );
                Vec::new()
            });
        crate::engine::widget_guidance::build_widgets_section(
            &widgets,
            crate::engine::widget_guidance::WidgetAudience::CodingAgent,
        )
    }

    /// The workspace memory view section for a coding agent's appended
    /// prompt, or empty on Classic, or empty while
    /// `workspace_view_bytes_coding_agent` stays at its default of 0 (ADR
    /// 0362's amendment).
    async fn coding_agent_memory(&self) -> String {
        use crate::engine::summary_tree::module::{tree_ready, Surface, ViewBudgets};
        if !tree_ready(&self.pool).await {
            return String::new();
        }
        let budgets = ViewBudgets::resolve(&self.pool, Surface::CodingAgent, None).await;
        if budgets.workspace == 0 {
            return String::new();
        }
        match self
            .view_snapshots
            .workspace_view(&self.pool, budgets.workspace)
            .await
        {
            Ok(view) => crate::engine::summary_tree::view::coding_agent_section(&view),
            Err(e) => {
                log!(
                    "[AgentSession] The workspace memory view could not be read: {}",
                    e
                );
                String::new()
            }
        }
    }
}
