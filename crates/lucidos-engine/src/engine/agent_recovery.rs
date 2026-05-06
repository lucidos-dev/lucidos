use super::agent_session::change_description_fallback;
use super::change_ops::branch_is_hardened;
use super::claude_code::WORKTREE_WORKSPACE_MARKER;
use super::git_ops::{
    auto_commit_worktree, default_local_branch, describe_branch_changes, files_require_restart,
    find_worktree_for_branch, git_cmd, main_worktree, proposal_files_for_branch, worktrees_dir,
};
use super::thread_events::{EngineReason, EventChannel, MessageOrigin};
use super::LucidosEngine;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

/// Tag stamped on `CodingAgentIdled.reason` when the engine surfaces a
/// mid-turn-crashed CC session as "interrupted, click to continue" instead of
/// auto-resuming it. The frontend reads this constant to render the continue
/// affordance; the spawn dispatcher ignores `CodingAgentIdled` entirely so
/// the user's click is what produces the next spawn (via `ContinueSignal`).
pub const ENGINE_RESTART_INTERRUPT_REASON: &str = "engine_restart_interrupt";

/// Tag stamped on `ContinueSignal.reason` when the user clicks the
/// "click to continue" affordance after a mid-turn interrupt. The continue
/// endpoint emits with this reason; the spawn dispatcher classifies it as a
/// `SpawnTrigger::ContinueSignal` and starts the next CC turn.
pub const USER_CLICKED_CONTINUE_REASON: &str = "user_clicked_continue";

/// Remove a stale worktree directory and delete its branch.
/// Best-effort — failures are silently ignored since the worktree
/// will just be skipped again on next restart.
async fn cleanup_stale_worktree(wt_path: &Path, branch_name: &str, repo_root: &Path) {
    let _ = git_cmd(
        &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
        repo_root,
    )
    .await;
    let _ = git_cmd(&["branch", "-D", branch_name], repo_root).await;
}

/// Add the workspace marker to the worktree's git exclude so it doesn't
/// appear as an untracked file (important for external repos).
/// Best-effort — failure is logged but not fatal.
async fn write_marker_git_exclude(wt_path: &Path) {
    let git_dir = wt_path.join(".git");
    let exclude_dir = if tokio::fs::metadata(&git_dir)
        .await
        .map(|m| m.is_file())
        .unwrap_or(false)
    {
        if let Ok(content) = tokio::fs::read_to_string(&git_dir).await {
            content
                .trim()
                .strip_prefix("gitdir: ")
                .map(|p| wt_path.join(p).join("info"))
        } else {
            None
        }
    } else {
        Some(git_dir.join("info"))
    };
    if let Some(info_dir) = exclude_dir {
        if let Err(e) = tokio::fs::create_dir_all(&info_dir).await {
            log!("[Recovery] Failed to create git info dir: {}", e);
            return;
        }
        let exclude_file = info_dir.join("exclude");
        let already_excluded = tokio::fs::read_to_string(&exclude_file)
            .await
            .map(|c| c.lines().any(|l| l.trim() == WORKTREE_WORKSPACE_MARKER))
            .unwrap_or(false);
        if !already_excluded {
            match tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&exclude_file)
                .await
            {
                Ok(mut f) => {
                    use tokio::io::AsyncWriteExt;
                    if let Err(e) = f
                        .write_all(format!("\n{}\n", WORKTREE_WORKSPACE_MARKER).as_bytes())
                        .await
                    {
                        log!("[Recovery] Failed to write git exclude: {}", e);
                    }
                }
                Err(e) => log!("[Recovery] Failed to open git exclude: {}", e),
            }
        }
    }
}

