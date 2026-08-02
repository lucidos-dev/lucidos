//! The recovery `impl LucidosEngine` block: stale-waiting-session
//! settlement and orphaned-worktree recovery.

use super::super::agent_session::change_description_fallback;
use super::super::change_ops::branch_is_hardened;
use super::super::claude_code::{WORKTREE_EXCLUDE_PATHS, WORKTREE_WORKSPACE_MARKER};
use super::super::git_ops::{
    add_paths_to_worktree_exclude, auto_commit_worktree, default_local_branch,
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
    /// non-empty `changed_files` list (typically from `proposal_files_for_branch`)
    /// so this never creates a phantom `changes` row with `file_count=0`.
    ///
    /// `origin` is forwarded to the emitted `ChangeProposed` event so the route
    /// popover can render "Engine · Stale session cleanup" / "Engine · Orphan
    /// recovery". Both engine-internal callers (stale-session + orphan) supply
    /// the appropriate `MessageOrigin::Engine { reason }`.
    ///
    /// `incomplete` is derived from the prior turn's actual terminal event
    /// via `last_turn_ended_cleanly` — a clean `ResponseGenerated` gets
    /// `incomplete: false`, anything else (mid-turn crash, cancel, fail, or
    /// no terminal at all) gets `incomplete: true`. Hardcoding `true` here
    /// would mislabel work from a clean turn whose per-idle `propose_change`
    /// simply never landed (e.g. the engine died between idle and proposal).
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
        // Marker survives worktree removal (keyed by repo_root + branch_name),
        // so this lookup works even when the cleanup worker has already removed
        // the worktree under us. Without this check, propose_change downgrades
        // hardened=true → false and Apply re-runs `/harden` on already-hardened
        // work.
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

    /// Handle ending a stale waiting Claude Code session (no live process) after engine restart.
    /// Looks up the branch from SessionStarted events, proposes changes, and cleans up.
    ///
    /// `actor` identifies who initiated the operation. HTTP-driven entry points
    /// (Apply Now, Cancel-with-apply, Archive) plumb the user's device through so
    /// any resulting `ChangeApplied` / `ChangeApplyFailed` events stamp the
    /// real actor instead of falling back to the "Lucidos Engine" chip.
    /// Engine-internal restart-recovery callers pass `None`.
    pub(crate) async fn end_stale_waiting_session(
        self: &Arc<Self>,
        thread_id: Uuid,
        discard: bool,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Look up branch name from SessionStarted event
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
                // No SessionStarted event at all — this thread never had a Claude Code session.
                // Return an error instead of emitting SessionEnded, which would pollute
                // regular chat threads with CC-only events (causes spurious "Session ended
                // automatically" banners in the UI).
                return Err("No Claude Code session found for this thread".into());
            }
            Some(_) => {
                // SessionStarted exists but branch is empty — stale Claude Code session with no branch.
                // Phase 4: Mark the orphaned session as idle (turn boundary)
                // rather than terminating it. The thread stays alive — the user
                // can still send a follow-up that re-spawns CC via --resume.
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

        // If worktree exists, commit uncommitted changes (unless discarding) and remove it
        if let Some(ref wt) = wt_path {
            if !discard {
                auto_commit_worktree(wt, "Coding agent changes (auto-committed)").await;
            }
            if let Some(wt_str) = wt.to_str() {
                let _ = git_cmd(&["worktree", "remove", "--force", wt_str], &repo_root).await;
            } else {
                log!(
                    "[Recovery] skipped worktree remove (non-UTF8 path): {}",
                    wt.display()
                );
            }
        }

        // `Some(files)` only when the branch has commits AND a non-empty net diff.
        // Commits whose changes cancel out (commit + revert) get cleaned up like
        // a no-op branch.
        let proposal_files = proposal_files_for_branch(&repo_root, &branch_name).await;

        let mut proposed_change = false;
        if discard {
            // User explicitly chose "Discard & End Session" on a thread with no
            // live Claude Code subprocess. Plumb the user's actor through to the
            // resulting ChangeDiscarded events so the chat chip reads "You"
            // instead of falling back to the engine label.
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
            // `proposal_files` is None: either the branch has no commits, or
            // its commits cancel out (commit + revert).
            //
            // Discard intent (`discard=true`) is already handled by the
            // `if discard` arm above — it emits ChangeDiscarded and deletes
            // the branch unconditionally. The only callers that land here are
            // `discard=false`: the Apply button (apply_now), Archive, and
            // UserStop. For those, deleting the branch silently is a bug:
            // when `proposal_files_for_branch` returns None for any reason
            // we don't understand (transient git failure, empty net diff on
            // commits that the user does care about, projection gaps), the
            // user's two-commit branch gets `git branch -D`'d and the work
            // is unrecoverable. Apply then fails with "No changes to apply
            // — branch is already merged or has no commits", which is a lie
            // the engine itself just constructed.
            //
            // Keep the branch. Orphaned empty branches are cheap (just a ref
            // SHA); the worktree_cleanup periodic sweep GCs fully-merged
            // branches, and engine startup recovery re-attaches any branch
            // that still has work to be done.
            log!(
                "[Recovery] recovery: branch {} has no proposable diff — keeping branch (discard=false)",
                branch_name
            );
        }

        // Phase 4: SessionEnded is terminal-only. Emit `CodingAgentIdled` to
        // mark the orphaned turn as ended without terminating the thread —
        // ChangeProposed/ChangeDiscarded events emitted earlier already drive
        // the panel state for change-bearing branches.
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
                        // Worktree was removed above (`worktree remove --force`)
                        // before this idle fires. Recording the now-deleted path
                        // would mislead the resolver into trying to reuse it.
                        // Leave None — the next spawn falls through to
                        // `git worktree list` lookup or fresh deterministic
                        // path generation.
                        worktree_path: None,
                        // No worktree → no SHA to record. The next spawn will
                        // skip external-edit detection until a real CC turn
                        // populates the field on its own idle.
                        worktree_head_sha: None,
                        bg_bash_pending: false,
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                "[Recovery] CodingAgentIdled",
            )
            .await;

        // No auto-apply / auto-discard tail: any pending change on the branch
        // is left for the user to resolve from Review. The Apply Now flow
        // (`apply_now`) chains its own POST /changes/<id>/apply once the
        // proposal lands via SSE, so we don't need to fuse propose+apply here.

        self.broadcast_changes_updated().await;

        Ok(())
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

        // Collect all repos to scan: Lucidos repo (no repo_id) + registered external repos.
        // Deduplicate by canonical path so the same repo isn't scanned twice
        // (e.g., Lucidos repo registered as both the built-in repo and an external repo).
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
                        if br.starts_with("claude-code/") {
                            // Fast path: worktrees outside this workspace's worktrees
                            // dir cannot belong to us — skip without reading the marker.
                            // Saves a stat+read per cross-workspace worktree, which
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
        // Recovery sweep is best-effort — a DB error degrades to "nothing pending" so the
        // startup classifier doesn't block boot. The error is logged so the sweep's
        // partial behaviour is visible in the logs rather than silent.
        let pending_changes_list = proj.list_pending().await.unwrap_or_else(|e| {
            log!(
                "[Recovery] list_pending: {} — recovery proceeds without pending change context",
                e
            );
            Vec::new()
        });
        let completed_change_branches_list = proj.list_completed_branches().await.unwrap_or_else(|e| {
            log!(
                "[Recovery] list_completed_branches: {} — recovery proceeds without completed branch context",
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
            sqlx::query_as::<_, BranchStatus>(
                "WITH last_lifecycle AS ( \
                    SELECT DISTINCT ON (thread_id) thread_id, event_type, sequence \
                    FROM events \
                    WHERE event_type IN ('SessionStarted', 'CodingAgentIdled', 'SessionEnded', 'ResponseGenerated') \
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
                 SELECT sb.branch, 'idle' AS status \
                 FROM last_lifecycle ll \
                 JOIN session_branches sb ON sb.thread_id = ll.thread_id \
                 WHERE ll.event_type IN ('CodingAgentIdled', 'ResponseGenerated') \
                   AND NOT EXISTS ( \
                       SELECT 1 FROM events e3 \
                       WHERE e3.thread_id = ll.thread_id \
                         AND e3.sequence > ll.sequence \
                         AND e3.event_type IN ('SessionStarted', 'CodingAgentUserMessageSent', 'MessageReceived', \
                             'CodingAgentPromptSent', 'CodingAgentToolCalled', 'CodingAgentTextStreamed', 'ContinuationStarted') \
                   ) \
                 UNION ALL \
                 SELECT sb.branch, 'running' AS status \
                 FROM last_lifecycle ll \
                 JOIN session_branches sb ON sb.thread_id = ll.thread_id \
                 WHERE ll.event_type = 'SessionStarted' \
                 UNION ALL \
                 SELECT sb.branch, 'running' AS status \
                 FROM last_lifecycle ll \
                 JOIN session_branches sb ON sb.thread_id = ll.thread_id \
                 WHERE ll.event_type IN ('CodingAgentIdled', 'ResponseGenerated') \
                   AND EXISTS ( \
                       SELECT 1 FROM events e3 \
                       WHERE e3.thread_id = ll.thread_id \
                         AND e3.sequence > ll.sequence \
                         AND e3.event_type IN ('CodingAgentUserMessageSent', 'MessageReceived', 'CodingAgentPromptSent', \
                             'CodingAgentToolCalled', 'CodingAgentTextStreamed', 'ContinuationStarted') \
                   )"
            ).fetch_all(pool),
            sqlx::query_as::<_, BranchThread>(
                "SELECT DISTINCT ON (payload->>'branch') thread_id, payload->>'branch' AS branch FROM events \
                 WHERE event_type = 'SessionStarted' AND payload->>'branch' IS NOT NULL \
                   AND payload->>'branch' != '' AND thread_id IS NOT NULL \
                 ORDER BY payload->>'branch', sequence DESC"
            ).fetch_all(pool),
        );

        // A failed classification query yields an empty set, which would silently
        // misclassify every branch (e.g. re-recover already-completed sessions).
        // Log on Err so the failure shows up in startup logs even if recovery
        // proceeds with degraded data.
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

        let completed_change_branches: std::collections::HashSet<String> =
            completed_change_branches_list.into_iter().collect();

        log!("[Recovery] Worktree scan: {}ms, DB classification: {}ms (worktrees={}, idle={}, running={})",
            t_worktree_scan.as_millis(),
            (t0.elapsed() - t_worktree_scan).as_millis(),
            to_recover.len(),
            idle_branches.len(),
            actively_running_branches.len());

        // Phase 2: DB-based discovery for lost worktrees.
        // The worktree scan above only finds branches with existing worktree
        // directories and valid workspace markers. If a worktree directory was
        // cleaned up (macOS temp cleanup, git prune, crash during setup) but the
        // session was actively running, the branch is invisible to the scan.
        // Detect these from the DB and create fresh worktrees so they enter the
        // normal recovery pipeline.
        // Owned set required because we push to `to_recover` in the loop below.
        let discovered_branches: std::collections::HashSet<String> =
            to_recover.iter().map(|(_, br, _, _)| br.clone()).collect();

        // Helper: emit CodingAgentIdled to unstick a thread whose session
        // can't be recovered (worktree gone, branch missing, git error). Phase
        // 4 made SessionEnded terminal-only — the thread stays alive, and the
        // user can re-spawn CC by sending a new message. Phase 5.3 stamps
        // `reason: Some("engine_restart_interrupt")` so the UI can render a
        // "continue?" affordance instead of treating the idle as natural.
        let end_stuck_session = |engine: &Arc<Self>, thread_id: Uuid| {
            let bus = engine.event_bus.clone();
            let engine = engine.clone();
            async move {
                // Same preserve rule as the `to_recover` loop below: a thread
                // parked on an unanswered question is a stable checkpoint, and
                // the `CodingAgentIdled` this closure emits is in the
                // predicate's exclusion list — idling here would expire the
                // card and defeat the guard. Even with the worktree/branch
                // unrecoverable, answering still resumes via the
                // no-live-subprocess `ContinuationRequested` → `--resume` path
                // (which re-resolves or recreates the worktree).
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
                            // No worktree to record — this path fires when
                            // recovery cannot locate the worktree at all
                            // (branch missing, git error). The thread stays
                            // alive; the next spawn will resolve a path via
                            // `git worktree list` fallback or new deterministic
                            // path generation.
                            worktree_path: None,
                            // Same reason as worktree_path — without a worktree
                            // there's no SHA to snapshot. External-edit
                            // detection skips the next spawn until a real CC
                            // turn populates the field.
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
            if already_recovered.contains(branch)
                || completed_change_branches.contains(branch)
                || idle_branches.contains(branch)
            {
                continue;
            }

            let mut found_repo: Option<(PathBuf, Option<String>)> = None;
            for (repo_root, repo_id) in &repos_to_scan {
                let branch_exists = git_cmd(
                    &["rev-parse", "--verify", &format!("refs/heads/{}", branch)],
                    repo_root,
                )
                .await
                .map(|o| o.status.success())
                .unwrap_or(false);

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
                    // NEVER delete a valid worktree here. A deterministic
                    // `thread-<short>` dir on disk is one of: (a) a valid live
                    // worktree ON THIS BRANCH — REUSE it as-is (it holds the user's
                    // checkout; a partial-setup leftover such as a missing marker is
                    // repaired below, not nuked); (b) a valid live worktree on a
                    // DIFFERENT branch — the shared per-thread path is occupied by
                    // another branch of the same thread (e.g. a duplicate
                    // stale-resume branch), so we must NOT reuse it (recovery mode
                    // skips branch verification → we'd resume this branch against the
                    // wrong checkout) and must NOT delete it (it holds the other
                    // branch's work); skip and let the occupant recover on its own
                    // pass; (c) a genuinely stranded/broken dir
                    // (`.git/worktrees/<name>` admin gone → nothing recoverable) —
                    // cleared by `clear_stranded_worktree_dir` so the add can
                    // recreate it; or (d) absent — created by the add. The
                    // background WorktreeCleanup worker is the SOLE deleter of a
                    // valid worktree. This replaced an unconditional `remove_dir_all`
                    // that nuked a live worktree out from under recovery — 2026-07-02.
                    let is_live_worktree =
                        matches!(tokio::fs::try_exists(&wt_path).await, Ok(true))
                            && crate::engine::git_ops::is_live_worktree_at(&wt_path).await;
                    let on_our_branch = is_live_worktree
                        && crate::engine::git_ops::worktree_current_branch(&wt_path)
                            .await
                            .as_deref()
                            == Some(branch.as_str());

                    if is_live_worktree && !on_our_branch {
                        // Case (b): shared path occupied by a different branch's live
                        // worktree. Reusing it would resume THIS branch on the wrong
                        // checkout; deleting it would destroy the other branch's work.
                        // Skip — the occupant is recovered on its own pass, and
                        // duplicate branches for one thread are reconciled elsewhere.
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
                        // Dir absent, or present-but-not-a-valid-worktree (stranded).
                        // `clear_stranded_worktree_dir` removes the dir ONLY when
                        // genuinely stranded (git admin gone); a valid worktree is
                        // left untouched (and can't reach here — handled above).
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
                        // Add engine-injected paths to the worktree's git exclude
                        // so external repos don't see them as untracked or
                        // accidentally commit them.
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
                        // The branch ref vanished from every repo. Before giving up on
                        // the session, try to recreate the branch from a surviving
                        // worktree's HEAD so the recorded `cc_session_id` can still
                        // `--resume` on the original branch (the thread-9e37697e
                        // recovery-resilience goal). Ending the session — which drops
                        // the session id and forces a fresh branch from main, discarding
                        // the conversation — is the genuine last resort, not the first.
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
                                // `marker_repo_id: None` — purely cosmetic in the resume
                                // loop (it logs the id; repo selection uses `repo_root`).
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
                    // Session was actively running at shutdown — resume it. The
                    // pending row was populated by per-commit emits during the
                    // dying turn, so its description / files reflect half-
                    // finished work the user never confirmed. Flip the row to
                    // `incomplete: true` so the Apply UI requires explicit
                    // confirmation; the resumed session's next clean idle
                    // re-emits with `incomplete: false` and clears the flag.
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
            if already_recovered.contains(&branch_name) {
                log!(
                    "[Recovery] Skipping worktree {} — recovery thread already exists",
                    wt_path.display()
                );
                continue;
            }
            // Skip non-in-flight branches so applied/idle threads don't
            // surface a misleading "Continue?" affordance. Pending change
            // counts as in-flight — CC was awaiting Apply when the engine
            // died.
            let in_flight = !idle_branches.contains(&branch_name)
                && !completed_change_branches.contains(&branch_name);
            let has_pending_change = pending_by_branch.contains_key(&branch_name);
            if !in_flight && !has_pending_change {
                log!(
                    "[Recovery] Skipping clean worktree {} — branch {} has no in-flight signal; cleanup worker will reclaim",
                    wt_path.display(),
                    branch_name
                );
                continue;
            }
            // Reuse the branch's originating thread; skip (don't fabricate)
            // when none owns it. See `orphan_recovery_target` for why skipping
            // beats the old phantom-thread path.
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

            // Preserve a thread parked on an unanswered question: it's a stable
            // checkpoint, not an interrupted turn. Leave it
            // waiting_for_user_answer (no abort, no idle), worktree intact — the
            // card stays answerable and answering resumes via
            // ContinuationRequested → --resume. (Not added to recovering_threads:
            // the catch-all settle only touches status='running', and the cleanup
            // worker won't reclaim a non-terminal thread's worktree.)
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

            // Look up the prior cc_session_id so the synthetic CodingAgentIdled
            // below carries it forward — when the user later clicks "continue",
            // the spawn dispatcher's SpawnRequest::Continue handler resolves the
            // same sid via `lookup_latest_cc_session_id` and passes it to
            // `--resume`. The shared lookup reads it from both `CodingAgentIdled`
            // and the `Init`-time `CodingAgentSettingsChanged`, so a turn
            // interrupted before its first idle still resumes instead of falling
            // back to a fresh session + reconstructed summary.
            let cc_session_id: Option<String> =
                crate::engine::agent_session::lookup_latest_cc_session_id(self.pool(), thread_id)
                    .await;

            let is_external_repo = is_external_repo_path(&repo_root, &lucidos_repo_root);
            // Phase 5.3: do NOT auto-spawn CC for mid-turn-crashed sessions.
            // Surface the interruption as a synthetic CodingAgentIdled with
            // `reason = engine_restart_interrupt` so the UI can render a
            // "continue?" affordance. The user's click POSTs to
            // /api/v1/threads/<id>/continue, which emits ContinuationRequested — the
            // spawn dispatcher then re-enters CC via `--resume` against the
            // recorded `cc_session_id`. The worktree stays on disk: the
            // dispatcher resolves it on next spawn, and the cleanup worker's
            // Tier 0 won't reclaim it until the thread reaches a terminal
            // idle state with no pending change.
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
            // Emit the boundary `ResponseAborted` FIRST so the UI shows the
            // "Response interrupted" panel above the synthetic Idled. The
            // dispatcher classifies on `CodingAgentIdled.reason`, so order
            // doesn't affect spawn decisions.
            //
            // Idempotency: `/api/v1/restart` pre-emits a `ResponseAborted{actor:
            // device}` for in-flight CC threads BEFORE shutdown so the
            // post-restart timeline reads "You restarted". If that event
            // exists newer than the latest start, skip our emit — emitting
            // again would double-render the AbortPanel and overwrite the
            // device attribution with `engine`.
            let abort_already_exists: bool = sqlx::query_scalar(
                "SELECT EXISTS ( \
                    SELECT 1 FROM events WHERE aggregate_id = $1 \
                      AND event_type = 'ResponseAborted' \
                      AND sequence > COALESCE( \
                          (SELECT MAX(sequence) FROM events WHERE aggregate_id = $1 \
                             AND event_type IN ('MessageReceived','CodingAgentUserMessageSent','TriggerStarted')), 0))",
            )
            .bind(thread_id.to_string())
            .fetch_one(self.pool())
            .await
            .unwrap_or(false);

            if !abort_already_exists {
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
                    // The host system killed the previous CC turn (engine
                    // crashed mid-turn / OS killed the process). The recovery
                    // path is just marking it. Engine-deliberate work uses
                    // `Engine{...}` instead.
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

            // Resume vs manual Continue, by cause. A user-initiated *Switch to new
            // version* left a device-attributed teardown boundary → auto-resume
            // (queued; `main.rs` emits `ContinuationRequested` once the spawn
            // dispatcher is subscribed — recovery runs before it). A crash /
            // involuntary death left no such boundary → surface the manual
            // "Continue" affordance and do NOT auto-resume, so work that may have
            // crashed the engine can't loop.
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
                                // Snapshot the worktree's HEAD so the next spawn
                                // can detect external edits the user made while
                                // the engine was down. Best-effort — failures
                                // (e.g. branch with zero commits) yield None.
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

        // Catch-all (defense-in-depth): settle any coding-agent thread the
        // projection still shows `running` that this pass neither resumed nor
        // settled — the floor that would have caught thread-72120ca6. See
        // `settle_orphaned_running_coding_agent_threads`.
        settle_orphaned_running_coding_agent_threads(
            self.pool(),
            &self.event_bus,
            &recovering_threads,
        )
        .await;

        recovering_threads.into_iter().collect()
    }
}

/// True when the thread's most recent `UserQuestionAsked` has no later answer,
/// terminal, or agent progression: it is parked waiting for the user, and the
/// card on screen is still live. Such a thread is a stable,
/// resumable checkpoint: recovery must preserve it (no abort, no idle) across a
/// restart so the card stays answerable; answering resumes via the existing
/// no-live-subprocess `ContinuationRequested` → `--resume` path
/// (`ensure_resume_after_answer`). Holds for both a user switch and a crash
/// (SIGKILL emits nothing) — a pending question survives either way.
///
/// Shared predicate for BOTH sides of that invariant. The teardown emit
/// (`emit_teardown_abort_unless_question_parked`) consults it to skip the
/// boundary `ResponseAborted` — a question-parked session is still MID-TURN
/// (subprocess alive, blocked in the AskUserQuestion hook), so the
/// `is_in_flight()` filter cannot exclude it — and this recovery pass consults
/// it to skip the abort/idle pair. One definition keeps "no boundary lands at
/// teardown" and "recovery preserves" from drifting apart: a `ResponseAborted`
/// in the exclusion list below flips this to false, so a teardown that emitted
/// one would defeat the preserve guard on the very next boot.
pub(crate) async fn thread_has_unanswered_question(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    // `$1` is bound as the thread id (text). One shared fragment (below) so this
    // per-thread bool check and every set-based sweep filter key on the SAME
    // "parked on an unanswered question" definition — the DRY anchor for the
    // preserve guard across teardown + both recovery sweeps.
    let sql = format!("SELECT {}", unanswered_question_exists_sql("$1"));
    sqlx::query_scalar::<_, bool>(&sql)
        .bind(thread_id.to_string())
        .fetch_one(pool)
        .await
        .unwrap_or(false)
}

/// True when an engine teardown must leave this thread exactly as it is because
/// its session is parked on an unanswered `AskUserQuestion`. Thin wrapper over
/// [`thread_has_unanswered_question`] so the two teardown sites cannot diverge
/// on either half of the decision (the cause gate and the predicate), and so the
/// skip is always logged.
///
/// A parked session is a stable checkpoint, not an interrupted turn: the card
/// must stay answerable across the restart, and answering resumes it via
/// `ContinuationRequested` then `--resume`. That requires the
/// `UserQuestionAsked` to still be the thread's newest event when the next
/// engine boots. Two teardown sites can break that, and both consult this:
///
/// * `shutdown_agent_sessions` would send the graceful `interrupt` (Claude
///   Code's Esc), which cancels the pending `AskUserQuestion` and makes CC
///   record a rejection the user never made as a `CodingAgentToolResult`.
/// * the stop / chat-cancel arms of `run_session` would emit a terminal AND
///   flush any buffered agent text (`kill_cc_and_flush` runs BEFORE the
///   `external_terminal_emitted` dedup check, and that flag only covers sessions
///   `abort_in_flight_for_restart` actually walked, not one inserted after that
///   pass by a slow `--resume` racing the restart).
///
/// Every one of those events is park-ending, so any of them kills the card and
/// strands the turn with no terminator: the 2026-08-01 "Working forever" report
/// (`docs/plans/2026-08-01-preserve-question-parked-session-through-teardown.md`).
/// Callers still cancel the agent runtime, so the subprocess cannot outlive the
/// engine.
///
/// Gated on `is_shutdown`, so outside a teardown this never fires: a user Stop /
/// Apply / Discard / Archive with a question on screen is untouched, because
/// those are deliberate user actions that DO end the turn and they cancel-stamp
/// the card themselves. The gate is a *window*, not an actor test, and the
/// `emit_stop_terminal` call site widens it to `is_shutdown ||
/// engine.is_shutting_down()` to cover a session inserted after
/// `shutdown_agent_sessions` took its flag pass. So a raw Stop that lands inside
/// the teardown window IS swallowed too. That is deliberate: the engine is on
/// its way out either way, and preserving the card costs the user nothing that
/// the restart was not about to take anyway.
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
/// the thread's `aggregate_id` (text): `"$1"` for the single-thread bool check
/// ([`thread_has_unanswered_question`]), or a column reference such as
/// `"pt.aggregate_id"` / `"e.thread_id::text"` for a set-based sweep filter.
///
/// This is the SINGLE source of truth for the preserve guard. Every restart
/// abort/cleanup path — the teardown boundary emit
/// (`emit_teardown_abort_unless_question_parked`), the chat orphan sweep
/// (`recover_orphaned_threads`), the orphan-tool-call sweep
/// (`recover_orphan_tool_calls`), and the coding-agent recovery pass — resolves
/// through this fragment, so "parked on a question ⇒ never aborted, card stays
/// answerable" cannot drift between paths. A `ResponseAborted` (or any terminal)
/// after the `UserQuestionAsked` flips it to false, so a path that wrongly
/// emitted one would defeat every OTHER path's guard on the next boot — which is
/// exactly why all of them must consult this one fragment.
///
/// "Parked" means the question is still the last thing that happened on the
/// thread: no answer, no terminal, AND no agent progression. The progression
/// half comes from [`ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES`], the same
/// constant the frontend mirrors to strike the card through, so "the card is
/// dead" and "the thread is no longer preserved" cannot disagree. Before
/// 2026-08-01 this list was terminals-only, and a `CodingAgentToolResult` from
/// an Esc'd `AskUserQuestion` left a thread reported as preserved while its card
/// was already unanswerable and its turn had no terminator, so recovery skipped
/// it and it read "Working" forever (see
/// `docs/plans/2026-08-01-preserve-question-parked-session-through-teardown.md`).
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
/// [`ThreadEvent::QUESTION_OVERTAKEN_EVENT_TYPES`], and why each is absent
/// there. `UserQuestionAnswered` is the overtaken check's own pairing key (it
/// looks the answer up by `tool_use_id` rather than by type). `ResponseGenerated`
/// and `SessionEnded` are omitted from the shared constant because
/// `UserQuestionAsked` is CC-only on the production path and CC turns end with
/// `CodingAgentIdled`; the preserve guard still has to treat them as park-ending,
/// because either one means the turn that owned the question is over.
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

/// True when the newest `ResponseAborted` (after the thread's last start-or-resume)
/// is an **engine-shutdown teardown carrying a device actor** — the fingerprint of a
/// user-initiated *Switch to new version* (the teardown boundary emit stamps the
/// device that clicked switch onto in-flight threads). A crash (SIGKILL) emits no
/// teardown boundary, so this is false → the thread keeps the manual "Continue"
/// affordance and is NOT auto-resumed: work that may have crashed the engine can't
/// loop.
///
/// **Both halves of the fingerprint are load-bearing.** The device actor alone is not
/// enough: `AbortCause::StaleSettle` deliberately carries the actor of the user button
/// that exposed a stuck row (Stop / Apply / Discard / Archive / Interrupt — see
/// `claude_code::settle_stuck_running_thread`), so an actor-only predicate reads a user
/// *Stop* as a *Switch* and auto-resumes work the user just abandoned. Only
/// `EngineShutdown` is a teardown boundary; every other cause is either a crash-shaped
/// terminal or a projection cleanup, and none of them should resume.
///
/// The start set includes `ContinuationStarted` / `OrphanRecoveryStarted` — a
/// prior auto-resume's start — so once a switch-abort has been consumed by a
/// resume, it no longer counts. That is the loop-breaker: if the auto-resumed CC
/// crashes the engine again *before* emitting any lifecycle event (so
/// `already_recovered` doesn't yet cover it), the next boot sees the resume start
/// as newer than the device abort → this returns false → manual Continue.
///
/// Shared by the coding-agent resume gate (`recover_orphaned_worktrees`) and the
/// chat/trigger one (`chat::recovery::switch_resume_candidates`) — the single
/// definition of "was this a user switch", so the two can never drift.
pub(crate) async fn switch_was_user_initiated(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(&format!(
        "SELECT EXISTS ( \
            SELECT 1 FROM events WHERE aggregate_id = $1 \
              AND {SWITCH_TEARDOWN_ABORT_SQL} \
              AND sequence > COALESCE( \
                  (SELECT MAX(sequence) FROM events WHERE aggregate_id = $1 \
                     AND event_type IN ({THREAD_START_EVENTS_SQL})), 0))"
    ))
    .bind(thread_id.to_string())
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

/// SQL list of the events that begin (or restart) a thread's turn. A switch abort
/// only counts while no newer start supersedes it — the resume loop-breaker.
pub(crate) const THREAD_START_EVENTS_SQL: &str = "'MessageReceived',\
    'CodingAgentUserMessageSent','TriggerStarted','ContinuationStarted',\
    'OrphanRecoveryStarted'";

/// Settle any coding-agent thread still `running` in the projection that boot
/// recovery neither resumed (in `recovering`) nor settled. After a restart
/// there are NO live subprocesses, so a `running` coding-agent thread with no
/// recovery is a permanent zombie — the in-memory watchdogs only scan live
/// `agent_sessions` (empty at boot), so nothing else would ever clear it. This
/// is the floor that would have caught thread-72120ca6: it stayed `running`
/// across the restart because the skip paths in `recover_orphaned_worktrees`
/// (`continue` on already-has-pending-change / no-in-flight-signal / duplicate /
/// no-originating-thread) drop a worktree WITHOUT settling the projection.
///
/// Scoped to coding-agent threads on purpose: chat orphans are settled by
/// `recover_orphaned_threads`, and a chat thread blocked on a child legitimately
/// sits `running` pending parent-resume — settling it here would break that. A
/// coding-agent thread, by contrast, exits its subprocess at every turn
/// boundary, so at boot a `running` one has no live session.
/// `settle_stuck_running_thread` re-checks `running` per thread (idempotent), so
/// a thread settled by another path in the meantime is a no-op.
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
