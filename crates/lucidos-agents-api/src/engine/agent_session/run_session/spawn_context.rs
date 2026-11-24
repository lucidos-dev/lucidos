use crate::engine::agent_session::prompts::{
    app_worktree_recovery_system_prompt, app_worktree_system_prompt,
    conflict_resolution_system_prompt, external_repo_recovery_system_prompt,
    external_repo_system_prompt, lane_thread_system_prompt, recovery_system_prompt, spawn_prompts,
    sub_thread_section, worktree_system_prompt, SessionContext, SpawnPromptInputs,
};
use crate::engine::agent_session::spawn::{
    recover_renamed_branch, resolve_branch_for_resume, BranchResolution, FreshBranch,
};
use crate::engine::git_ops::{
    add_paths_to_worktree_exclude, install_coding_agent_diff_hook, resolve_worktree_base,
    worktree_add, worktree_current_branch, BranchScope,
};
use crate::engine::worktree_artifacts::{WORKTREE_EXCLUDE_PATHS, WORKTREE_WORKSPACE_MARKER};
use crate::engine::worktree_provision::{provision_generated_files, provision_node_modules};
use crate::engine::LucidosEngine;
use std::path::PathBuf;
use uuid::Uuid;

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
    /// Tells the agent which branch its worktree is on, when that is not the
    /// tracked one it expects.
    pub(super) branch_note: Option<String>,
    pub(super) resume_session_id: Option<String>,
    /// True when the recorded branch was gone and no rename of it was proven,
    /// which is why this session starts fresh.
    pub(super) branch_gone: bool,
    pub(super) worktree_created: bool,
    pub(super) branch_created: bool,
}