impl LucidosEngine {
    /// Gather branch metadata and propose a Change record. Callers must pass a
    /// non-empty `changed_files` list (typically from `proposal_files_for_branch`)
    /// so this never creates a phantom `changes` row with `file_count=0`.
    ///
    /// `origin` is forwarded to the emitted `ChangeProposed` event so the route
    /// popover can render "Engine · Stale session cleanup" / "Engine · Orphan
    /// recovery". Both engine-internal callers (stale-session + orphan) supply
    /// the appropriate `MessageOrigin::Engine { reason }`.
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
        let request_id = Uuid::new_v4();
        let repo_root_str = repo_root.to_string_lossy();
        // Marker survives worktree removal (keyed by repo_root + branch_name),
        // so recovery can read it after `safe_cleanup_worktree` deleted the worktree.
        // Without this check, propose_change downgrades hardened=true → false and
        // Apply re-runs `/harden` on already-hardened work.
        let hardened = branch_is_hardened(self.pool(), self.changes(), repo_root, branch_name).await;
        self.propose_change(crate::engine::change_ops::ProposeChangeInput {
            request_id,
            thread_id,
            branch_name,
            repo_root: &repo_root_str,
            description: &description,
            files: changed_files,
            requires_restart,
            channel: EventChannel::CodingAgent,
            hardened,
            origin,
        })
        .await
    }

    /// Safe worktree cleanup: before deleting, check if the branch has commits
    /// ahead of main. If so, auto-propose a Change to prevent work loss.
    /// For external repos, the branch is kept but no change is proposed — the
    /// user manages push/PR workflows for external repos independently.
    /// Returns true if cleanup proceeded (with or without proposal), false if
    /// we couldn't save the work and skipped cleanup entirely.
    async fn safe_cleanup_worktree(
        self: &Arc<Self>,
        wt_path: &Path,
        branch_name: &str,
        repo_root: &Path,
        thread_id: Option<Uuid>,
        is_external_repo: bool,
    ) -> bool {
        auto_commit_worktree(wt_path, "Claude Code changes (auto-committed on cleanup)").await;

        if let Some(changed_files) = proposal_files_for_branch(repo_root, branch_name).await {
            if is_external_repo {
                log!("[Recovery] External repo branch {} has commits — keeping branch, no change proposed", branch_name);
            } else {
                let tid = thread_id.unwrap_or_else(Uuid::new_v4);
                match self
                    .propose_branch_changes(
                        tid,
                        branch_name,
                        repo_root,
                        &changed_files,
                        Some(MessageOrigin::engine(EngineReason::OrphanRecovery)),
                    )
                    .await
                {
                    Ok(change_id) => {
                        log!("[Recovery] SAFETY: Auto-proposed change {} for branch {} — branch has commits ahead of main that would have been deleted",
                            change_id, branch_name);
                    }
                    Err(e) => {
                        log!("[Recovery] SAFETY: Failed to auto-propose change for branch {} — SKIPPING cleanup to prevent work loss: {}",
                            branch_name, e);
                        return false;
                    }
                }
            }
            // Keep the branch — for Lucidos the proposed Change references it,
            // for external repos the user manages it independently.
            let _ = git_cmd(
                &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
                repo_root,
            )
            .await;
            true
        } else {
            cleanup_stale_worktree(wt_path, branch_name, repo_root).await;
            true
        }
    }

    /// Handle ending a stale waiting CC session (no live process) after engine restart.
    /// Looks up the branch from SessionStarted events, proposes changes, and cleans up.
    ///
    /// `actor` identifies who initiated the operation. HTTP-driven entry points
    /// (Apply Now, Cancel-with-apply, Done) plumb the user's device through so
    /// any resulting `ChangeApplied` / `ChangeApplyFailed` events stamp the
    /// real actor instead of falling back to the "Lucidos Engine" chip.
    /// Engine-internal restart-recovery callers pass `None`.
    pub(crate) async fn end_stale_waiting_session(
        self: &Arc<Self>,
        thread_id: Uuid,
        auto_apply: bool,
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
                // No SessionStarted event at all — this thread never had a CC session.
                // Return an error instead of emitting SessionEnded, which would pollute
                // regular chat threads with CC-only events (causes spurious "Session ended
                // automatically" banners in the UI).
                return Err("No Claude Code session found for this thread".into());
            }
            Some(_) => {
                // SessionStarted exists but branch is empty — stale CC session with no branch.
                if auto_apply {
                    self.event_bus
                        .emit_or_log(
                            crate::engine::event_bus::BusEvent::Thread {
                                thread_id,
                                event:
                                    crate::engine::thread_events::ThreadEvent::ChangeApplyFailed {
                                        change_id: String::new(),
                                        error: "No branch found for session — nothing to apply"
                                            .to_string(),
                                        actor: actor.clone(),
                                    },
                                meta: crate::engine::thread_events::EventMeta::NONE,
                            },
                            "[Recovery] ChangeApplyFailed",
                        )
                        .await;
                }
                // Phase 4: Mark the orphaned session as idle (turn boundary)
                // rather than terminating it. The thread stays alive — the user
                // can still send a follow-up that re-spawns CC via --resume.
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
                                has_changes: false,
                                is_external_repo: false,
                                requires_restart: false,
                                cc_session_id: None,
                                agent: crate::runtime::AgentKind::ClaudeCode,
                                reason: None,
                                worktree_path: None,
                                worktree_head_sha: None,
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
            "[ClaudeCode] Ending stale waiting session for thread {} (branch {})",
            thread_id,
            branch_name
        );

        let repo_root = main_worktree().await;

        let wt_path = find_worktree_for_branch(&repo_root, &branch_name).await;

        // If worktree exists, commit uncommitted changes (unless discarding) and remove it
        if let Some(ref wt) = wt_path {
            if !discard {
                auto_commit_worktree(wt, "Claude Code changes (auto-committed)").await;
            }
            let _ = git_cmd(
                &["worktree", "remove", "--force", wt.to_str().unwrap()],
                &repo_root,
            )
            .await;
        }

        // `Some(files)` only when the branch has commits AND a non-empty net diff.
        // Commits whose changes cancel out (commit + revert) get cleaned up like
        // a no-op branch.
        let proposal_files = proposal_files_for_branch(&repo_root, &branch_name).await;

        let mut proposed_change = false;
        if discard {
            // User chose "Discard & End Session" — delete the branch, don't propose changes
            log!(
                "[ClaudeCode] Discarding stale session changes (branch {})",
                branch_name
            );
            self.discard_pending_for_thread(thread_id).await;
            if let Err(e) = git_cmd(&["branch", "-D", &branch_name], &repo_root).await {
                log!(
                    "[ClaudeCode] Failed to delete branch {}: {}",
                    branch_name,
                    e
                );
            }
        } else if let Some(changed_files) = proposal_files {
            let is_external = match self.is_external_repo_thread(thread_id).await {
                Ok(v) => v,
                Err(e) => {
                    log!(
                        "[ClaudeCode] Failed to check external repo status for thread {}: {}",
                        thread_id,
                        e
                    );
                    false
                }
            };

            if is_external {
                log!(
                    "[ClaudeCode] External repo branch {} — keeping branch, no change proposed",
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
                            "[ClaudeCode] Proposed change from stale session (branch {})",
                            branch_name
                        );
                    }
                    Err(e) => log!(
                        "[ClaudeCode] Failed to propose change from stale session: {}",
                        e
                    ),
                }
            }
        } else {
            // No commits, or commits with empty net diff — delete the branch if no pending change
            if !self.changes().has_pending_for_branch(&branch_name).await {
                let _ = git_cmd(&["branch", "-D", &branch_name], &repo_root).await;
            }
        }

        // Phase 4: SessionEnded is terminal-only. Emit `CodingAgentIdled` to
        // mark the orphaned turn as ended without terminating the thread —
        // ChangeProposed/ChangeDiscarded events emitted earlier already drive
        // the panel state for change-bearing branches.
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
                        has_changes: proposed_change,
                        is_external_repo: false,
                        requires_restart: false,
                        cc_session_id: None,
                        agent: crate::runtime::AgentKind::ClaudeCode,
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
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                "[Recovery] CodingAgentIdled",
            )
            .await;

        // Auto-apply path: every pending change on this branch must reach a
        // terminal event (ChangeApplied / ChangeApplyFailed / ChangeDiscarded)
        // before we return. Otherwise the UI's transient "Applying..." spinner
        // — set when the user clicked Apply — has no event to react to and
        // hangs forever, hiding the failure. Two failure modes were silent
        // before this block was added:
        //   (a) `apply_change` errored → only logged, no event.
        //   (b) Branch already merged or deleted → `proposal_files == None`
        //       so we never even tried to apply, and the row stayed pending.
        if auto_apply {
            let orphans: Vec<_> = self
                .changes()
                .list_pending()
                .await
                .into_iter()
                .filter(|c| c.branch_name == branch_name)
                .collect();

            if proposed_change {
                for change in &orphans {
                    log!(
                        "[ClaudeCode] Auto-applying stale session change: {}",
                        change.id
                    );
                    if let Err(e) = self.apply_change(change.id, actor.clone()).await {
                        log!(
                            "[ClaudeCode] Auto-apply of stale session change {} failed: {}",
                            change.id,
                            e
                        );
                        self.emit_apply_failed(
                            thread_id,
                            change.id,
                            &e.to_string(),
                            actor.clone(),
                        )
                        .await;
                    }
                }
            } else {
                // Branch is gone or has no commits but pending rows exist.
                // Each orphan needs a terminal event — discard succeeds → the
                // generic "branch gone" reason; discard fails → that error
                // reaches the user too (no silent log-and-continue).
                for change in &orphans {
                    let msg = match self.discard_change(change.id, actor.clone()).await {
                        Ok(()) => "Branch already merged or removed — nothing to apply.".to_string(),
                        Err(e) => {
                            log!(
                                "[ClaudeCode] Failed to discard orphan change {}: {}",
                                change.id,
                                e
                            );
                            format!("Failed to discard orphan change: {}", e)
                        }
                    };
                    self.emit_apply_failed(thread_id, change.id, &msg, actor.clone())
                        .await;
                }
            }
        }

        self.broadcast_changes_updated().await;

        Ok(())
    }

    /// Detect orphaned Claude Code worktrees from a previous engine run and
    /// start new CC sessions on them instead of proposing pending changes.
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
        let mut repos_to_scan: Vec<(PathBuf, Option<String>)> = vec![(lucidos_repo_root, None)];
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
        let pending_changes_list = proj.list_pending().await;
        let completed_change_branches_list = proj.list_completed_branches().await;
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
                    WHERE event_type IN ('SessionRecovered', 'SessionResumed', 'OrphanRecoveryStarted') \
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
                             'CodingAgentPromptSent', 'CodingAgentToolCalled', 'CodingAgentTextStreamed', 'SessionRecovered') \
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
                             'CodingAgentToolCalled', 'CodingAgentTextStreamed', 'SessionRecovered') \
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

        let pending_branches: std::collections::HashSet<String> =
            pending_changes_list.into_iter().map(|c| c.branch_name).collect();

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
            async move {
                bus.emit_or_log(
                    crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
                            has_changes: false,
                            is_external_repo: false,
                            requires_restart: false,
                            cc_session_id: None,
                            agent: crate::runtime::AgentKind::ClaudeCode,
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

                    let wt_id = Uuid::new_v4().as_simple().to_string();
                    let wt_path =
                        worktrees_dir(self.workspace_path()).join(format!("cc-{}", wt_id));
                    let wt_str = wt_path.to_string_lossy();

                    match git_cmd(
                        &[
                            "-c",
                            "filter.git-crypt.smudge=",
                            "-c",
                            "filter.git-crypt.clean=",
                            "-c",
                            "filter.git-crypt.required=false",
                            "worktree",
                            "add",
                            &wt_str,
                            branch,
                        ],
                        &repo_root,
                    )
                    .await
                    {
                        Ok(o) if o.status.success() => {
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
                            // Add marker to worktree's git exclude so external repos
                            // don't see it as an untracked file.
                            write_marker_git_exclude(&wt_path).await;
                            log!("[Recovery] Created fresh worktree for lost session: {} (branch {})", wt_path.display(), branch);
                            to_recover.push((wt_path, branch.clone(), repo_id, repo_root));
                        }
                        Ok(o) => {
                            let stderr = String::from_utf8_lossy(&o.stderr);
                            log!(
                                "[Recovery] Failed to create worktree for branch {}: {}",
                                branch,
                                stderr.trim()
                            );
                            if let Some(&thread_id) = branch_to_thread.get(branch) {
                                log!("[Recovery] Ending stuck session for thread {} — worktree creation failed", thread_id);
                                end_stuck_session(self, thread_id).await;
                            }
                        }
                        Err(e) => {
                            log!(
                                "[Recovery] git worktree add failed for branch {}: {}",
                                branch,
                                e
                            );
                            if let Some(&thread_id) = branch_to_thread.get(branch) {
                                log!(
                                    "[Recovery] Ending stuck session for thread {} — git error",
                                    thread_id
                                );
                                end_stuck_session(self, thread_id).await;
                            }
                        }
                    }
                }
                None => {
                    if let Some(&thread_id) = branch_to_thread.get(branch) {
                        log!("[Recovery] Ending stuck session for thread {} — branch {} not found in any repo", thread_id, branch);
                        end_stuck_session(self, thread_id).await;
                    }
                }
            }
        }

        let mut recovering_threads: std::collections::HashSet<uuid::Uuid> =
            std::collections::HashSet::new();
        let mut cleaned_up: usize = 0;

        for (wt_path, branch_name, marker_repo_id, repo_root) in to_recover {
            if pending_branches.contains(&branch_name) {
                if actively_running_branches.contains(&branch_name) {
                    // Session was actively running at shutdown — resume it. The
                    // pending change (from a prior idle in this thread) stays put;
                    // the resumed session's next idle will propose_change, which
                    // dedupes by branch and updates this row's metadata in place.
                    log!("[Recovery] Resuming active session with pending change for branch {} (preserved)", branch_name);
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
            let is_external = marker_repo_id.is_some();
            if idle_branches.contains(&branch_name) {
                log!(
                    "[Recovery] Cleaning up stale worktree {} — session was idle before restart",
                    wt_path.display()
                );
                let tid = branch_to_thread.get(&branch_name).copied();
                self.safe_cleanup_worktree(&wt_path, &branch_name, &repo_root, tid, is_external)
                    .await;
                cleaned_up += 1;
                continue;
            }
            if completed_change_branches.contains(&branch_name) {
                log!("[Recovery] Cleaning up stale worktree {} — change already applied/discarded for branch {}", wt_path.display(), branch_name);
                let tid = branch_to_thread.get(&branch_name).copied();
                self.safe_cleanup_worktree(&wt_path, &branch_name, &repo_root, tid, is_external)
                    .await;
                cleaned_up += 1;
                continue;
            }
            if !branch_to_thread.contains_key(&branch_name)
                && !actively_running_branches.contains(&branch_name)
            {
                log!("[Recovery] Cleaning up stale worktree {} — no original thread found for branch {} (likely from a previous DB context)", wt_path.display(), branch_name);
                self.safe_cleanup_worktree(&wt_path, &branch_name, &repo_root, None, is_external)
                    .await;
                cleaned_up += 1;
                continue;
            }
            // Git-level check: if the branch has no commits ahead of main and no
            // uncommitted changes, it may be already fully merged. This catches
            // sessions the SQL idle_branches query misses — e.g., when an
            // Apply-time hardening session emitted SessionStarted after
            // CodingAgentIdled (making the SQL think it's non-idle), but the
            // branch was actually done.
            //
            // EXCEPTION: if the session was actively running (last lifecycle event
            // is SessionStarted, not CodingAgentIdled/SessionEnded), it MUST be
            // resumed even with no git changes — it was killed before producing
            // any output.
            let branch_has_changes = {
                let has_commits = match git_cmd(&["log", "main..HEAD", "--oneline"], &wt_path).await
                {
                    Ok(o) if o.status.success() => {
                        let stdout = String::from_utf8_lossy(&o.stdout);
                        !stdout.trim().is_empty()
                    }
                    _ => true, // If git fails, assume there are changes (safer)
                };
                let has_uncommitted = match git_cmd(&["status", "--porcelain"], &wt_path).await {
                    Ok(o) if o.status.success() => {
                        let stdout = String::from_utf8_lossy(&o.stdout);
                        !stdout.trim().is_empty()
                    }
                    _ => true,
                };
                has_commits || has_uncommitted
            };
            if !branch_has_changes {
                if actively_running_branches.contains(&branch_name) {
                    log!("[Recovery] Resuming worktree {} — branch {} has no git changes but session was actively running", wt_path.display(), branch_name);
                } else {
                    log!("[Recovery] Cleaning up stale worktree {} — branch {} already merged to main (no diff)", wt_path.display(), branch_name);
                    let tid = branch_to_thread.get(&branch_name).copied();
                    self.safe_cleanup_worktree(
                        &wt_path,
                        &branch_name,
                        &repo_root,
                        tid,
                        is_external,
                    )
                    .await;
                    cleaned_up += 1;
                    continue;
                }
            }
            // Find the original thread for this branch — reuse it instead of
            // creating a new recovery thread.
            let thread_id = match branch_to_thread.get(&branch_name) {
                Some(&tid) => {
                    log!(
                        "[Recovery] Reusing original thread {} for branch {}",
                        tid,
                        branch_name
                    );
                    tid
                }
                None => {
                    log!(
                        "[Recovery] No original thread found for branch {} — creating new thread",
                        branch_name
                    );
                    Uuid::new_v4()
                }
            };

            // Prevent duplicate recovery for the same thread (e.g., two branches
            // mapping to the same thread_id from stale resume retries).
            if !recovering_threads.insert(thread_id) {
                log!("[Recovery] Skipping duplicate recovery for thread {} (branch {}) — already recovering", thread_id, branch_name);
                cleanup_stale_worktree(&wt_path, &branch_name, &repo_root).await;
                cleaned_up += 1;
                continue;
            }

            // Look up the cc_session_id from the most recent idle event so the
            // synthetic CodingAgentIdled below carries it forward — when the
            // user later clicks "continue", the spawn dispatcher's
            // SpawnRequest::Continue handler resolves the same sid via
            // `lookup_latest_cc_session_id` and passes it to `--resume`.
            let cc_session_id: Option<String> = match sqlx::query_scalar(
                "SELECT payload->>'cc_session_id' FROM events \
                 WHERE thread_id = $1 AND event_type = 'CodingAgentIdled' \
                   AND payload->>'cc_session_id' IS NOT NULL \
                 ORDER BY sequence DESC LIMIT 1",
            )
            .bind(thread_id)
            .fetch_optional(self.pool())
            .await
            {
                Ok(sid) => sid.filter(|s: &String| !s.is_empty()),
                Err(e) => {
                    log!(
                        "[Recovery] Failed to look up cc_session_id for thread {}: {}",
                        thread_id,
                        e
                    );
                    None
                }
            };

            let is_external_repo = marker_repo_id.is_some();
            // Phase 5.3: do NOT auto-spawn CC for mid-turn-crashed sessions.
            // Surface the interruption as a synthetic CodingAgentIdled with
            // `reason = engine_restart_interrupt` so the UI can render a
            // "continue?" affordance. The user's click POSTs to
            // /api/threads/<id>/continue, which emits ContinueSignal — the
            // spawn dispatcher then re-enters CC via `--resume` against the
            // recorded `cc_session_id`. The worktree stays on disk: the
            // dispatcher resolves it on next spawn.
            log!("[Recovery] Surfacing interrupted CC session for user-driven continue: {} (branch {}, thread {}{}, cc_session: {})",
                wt_path.display(), branch_name, thread_id,
                marker_repo_id.as_ref().map(|r| format!(", repo {}", r)).unwrap_or_default(),
                cc_session_id.as_deref().unwrap_or("none"));

            // Compute requires_restart from the branch's actual files so the
            // Apply button shows the correct label even before CC re-enters.
            let requires_restart = proposal_files_for_branch(&repo_root, &branch_name)
                .await
                .map(|files| files_require_restart(&files))
                .unwrap_or(false);
            let has_changes = pending_branches.contains(&branch_name);

            let meta = crate::engine::thread_events::EventMeta {
                channel: Some(EventChannel::CodingAgent),
                ..crate::engine::thread_events::EventMeta::NONE
            };
            self.event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
                            has_changes,
                            is_external_repo,
                            requires_restart,
                            cc_session_id,
                            agent: crate::runtime::AgentKind::ClaudeCode,
                            reason: Some(ENGINE_RESTART_INTERRUPT_REASON.to_string()),
                            worktree_path: Some(wt_path.to_string_lossy().into_owned()),
                            // Snapshot the worktree's HEAD so the next spawn
                            // can detect external edits the user made while
                            // the engine was down. Best-effort — failures
                            // (e.g. branch with zero commits) yield None.
                            worktree_head_sha:
                                crate::engine::agent_session::external_edits_for_recovery_head_sha(&wt_path).await,
                        },
                        meta,
                    },
                    "[Recovery] CodingAgentIdled (engine_restart_interrupt)",
                )
                .await;
        }

        if cleaned_up > 0 {
            log!("[Recovery] Cleaned up {} stale worktrees", cleaned_up);
        }

        recovering_threads.into_iter().collect()
    }

}

