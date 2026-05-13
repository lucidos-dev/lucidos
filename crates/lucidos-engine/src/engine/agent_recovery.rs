use super::agent_session::change_description_fallback;
use super::agent_session::resume::deterministic_worktree_path;
use super::change_ops::branch_is_hardened;
use super::claude_code::{WORKTREE_EXCLUDE_PATHS, WORKTREE_WORKSPACE_MARKER};
use super::git_ops::{
    add_paths_to_worktree_exclude, auto_commit_worktree, default_local_branch,
    describe_branch_changes, files_require_restart, find_worktree_for_branch, git_cmd,
    is_external_repo_path, main_worktree, proposal_files_for_branch, worktree_add,
    worktrees_dir,
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

/// Tag stamped on `ContinueSignal.reason` when the user answers an
/// `AskUserQuestion` after the CC subprocess has been torn down at idle.
/// `notify()` is a no-op in that window, so this signal makes the spawn
/// dispatcher boot a fresh `--resume` subprocess; the resumed CC re-runs
/// the hook, which reads the persisted answer from the DB.
pub const ANSWERED_AFTER_IDLE_REASON: &str = "answered_after_idle";

/// Should the SpawnConsumer's `Continue` handler emit `ContinuationStarted`
/// for a `ContinueSignal` carrying this `reason`?
///
/// `ContinuationStarted` opens a new "Resumed after engine restart" exchange
/// in the timeline. That label is only honest when the continuation is in
/// response to an actual mid-turn engine restart (i.e. the user clicked the
/// Continue button). For `answered_after_idle` the user answered an
/// `AskUserQuestion` after CC's subprocess was torn down at idle — the
/// follow-up CC events should attach to the existing `UserQuestionAsked`
/// exchange instead of being mislabeled as a recovery.
///
/// Default-deny on unknown / missing reasons: a future `ContinueSignal`
/// reason must opt-in explicitly rather than inheriting a "Resumed after
/// engine restart" boundary by accident.
pub fn continue_should_open_resume_exchange(reason: Option<&str>) -> bool {
    reason == Some(USER_CLICKED_CONTINUE_REASON)
}

/// User message the spawn consumer hands to `run_direct_agent` when actuating
/// a `SpawnRequest::Continue`. **Must be non-empty.**
///
/// `claude --print --resume` reads stdin in stream-json mode and waits
/// indefinitely for at least one input line before emitting its `system/init`
/// event. The engine keeps the input channel open across the session lifetime,
/// so EOF never arrives on its own — without an explicit input, CC parks
/// forever, `events_rx.recv()` never resolves, and the thread sits "Running"
/// until the next engine restart tears the subprocess down.
///
/// The string mirrors the placeholder CC itself injects on `--resume` of an
/// unfinished tool_use (see `agent_session/run_session.rs` and
/// `agent_session/reconstruct.rs`), so CC ingests it as a plain user turn and
/// proceeds against the resumed conversation state. The richer recovery
/// payload (system-prompt override, pending-merge context, etc.) will replace
/// this call site later; until then this constant guarantees the non-empty
/// stdin precondition.
pub const CONTINUE_RESUME_USER_MESSAGE: &str = "Continue from where you left off.";

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
        let repo_root_str = repo_root.to_string_lossy();
        // Marker survives worktree removal (keyed by repo_root + branch_name),
        // so this lookup works even when the cleanup worker has already removed
        // the worktree under us. Without this check, propose_change downgrades
        // hardened=true → false and Apply re-runs `/harden` on already-hardened
        // work.
        let hardened = branch_is_hardened(self.pool(), self.changes(), repo_root, branch_name).await;
        self.propose_change(crate::engine::change_ops::ProposeChangeInput {
            thread_id,
            branch_name,
            repo_root: &repo_root_str,
            description: &description,
            files: changed_files,
            requires_restart,
            channel: EventChannel::CodingAgent,
            hardened,
            origin,
            // Engine-driven recovery has no information about whether the
            // pre-restart turn ended cleanly; the per-turn idle path is the
            // only place where failure status is known. Keep `false` here so
            // recovered changes don't spuriously trigger the apply warning.
            incomplete: false,
        })
        .await
    }

    /// Handle ending a stale waiting CC session (no live process) after engine restart.
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
                // No SessionStarted event at all — this thread never had a CC session.
                // Return an error instead of emitting SessionEnded, which would pollute
                // regular chat threads with CC-only events (causes spurious "Session ended
                // automatically" banners in the UI).
                return Err("No Claude Code session found for this thread".into());
            }
            Some(_) => {
                // SessionStarted exists but branch is empty — stale CC session with no branch.
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
            // User explicitly chose "Discard & End Session" on a thread with no
            // live CC subprocess. Plumb the user's actor through to the
            // resulting ChangeDiscarded events so the chat chip reads "You"
            // instead of falling back to the engine label.
            log!(
                "[ClaudeCode] Discarding stale session changes (branch {})",
                branch_name
            );
            self.discard_pending_for_thread(thread_id, actor.clone()).await;
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

        // No auto-apply / auto-discard tail: any pending change on the branch
        // is left for the user to resolve from Review. The Apply Now flow
        // (`apply_now`) chains its own POST /changes/<id>/apply once the
        // proposal lands via SSE, so we don't need to fuse propose+apply here.

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

                    let wt_path = lost_session_worktree_path(
                        self.workspace_path(),
                        branch_to_thread.get(branch).copied(),
                    );
                    // The deterministic `thread-<short>` path collides with any
                    // partial-setup leftover (worktree_add succeeded but marker
                    // write crashed, etc.) — the worktree scan would have skipped
                    // it as not-discovered, but `git worktree add` refuses to
                    // create into an existing dir. Clear it before retrying. The
                    // random `cc-<uuid>` branch can't collide, but the cost is
                    // identical and keeps the call site uniform.
                    if matches!(tokio::fs::try_exists(&wt_path).await, Ok(true)) {
                        if let Err(e) = tokio::fs::remove_dir_all(&wt_path).await {
                            log!(
                                "[Recovery] Failed to clear stale worktree dir {}: {}",
                                wt_path.display(),
                                e
                            );
                        }
                    }

                    match worktree_add(&repo_root, &wt_path, &[branch]).await {
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
                            // Add engine-injected paths to the worktree's git exclude
                            // so external repos don't see them as untracked or
                            // accidentally commit them.
                            add_paths_to_worktree_exclude(&wt_path, WORKTREE_EXCLUDE_PATHS).await;
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
            // Skip non-in-flight branches so applied/idle threads don't
            // surface a misleading "Continue?" affordance. Pending change
            // counts as in-flight — CC was awaiting Apply when the engine
            // died.
            let in_flight = !idle_branches.contains(&branch_name)
                && !completed_change_branches.contains(&branch_name);
            let has_pending_change = pending_branches.contains(&branch_name);
            if !in_flight && !has_pending_change {
                log!(
                    "[Recovery] Skipping clean worktree {} — branch {} has no in-flight signal; cleanup worker will reclaim",
                    wt_path.display(),
                    branch_name
                );
                continue;
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

            let is_external_repo = is_external_repo_path(&repo_root, &lucidos_repo_root);
            // Phase 5.3: do NOT auto-spawn CC for mid-turn-crashed sessions.
            // Surface the interruption as a synthetic CodingAgentIdled with
            // `reason = engine_restart_interrupt` so the UI can render a
            // "continue?" affordance. The user's click POSTs to
            // /api/threads/<id>/continue, which emits ContinueSignal — the
            // spawn dispatcher then re-enters CC via `--resume` against the
            // recorded `cc_session_id`. The worktree stays on disk: the
            // dispatcher resolves it on next spawn, and the cleanup worker's
            // Tier 0 won't reclaim it until the thread reaches a terminal
            // idle state with no pending change.
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
            // Emit the boundary `ResponseAborted` FIRST so the UI shows the
            // "Response interrupted" panel above the synthetic Idled. The
            // dispatcher classifies on `CodingAgentIdled.reason`, so order
            // doesn't affect spawn decisions.
            //
            // Idempotency: `/api/restart` pre-emits a `ResponseAborted{actor:
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
                        &["MessageReceived", "CodingAgentUserMessageSent", "TriggerStarted"],
                    )
                    .await;
                let abort_meta = crate::engine::thread_events::EventMeta {
                    channel: Some(EventChannel::CodingAgent),
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
                        persist_scope: None,
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                "[Recovery] CodingAgentPermissionResolved (orphan)",
            )
            .await;
    }
}

/// CC keys its session JSONL by CWD, so a resumed subprocess must boot in the
/// same path its prior turn used. Reuse the thread's deterministic path when
/// we have a mapping; fall back to `cc-<random>` only for orphan branches with
/// no thread to resume against.
fn lost_session_worktree_path(
    workspace_path: &Path,
    branch_thread_id: Option<Uuid>,
) -> PathBuf {
    match branch_thread_id {
        Some(thread_id) => deterministic_worktree_path(workspace_path, thread_id),
        None => {
            let wt_id = Uuid::new_v4().as_simple().to_string();
            worktrees_dir(workspace_path).join(format!("cc-{}", wt_id))
        }
    }
}
#[cfg(test)]
#[path = "agent_recovery_tests.rs"]
mod tests;

/// Integration tests for the Phase 5.3 recovery contract. These need a real
/// Postgres connection; they run as part of `cargo test -p lucidos-engine` only
/// when the test pool is reachable.
#[cfg(test)]
#[path = "agent_recovery_integration_tests.rs"]
mod integration_tests;
