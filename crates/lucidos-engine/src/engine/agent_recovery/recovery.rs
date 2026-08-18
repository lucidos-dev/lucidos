//! The recovery `impl LucidosEngine` block: stale-waiting-session
//! settlement and orphaned-worktree recovery.

use super::super::agent_session::change_description_fallback;
use super::super::change_ops::branch_is_hardened;
use super::super::claude_code::{WORKTREE_EXCLUDE_PATHS, WORKTREE_WORKSPACE_MARKER};
use super::super::git_ops::{
    add_paths_to_worktree_exclude, commit_worktree_or_err, default_local_branch,
    describe_branch_changes, files_require_restart, find_worktree_for_branch, git_cmd,
    is_external_repo_path, main_worktree, proposal_files_for_branch, worktree_add, worktrees_dir,
};
use super::super::thread_events::{EngineReason, EventChannel, MessageOrigin};
use super::super::LucidosEngine;
use super::*;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

impl LucidosEngine {
    /// Gather branch metadata and propose a Change record. Callers must pass a
    /// non-empty `changed_files` list, so this never creates a phantom `changes`
    /// row with `file_count=0`.
    ///
    /// `origin` reaches the emitted `ChangeProposed`, so the route popover can
    /// name which engine path proposed the change.
    async fn propose_branch_changes(
        &self,
        thread_id: Uuid,
        branch_name: &str,
        repo_root: &Path,
        changed_files: &[String],
        origin: Option<MessageOrigin>,
    ) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
        debug_assert!(
            !changed_files.is_empty(),
            "propose_branch_changes called with empty file list — caller must filter"
        );
        let requires_restart = files_require_restart(changed_files);
        let fallback = change_description_fallback(self.pool(), thread_id, branch_name).await;
        let base = default_local_branch(repo_root).await;
        let log_range = format!("{}..{}", base, branch_name);
        let description = describe_branch_changes(repo_root, &log_range, &fallback, None).await;
        let repo_root_str = repo_root.to_string_lossy();
        // The marker is keyed by repo root plus branch name, so it survives the
        // cleanup worker removing the worktree. Without this lookup,
        // `propose_change` downgrades hardened to false and Apply re-runs
        // `/harden` on already-hardened work.
        let hardened =
            branch_is_hardened(self.pool(), self.changes(), repo_root, branch_name).await;
        let incomplete = !last_turn_ended_cleanly(self.pool(), thread_id).await;
        self.propose_change(crate::engine::change_ops::ProposeChangeInput {
            thread_id,
            branch_name,
            repo_root: &repo_root_str,
            description: &description,
            files: changed_files,
            requires_restart,
            channel: EventChannel::ClaudeCode,
            hardened,
            origin,
            incomplete,
        })
        .await
    }

    /// End a stale waiting Claude Code session (no live process) after an engine
    /// restart.
    ///
    /// `actor` is who initiated it. HTTP entry points plumb the user's device
    /// through, so any resulting change event stamps the real actor rather than
    /// the "Lucidos Engine" chip. Engine-internal recovery callers pass `None`.
    pub(crate) async fn end_stale_waiting_session(
        self: &Arc<Self>,
        thread_id: Uuid,
        discard: bool,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let branch: Option<String> = sqlx::query_scalar(
            "SELECT payload->>'branch' FROM events \
             WHERE event_type = 'SessionStarted' AND thread_id = $1 \
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(thread_id)
        .fetch_optional(self.pool())
        .await?;

        let branch_name = match branch {
            Some(b) if !b.is_empty() => b,
            None => {
                // This thread never had a Claude Code session. Erroring beats
                // emitting `SessionEnded`, which would pollute regular chat
                // threads with coding-agent events.
                return Err("No Claude Code session found for this thread".into());
            }
            Some(_) => {
                // A session with no branch. Idle it at the turn boundary rather
                // than terminating it: the thread stays alive, so a follow-up
                // can re-spawn the agent through `--resume`.
                let coding_agent = self.thread_coding_agent(thread_id).await;
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
                                has_changes: false,
                                is_external_repo: false,
                                requires_restart: false,
                                cc_session_id: None,
                                coding_agent,
                                reason: None,
                                worktree_path: None,
                                worktree_head_sha: None,
                                bg_bash_pending: false,
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        },
                        "[Recovery] CodingAgentIdled (no branch)",
                    )
                    .await;
                return Ok(());
            }
        };

        log!(
            "[Recovery] Ending stale waiting session for thread {} (branch {})",
            thread_id,
            branch_name
        );

        let repo_root = main_worktree().await;

        let wt_path = find_worktree_for_branch(&repo_root, &branch_name).await;

        // The removal is `--force`, so it discards whatever is still
        // uncommitted. That is safe only once the rescue commit has LANDED,
        // which is why this uses `commit_worktree_or_err` rather than the silent
        // `auto_commit_worktree`: `git add` and `git commit` fail for ordinary
        // reasons in a real repo. On a failed rescue we keep the worktree. The
        // branch is still proposed below, and the cleanup worker owns
        // reclamation (ADR 0035).
        if let Some(ref wt) = wt_path {
            let mut rescued = true;
            if !discard {
                match commit_worktree_or_err(wt, "Coding agent changes (auto-committed)").await {
                    Ok(_) => {}
                    Err(e) => {
                        rescued = false;
                        log!(
                            "[Recovery] Could not auto-commit {} before ending the stale session: {}. Keeping the worktree so its uncommitted work is not force-removed",
                            wt.display(),
                            e
                        );
                    }
                }
            }
            if rescued {
                match wt.to_str() {
                    Some(wt_str) => {
                        match git_cmd(&["worktree", "remove", "--force", wt_str], &repo_root).await
                        {
                            Ok(o) if o.status.success() => {}
                            Ok(o) => log!(
                                "[Recovery] git worktree remove failed for {}: {}",
                                wt.display(),
                                String::from_utf8_lossy(&o.stderr).trim()
                            ),
                            Err(e) => log!(
                                "[Recovery] git worktree remove errored for {}: {}",
                                wt.display(),
                                e
                            ),
                        }
                    }
                    None => log!(
                        "[Recovery] skipped worktree remove (non-UTF8 path): {}",
                        wt.display()
                    ),
                }
            }
        }

        // `Some(files)` only when the branch has commits AND a non-empty net
        // diff, so commits that cancel out read as a no-op branch.
        let proposal_files = proposal_files_for_branch(&repo_root, &branch_name).await;

        let mut proposed_change = false;
        if discard {
            log!(
                "[Recovery] Discarding stale session changes (branch {})",
                branch_name
            );
            self.discard_pending_for_thread(thread_id, actor.clone())
                .await;
            if let Err(e) = git_cmd(&["branch", "-D", &branch_name], &repo_root).await {
                log!("[Recovery] Failed to delete branch {}: {}", branch_name, e);
            }
        } else if let Some(changed_files) = proposal_files {
            let is_external = match self.is_external_repo_thread(thread_id).await {
                Ok(v) => v,
                Err(e) => {
                    log!(
                        "[Recovery] Failed to check external repo status for thread {}: {}",
                        thread_id,
                        e
                    );
                    false
                }
            };

            if is_external {
                log!(
                    "[Recovery] External repo branch {} — keeping branch, no change proposed",
                    branch_name
                );
            } else {
                match self
                    .propose_branch_changes(
                        thread_id,
                        &branch_name,
                        &repo_root,
                        &changed_files,
                        Some(MessageOrigin::engine(EngineReason::StaleSession)),
                    )
                    .await
                {
                    Ok(_) => {
                        proposed_change = true;
                        log!(
                            "[Recovery] Proposed change from stale session (branch {})",
                            branch_name
                        );
                    }
                    Err(e) => log!(
                        "[Recovery] Failed to propose change from stale session: {}",
                        e
                    ),
                }
            }
        } else {
            // Keep the branch. An unexplained `None` (a transient git failure, a
            // projection gap) would otherwise `git branch -D` work the user
            // still wants, and that is unrecoverable. An orphaned empty branch
            // is just a ref, and the cleanup sweep collects it once it is fully
            // merged.
            log!(
                "[Recovery] recovery: branch {} has no proposable diff — keeping branch (discard=false)",
                branch_name
            );
        }

        // `SessionEnded` is terminal-only, so mark the orphaned turn as ended
        // with `CodingAgentIdled` instead. The change events emitted above
        // already drive the panel state.
        let coding_agent = self.thread_coding_agent(thread_id).await;
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
                        has_changes: proposed_change,
                        is_external_repo: false,
                        requires_restart: false,
                        cc_session_id: None,
                        coding_agent,
                        reason: None,
                        // The removal above is conditional, so this path may or
                        // may not still exist. Recording it either way misleads
                        // the resolver, so leave it None and let the next spawn
                        // look the worktree up itself.
                        worktree_path: None,
                        // No worktree, so no SHA. External-edit detection stays
                        // off until a real turn populates the field.
                        worktree_head_sha: None,
                        bg_bash_pending: false,
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                "[Recovery] CodingAgentIdled",
            )
            .await;

        // No auto-apply tail: a pending change is the user's to resolve from
        // Review. Apply Now chains its own apply call once the proposal lands
        // over SSE.

        self.broadcast_changes_updated().await;

        Ok(())
    }

    /// Boot floor: withdraw every *Switch to new version* resume promise this
    /// boot did not keep. Thin wrapper over
    /// [`settle_unresumed_switch_threads`], which documents the whole contract;
    /// `main.rs` calls this after both resume drains with the union of the ids
    /// they actuated.
    pub async fn settle_unresumed_switch_threads(&self, resumed: &std::collections::HashSet<Uuid>) {
        settle_unresumed_switch_threads(self.pool(), &self.event_bus, resumed).await;
    }

    /// Detect orphaned coding-agent worktrees from a previous engine run and
    /// start new coding-agent sessions on them instead of proposing pending changes.
    pub async fn recover_orphaned_worktrees(self: &Arc<Self>) -> Vec<uuid::Uuid> {
        #[derive(sqlx::FromRow)]
        struct BranchThread {
            thread_id: Option<Uuid>,
            branch: Option<String>,
        }

        #[derive(sqlx::FromRow)]
        struct BranchStatus {
            branch: Option<String>,
            status: Option<String>,
        }

        let t0 = std::time::Instant::now();
        let lucidos_repo_root = main_worktree().await;
        let ws_id = self.workspace_path.to_string_lossy().to_string();
        // Trailing separator avoids false-positive prefix matches against sibling
        // dirs (e.g. `worktrees-old/`) or workspaces whose paths share a prefix.
        let mut ws_worktrees_prefix = worktrees_dir(self.workspace_path())
            .to_string_lossy()
            .to_string();
        if !ws_worktrees_prefix.ends_with(std::path::MAIN_SEPARATOR) {
            ws_worktrees_prefix.push(std::path::MAIN_SEPARATOR);
        }

        // Deduplicate by canonical path, so a Lucidos repo that is also
        // registered as an external repo is not scanned twice.
        let lucidos_canonical = lucidos_repo_root
            .canonicalize()
            .unwrap_or_else(|_| lucidos_repo_root.clone());
        let mut seen_repo_paths: std::collections::HashSet<PathBuf> =
            std::collections::HashSet::from([lucidos_canonical]);
        let mut repos_to_scan: Vec<(PathBuf, Option<String>)> =
            vec![(lucidos_repo_root.clone(), None)];
        match crate::core::repositories::RepositoryStore::list(self.pool()).await {
            Ok(repos) => {
                for repo in repos {
                    let path = PathBuf::from(&repo.path);
                    if !path.exists() {
                        log!(
                            "[Recovery] Skipping external repo '{}' — path does not exist: {}",
                            repo.name,
                            repo.path
                        );
                        continue;
                    }
                    let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                    if !seen_repo_paths.insert(canonical) {
                        log!("[Recovery] Skipping external repo '{}' — same path as already-scanned repo", repo.name);
                        continue;
                    }
                    repos_to_scan.push((path, Some(repo.id.to_string())));
                }
            }
            Err(e) => {
                log!("[Recovery] Failed to list external repos: {}", e);
            }
        }

        // (worktree_path, branch_name, repo_id, repo_root)
        let mut to_recover: Vec<(PathBuf, String, Option<String>, PathBuf)> = Vec::new();

        for (repo_root, _scan_repo_id) in &repos_to_scan {
            let wt_output = match git_cmd(&["worktree", "list", "--porcelain"], repo_root).await {
                Ok(o) => o,
                Err(e) => {
                    log!(
                        "[Recovery] Failed to list worktrees for {}: {}",
                        repo_root.display(),
                        e
                    );
                    continue;
                }
            };

            let wt_text = String::from_utf8_lossy(&wt_output.stdout);
            let mut worktree_path: Option<String> = None;
            let mut branch: Option<String> = None;

            for line in wt_text.lines().chain(std::iter::once("")) {
                if let Some(path) = line.strip_prefix("worktree ") {
                    worktree_path = Some(path.to_string());
                } else if let Some(b) = line.strip_prefix("branch refs/heads/") {
                    branch = Some(b.to_string());
                } else if line.is_empty() {
                    if let (Some(ref wt), Some(ref br)) = (&worktree_path, &branch) {
                        if crate::engine::git_ops::is_coding_agent_branch(br) {
                            // A worktree outside this workspace's worktrees dir
                            // cannot be ours, so skip the marker read. That read
                            // dominates the scan in multi-workspace setups.
                            if wt.starts_with(&ws_worktrees_prefix) {
                                let marker_path = PathBuf::from(wt).join(WORKTREE_WORKSPACE_MARKER);
                                match tokio::fs::read_to_string(&marker_path).await {
                                    Ok(content) => {
                                        let mut lines = content.trim().lines();
                                        let owner = lines.next().unwrap_or("");
                                        let marker_repo_id = lines.next().map(|s| s.to_string());
                                        if owner == ws_id {
                                            to_recover.push((
                                                PathBuf::from(wt),
                                                br.clone(),
                                                marker_repo_id,
                                                repo_root.clone(),
                                            ));
                                        } else {
                                            log!("[Recovery] Skipping worktree {} — owned by workspace {}", wt, owner);
                                        }
                                    }
                                    Err(_) => {
                                        log!(
                                            "[Recovery] Skipping worktree {} — no workspace marker",
                                            wt
                                        );
                                    }
                                }
                            }
                        }
                    }
                    worktree_path = None;
                    branch = None;
                }
            }
        }

        let t_worktree_scan = t0.elapsed();

        // CodingAgentIdled is suppressed during engine shutdown, so a session killed
        // mid-work has no trailing terminal event. Such a session won't be classified
        // as 'idle' by branch_classification and must instead be resumed.
        let pool = self.pool();
        let proj = self.changes();
        // Best-effort: a DB error degrades to "nothing pending" rather than
        // blocking boot, and is logged so the partial sweep is visible.
        let pending_changes_list = proj.list_pending().await.unwrap_or_else(|e| {
            log!(
                "[Recovery] list_pending: {} — recovery proceeds without pending change context",
                e
            );
            Vec::new()
        });
        let change_branches_list: Vec<(String, Uuid)> = pending_changes_list
            .iter()
            .filter_map(|c| c.thread_id.map(|tid| (c.branch_name.clone(), tid)))
            .collect();

        let (
            already_recovered_result,
            branch_classification_result,
            branch_threads_result,
        ) = tokio::join!(
            sqlx::query_scalar::<_, String>(
                "SELECT sub.branch FROM ( \
                    SELECT DISTINCT ON (payload->>'branch') \
                        payload->>'branch' AS branch, thread_id, sequence \
                    FROM events \
                    WHERE event_type IN ('ContinuationStarted', 'OrphanRecoveryStarted') \
                      AND payload->>'branch' IS NOT NULL \
                    ORDER BY payload->>'branch', sequence DESC \
                 ) sub \
                 WHERE EXISTS ( \
                     SELECT 1 FROM events e2 \
                     WHERE e2.thread_id = sub.thread_id \
                       AND e2.sequence > sub.sequence \
                       AND e2.event_type IN ('CodingAgentIdled', 'ResponseGenerated', 'SessionEnded') \
                 )"
            ).fetch_all(pool),
            sqlx::query_as::<_, BranchStatus>(&BRANCH_CLASSIFICATION_SQL).fetch_all(pool),
            sqlx::query_as::<_, BranchThread>(
                "SELECT DISTINCT ON (payload->>'branch') thread_id, payload->>'branch' AS branch FROM events \
                 WHERE event_type = 'SessionStarted' AND payload->>'branch' IS NOT NULL \
                   AND payload->>'branch' != '' AND thread_id IS NOT NULL \
                 ORDER BY payload->>'branch', sequence DESC"
            ).fetch_all(pool),
        );

        // An empty set from a failed query silently misclassifies every branch,
        // so log the error even though recovery proceeds with degraded data.
        fn unwrap_logged<T: Default, E: std::fmt::Display>(label: &str, r: Result<T, E>) -> T {
            r.unwrap_or_else(|e| {
                log!("[Recovery] {} query failed: {}", label, e);
                T::default()
            })
        }

        let pending_by_branch: std::collections::HashMap<String, crate::core::changes::Change> =
            pending_changes_list
                .into_iter()
                .map(|c| (c.branch_name.clone(), c))
                .collect();

        let already_recovered: std::collections::HashSet<String> =
            unwrap_logged("already_recovered", already_recovered_result)
                .into_iter()
                .collect();

        let mut idle_branches: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut actively_running_branches: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for row in unwrap_logged("branch_classification", branch_classification_result) {
            let branch = match row.branch {
                Some(b) if !b.is_empty() => b,
                _ => continue,
            };
            match row.status.as_deref() {
                Some("idle") => {
                    idle_branches.insert(branch);
                }
                Some("running") => {
                    actively_running_branches.insert(branch);
                }
                _ => {}
            }
        }

        let mut branch_to_thread: std::collections::HashMap<String, Uuid> =
            unwrap_logged("branch_threads", branch_threads_result)
                .into_iter()
                .filter_map(|r| match (r.thread_id, r.branch) {
                    (Some(tid), Some(br)) if !br.is_empty() => Some((br, tid)),
                    _ => None,
                })
                .collect();
        for (br, tid) in change_branches_list {
            branch_to_thread.entry(br).or_insert(tid);
        }

        log!("[Recovery] Worktree scan: {}ms, DB classification: {}ms (worktrees={}, idle={}, running={})",
            t_worktree_scan.as_millis(),
            (t0.elapsed() - t_worktree_scan).as_millis(),
            to_recover.len(),
            idle_branches.len(),
            actively_running_branches.len());

        // DB-based discovery for lost worktrees. The scan above sees only
        // branches whose worktree directory still exists, so a running session
        // whose directory was cleaned up is invisible to it. Find those in the
        // DB and create fresh worktrees, so they enter the normal pipeline.
        // Owned set, because the loop below pushes to `to_recover`.
        let discovered_branches: std::collections::HashSet<String> =
            to_recover.iter().map(|(_, br, _, _)| br.clone()).collect();

        // Unstick a thread whose session cannot be recovered (worktree gone,
        // branch missing, git error). The thread stays alive, and the
        // `engine_restart_interrupt` reason tells the UI to offer Continue
        // rather than treat the idle as natural.
        let end_stuck_session = |engine: &Arc<Self>, thread_id: Uuid| {
            let bus = engine.event_bus.clone();
            let engine = engine.clone();
            async move {
                // Same preserve rule as the loop below. The `CodingAgentIdled`
                // this closure emits is park-ending, so idling here would expire
                // a live question card. Answering still resumes, even with the
                // worktree unrecoverable.
                if thread_has_unanswered_question(engine.pool(), thread_id).await {
                    log!(
                        "[Recovery] Preserving stuck thread {} — parked on an unanswered question (no idle emitted)",
                        thread_id
                    );
                    return;
                }
                let coding_agent = engine.thread_coding_agent(thread_id).await;
                bus.emit_or_log(
                    crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
                            has_changes: false,
                            is_external_repo: false,
                            requires_restart: false,
                            cc_session_id: None,
                            coding_agent,
                            reason: Some(ENGINE_RESTART_INTERRUPT_REASON.to_string()),
                            // No worktree to record: this path fires when
                            // recovery cannot locate one at all. The next spawn
                            // resolves a path itself.
                            worktree_path: None,
                            // No worktree, so no SHA to snapshot.
                            worktree_head_sha: None,
                            bg_bash_pending: false,
                        },
                        meta: crate::engine::thread_events::EventMeta::NONE,
                    },
                    &format!("[Recovery] CodingAgentIdled for stuck thread {}", thread_id),
                )
                .await;
            }
        };

        for branch in &actively_running_branches {
            if discovered_branches.contains(branch) {
                continue;
            }
            // `idle_branches` can still hold this branch. The classifier emits
            // one row per THREAD, so two threads sharing a branch name can
            // disagree, and a disagreement is no mandate to recreate anything.
            //
            // `already_recovered` is deliberately NOT consulted here. Every
            // branch in this loop is classified `running`, so the set can only
            // drop a live turn whose worktree is also gone. That would leave a
            // lost worktree behaving differently from a surviving one.
            if idle_branches.contains(branch) {
                continue;
            }

            let mut found_repo: Option<(PathBuf, Option<String>)> = None;
            for (repo_root, repo_id) in &repos_to_scan {
                // `or_unknown(false)`: an unanswered probe must not claim the
                // branch lives in THIS repo, or the scan below builds a worktree
                // against the wrong root. Neither direction is free. A `false`
                // puts a real recovery attempt in front of the destructive step,
                // where a `true` goes straight to the wrong repo with none
                // (`.claude/rules/rust.md`).
                let branch_exists = crate::engine::git_ops::git_answer(
                    &["rev-parse", "--verify", &format!("refs/heads/{}", branch)],
                    repo_root,
                )
                .await
                .or_unknown(false);

                if branch_exists {
                    found_repo = Some((repo_root.clone(), repo_id.clone()));
                    break;
                }
            }

            match found_repo {
                Some((repo_root, repo_id)) => {
                    // Prune stale worktree entries so `git worktree add` doesn't
                    // fail with "already checked out" for a deleted directory.
                    let _ = git_cmd(&["worktree", "prune"], &repo_root).await;

                    let wt_path = lost_session_worktree_path(
                        self.workspace_path(),
                        branch_to_thread.get(branch).copied(),
                    );
                    // NEVER delete a valid worktree here: reclamation belongs to
                    // the cleanup worker alone (ADR 0035). A deterministic
                    // `thread-<short>` dir on disk is one of four things:
                    //
                    // * a live worktree ON THIS BRANCH: reuse it as-is. A
                    //   partial-setup leftover is repaired below, not deleted.
                    // * a live worktree on a DIFFERENT branch: skip it. Reusing
                    //   it resumes against the wrong checkout, and deleting it
                    //   destroys the other branch's work.
                    // * a stranded dir whose git admin is gone: cleared by
                    //   `clear_stranded_worktree_dir` so the add can recreate it.
                    // * absent: created by the add.
                    let is_live_worktree =
                        matches!(tokio::fs::try_exists(&wt_path).await, Ok(true))
                            && crate::engine::git_ops::is_live_worktree_at(&wt_path).await;
                    let on_our_branch = is_live_worktree
                        && crate::engine::git_ops::worktree_current_branch(&wt_path)
                            .await
                            .as_deref()
                            == Some(branch.as_str());

                    if is_live_worktree && !on_our_branch {
                        // The shared path is occupied by another branch's live
                        // worktree, so skip it. The occupant recovers on its own
                        // pass.
                        log!(
                            "[Recovery] Skipping lost branch {} — shared worktree {} is live on a different branch (not reused, not deleted)",
                            branch,
                            wt_path.display()
                        );
                        continue;
                    }

                    let prepared = if on_our_branch {
                        log!(
                            "[Recovery] Reusing existing valid worktree for lost session: {} (branch {})",
                            wt_path.display(),
                            branch
                        );
                        true
                    } else {
                        // Absent, or present but stranded.
                        // `clear_stranded_worktree_dir` removes the dir only
                        // when the git admin is gone.
                        crate::engine::git_ops::clear_stranded_worktree_dir(&repo_root, &wt_path)
                            .await;
                        match worktree_add(&repo_root, &wt_path, &[branch]).await {
                            Ok(o) if o.status.success() => {
                                log!("[Recovery] Created fresh worktree for lost session: {} (branch {})", wt_path.display(), branch);
                                true
                            }
                            Ok(o) => {
                                log!(
                                    "[Recovery] Failed to create worktree for branch {}: {}",
                                    branch,
                                    String::from_utf8_lossy(&o.stderr).trim()
                                );
                                false
                            }
                            Err(e) => {
                                log!(
                                    "[Recovery] git worktree add failed for branch {}: {}",
                                    branch,
                                    e
                                );
                                false
                            }
                        }
                    };

                    if prepared {
                        let marker = wt_path.join(WORKTREE_WORKSPACE_MARKER);
                        let marker_content = if let Some(ref rid) = repo_id {
                            format!("{}\n{}", ws_id, rid)
                        } else {
                            ws_id.clone()
                        };
                        if let Err(e) = tokio::fs::write(&marker, &marker_content).await {
                            log!(
                                "[Recovery] Failed to write workspace marker for {}: {}",
                                branch,
                                e
                            );
                        }
                        // Exclude engine-injected paths, so an external repo does
                        // not see them as untracked and commit them.
                        add_paths_to_worktree_exclude(&wt_path, WORKTREE_EXCLUDE_PATHS).await;
                        to_recover.push((wt_path, branch.clone(), repo_id, repo_root));
                    } else if let Some(&thread_id) = branch_to_thread.get(branch) {
                        log!(
                            "[Recovery] Ending stuck session for thread {}: worktree unavailable",
                            thread_id
                        );
                        end_stuck_session(self, thread_id).await;
                    }
                }
                None => {
                    if let Some(&thread_id) = branch_to_thread.get(branch) {
                        // The branch ref vanished from every repo. Try to
                        // recreate it from a surviving worktree's HEAD first, so
                        // the recorded `cc_session_id` can still `--resume` on
                        // the original branch. Ending the session drops that id
                        // and forces a fresh branch from main, discarding the
                        // conversation, so it is the last resort.
                        match recover_branch_ref_from_worktree(
                            self.workspace_path(),
                            thread_id,
                            branch,
                        )
                        .await
                        {
                            Some((repo_root, wt_path)) => {
                                log!(
                                    "[Recovery] Recovered branch {} for thread {} from surviving worktree — routing into resume instead of ending session",
                                    branch,
                                    thread_id
                                );
                                // `marker_repo_id` is only logged; repo selection
                                // uses `repo_root`.
                                to_recover.push((wt_path, branch.clone(), None, repo_root));
                            }
                            None => {
                                log!("[Recovery] Ending stuck session for thread {} — branch {} not found in any repo and no recoverable worktree", thread_id, branch);
                                end_stuck_session(self, thread_id).await;
                            }
                        }
                    }
                }
            }
        }

        let mut recovering_threads: std::collections::HashSet<uuid::Uuid> =
            std::collections::HashSet::new();

        for (wt_path, branch_name, marker_repo_id, repo_root) in to_recover {
            if let Some(pending_change) = pending_by_branch.get(&branch_name) {
                if actively_running_branches.contains(&branch_name) {
                    // The session was running at shutdown, so its pending row
                    // describes half-finished work the user never confirmed.
                    // Flip it to `incomplete` so Apply asks for confirmation.
                    // The next clean idle clears the flag.
                    log!("[Recovery] Resuming active session with pending change for branch {} — marking change incomplete", branch_name);
                    if let Some(&tid) = branch_to_thread.get(&branch_name) {
                        mark_pending_change_incomplete(&self.event_bus, tid, pending_change).await;
                    }
                } else {
                    log!(
                        "[Recovery] Skipping worktree {} — already has pending change",
                        wt_path.display()
                    );
                    continue;
                }
            }
            // A completed prior recovery settles only the turn it recovered, and
            // the set stays true forever once a thread has been resumed once. So
            // a live classification outranks it: an in-flight turn at boot has no
            // live subprocess, whoever recovered the last one.
            if !actively_running_branches.contains(&branch_name)
                && already_recovered.contains(&branch_name)
            {
                log!(
                    "[Recovery] Skipping worktree {} — recovery thread already exists",
                    wt_path.display()
                );
                continue;
            }
            let has_pending_change = pending_by_branch.contains_key(&branch_name);
            if !branch_awaits_recovery(&branch_name, &idle_branches, has_pending_change) {
                log!(
                    "[Recovery] Skipping clean worktree {} — branch {} has no in-flight signal; cleanup worker will reclaim",
                    wt_path.display(),
                    branch_name
                );
                continue;
            }
            let Some(thread_id) = orphan_recovery_target(&branch_to_thread, &branch_name) else {
                log!(
                    "[Recovery] No originating thread for orphaned worktree {} (branch {}) — skipping; cleanup worker will reclaim",
                    wt_path.display(),
                    branch_name
                );
                continue;
            };
            log!(
                "[Recovery] Reusing original thread {} for branch {}",
                thread_id,
                branch_name
            );

            // Preserve a thread parked on an unanswered question: a stable
            // checkpoint, not an interrupted turn. No abort, no idle, worktree
            // intact, so the card stays answerable. Deliberately not added to
            // `recovering_threads`: the catch-all settle only touches `running`.
            if thread_has_unanswered_question(self.pool(), thread_id).await {
                log!(
                    "[Recovery] Preserving thread {} — parked on an unanswered question (branch {})",
                    thread_id,
                    branch_name
                );
                continue;
            }

            // Prevent duplicate recovery for the same thread (e.g., two branches
            // mapping to the same thread_id from stale resume retries).
            if !recovering_threads.insert(thread_id) {
                log!("[Recovery] Skipping duplicate recovery for thread {} (branch {}) — already recovering", thread_id, branch_name);
                cleanup_stale_worktree(&wt_path).await;
                continue;
            }

            // Carry the prior session id onto the synthetic `CodingAgentIdled`,
            // so a later Continue resumes it. The shared lookup also reads the
            // `Init`-time `CodingAgentSettingsChanged`, so a turn interrupted
            // before its first idle still resumes.
            let cc_session_id: Option<String> =
                crate::engine::agent_session::lookup_latest_cc_session_id(self.pool(), thread_id)
                    .await;

            let is_external_repo = is_external_repo_path(&repo_root, &lucidos_repo_root);
            // Never auto-spawn the agent for a mid-turn crash. Surface the
            // interruption as a synthetic `CodingAgentIdled` carrying
            // `engine_restart_interrupt`, and let the user's Continue re-enter
            // through `--resume`. The worktree stays on disk: Tier 0 of the
            // cleanup worker leaves it until the thread reaches a terminal idle.
            log!("[Recovery] Surfacing interrupted Claude Code session for user-driven continue: {} (branch {}, thread {}{}, cc_session: {})",
                wt_path.display(), branch_name, thread_id,
                marker_repo_id.as_ref().map(|r| format!(", repo {}", r)).unwrap_or_default(),
                cc_session_id.as_deref().unwrap_or("none"));

            // Compute requires_restart from the branch's actual files so the
            // Apply button shows the correct label even before CC re-enters.
            let requires_restart = proposal_files_for_branch(&repo_root, &branch_name)
                .await
                .map(|files| files_require_restart(&files))
                .unwrap_or(false);
            let has_changes = pending_by_branch.contains_key(&branch_name);

            let meta = crate::engine::thread_events::EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..crate::engine::thread_events::EventMeta::NONE
            };
            // Emit the boundary `ResponseAborted` FIRST, so the UI shows the
            // "Response interrupted" panel above the synthetic idle. The
            // dispatcher classifies on `CodingAgentIdled.reason`, so the order
            // does not affect spawn decisions.
            if !boundary_abort_already_emitted(self.pool(), thread_id).await {
                let originating_event_id =
                    crate::engine::agent_session::latest_originating_event_id(
                        self.pool(),
                        thread_id,
                        crate::engine::agent_session::CC_ORIGINATING_EVENT_TYPES,
                    )
                    .await;
                let abort_meta = crate::engine::thread_events::EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    request_event_id: originating_event_id,
                    // The host killed the previous turn; recovery only marks it.
                    // Engine-deliberate work uses `Engine { .. }` instead.
                    actor: Some(MessageOrigin::system()),
                    ..crate::engine::thread_events::EventMeta::NONE
                };
                crate::engine::thread_events::emit_response_aborted(
                    &self.event_bus,
                    thread_id,
                    crate::engine::thread_events::AbortCause::RecoveryAfterRestart,
                    String::new(),
                    vec![],
                    None,
                    None,
                    abort_meta,
                    "[Recovery] ResponseAborted (engine_restart_interrupt)",
                )
                .await;
            }

            // Resume or manual Continue, by cause. A user switch left a
            // device-attributed teardown boundary, so auto-resume it. The resume
            // is queued: `main.rs` emits `ContinuationRequested` once the spawn
            // dispatcher is subscribed, and recovery runs before it. A crash left
            // no boundary, so offer Continue instead and never auto-resume: work
            // that crashed the engine must not loop.
            if switch_was_user_initiated(self.pool(), thread_id).await {
                self.enqueue_switch_resume(thread_id);
                log!(
                    "[Recovery] Queued auto-resume after user switch for thread {} (branch {})",
                    thread_id,
                    branch_name
                );
            } else {
                let coding_agent = self.thread_coding_agent(thread_id).await;
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
                                has_changes,
                                is_external_repo,
                                requires_restart,
                                cc_session_id,
                                coding_agent,
                                reason: Some(ENGINE_RESTART_INTERRUPT_REASON.to_string()),
                                worktree_path: Some(wt_path.to_string_lossy().into_owned()),
                                // Snapshot HEAD, so the next spawn can detect
                                // edits made while the engine was down.
                                worktree_head_sha:
                                    crate::engine::agent_session::external_edits_for_recovery_head_sha(&wt_path).await,
                                bg_bash_pending: false,
                            },
                            meta,
                        },
                        "[Recovery] CodingAgentIdled (engine_restart_interrupt)",
                    )
                    .await;
            }
        }

        // Catch-all: settle any coding-agent thread the projection still shows
        // `running` that this pass neither resumed nor settled.
        settle_orphaned_running_coding_agent_threads(
            self.pool(),
            &self.event_bus,
            &recovering_threads,
        )
        .await;

        recovering_threads.into_iter().collect()
    }
}