/// Re-emit `CodingAgentPermissionResolved` for every persisted
/// `CodingAgentPermissionRequest` that has no paired resolution.
///
/// `pending_cc_permission` lives only in memory, so any CC subprocess that
/// was blocked on a permission card before an engine restart is dead — the
/// MCP HTTP request was severed, the in-memory waiter is gone, and any
/// subsequent click on the (still rendered) PermissionCard hits a 404 from
/// `submit_mcp_consent`. Emitting the resolution clears the card buttons
/// (the projection flips status from `waiting_for_user_answer` back to
/// `running`); the follow-up `orphan running → idle` reset in `main.rs`
/// then settles the thread to `idle` since no live CC owns it.
pub async fn recover_orphan_cc_permission_requests(
    pool: &sqlx::PgPool,
    event_bus: &crate::engine::event_bus::EventBus,
) {
    let rows: Vec<(Uuid, String)> = match sqlx::query_as(
        "SELECT e.thread_id, e.payload->>'request_id' AS request_id \
         FROM events e \
         WHERE e.event_type = 'CodingAgentPermissionRequest' \
           AND e.thread_id IS NOT NULL \
           AND NOT EXISTS ( \
             SELECT 1 FROM events r \
             WHERE r.event_type = 'CodingAgentPermissionResolved' \
               AND r.payload->>'request_id' = e.payload->>'request_id' \
           )",
    )
    .fetch_all(pool)
    .await
    {
        Ok(r) => r,
        Err(e) => {
            log!("[Recovery] orphan CC permission query failed: {}", e);
            return;
        }
    };

    if rows.is_empty() {
        return;
    }

    log!(
        "[Recovery] Auto-resolving {} orphan CC permission request(s) (CC subprocess gone after restart)",
        rows.len()
    );

    for (thread_id, request_id) in rows {
        event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::CodingAgentPermissionResolved {
                        request_id,
                        allowed: false,
                        reason: Some(
                            "Coding agent terminated before answering — request expired".to_string(),
                        ),
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                "[Recovery] CodingAgentPermissionResolved (orphan)",
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::path::PathBuf;

    /// Groups branch sets used to filter recovery candidates.
    #[derive(Default)]
    pub(crate) struct RecoveryFilter {
        pub pending: HashSet<String>,
        pub already_recovered: HashSet<String>,
        pub idle: HashSet<String>,
        pub merged: HashSet<String>,
        pub actively_running: HashSet<String>,
        pub completed_change: HashSet<String>,
        pub known: HashSet<String>,
        pub discovered: HashSet<String>,
    }

    /// Simulates the recovery filtering logic from `recover_orphaned_worktrees`.
    /// Returns the branches that would be recovered (not skipped).
    pub(crate) fn filter_recovery_candidates(
        candidates: &[(PathBuf, String)],
        f: &RecoveryFilter,
    ) -> Vec<String> {
        candidates
            .iter()
            .filter(|(_, branch)| {
                (!f.pending.contains(branch) || f.actively_running.contains(branch))
                    && !f.already_recovered.contains(branch)
                    && !f.idle.contains(branch)
                    && !f.completed_change.contains(branch)
                    && (!f.merged.contains(branch) || f.actively_running.contains(branch))
                    && (f.known.contains(branch) || f.actively_running.contains(branch))
            })
            .map(|(_, branch)| branch.clone())
            .collect()
    }

    /// Identifies actively running branches whose worktrees were lost (not found
    /// by the worktree scan). These need fresh worktrees created for recovery.
    /// Returns branches that should get new worktrees.
    pub(crate) fn find_worktreeless_active_branches(f: &RecoveryFilter) -> Vec<String> {
        f.actively_running
            .iter()
            .filter(|branch| {
                !f.discovered.contains(*branch)
                    && !f.already_recovered.contains(*branch)
                    && !f.completed_change.contains(*branch)
                    && !f.idle.contains(*branch)
            })
            .cloned()
            .collect()
    }

    #[test]
    fn lost_worktree_active_session_discovered_from_db() {
        // Bug: A CC session was actively running when the engine restarted.
        // The worktree directory was cleaned up (macOS temp cleanup, git prune)
        // before the next engine started. The worktree scan finds nothing, but
        // the DB shows the session was actively running (last lifecycle event
        // is SessionStarted with no subsequent idle/end).
        // Without DB-based discovery, the session is silently lost.
        let result = find_worktreeless_active_branches(&RecoveryFilter {
            actively_running: HashSet::from(["claude-code/lost-worktree-active".to_string()]),
            ..Default::default()
        });
        assert_eq!(
            result,
            vec!["claude-code/lost-worktree-active".to_string()],
            "Actively running session with lost worktree must be discovered from DB"
        );
    }

    #[test]
    fn lost_worktree_already_discovered_not_duplicated() {
        // Branch already found by worktree scan — don't create a second worktree.
        let result = find_worktreeless_active_branches(&RecoveryFilter {
            discovered: HashSet::from(["claude-code/has-worktree".to_string()]),
            actively_running: HashSet::from(["claude-code/has-worktree".to_string()]),
            ..Default::default()
        });
        assert!(
            result.is_empty(),
            "Branch already discovered via worktree scan must not get a duplicate worktree"
        );
    }

    #[test]
    fn lost_worktree_already_recovered_skipped() {
        let result = find_worktreeless_active_branches(&RecoveryFilter {
            actively_running: HashSet::from(["claude-code/recovered-before".to_string()]),
            already_recovered: HashSet::from(["claude-code/recovered-before".to_string()]),
            ..Default::default()
        });
        assert!(
            result.is_empty(),
            "Branch already recovered must not be re-discovered"
        );
    }

    #[test]
    fn lost_worktree_completed_change_skipped() {
        let result = find_worktreeless_active_branches(&RecoveryFilter {
            actively_running: HashSet::from(["claude-code/done".to_string()]),
            completed_change: HashSet::from(["claude-code/done".to_string()]),
            ..Default::default()
        });
        assert!(
            result.is_empty(),
            "Branch with completed change must not be re-discovered"
        );
    }

    #[test]
    fn lost_worktree_mixed_scenario() {
        // Multiple branches with lost worktrees — mix of states.
        let result = find_worktreeless_active_branches(&RecoveryFilter {
            discovered: HashSet::from(["claude-code/has-worktree".to_string()]),
            actively_running: HashSet::from([
                "claude-code/has-worktree".to_string(),
                "claude-code/needs-recovery".to_string(),
                "claude-code/already-done".to_string(),
            ]),
            completed_change: HashSet::from(["claude-code/already-done".to_string()]),
            ..Default::default()
        });
        assert_eq!(
            result,
            vec!["claude-code/needs-recovery".to_string()],
            "Only the lost-worktree branch needing recovery should be returned"
        );
    }

    #[test]
    fn idle_sessions_are_skipped_during_recovery() {
        let candidates = vec![
            (
                PathBuf::from("/tmp/wt-1"),
                "claude-code/20260317-100000".to_string(),
            ),
            (
                PathBuf::from("/tmp/wt-2"),
                "claude-code/20260317-110000".to_string(),
            ),
            (
                PathBuf::from("/tmp/wt-3"),
                "claude-code/20260317-120000".to_string(),
            ),
        ];

        // wt-2 was idle before restart — should be skipped
        let result = filter_recovery_candidates(
            &candidates,
            &RecoveryFilter {
                idle: HashSet::from(["claude-code/20260317-110000".to_string()]),
                known: HashSet::from([
                    "claude-code/20260317-100000".to_string(),
                    "claude-code/20260317-110000".to_string(),
                    "claude-code/20260317-120000".to_string(),
                ]),
                ..Default::default()
            },
        );
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"claude-code/20260317-100000".to_string()));
        assert!(result.contains(&"claude-code/20260317-120000".to_string()));
        assert!(!result.contains(&"claude-code/20260317-110000".to_string()));
    }

    #[test]
    fn all_skip_conditions_work_together() {
        let candidates = vec![
            (PathBuf::from("/tmp/wt-a"), "claude-code/a".to_string()), // has pending change
            (PathBuf::from("/tmp/wt-b"), "claude-code/b".to_string()), // already recovered
            (PathBuf::from("/tmp/wt-c"), "claude-code/c".to_string()), // was idle
            (PathBuf::from("/tmp/wt-d"), "claude-code/d".to_string()), // needs recovery
            (PathBuf::from("/tmp/wt-e"), "claude-code/e".to_string()), // already merged
        ];

        let result = filter_recovery_candidates(
            &candidates,
            &RecoveryFilter {
                pending: HashSet::from(["claude-code/a".to_string()]),
                already_recovered: HashSet::from(["claude-code/b".to_string()]),
                idle: HashSet::from(["claude-code/c".to_string()]),
                merged: HashSet::from(["claude-code/e".to_string()]),
                known: HashSet::from([
                    "claude-code/a".to_string(),
                    "claude-code/b".to_string(),
                    "claude-code/c".to_string(),
                    "claude-code/d".to_string(),
                    "claude-code/e".to_string(),
                ]),
                ..Default::default()
            },
        );
        assert_eq!(result, vec!["claude-code/d".to_string()]);
    }

    #[test]
    fn no_candidates_means_no_recovery() {
        let result = filter_recovery_candidates(&[], &RecoveryFilter::default());
        assert!(result.is_empty());
    }

    #[test]
    fn all_idle_means_no_recovery() {
        let candidates = vec![
            (PathBuf::from("/tmp/wt-1"), "claude-code/x".to_string()),
            (PathBuf::from("/tmp/wt-2"), "claude-code/y".to_string()),
        ];
        let result = filter_recovery_candidates(
            &candidates,
            &RecoveryFilter {
                idle: HashSet::from(["claude-code/x".to_string(), "claude-code/y".to_string()]),
                known: HashSet::from(["claude-code/x".to_string(), "claude-code/y".to_string()]),
                ..Default::default()
            },
        );
        assert!(result.is_empty());
    }

    #[test]
    fn merged_branch_skipped_during_recovery() {
        // A session whose branch was already merged to main (e.g., changes applied,
        // then an Apply-time hardening session emitted SessionStarted after
        // CodingAgentIdled, then engine restarted). The SQL idle_branches check
        // misses this because SessionStarted after CodingAgentIdled makes it
        // look non-idle. But git shows no diff vs main.
        let candidates = vec![
            (
                PathBuf::from("/tmp/wt-1"),
                "claude-code/already-merged".to_string(),
            ),
            (
                PathBuf::from("/tmp/wt-2"),
                "claude-code/has-changes".to_string(),
            ),
        ];

        let merged = HashSet::from(["claude-code/already-merged".to_string()]);
        let known = HashSet::from([
            "claude-code/already-merged".to_string(),
            "claude-code/has-changes".to_string(),
        ]);

        let result = filter_recovery_candidates(
            &candidates,
            &RecoveryFilter {
                merged,
                known,
                ..Default::default()
            },
        );
        assert_eq!(result, vec!["claude-code/has-changes".to_string()]);
    }

    #[test]
    fn completed_change_branches_skipped_during_recovery() {
        // Branches with applied or discarded changes should not be re-recovered.
        // The original session completed and proposed changes that were resolved.
        let candidates = vec![
            (
                PathBuf::from("/tmp/wt-1"),
                "claude-code/applied".to_string(),
            ),
            (
                PathBuf::from("/tmp/wt-2"),
                "claude-code/discarded".to_string(),
            ),
            (
                PathBuf::from("/tmp/wt-3"),
                "claude-code/needs-recovery".to_string(),
            ),
        ];
        let completed = HashSet::from([
            "claude-code/applied".to_string(),
            "claude-code/discarded".to_string(),
        ]);
        let known = HashSet::from([
            "claude-code/applied".to_string(),
            "claude-code/discarded".to_string(),
            "claude-code/needs-recovery".to_string(),
        ]);

        let result = filter_recovery_candidates(
            &candidates,
            &RecoveryFilter {
                completed_change: completed,
                known,
                ..Default::default()
            },
        );
        assert_eq!(result, vec!["claude-code/needs-recovery".to_string()]);
    }

    #[test]
    fn unknown_branches_skipped_unless_actively_running() {
        // Branches with no original thread in the DB (from a reset DB context)
        // should be skipped. Only actively running sessions are recovered.
        let candidates = vec![
            (
                PathBuf::from("/tmp/wt-1"),
                "claude-code/unknown-idle".to_string(),
            ),
            (
                PathBuf::from("/tmp/wt-2"),
                "claude-code/unknown-running".to_string(),
            ),
            (PathBuf::from("/tmp/wt-3"), "claude-code/known".to_string()),
        ];
        let actively_running = HashSet::from(["claude-code/unknown-running".to_string()]);
        let known = HashSet::from(["claude-code/known".to_string()]);

        let result = filter_recovery_candidates(
            &candidates,
            &RecoveryFilter {
                actively_running,
                known,
                ..Default::default()
            },
        );
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"claude-code/unknown-running".to_string()));
        assert!(result.contains(&"claude-code/known".to_string()));
        // unknown-idle is skipped — no DB record, not running
        assert!(!result.contains(&"claude-code/unknown-idle".to_string()));
    }

    #[test]
    fn resumed_after_idle_is_not_treated_as_idle() {
        // A session that idled, then resumed working (user sent follow-up),
        // then died mid-work should NOT be in idle_branches.
        // The SQL query excludes sessions with activity after last idle —
        // including MessageReceived (user follow-up from chat.rs),
        // CodingAgentPromptSent (automated or user audit trail from CC event loop),
        // CodingAgentUserMessageSent, CodingAgentToolCalled, CodingAgentTextStreamed.
        // This test validates the filtering logic: if the SQL correctly
        // excludes such sessions from idle_branches, they get recovered.
        let candidates = vec![
            (
                PathBuf::from("/tmp/wt-1"),
                "claude-code/truly-idle".to_string(),
            ),
            (
                PathBuf::from("/tmp/wt-2"),
                "claude-code/resumed-then-died".to_string(),
            ),
        ];

        // Only the truly idle session should be in idle_branches.
        // The resumed-then-died session has CC activity after its last idle
        // event, so the SQL query excludes it from idle_branches.
        let result = filter_recovery_candidates(
            &candidates,
            &RecoveryFilter {
                idle: HashSet::from(["claude-code/truly-idle".to_string()]),
                known: HashSet::from([
                    "claude-code/truly-idle".to_string(),
                    "claude-code/resumed-then-died".to_string(),
                ]),
                ..Default::default()
            },
        );
        assert_eq!(result, vec!["claude-code/resumed-then-died".to_string()]);
    }

    #[test]
    fn restarted_after_idle_is_not_treated_as_idle() {
        // A session that idled, then had a new SessionStarted (e.g., an
        // Apply-time hardening session), then the process died should NOT be
        // in idle_branches. The SQL NOT EXISTS clause must include SessionStarted
        // so that a new session start after CodingAgentIdled marks it as non-idle.
        // Bug: without SessionStarted in the NOT EXISTS check, the idle query
        // incorrectly classifies this as idle and skips recovery.
        let candidates = vec![
            (
                PathBuf::from("/tmp/wt-1"),
                "claude-code/truly-idle".to_string(),
            ),
            (
                PathBuf::from("/tmp/wt-2"),
                "claude-code/rehardened-then-died".to_string(),
            ),
        ];
        // The rehardened-then-died session has a SessionStarted after its last
        // CodingAgentIdled, so the SQL query must exclude it from idle_branches.
        let result = filter_recovery_candidates(
            &candidates,
            &RecoveryFilter {
                idle: HashSet::from(["claude-code/truly-idle".to_string()]),
                known: HashSet::from([
                    "claude-code/truly-idle".to_string(),
                    "claude-code/rehardened-then-died".to_string(),
                ]),
                ..Default::default()
            },
        );
        assert_eq!(result, vec!["claude-code/rehardened-then-died".to_string()]);
    }

    #[test]
    fn actively_running_session_with_pending_change_still_recovered() {
        // Contract: actively-running CC sessions are recovered AND their pending
        // change is preserved. The resumed session's next idle dedupes via
        // propose_change updating the existing row in place.
        let candidates = vec![
            (
                PathBuf::from("/tmp/wt-1"),
                "claude-code/active-with-pending".to_string(),
            ),
            (
                PathBuf::from("/tmp/wt-2"),
                "claude-code/idle-with-pending".to_string(),
            ),
            (
                PathBuf::from("/tmp/wt-3"),
                "claude-code/active-no-pending".to_string(),
            ),
        ];

        let pending = HashSet::from([
            "claude-code/active-with-pending".to_string(),
            "claude-code/idle-with-pending".to_string(),
        ]);

        let actively_running = HashSet::from([
            "claude-code/active-with-pending".to_string(),
            "claude-code/active-no-pending".to_string(),
        ]);

        let known = HashSet::from([
            "claude-code/active-with-pending".to_string(),
            "claude-code/idle-with-pending".to_string(),
            "claude-code/active-no-pending".to_string(),
        ]);

        let result = filter_recovery_candidates(
            &candidates,
            &RecoveryFilter {
                pending,
                actively_running,
                known,
                ..Default::default()
            },
        );

        // wt-1: actively running + pending → recover (pending preserved by recovery loop)
        // wt-2: idle + pending → skip (user reviews the change at their leisure)
        // wt-3: actively running + no pending → recover normally
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"claude-code/active-with-pending".to_string()));
        assert!(result.contains(&"claude-code/active-no-pending".to_string()));
        assert!(!result.contains(&"claude-code/idle-with-pending".to_string()));
    }

    #[test]
    fn followup_before_cc_output_not_treated_as_idle() {
        // Bug fix: user sends a follow-up to an idle CC session. chat.rs emits
        // MessageReceived and routes the message to CC via msg_tx. The CC event
        // loop picks it up and emits CodingAgentPromptSent. But the engine restarts
        // BEFORE CC produces any tool calls or text output.
        //
        // Old behavior: the SQL idle_branches query only checked for
        // CodingAgentUserMessageSent (never emitted!), CodingAgentToolCalled, and
        // CodingAgentTextStreamed. With none of those present, the session was
        // incorrectly classified as idle and the worktree was cleaned up — silently
        // losing the user's follow-up.
        //
        // Fix: the SQL now also checks for MessageReceived and CodingAgentPromptSent.
        // Both are emitted during the follow-up flow, so the session is correctly
        // classified as active even before CC produces output.
        //
        // This test validates the filter logic side: if the SQL correctly excludes
        // the session from idle_branches (because MessageReceived exists after idle),
        // the filter recovers it.
        let candidates = vec![
            (
                PathBuf::from("/tmp/wt-1"),
                "claude-code/truly-idle".to_string(),
            ),
            (
                PathBuf::from("/tmp/wt-2"),
                "claude-code/followup-no-output".to_string(),
            ),
        ];

        // The SQL correctly excluded followup-no-output from idle_branches
        // because MessageReceived exists after CodingAgentIdled.
        let result = filter_recovery_candidates(
            &candidates,
            &RecoveryFilter {
                idle: HashSet::from(["claude-code/truly-idle".to_string()]),
                actively_running: HashSet::from(["claude-code/followup-no-output".to_string()]),
                known: HashSet::from([
                    "claude-code/truly-idle".to_string(),
                    "claude-code/followup-no-output".to_string(),
                ]),
                ..Default::default()
            },
        );
        assert_eq!(
            result,
            vec!["claude-code/followup-no-output".to_string()],
            "Session with user follow-up (MessageReceived after idle) but no CC output yet \
             must be recovered — the follow-up would otherwise be silently lost"
        );
    }

    #[test]
    fn idled_then_resumed_session_with_pending_change_still_recovered() {
        // Real-world scenario: "Execute and Fix E2E Tests" thread.
        // 1. Session ran, idled (CodingAgentIdled)
        // 2. User sent follow-up ("yeah nice run it now")
        // 3. CC resumed (tool calls for running e2e tests)
        // 4. Engine killed mid-work → ChangeProposed auto-emitted
        //
        // The session is NOT in idle_branches (CC activity after idle).
        // The session IS in actively_running_branches (post-idle CC activity
        // detected by the UNION clause in the SQL query).
        // The session HAS a pending change.
        // → Must bypass the pending check and be recovered.
        //
        // Note: this test validates the filter logic. The SQL query that
        // populates actively_running_branches handles the idle→resumed
        // detection via a UNION clause checking for activity events
        // (MessageReceived, CodingAgentPromptSent, CodingAgentToolCalled, etc.)
        // after the last CodingAgentIdled.
        let candidates = vec![
            (
                PathBuf::from("/tmp/wt-1"),
                "claude-code/idled-resumed-pending".to_string(),
            ),
            (
                PathBuf::from("/tmp/wt-2"),
                "claude-code/truly-idle-pending".to_string(),
            ),
        ];

        let result = filter_recovery_candidates(
            &candidates,
            &RecoveryFilter {
                pending: HashSet::from([
                    "claude-code/idled-resumed-pending".to_string(),
                    "claude-code/truly-idle-pending".to_string(),
                ]),
                actively_running: HashSet::from(["claude-code/idled-resumed-pending".to_string()]),
                idle: HashSet::from(["claude-code/truly-idle-pending".to_string()]),
                known: HashSet::from([
                    "claude-code/idled-resumed-pending".to_string(),
                    "claude-code/truly-idle-pending".to_string(),
                ]),
                ..Default::default()
            },
        );

        assert_eq!(
            result,
            vec!["claude-code/idled-resumed-pending".to_string()],
            "Session that idled, resumed via follow-up, then was killed must be recovered \
             even with a pending change — the pending change is stale"
        );
    }

    #[test]
    fn recovery_reuses_original_thread_when_available() {
        use std::collections::HashMap;
        use uuid::Uuid;

        let original_thread = Uuid::new_v4();
        let branch_to_thread: HashMap<String, Uuid> =
            HashMap::from([("claude-code/20260317-100000".to_string(), original_thread)]);

        // Branch with known thread → reuse original
        let branch = "claude-code/20260317-100000";
        let thread_id = branch_to_thread
            .get(branch)
            .copied()
            .unwrap_or_else(Uuid::new_v4);
        assert_eq!(thread_id, original_thread);

        // Branch without known thread → creates new UUID
        let unknown_branch = "claude-code/20260317-999999";
        let thread_id2 = branch_to_thread
            .get(unknown_branch)
            .copied()
            .unwrap_or_else(Uuid::new_v4);
        assert_ne!(thread_id2, original_thread);
    }

    #[test]
    fn interrupted_session_without_idle_event_gets_recovered() {
        // During shutdown, CodingAgentIdled is suppressed (shutting_down flag).
        // A session that was actively working when the engine restarted will
        // NOT have CodingAgentIdled as its last event → NOT in idle_branches
        // → recover_orphaned_worktrees picks it up for recovery.
        let candidates = vec![
            (
                PathBuf::from("/tmp/wt-idle"),
                "claude-code/genuinely-idle".to_string(),
            ),
            (
                PathBuf::from("/tmp/wt-interrupted"),
                "claude-code/interrupted-mid-work".to_string(),
            ),
        ];

        // Only the genuinely idle session has CodingAgentIdled.
        // The interrupted session's CodingAgentIdled was suppressed during shutdown,
        // so it's NOT in idle_branches.
        let result = filter_recovery_candidates(
            &candidates,
            &RecoveryFilter {
                idle: HashSet::from(["claude-code/genuinely-idle".to_string()]),
                known: HashSet::from([
                    "claude-code/genuinely-idle".to_string(),
                    "claude-code/interrupted-mid-work".to_string(),
                ]),
                ..Default::default()
            },
        );
        assert_eq!(
            result,
            vec!["claude-code/interrupted-mid-work".to_string()],
            "Session interrupted during shutdown (no CodingAgentIdled) must be recovered"
        );
    }

    #[test]
    fn completed_session_with_response_generated_skips_recovery() {
        // Some CC sessions (recovery, Apply-time hardening) complete with
        // ResponseGenerated but never emit CodingAgentIdled. Without
        // ResponseGenerated in the idle query, these sessions were incorrectly
        // recovered on every restart.
        let candidates = vec![
            (
                PathBuf::from("/tmp/wt-1"),
                "claude-code/completed-no-idle".to_string(),
            ),
            (
                PathBuf::from("/tmp/wt-2"),
                "claude-code/actually-interrupted".to_string(),
            ),
        ];

        let result = filter_recovery_candidates(
            &candidates,
            &RecoveryFilter {
                idle: HashSet::from(["claude-code/completed-no-idle".to_string()]),
                known: HashSet::from([
                    "claude-code/completed-no-idle".to_string(),
                    "claude-code/actually-interrupted".to_string(),
                ]),
                ..Default::default()
            },
        );
        assert_eq!(
            result,
            vec!["claude-code/actually-interrupted".to_string()],
            "Session completed with ResponseGenerated (no CodingAgentIdled) must NOT be recovered"
        );
    }

    #[test]
    fn running_session_with_no_git_changes_still_recovered() {
        // Bug: A CC session killed mid-work before producing any git changes
        // was incorrectly skipped by the git-level "already merged" check.
        // The session had SessionStarted as its last lifecycle event (not idle),
        // and the idle_branches SQL correctly identified it as non-idle,
        // but the git check saw no commits and no uncommitted changes and
        // skipped it. These sessions MUST be resumed — they just hadn't
        // produced any output yet before the engine restarted.
        let candidates = vec![
            (
                PathBuf::from("/tmp/wt-1"),
                "claude-code/no-changes-but-running".to_string(),
            ),
            (
                PathBuf::from("/tmp/wt-2"),
                "claude-code/truly-merged".to_string(),
            ),
        ];

        let result = filter_recovery_candidates(
            &candidates,
            &RecoveryFilter {
                merged: HashSet::from([
                    "claude-code/no-changes-but-running".to_string(),
                    "claude-code/truly-merged".to_string(),
                ]),
                actively_running: HashSet::from(["claude-code/no-changes-but-running".to_string()]),
                known: HashSet::from([
                    "claude-code/no-changes-but-running".to_string(),
                    "claude-code/truly-merged".to_string(),
                ]),
                ..Default::default()
            },
        );

        // The actively running session must be recovered despite no git changes.
        // The truly merged session (not actively running) is correctly skipped.
        assert_eq!(
            result,
            vec!["claude-code/no-changes-but-running".to_string()],
            "Running session with no git changes must be recovered, truly merged must be skipped"
        );
    }

    #[test]
    fn double_restart_incomplete_recovery_not_in_already_recovered() {
        // Bug: Double engine restart. Restart #1 emits SessionRecovered, starts CC
        // recovery. Restart #2 kills CC before it finishes. The already_recovered
        // SQL now checks that the latest SessionRecovered was followed by a
        // completion event (CodingAgentIdled, ResponseGenerated, SessionEnded).
        // With no completion → branch NOT in already_recovered → re-recovered.
        //
        // Event sequence:
        //   SessionStarted → ... → CodingAgentIdled → SessionRecovered → [killed]
        //
        // Expected sets after SQL queries on restart #2:
        //   already_recovered: empty (SessionRecovered not followed by completion)
        //   idle_branches: empty (SessionRecovered counts as activity after idle)
        //   actively_running_branches: contains branch (SessionRecovered after idle)
        let candidates = vec![(
            PathBuf::from("/tmp/wt-1"),
            "claude-code/double-restart".to_string(),
        )];

        let result = filter_recovery_candidates(
            &candidates,
            &RecoveryFilter {
                actively_running: HashSet::from(["claude-code/double-restart".to_string()]),
                known: HashSet::from(["claude-code/double-restart".to_string()]),
                ..Default::default()
            },
        );
        assert_eq!(
            result,
            vec!["claude-code/double-restart".to_string()],
            "Incomplete recovery (killed before completion) must be re-recovered on next restart"
        );
    }

    #[test]
    fn double_restart_completed_recovery_stays_in_already_recovered() {
        // Counterpart to the above: if recovery DID complete (SessionRecovered
        // followed by CodingAgentIdled), the branch IS in already_recovered
        // and should NOT be re-recovered.
        //
        // Event sequence:
        //   SessionStarted → ... → CodingAgentIdled → SessionRecovered →
        //   CodingAgentPromptSent → SessionStarted → ... → CodingAgentIdled
        //
        // Expected sets:
        //   already_recovered: contains branch (recovery completed)
        //   idle_branches: contains branch (CodingAgentIdled is last, no activity after)
        let candidates = vec![(
            PathBuf::from("/tmp/wt-1"),
            "claude-code/completed-recovery".to_string(),
        )];

        let result = filter_recovery_candidates(
            &candidates,
            &RecoveryFilter {
                already_recovered: HashSet::from(["claude-code/completed-recovery".to_string()]),
                idle: HashSet::from(["claude-code/completed-recovery".to_string()]),
                known: HashSet::from(["claude-code/completed-recovery".to_string()]),
                ..Default::default()
            },
        );
        assert!(
            result.is_empty(),
            "Completed recovery (followed by CodingAgentIdled) must NOT be re-recovered"
        );
    }

    #[test]
    fn double_restart_lost_worktree_incomplete_recovery_rediscovered() {
        // Double-restart variant where the worktree was also lost.
        // SessionRecovered emitted but not followed by completion →
        // NOT in already_recovered → branch rediscovered for fresh worktree.
        let result = find_worktreeless_active_branches(&RecoveryFilter {
            actively_running: HashSet::from(["claude-code/double-restart-lost".to_string()]),
            ..Default::default()
        });
        assert_eq!(
            result,
            vec!["claude-code/double-restart-lost".to_string()],
            "Lost worktree with incomplete recovery must get a fresh worktree"
        );
    }

    #[test]
    fn duplicate_repo_scan_does_not_duplicate_recovery() {
        use std::collections::HashMap;
        use uuid::Uuid;

        let thread_id = Uuid::new_v4();
        let branch_a = "claude-code/20260416-162934-stale".to_string();
        let branch_b = "claude-code/20260416-162942-real".to_string();

        let branch_to_thread: HashMap<String, Uuid> =
            HashMap::from([(branch_a.clone(), thread_id), (branch_b.clone(), thread_id)]);

        let candidates = vec![
            (PathBuf::from("/tmp/wt-1"), branch_a),
            (PathBuf::from("/tmp/wt-2"), branch_b),
        ];

        let result = filter_recovery_candidates(
            &candidates,
            &RecoveryFilter {
                known: HashSet::from([
                    "claude-code/20260416-162934-stale".to_string(),
                    "claude-code/20260416-162942-real".to_string(),
                ]),
                ..Default::default()
            },
        );
        assert_eq!(result.len(), 2, "Both branches pass branch-level filtering");

        let mut seen_threads: HashSet<Uuid> = HashSet::new();
        let mut recovered: Vec<String> = Vec::new();
        for (_, branch) in &candidates {
            let tid = branch_to_thread.get(branch).copied().unwrap();
            if seen_threads.insert(tid) {
                recovered.push(branch.clone());
            }
        }
        assert_eq!(
            recovered.len(),
            1,
            "Only one recovery per thread_id — duplicate must be skipped"
        );
    }

    /// Phase 5.3 contract: the constant the recovery path stamps on synthetic
    /// `CodingAgentIdled.reason` must be a stable string the frontend can
    /// match on. Any rename here is a frontend break — keep this test as the
    /// canary.
    #[test]
    fn engine_restart_interrupt_reason_constant_is_stable() {
        assert_eq!(super::ENGINE_RESTART_INTERRUPT_REASON, "engine_restart_interrupt");
    }

    /// Phase 5.3 contract: a synthetic `CodingAgentIdled` carrying
    /// `reason = engine_restart_interrupt` must round-trip through serde so
    /// the projection + SSE consumers see the reason field intact. Without
    /// this, the UI's "continue?" affordance never renders.
    #[test]
    fn coding_agent_idled_engine_restart_interrupt_roundtrips() {
        use crate::engine::thread_events::ThreadEvent;
        use crate::runtime::AgentKind;
        let event = ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: true,
            cc_session_id: Some("sid-abc".to_string()),
            agent: AgentKind::ClaudeCode,
            reason: Some(super::ENGINE_RESTART_INTERRUPT_REASON.to_string()),
            worktree_path: None,
            worktree_head_sha: None,
        };
        let json = serde_json::to_value(&event).expect("serializes");
        assert_eq!(json["type"], "CodingAgentIdled");
        assert_eq!(json["reason"], "engine_restart_interrupt");
        let back: ThreadEvent = serde_json::from_value(json).expect("deserializes");
        match back {
            ThreadEvent::CodingAgentIdled {
                reason,
                cc_session_id,
                has_changes,
                requires_restart,
                ..
            } => {
                assert_eq!(reason.as_deref(), Some("engine_restart_interrupt"));
                assert_eq!(cc_session_id.as_deref(), Some("sid-abc"));
                assert!(has_changes);
                assert!(requires_restart);
            }
            other => panic!("expected CodingAgentIdled, got {:?}", other),
        }
    }

    /// Most idles carry no reason — make sure that case still round-trips
    /// without leaking a `"reason": null` into the wire payload (the
    /// `skip_serializing_if = "Option::is_none"` attribute owns this).
    #[test]
    fn coding_agent_idled_without_reason_skips_field_in_wire_format() {
        use crate::engine::thread_events::ThreadEvent;
        use crate::runtime::AgentKind;
        let event = ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            agent: AgentKind::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
        };
        let json = serde_json::to_value(&event).expect("serializes");
        assert!(
            json.get("reason").is_none(),
            "reason: None must be skipped on the wire to keep payloads minimal: {}",
            json
        );
    }
}