#[easy_ext::ext(SpawnContextExt)]
pub(crate) impl LucidosEngine {
    /// Resolves which worktree + branch this spawn runs in, returning the
    /// [`SpawnWorktreeContext`] whose `*_created` flags the failure-path cleanup
    /// depends on. The parameter list mirrors the caller's already-destructured
    /// spawn request one-to-one — bundling it into a struct here would only move
    /// the same fields, so the arity lint is allowed rather than worked around.
    #[allow(clippy::too_many_arguments)]
    async fn resolve_run_worktree_context(
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
        let mut branch_note: Option<String> = None;
        let mut branch_gone = false;
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
            log_legacy!(
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
                gone,
            } = resolve_branch_for_resume(&fresh, resume_session_id, resume_branch.as_deref())
                .await;
            resume_session_id = validated_sid;
            branch_gone = gone.is_some();

            let caller_worktree = match resume_worktree_path.as_ref() {
                Some(p) if crate::engine::git_ops::is_live_worktree_at(p).await => Some(p.clone()),
                _ => None,
            };
            if let Some(ref caller) = caller_worktree {
                log_legacy!(
                    "[AgentSession] Using caller-supplied worktree path for resume: {}",
                    caller.display()
                );
            } else if let Some(ref skipped) = resume_worktree_path {
                log_legacy!(
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
                crate::engine::session_queries::resolve_worktree_path(
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

            let existing_worktree = if crate::engine::git_ops::is_live_worktree_at(&resolved_path)
                .await
            {
                Some(resolved_path.clone())
            } else {
                if resolved_path.exists() {
                    log_legacy!(
                            "[AgentSession] worktree path {} exists but is not a live linked worktree (stranded) — recreating instead of running against the enclosing repo",
                            resolved_path.display()
                        );
                }
                None
            };

            let wt_path = if let Some(ref existing) = existing_worktree {
                log_legacy!(
                    "[AgentSession] Reusing existing worktree at {} for branch {}",
                    existing.display(),
                    branch_name
                );
                existing.clone()
            } else {
                resolved_path
            };

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
                            // A branch that provably continues the tracked one
                            // (e.g. the agent's own `git checkout -b feature-1234`)
                            // keeps the session. An external repo adopts it, as a
                            // skill's ticket branch. An internal thread resumes on
                            // it but keeps tracking the engine-named branch, so
                            // Apply has a stable one. Anything else is refused.
                            //
                            // The proof needs git to have NAMED a different
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
                            let continuing = if probe_named_a_branch {
                                crate::engine::agent_session::external_edits::branch_continuing_tracked(
                                    repo_root,
                                    existing,
                                    &branch_name,
                                    idle_anchor,
                                )
                                .await
                            } else {
                                None
                            };
                            match continuing {
                                Some(new_branch) if is_external_repo => {
                                    log_legacy!(
                                        "[AgentSession] Adopting renegade branch '{}' (was expecting '{}') for thread {}",
                                        new_branch,
                                        branch_name,
                                        thread_id
                                    );
                                    branch_note = Some(
                                        crate::engine::agent_session::external_edits::build_adoption_note(&new_branch),
                                    );
                                    new_branch
                                }
                                Some(side_branch) => {
                                    log_legacy!(
                                        "[AgentSession] Resuming thread {} on its side branch '{}', still tracking '{}'",
                                        thread_id,
                                        side_branch,
                                        branch_name
                                    );
                                    branch_note = Some(
                                        crate::engine::agent_session::external_edits::side_branch_note(
                                            &side_branch,
                                            &branch_name,
                                        ),
                                    );
                                    branch_name
                                }
                                None => {
                                    log_legacy!(
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
                    // The recorded branch is gone. An external repo's own
                    // workflow may have renamed it with `git branch -m`: a
                    // proven rename keeps the session, as in the mismatch
                    // arm above. A refusal starts fresh, never refuses.
                    let recovered = match gone {
                        Some(gone) if is_external_repo => {
                            recover_renamed_branch(repo_root, existing, gone, idle_anchor).await
                        }
                        _ => None,
                    };
                    if let Some(recovered) = recovered {
                        branch_gone = false;
                        branch_note = Some(recovered.adoption_note);
                        resume_session_id = recovered.session_id;
                        recovered.branch_name
                    } else {
                        match worktree_current_branch(existing).await {
                            Some(a) if a != branch_name => {
                                log_legacy!(
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
                }
            } else {
                branch_name
            };

            let carried_base = match crate::engine::successor::carried_base(self.pool(), thread_id)
                .await
            {
                Ok(base) => base,
                Err(e) => {
                    return Err(format!(
                        "Could not read where this legacy successor thread's branch starts: {e}. Resend the message to retry."
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
                            log_legacy!(
                                "[AgentSession] Failed to create sparse app worktree: {}",
                                e
                            );
                            return Err(format!(
                                "Failed to create sparse-checkout app worktree: {e}"
                            )
                            .into());
                        }
                        Ok(created) => {
                            app_branch_created = Some(created);
                            log_legacy!(
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
                                log_legacy!(
                                    "[AgentSession] Resumed worktree at {} on existing branch {}",
                                    wt_path.display(),
                                    branch_name
                                );
                            } else {
                                log_legacy!(
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
                            log_legacy!("[AgentSession] Failed to create worktree: {}", e);
                            return Err(format!(
                                "Failed to create git worktree for coding-agent isolation: {}",
                                e
                            )
                            .into());
                        }
                    }
                } // end non-app spawn branch
            }

            log_legacy!(
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
                log_legacy!(
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
                log_legacy!(
                    "[AgentSession] recreated missing worktree {} from branch {}",
                    wt_path.display(),
                    branch_name
                );
            }

            // Apps don't share Lucidos's node_modules: they are standalone
            // static folders.
            if !is_external_repo && !is_app_spawn {
                provision_node_modules(repo_root, &wt_path).await;

                provision_generated_files(&wt_path).await;
            }

            log_legacy!(
                "[AgentSession] [TIMING] node_modules and generated files done: {:?}",
                cc_start.elapsed()
            );

            let marker = wt_path.join(WORKTREE_WORKSPACE_MARKER);
            let ws_id = self.workspace_path.to_string_lossy().to_string();
            let marker_content = match (is_external_repo, repo_id.as_ref()) {
                (true, Some(rid)) => format!("{}\n{}", ws_id, rid),
                _ => ws_id,
            };
            if let Err(e) = tokio::fs::write(&marker, &marker_content).await {
                log_legacy!("[AgentSession] Failed to write workspace marker: {}", e);
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
            } else if branch_created && !is_app_spawn {
                if let Err(e) = crate::engine::git_ops::carry_child_planned(
                    self.pool(),
                    repo_root,
                    thread_id,
                    &branch_name,
                )
                .await
                {
                    log_legacy!(
                        "[AgentSession] Could not carry the parent's plan marker: {}",
                        e
                    );
                }
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
                    log_legacy!(
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
        // A resume drops the session context, and Codex reports no lifetime,
        // so neither pays for the read.
        let cache_ttl = match (coding_agent, &resume_session_id) {
            (crate::runtime::CodingAgent::ClaudeCode, None) => {
                crate::engine::cache_ttl::repo_cache_ttl(&self.pool, repo_id.as_deref()).await
            }
            _ => crate::engine::cache_ttl::CacheTtl::Unknown,
        };
        let (system_prompt, session_context) = spawn_prompts(SpawnPromptInputs {
            flavor: system_prompt,
            coding_agent,
            workspace_sections: &[&literacy, &memory, &widgets],
            branch: &branch_name,
            base_ref: base_ref.as_deref(),
            repo_id: repo_id.as_deref(),
            app_id: app_spawn_id.as_deref(),
            sub_thread_section: &sub_thread_section(&self.pool, thread_id).await,
            cache_ttl,
        });
        Ok(SpawnWorktreeContext {
            cwd,
            system_prompt,
            session_context,
            branch_name,
            worktree_path,
            interactive_session,
            branch_note,
            resume_session_id,
            branch_gone,
            worktree_created,
            branch_created,
        })
    }
}

#[easy_ext::ext(AgentPromptDataExt)]
pub(crate) impl LucidosEngine {
    /// The widgets section for a coding agent's appended prompt: the embed
    /// rule and every built-in and reusable widget. A failed read leaves the
    /// section without its list rather than failing the spawn.
    fn coding_agent_widgets(&self) -> String {
        let widgets = self
            .app_manager
            .list_reusable_widgets()
            .unwrap_or_else(|e| {
                log_legacy!(
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
                log_legacy!(
                    "[AgentSession] The workspace memory view could not be read: {}",
                    e
                );
                String::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::agent_session::resume::IdleAnchor;
    use crate::engine::git_ops::git_cmd;
    use crate::test_support::{boot_mock_engine, in_engine_child, setup_test_db, teardown_test_db};
    use std::path::Path;

    fn child_name(test: &str) -> String {
        format!("{}::{test}", module_path!().split_once("::").unwrap().1)
    }

    async fn git(dir: &Path, args: &[&str]) -> String {
        let out = git_cmd(args, dir).await.unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    async fn commit_file(repo: &Path, name: &str, content: &str) {
        let path = repo.join(name);
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&path, content).await.unwrap();
        git(repo, &["add", "."]).await;
        git(repo, &["commit", "-m", name]).await;
    }

    async fn repo_with_main() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        tokio::fs::create_dir_all(&repo).await.unwrap();
        git(&repo, &["init", "-b", "main"]).await;
        git(&repo, &["config", "user.email", "t@example.com"]).await;
        git(&repo, &["config", "user.name", "t"]).await;
        commit_file(&repo, "init.txt", "initial").await;
        (tmp, repo)
    }

    #[derive(Default)]
    struct Spawn<'a> {
        external: bool,
        repo_id: Option<&'a str>,
        app: Option<&'a str>,
        resume_worktree_path: Option<PathBuf>,
        resume_branch: Option<&'a str>,
    }

    async fn spawn(
        engine: &LucidosEngine,
        repo: &Path,
        thread_id: Uuid,
        s: Spawn<'_>,
    ) -> Result<SpawnWorktreeContext, Box<dyn std::error::Error + Send + Sync>> {
        let app = s.app.map(str::to_string);
        let repo_id = s.repo_id.map(str::to_string);
        engine
            .resolve_run_worktree_context(
                None,
                &None,
                None,
                None,
                &app,
                app.is_some(),
                s.external,
                s.external.then(|| "example".to_string()),
                Some("example".to_string()),
                "ws",
                repo,
                &repo_id,
                &IdleAnchor::Absent,
                s.resume_worktree_path,
                s.resume_branch.map(str::to_string),
                s.resume_branch.map(|_| "sid".to_string()),
                thread_id,
                std::time::Instant::now(),
                crate::runtime::CodingAgent::ClaudeCode,
            )
            .await
    }

    fn same_dir(a: &Path, b: &Path) -> bool {
        std::fs::canonicalize(a).unwrap() == std::fs::canonicalize(b).unwrap()
    }

    /// A fresh Lucidos-source spawn on a shared checkout parked on another
    /// thread's branch.
    #[tokio::test]
    async fn a_fresh_lucidos_source_spawn_owns_a_main_based_branch_with_provisioned_files() {
        if !in_engine_child(&child_name(
            "a_fresh_lucidos_source_spawn_owns_a_main_based_branch_with_provisioned_files",
        ))
        .await
        {
            return;
        }
        let (_pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        let (_tmp, repo) = repo_with_main().await;
        let script = "[ -e seen ] || { if [ -f node_modules/.package-lock.json ]; then echo yes > seen; else echo no > seen; fi; }\n";
        commit_file(&repo, "scripts/codegen.sh", script).await;
        tokio::fs::create_dir_all(repo.join("node_modules"))
            .await
            .unwrap();
        tokio::fs::write(repo.join("node_modules/.package-lock.json"), "{}")
            .await
            .unwrap();
        let main_sha = git(&repo, &["rev-parse", "main"]).await;
        git(&repo, &["checkout", "-b", "lucidos-parked-other-thread"]).await;
        commit_file(&repo, "phantom.txt", "another thread's work").await;

        let ctx = spawn(
            &engine,
            &repo,
            Uuid::new_v4(),
            Spawn {
                repo_id: Some("rid"),
                ..Spawn::default()
            },
        )
        .await
        .expect("a fresh spawn");

        let wt = ctx.worktree_path.clone().unwrap();
        assert!(ctx.worktree_created && ctx.branch_created);
        assert!(
            ctx.branch_name
                .starts_with("lucidos-claude-code-repo-example-"),
            "{}",
            ctx.branch_name
        );
        assert_eq!(git(&wt, &["rev-parse", "HEAD"]).await, main_sha);
        assert!(!wt.join("phantom.txt").exists());
        assert_eq!(
            tokio::fs::read_to_string(wt.join("seen"))
                .await
                .unwrap()
                .trim(),
            "yes",
            "codegen ran before node_modules was provisioned"
        );
        let marker = tokio::fs::read_to_string(wt.join(WORKTREE_WORKSPACE_MARKER))
            .await
            .unwrap();
        assert_eq!(
            marker.lines().count(),
            1,
            "an internal marker has no second line"
        );
        engine.pool().close().await;
        teardown_test_db(&db).await;
    }

    /// An external repo's marker carries the repo id on a second line.
    #[tokio::test]
    async fn an_external_repo_spawn_writes_its_repo_id_on_the_marker_second_line() {
        if !in_engine_child(&child_name(
            "an_external_repo_spawn_writes_its_repo_id_on_the_marker_second_line",
        ))
        .await
        {
            return;
        }
        let (_pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        let (_tmp, repo) = repo_with_main().await;

        let ctx = spawn(
            &engine,
            &repo,
            Uuid::new_v4(),
            Spawn {
                external: true,
                repo_id: Some("rid"),
                ..Spawn::default()
            },
        )
        .await
        .expect("a fresh external spawn");

        let marker =
            tokio::fs::read_to_string(ctx.worktree_path.unwrap().join(WORKTREE_WORKSPACE_MARKER))
                .await
                .unwrap();
        assert_eq!(marker.lines().nth(1), Some("rid"));
        engine.pool().close().await;
        teardown_test_db(&db).await;
    }

    /// A thread's live worktree on a legacy-named branch, with `main` moved on.
    async fn resumable_worktree(repo: &Path) -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let wt = tmp.path().join("legacy-wt");
        git(
            repo,
            &[
                "worktree",
                "add",
                "-b",
                "claude-code/legacy",
                wt.to_str().unwrap(),
            ],
        )
        .await;
        commit_file(repo, "main-moved-on.txt", "later").await;
        (tmp, wt)
    }

    #[tokio::test]
    async fn a_resumed_spawn_keeps_its_worktree_branch_name_and_tree_and_owns_neither() {
        if !in_engine_child(&child_name(
            "a_resumed_spawn_keeps_its_worktree_branch_name_and_tree_and_owns_neither",
        ))
        .await
        {
            return;
        }
        let (_pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        let (_tmp, repo) = repo_with_main().await;
        let (_wt_tmp, wt) = resumable_worktree(&repo).await;
        let tip = git(&repo, &["rev-parse", "claude-code/legacy"]).await;

        let ctx = spawn(
            &engine,
            &repo,
            Uuid::new_v4(),
            Spawn {
                external: true,
                resume_branch: Some("claude-code/legacy"),
                ..Spawn::default()
            },
        )
        .await
        .expect("a resume");

        assert_eq!(ctx.branch_name, "claude-code/legacy");
        assert!(same_dir(&ctx.worktree_path.unwrap(), &wt));
        assert!(!ctx.worktree_created, "the worktree pre-existed");
        assert!(!ctx.branch_created, "the branch pre-existed");
        assert_eq!(ctx.resume_session_id.as_deref(), Some("sid"));
        assert_eq!(
            git(&repo, &["rev-parse", "claude-code/legacy"]).await,
            tip,
            "the spawn must not merge main into the reused branch"
        );
        engine.pool().close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn a_resume_whose_worktree_sits_on_an_unrelated_branch_is_refused() {
        if !in_engine_child(&child_name(
            "a_resume_whose_worktree_sits_on_an_unrelated_branch_is_refused",
        ))
        .await
        {
            return;
        }
        let (_pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        let (_tmp, repo) = repo_with_main().await;
        let (_wt_tmp, wt) = resumable_worktree(&repo).await;
        git(&wt, &["checkout", "--orphan", "stranger"]).await;
        git(&wt, &["commit", "-m", "unrelated root"]).await;

        let outcome = spawn(
            &engine,
            &repo,
            Uuid::new_v4(),
            Spawn {
                external: true,
                resume_branch: Some("claude-code/legacy"),
                resume_worktree_path: Some(wt.clone()),
                ..Spawn::default()
            },
        )
        .await;

        match outcome {
            Err(e) => assert!(e.to_string().contains("Refusing to spawn"), "{e}"),
            Ok(_) => panic!("a worktree on an unrelated branch must not be resumed"),
        }
        engine.pool().close().await;
        teardown_test_db(&db).await;
    }

    /// A recorded resume path holding only a bare `.git` directory, inside the
    /// repo, so git resolves it to the enclosing repo.
    #[tokio::test]
    async fn a_stranded_caller_worktree_path_is_not_reused() {
        if !in_engine_child(&child_name("a_stranded_caller_worktree_path_is_not_reused")).await {
            return;
        }
        let (_pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        let (_tmp, repo) = repo_with_main().await;
        let stranded = repo.join("stranded");
        tokio::fs::create_dir_all(stranded.join(".git"))
            .await
            .unwrap();

        let ctx = spawn(
            &engine,
            &repo,
            Uuid::new_v4(),
            Spawn {
                external: true,
                resume_worktree_path: Some(stranded.clone()),
                ..Spawn::default()
            },
        )
        .await
        .expect("a fresh worktree replaces the stranded path");

        assert_ne!(ctx.worktree_path.unwrap(), stranded);
        assert!(ctx.worktree_created);
        engine.pool().close().await;
        teardown_test_db(&db).await;
    }

    /// The thread's own worktree path holds a bare `.git` directory, as a
    /// cleanup that raced the resume leaves it.
    #[tokio::test]
    async fn a_stranded_thread_worktree_is_recreated_not_reused() {
        if !in_engine_child(&child_name(
            "a_stranded_thread_worktree_is_recreated_not_reused",
        ))
        .await
        {
            return;
        }
        let (_pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        let (_tmp, repo) = repo_with_main().await;
        let thread_id = Uuid::new_v4();
        let path = crate::engine::session_queries::resolve_worktree_path(
            engine.pool(),
            thread_id,
            engine.workspace_path(),
            &repo,
            None,
        )
        .await;
        tokio::fs::create_dir_all(path.join(".git")).await.unwrap();

        let ctx = spawn(
            &engine,
            &repo,
            thread_id,
            Spawn {
                external: true,
                ..Spawn::default()
            },
        )
        .await
        .expect("the stranded residue is cleared and the worktree created");

        assert_eq!(ctx.worktree_path.as_deref(), Some(path.as_path()));
        assert!(ctx.worktree_created);
        assert!(
            path.join(".git").is_file(),
            "a linked worktree has a .git file"
        );
        engine.pool().close().await;
        teardown_test_db(&db).await;
    }

    #[tokio::test]
    async fn an_app_spawn_checks_out_only_its_own_app_folder() {
        if !in_engine_child(&child_name(
            "an_app_spawn_checks_out_only_its_own_app_folder",
        ))
        .await
        {
            return;
        }
        let (_pool, db) = setup_test_db().await;
        let (engine, _workspace) = boot_mock_engine(&db).await;
        let (_tmp, repo) = repo_with_main().await;
        commit_file(&repo, "data/apps/a1/manifest.json", "{}").await;
        commit_file(&repo, "data/apps/a2/index.html", "other app").await;

        let ctx = spawn(
            &engine,
            &repo,
            Uuid::new_v4(),
            Spawn {
                app: Some("a1"),
                ..Spawn::default()
            },
        )
        .await
        .expect("an app spawn");

        let wt = ctx.worktree_path.unwrap();
        assert_eq!(ctx.cwd, wt.join("data/apps/a1"));
        assert!(wt.join("data/apps/a1/manifest.json").exists());
        assert!(
            !wt.join("data/apps/a2").exists(),
            "another app's folder must not be exposed to the agent"
        );
        engine.pool().close().await;
        teardown_test_db(&db).await;
    }
}