/// True when a discovered worktree's branch still owes the user a recovery: the
/// turn on it was open when the engine died, or a change on it is still waiting
/// to be applied. False means the branch is settled, so the cleanup worker can
/// have the disk.
///
/// **Every input must be a fact about the branch's CURRENT turn.** That is why
/// this exists as a named predicate rather than an inline `&&`. `idle_branches`
/// reads the newest lifecycle event, and a pending change is by definition
/// unresolved. A *historical* fact is not admissible however settled it sounds.
/// A coding-agent thread works one branch across many turns, so anything true of
/// an earlier turn stays true forever and silently retires the thread.
pub(crate) fn branch_awaits_recovery(
    branch: &str,
    idle_branches: &std::collections::HashSet<String>,
    has_pending_change: bool,
) -> bool {
    // Pending change wins over an idle classification: the agent reached its
    // idle and then waited for Apply, so the branch is settled only once the
    // user resolves the change.
    has_pending_change || !idle_branches.contains(branch)
}

/// True when the thread's most recent `UserQuestionAsked` has no later answer,
/// terminal, or agent progression: it is parked waiting for the user, and the
/// card on screen is still live. Such a thread is a stable, resumable
/// checkpoint, so recovery must preserve it across a restart with no abort and
/// no idle. Answering resumes it through the no-live-subprocess
/// `ContinuationRequested` path. A pending question survives a user switch and a
/// crash alike.
///
/// Shared predicate for BOTH sides of that invariant. The teardown emit consults
/// it to skip the boundary `ResponseAborted`, because a question-parked session
/// is still MID-TURN and the `is_in_flight()` filter cannot exclude it. This
/// recovery pass consults it to skip the abort and idle pair. One definition
/// keeps "no boundary lands at teardown" and "recovery preserves" from drifting
/// apart: a `ResponseAborted` is park-ending, so a teardown that emitted one
/// would defeat the guard on the very next boot.
pub(crate) async fn thread_has_unanswered_question(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    // `$1` is bound as the thread id (text). The shared fragment keeps this
    // per-thread check and every set-based sweep on one definition.
    let sql = format!("SELECT {}", unanswered_question_exists_sql("$1"));
    sqlx::query_scalar::<_, bool>(&sql)
        .bind(thread_id.to_string())
        .fetch_one(pool)
        .await
        .unwrap_or(false)
}