/// Integration tests for the Phase 5.3 recovery contract. These need a real
/// Postgres connection; they run as part of `cargo test -p lucidos-engine` only
/// when the test pool is reachable.
#[cfg(test)]
mod integration_tests {
    use std::sync::Arc;
    use uuid::Uuid;

    use super::{ENGINE_RESTART_INTERRUPT_REASON, USER_CLICKED_CONTINUE_REASON};
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{
        EventChannel, EventMeta, ThreadEvent,
    };
    use crate::test_support::{setup_test_db, teardown_test_db};

    /// The Phase 5.3 contract for the continue endpoint: emitting a
    /// `ContinueSignal` on the CC channel persists with the user's reason tag,
    /// and the spawn dispatcher's classifier (subscribed to the same bus) will
    /// see it as a `SpawnTrigger::ContinueSignal`. This test exercises the
    /// "endpoint emits → bus receives → dispatcher classifies" chain without
    /// the full Axum router (which requires a complete engine).
    #[tokio::test]
    async fn continue_signal_emission_classifies_as_spawn_trigger() {
        use crate::engine::agent_session::spawn_dispatcher::{SpawnDispatcher, SpawnRequest};

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let bus = Arc::new(bus);
        let (tx, mut spawn_rx) = tokio::sync::mpsc::unbounded_channel::<SpawnRequest>();
        let dispatcher = SpawnDispatcher::new(pool.clone(), bus.clone(), tx);
        let handle = tokio::spawn(async move { dispatcher.run().await });
        // Let the run loop subscribe before producing.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let thread_id = Uuid::new_v4();
        // Seed SessionStarted so the lifecycle contract accepts CC events.
        let cc_meta = EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        };
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SessionStarted {
                session_id: "sid-cont".into(),
                branch: "claude-code/cont".into(),
                repo_id: None,
            },
            meta: cc_meta.clone(),
        })
        .await
        .expect("session start emits")
        .expect("event persisted");

        // What the continue endpoint does:
        let res = bus
            .emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ContinueSignal {
                    reason: USER_CLICKED_CONTINUE_REASON.to_string(),
                },
                meta: cc_meta,
            })
            .await
            .expect("emit succeeds")
            .expect("event persisted");

        let received = tokio::time::timeout(std::time::Duration::from_secs(2), spawn_rx.recv())
            .await
            .expect("dispatcher must produce a SpawnRequest within 2s")
            .expect("channel must yield");
        assert_eq!(
            received,
            SpawnRequest::Continue {
                thread_id,
                event_id: res.event_id,
            },
            "ContinueSignal from the continue endpoint must produce SpawnRequest::Continue"
        );

        handle.abort();
        pool.close().await;
        teardown_test_db(&db_name).await;
    }

    /// Phase 5.3: when recovery emits a synthetic `CodingAgentIdled` with
    /// `reason = engine_restart_interrupt`, the spawn dispatcher's classifier
    /// must NOT treat it as a trigger. Without this guarantee, the very
    /// "interrupted" event we emit to surface the continue affordance would
    /// loop back into auto-spawning CC — exactly the behavior we removed.
    #[tokio::test]
    async fn synthetic_idled_with_engine_restart_interrupt_does_not_dispatch() {
        use crate::engine::agent_session::spawn_dispatcher::{SpawnDispatcher, SpawnRequest};
        use std::sync::atomic::Ordering;

        let (pool, db_name) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let bus = Arc::new(bus);
        let (tx, _spawn_rx) = tokio::sync::mpsc::unbounded_channel::<SpawnRequest>();
        let dispatcher = SpawnDispatcher::new(pool.clone(), bus.clone(), tx);
        let dispatch_count = dispatcher.dispatch_count.clone();
        let handle = tokio::spawn(async move { dispatcher.run().await });
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let thread_id = Uuid::new_v4();
        let cc_meta = EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        };
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::SessionStarted {
                session_id: "sid-int".into(),
                branch: "claude-code/int".into(),
                repo_id: None,
            },
            meta: cc_meta.clone(),
        })
        .await
        .expect("emit succeeds")
        .expect("persisted");

        // Simulate recovery surfacing the interrupt:
        bus.emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::CodingAgentIdled {
                has_changes: true,
                is_external_repo: false,
                requires_restart: false,
                cc_session_id: Some("sid-int".into()),
                agent: crate::runtime::AgentKind::ClaudeCode,
                reason: Some(ENGINE_RESTART_INTERRUPT_REASON.to_string()),
                worktree_path: None,
                worktree_head_sha: None,
            },
            meta: cc_meta,
        })
        .await
        .expect("emit succeeds")
        .expect("persisted");

        // Give the dispatcher time to (NOT) act.
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert_eq!(
            dispatch_count.load(Ordering::SeqCst),
            0,
            "synthetic CodingAgentIdled must not produce any dispatch — only the user's continue click does"
        );

        handle.abort();
        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