/// True when an engine teardown must leave this thread exactly as it is because
/// its session is parked on an unanswered `AskUserQuestion`. Thin wrapper over
/// [`thread_has_unanswered_question`], so the two teardown sites cannot diverge
/// and the skip is always logged. Callers still cancel the agent runtime, so no
/// subprocess outlives the engine.
///
/// The card must stay answerable across the restart, which needs the
/// `UserQuestionAsked` to still be the thread's newest event at the next boot.
/// Two teardown sites would break that, and both consult this:
///
/// * `shutdown_agent_sessions` sends the graceful interrupt, which cancels the
///   question and records a rejection the user never made.
/// * the stop and chat-cancel arms of `run_session` emit a terminal and flush
///   buffered agent text.
///
/// Gated on `is_shutdown`, so a user Stop, Apply, Discard or Archive outside a
/// teardown is untouched: each deliberately ends the turn and cancel-stamps the
/// card itself. The gate is a *window*, not an actor test, so a raw Stop inside
/// the teardown window is swallowed too.
pub(crate) async fn preserve_question_park_at_shutdown(
    pool: &sqlx::PgPool,
    site: &'static str,
    thread_id: Uuid,
    is_shutdown: bool,
) -> bool {
    if !is_shutdown || !thread_has_unanswered_question(pool, thread_id).await {
        return false;
    }
    log!(
        "[Shutdown] {}: preserving session {}, parked on an unanswered question \
         (no interrupt, no terminal, no text flush)",
        site,
        thread_id
    );
    true
}

/// Canonical "thread is parked on an unanswered `AskUserQuestion`" predicate, as
/// a correlated SQL `EXISTS(...)` body. `id_expr` is a SQL expression yielding
/// the thread's `aggregate_id` (text): `"$1"` for the single-thread bool check,
/// or a column reference such as `"pt.aggregate_id"` for a set-based sweep.
///
/// This is the SINGLE source of truth for the preserve guard. Every restart
/// abort or cleanup path resolves through this fragment, so "parked on a
/// question means never aborted" cannot drift between paths. A terminal after
/// the `UserQuestionAsked` flips it to false. A path that wrongly emitted one
/// would then defeat every OTHER path's guard on the next boot.
///
/// "Parked" means the question is still the last thing that happened: no answer,
/// no terminal, AND no agent progression. The progression half comes from
/// [`crate::engine::thread_events::ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES`],
/// the constant the frontend
/// mirrors to strike the card through. So "the card is dead" and "the thread is
/// no longer preserved" cannot disagree.
pub(crate) fn unanswered_question_exists_sql(id_expr: &str) -> String {
    format!(
        "EXISTS ( \
            SELECT 1 FROM events uqa \
            WHERE uqa.aggregate_id = {id} AND uqa.event_type = 'UserQuestionAsked' \
              AND NOT EXISTS ( \
                  SELECT 1 FROM events later \
                  WHERE later.aggregate_id = {id} AND later.sequence > uqa.sequence \
                    AND later.event_type IN ({park_ending}) \
              ) \
         )",
        id = id_expr,
        park_ending = &*PARK_ENDING_EVENT_TYPES_SQL,
    )
}

/// Park-ending events that are NOT in
/// [`crate::engine::thread_events::ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES`],
/// and why each is absent
/// there. `UserQuestionAnswered` is that check's own pairing key, looked up by
/// `tool_use_id` rather than by type. `ResponseGenerated` and `SessionEnded` are
/// absent because a coding-agent turn ends on `CodingAgentIdled`. The preserve
/// guard must still treat them as park-ending: either one means the turn that
/// owned the question is over.
const PARK_ENDING_EXTRA_EVENT_TYPES: &[&str] =
    &["UserQuestionAnswered", "ResponseGenerated", "SessionEnded"];

/// The park-ending event types as a SQL `IN (...)` body. Every name is a
/// compile-time literal from the two lists above, so there is nothing to
/// parameterize and nothing to escape. Built once: the predicate runs per
/// candidate thread in both recovery sweeps, and the list never varies.
static PARK_ENDING_EVENT_TYPES_SQL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    crate::engine::thread_events::ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES
        .iter()
        .chain(PARK_ENDING_EXTRA_EVENT_TYPES.iter())
        .map(|t| format!("'{t}'"))
        .collect::<Vec<_>>()
        .join(",")
});

/// True when the newest `ResponseAborted` after the thread's last start is an
/// **engine-shutdown teardown carrying a device actor**: the fingerprint of a
/// user-initiated *Switch to new version*. A crash emits no teardown boundary,
/// so this is false and the thread keeps the manual Continue affordance instead
/// of auto-resuming (ADR 0045).
///
/// **Both halves of the fingerprint are load-bearing.** The device actor alone
/// is not enough: `AbortCause::StaleSettle` deliberately carries the actor of
/// the user button that exposed a stuck row. An actor-only predicate would read
/// a user *Stop* as a *Switch* and resume work the user just abandoned.
///
/// The start set includes the resume starts, so once a switch abort has been
/// consumed by a resume it stops counting. That is the loop-breaker: an
/// auto-resume that crashes the engine again before emitting anything leaves the
/// resume start newer than the abort, so the next boot offers manual Continue.
///
/// Shared with the chat resume gate, so the two definitions cannot drift.
pub(crate) async fn switch_was_user_initiated(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(&format!(
        "SELECT EXISTS ( \
            SELECT 1 FROM events WHERE aggregate_id = $1 \
              AND {SWITCH_TEARDOWN_ABORT_SQL} \
              AND {unsuperseded})",
        unsuperseded = switch_abort_unsuperseded_sql("$1", "sequence"),
    ))
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

/// True when a `ResponseAborted` already covers the thread's **current** turn, so
/// the recovery pass must not emit a second boundary over the top of it.
///
/// `/api/v1/restart` pre-emits a `ResponseAborted { actor: device }` for
/// in-flight coding-agent threads BEFORE shutdown, so the post-restart timeline
/// reads "Paused by restart". Emitting again here would double-render the abort
/// panel and bury that device attribution under the system actor.
///
/// "Current turn" is the load-bearing half. It is why this shares
/// [`after_latest_thread_start_sql`] with the switch fingerprint rather than
/// spelling out a start set of its own. An abort older than the thread's newest
/// start belongs to a turn a later resume superseded. It says nothing about
/// whether THIS turn was interrupted.
pub(crate) async fn boundary_abort_already_emitted(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(&format!(
        "SELECT EXISTS ( \
            SELECT 1 FROM events WHERE aggregate_id = $1 \
              AND event_type = 'ResponseAborted' \
              AND {current_turn})",
        current_turn = after_latest_thread_start_sql("$1", "sequence"),
    ))
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await
    // A probe that could not run is UNKNOWN. Fall back to emitting: a duplicate
    // boundary is cosmetic noise, a missing one hides a real interruption.
    .unwrap_or(false)
}

/// [`boundary_abort_already_emitted`], narrowed to a boundary that **names this
/// turn**: same current-turn window, plus the abort's `request_event_id` must
/// equal the turn's own anchor.
///
/// The extra clause is what makes the answer safe for a caller that will SKIP
/// its own terminal on a `true`. The recovery pass can rely on the window alone,
/// because a spurious `true` costs it only a duplicate panel. An in-loop
/// terminal is the turn's ONLY terminator, so there a spurious `true` costs the
/// turn its terminator.
///
/// The window alone is turn-exact only for turns carrying one of
/// [`THREAD_START_EVENTS_SQL`], and two ordinary shapes carry none: a parent
/// woken by `ChildThreadCompleted`, and an `answered_after_idle` continuation
/// that deliberately withholds its `ContinuationStarted`. For those, a previous
/// turn's abort stays inside the window forever.
///
/// A `None` anchor proves nothing, so the caller treats it as "not covered" and
/// emits. Same fail-open direction as the sibling: a duplicate boundary is
/// cosmetic, a missing terminator is not.
pub(crate) async fn boundary_abort_covers_turn(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    request_event_id: Uuid,
) -> bool {
    sqlx::query_scalar::<_, bool>(&format!(
        "SELECT EXISTS ( \
            SELECT 1 FROM events WHERE aggregate_id = $1 \
              AND event_type = 'ResponseAborted' \
              AND payload->>'request_event_id' = $2 \
              AND {current_turn})",
        current_turn = after_latest_thread_start_sql("$1", "sequence"),
    ))
    .bind(thread_id.to_string())
    .bind(request_event_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

/// SQL predicate matching the teardown boundary abort of a user-initiated *Switch to
/// new version*: an `EngineShutdown` `ResponseAborted` stamped with the device that
/// clicked switch. Assumes the row is already scoped to one thread's events.
///
/// Shared by [`switch_was_user_initiated`] and the chat candidate scan so "what a
/// switch abort looks like" is defined exactly once.
pub(crate) const SWITCH_TEARDOWN_ABORT_SQL: &str = "event_type = 'ResponseAborted' \
     AND payload->'actor'->>'kind' = 'device' \
     AND payload->>'cause' = 'engine_shutdown'";

/// SQL list of the events that begin (or restart) a thread's turn. Reach for
/// [`after_latest_thread_start_sql`] rather than this constant: the list is only
/// ever useful inside that one predicate.
const THREAD_START_EVENTS_SQL: &str = "'MessageReceived',\
    'CodingAgentUserMessageSent','TriggerStarted','ContinuationStarted',\
    'OrphanRecoveryStarted'";

/// SQL boolean: the event at `seq_expr` is newer than every
/// [`THREAD_START_EVENTS_SQL`] event on the thread at `id_expr`, so it belongs
/// to that thread's **current** turn.
///
/// Every recovery read asking "which turn does this `ResponseAborted` belong
/// to?" goes through here, because the interesting failures are two of them
/// answering differently:
///
/// * Is the *Switch to new version* fingerprint still live, or did a resume
///   already consume it? ([`switch_abort_unsuperseded_sql`], the loop-breaker.)
/// * Does this turn still need an interruption boundary, or did the teardown
///   pre-emit already land one? ([`boundary_abort_already_emitted`].)
///
/// `id_expr` yields the thread's `aggregate_id` (text): `"$1"` for a bound
/// single-thread check, or a column reference for a set-based scan. `seq_expr`
/// yields the event's sequence in the same scope. The subquery aliases its own
/// `events` as `s`, so an unqualified `seq_expr` in an un-aliased outer query is
/// never captured by it.
fn after_latest_thread_start_sql(id_expr: &str, seq_expr: &str) -> String {
    format!(
        "{seq} > COALESCE(( \
             SELECT MAX(s.sequence) FROM events s \
             WHERE s.aggregate_id = {id} \
               AND s.event_type IN ({starts}) \
         ), 0)",
        seq = seq_expr,
        id = id_expr,
        starts = THREAD_START_EVENTS_SQL,
    )
}

/// The **resume loop-breaker**: the switch abort at `seq_expr` is still the newest
/// thing that happened on the thread at `id_expr`, so no resume has consumed it.
///
/// One definition for all three consumers, so a switch abort cannot be "consumed"
/// by one of them and still live for another: the coding-agent resume gate
/// ([`switch_was_user_initiated`]), the chat one
/// (`chat::recovery::switch_resume_candidates`), and the boot floor
/// ([`unresumed_switch_threads_sql`]). `ContinuationStarted` is in the start set,
/// so once a resume has actually begun the abort stops counting anywhere. This is
/// what stops an auto-resume that dies before emitting anything else from being
/// resumed again on the next boot, forever.
pub(crate) fn switch_abort_unsuperseded_sql(id_expr: &str, seq_expr: &str) -> String {
    after_latest_thread_start_sql(id_expr, seq_expr)
}

/// Coding-agent lifecycle events that mean **the turn is over**: the engine was
/// not mid-response when it died, so a restart must NOT re-open that turn with a
/// "Response interrupted" boundary and a Continue button.
///
/// This is the whole list of terminals a coding-agent turn can end on, minus two
/// deliberate absences:
///
/// * **`ResponseAborted`** IS the interrupted boundary, and an `EngineShutdown`
///   one carrying a device actor is the *Switch to new version* fingerprint
///   [`switch_was_user_initiated`] keys on. Counting it as turn-ended would
///   classify every switched-away session as idle and kill auto-resume.
/// * **`SessionEnded`** is listed, but only for the reasons that really end a
///   turn. The mid-turn ones are subtracted separately by
///   [`SESSION_ENDED_MID_TURN_REASONS_SQL`].
const TURN_ENDED_EVENT_TYPES_SQL: &str = "'CodingAgentIdled','ResponseGenerated',\
    'ResponseCanceled','ResponseFailed','SessionEnded'";

/// [`crate::engine::thread_events::SessionEndReason`] values that do NOT end the
/// turn. A `SessionEnded` carrying one is dropped from the lifecycle scan, so
/// the preceding `SessionStarted` becomes the newest lifecycle event again and
/// the branch classifies `running`.
///
/// * `stale_resume` is transient: the agent answered a stale `--resume` with an
///   empty Result, and the handler retries against a fresh session.
/// * `shutdown` is the engine going away mid-turn, the `SessionEnded`-shaped
///   twin of the `ResponseAborted { EngineShutdown }` boundary excluded above.
///   No production site emits it today, but the variant is live in the enum.
///   Re-adding that emit must not silently cost *Switch to new version* its
///   auto-resume.
///
/// Matched through `COALESCE(payload->>'reason','')`, and the `COALESCE` is
/// load-bearing. `reason` is absent on the oldest rows, where a bare `IN` yields
/// NULL and drops them from the scan instead of keeping them. They would fall
/// back to an older `SessionStarted` and classify `running`, which is the bogus
/// interrupt panel this classifier exists to prevent.
const SESSION_ENDED_MID_TURN_REASONS_SQL: &str = "'stale_resume','shutdown'";

/// Events proving a new turn began after the last turn-ended event, so the
/// session was live again when the engine died.
const TURN_PROGRESSION_EVENT_TYPES_SQL: &str = "'SessionStarted','CodingAgentUserMessageSent',\
    'MessageReceived','CodingAgentPromptSent','CodingAgentToolCalled',\
    'CodingAgentTextStreamed','ContinuationStarted'";

/// Classify every coding-agent branch as `running` or `idle`, one row per thread
/// that ever emitted a `SessionStarted` with a branch. `running` means a turn was
/// in flight when the engine died, so resume it or offer Continue. `idle` means
/// the turn ended, so leave the worktree to the cleanup worker.
///
/// A branch is `running` iff its newest lifecycle event is a `SessionStarted`,
/// or a turn-progression event landed after its newest turn-ended event.
/// Everything else is `idle`. There is no third state: the caller computes
/// `in_flight = !idle_branches.contains(branch)`, so a branch missing from both
/// sets would be treated as in-flight.
pub(crate) static BRANCH_CLASSIFICATION_SQL: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| {
        format!(
            "WITH last_lifecycle AS ( \
            SELECT DISTINCT ON (thread_id) thread_id, event_type, sequence \
            FROM events \
            WHERE event_type IN ('SessionStarted',{turn_ended}) \
              AND NOT (event_type = 'SessionEnded' \
                       AND COALESCE(payload->>'reason','') IN ({mid_turn_ends})) \
              AND thread_id IS NOT NULL \
            ORDER BY thread_id, sequence DESC \
         ), \
         session_branches AS ( \
            SELECT DISTINCT ON (thread_id) thread_id, payload->>'branch' AS branch \
            FROM events \
            WHERE event_type = 'SessionStarted' AND thread_id IS NOT NULL \
              AND payload->>'branch' IS NOT NULL AND payload->>'branch' != '' \
            ORDER BY thread_id, sequence DESC \
         ) \
         SELECT sb.branch, \
                CASE WHEN ll.event_type = 'SessionStarted' \
                       OR EXISTS ( \
                           SELECT 1 FROM events e3 \
                           WHERE e3.thread_id = ll.thread_id \
                             AND e3.sequence > ll.sequence \
                             AND e3.event_type IN ({progression}) \
                       ) \
                     THEN 'running' ELSE 'idle' END AS status \
         FROM last_lifecycle ll \
         JOIN session_branches sb ON sb.thread_id = ll.thread_id",
            turn_ended = TURN_ENDED_EVENT_TYPES_SQL,
            mid_turn_ends = SESSION_ENDED_MID_TURN_REASONS_SQL,
            progression = TURN_PROGRESSION_EVENT_TYPES_SQL,
        )
    });

/// Threads still holding an UNKEPT resume promise: a switch-teardown abort that
/// is the thread's newest `ResponseAborted`, with no start event after it, on a
/// thread the projection still shows `paused`. ADR 0045 records why the engine
/// discharges its own promise, and which paths leave one unkept.
///
/// Four clauses, each load-bearing:
///
/// * `t.status = 'paused'` scopes the sweep to threads still on the interruption.
/// * `t.state = 'active'` is the compose lifecycle, NOT the archive curtain. A
///   composing row is a draft and a discarded one a tombstone.
/// * The abort is the thread's newest `ResponseAborted`, the idempotency guard:
///   the withdrawal emits a `RecoveryAfterRestart` abort, so the thread stops
///   matching on the next boot.
/// * No newer [`THREAD_START_EVENTS_SQL`] event, the loop-breaker both resume
///   gates use.
///
/// **Archived threads are deliberately INCLUDED**, unlike the resume drains:
/// this revives nothing, it corrects a promise the engine could not keep. The
/// withdrawal inherits the abort's `request_event_id` AND its `actor`, the only
/// surviving record of who clicked switch.
fn unresumed_switch_threads_sql() -> String {
    format!(
        "SELECT e.aggregate_id::uuid AS thread_id, \
                e.payload->>'request_event_id' AS request_event_id, \
                e.payload->'actor' AS actor \
         FROM events e \
         JOIN thread_summaries t ON t.thread_id = e.aggregate_id::uuid \
         WHERE e.aggregate = 'thread' \
           AND t.state = 'active' \
           AND t.status = 'paused' \
           AND {abort} \
           AND e.sequence = ( \
               SELECT MAX(a.sequence) FROM events a \
               WHERE a.aggregate_id = e.aggregate_id \
                 AND a.event_type = 'ResponseAborted' \
           ) \
           AND {unsuperseded} \
         ORDER BY e.sequence ASC",
        abort = SWITCH_TEARDOWN_ABORT_SQL,
        unsuperseded = switch_abort_unsuperseded_sql("e.aggregate_id", "e.sequence"),
    )
}

/// Withdraw every resume promise this boot did not keep. It emits the
/// crash-shaped `ResponseAborted { RecoveryAfterRestart }` boundary that
/// `chat::recovery::recover_orphaned_threads` already uses for an interrupted
/// turn nobody is resuming. The frontend's newest-abort scan then re-arms
/// Continue on its own, because that boundary is not a switch abort (ADR 0045).
///
/// `resumed` is the union of what the two resume drains actuated, passed BY ID
/// rather than re-derived from the events table. A coding-agent resume has only
/// emitted `ContinuationRequested` by then, and that type is deliberately absent
/// from [`THREAD_START_EVENTS_SQL`]. A query-only exclusion would therefore
/// re-abort a thread that is resuming perfectly well.
///
/// Best-effort, like every boot sweep: a DB error degrades to "nothing to
/// withdraw" rather than blocking boot. Every withdrawal is logged by id, so the
/// sweep can never read as "resumed everything" when it did not.
///
/// Runs LAST in the boot sequence, after both drains: only then can it tell a
/// broken promise from a kept one. The boundary is crash-SHAPED, not
/// crash-ATTRIBUTED. The actor is carried over from the switch abort by
/// [`unresumed_switch_threads_sql`].
pub(crate) async fn settle_unresumed_switch_threads(
    pool: &sqlx::PgPool,
    bus: &crate::engine::event_bus::EventBus,
    resumed: &std::collections::HashSet<Uuid>,
) {
    type UnresumedSwitchRow = (Uuid, Option<String>, Option<serde_json::Value>);
    let rows: Vec<UnresumedSwitchRow> = match sqlx::query_as(&unresumed_switch_threads_sql())
        .fetch_all(pool)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            log!(
                "[Recovery] unresumed-switch sweep query failed: {}. \
                 Affected threads keep their paused status with no Continue",
                e
            );
            return;
        }
    };

    for (thread_id, request_event_id, switch_actor) in rows {
        if resumed.contains(&thread_id) {
            continue;
        }
        log!(
            "[Recovery] Switch-interrupted thread {} was not resumed by this boot. \
             Withdrawing the resume promise so its Continue affordance returns",
            thread_id
        );
        // Both fields are inherited from the switch abort rather than invented.
        // The id keeps the two panels in one exchange, and the actor keeps them
        // naming the same person.
        //
        // The fallback is unreachable, since the selection matches only a device
        // actor. Log it rather than silently attributing the user's restart to
        // the host.
        let actor = switch_actor
            .and_then(|v| match serde_json::from_value::<MessageOrigin>(v) {
                Ok(origin) => Some(origin),
                Err(e) => {
                    log!(
                        "[Recovery] Switch abort on thread {} has an unreadable actor: {}. \
                         Withdrawal falls back to the system actor",
                        thread_id,
                        e
                    );
                    None
                }
            })
            .unwrap_or_else(MessageOrigin::system);
        let meta = crate::engine::thread_events::EventMeta {
            // `.ok()` is safe: an unparseable value only costs grouping, and the
            // engine wrote the field itself.
            request_event_id: request_event_id.as_deref().and_then(|s| s.parse().ok()),
            actor: Some(actor),
            ..crate::engine::thread_events::EventMeta::NONE
        };
        crate::engine::thread_events::emit_response_aborted(
            bus,
            thread_id,
            crate::engine::thread_events::AbortCause::RecoveryAfterRestart,
            "This response was interrupted by an engine restart and did not resume.".to_string(),
            vec![],
            None,
            None,
            meta,
            "[Recovery] ResponseAborted (switch resume not kept)",
        )
        .await;
    }
}

/// Settle any coding-agent thread still `running` in the projection that boot
/// recovery neither resumed nor settled. After a restart there are NO live
/// subprocesses, so such a thread is a permanent zombie: the in-memory watchdogs
/// only scan live `agent_sessions`, which is empty at boot. The skip paths in
/// `recover_orphaned_worktrees` that DROP a worktree leave the projection
/// unsettled, and this is the floor under those. The question-park preserve is
/// not one of them: it keeps the worktree and must stay unsettled.
///
/// Scoped to coding-agent threads on purpose. Chat orphans are settled by
/// `recover_orphaned_threads`, and a chat thread blocked on a child legitimately
/// sits `running` pending parent-resume. A coding-agent thread exits its
/// subprocess at every turn boundary, so at boot a `running` one has no live
/// session. `settle_stuck_running_thread` re-checks per thread, so a thread
/// settled elsewhere in the meantime is a no-op.
pub(crate) async fn settle_orphaned_running_coding_agent_threads(
    pool: &sqlx::PgPool,
    bus: &crate::engine::event_bus::EventBus,
    recovering: &std::collections::HashSet<Uuid>,
) {
    let running: Vec<Uuid> = match sqlx::query_scalar::<_, Uuid>(
        "SELECT thread_id FROM thread_summaries \
         WHERE is_coding_agent = true AND status = 'running'",
    )
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            log!(
                "[Recovery] orphaned-running settle sweep query failed: {}",
                e
            );
            return;
        }
    };
    for tid in running {
        if recovering.contains(&tid) {
            continue;
        }
        match crate::engine::claude_code::settle_stuck_running_thread(
            pool,
            bus,
            tid,
            Some(MessageOrigin::system()),
            crate::engine::claude_code::SettleTerminal::StuckProjection,
        )
        .await
        {
            Ok(true) => log!(
                "[Recovery] Settled orphaned `running` coding-agent thread {} — no live session after restart and not picked up by recovery",
                tid
            ),
            Ok(false) => {}
            Err(e) => log!(
                "[Recovery] Failed to settle orphaned running thread {}: {}",
                tid,
                e
            ),
        }
    }
}
