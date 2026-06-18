use crate::engine::change_ops::{branch_is_hardened, now_epoch_millis};
use crate::engine::git_ops::{
    auto_commit_preserving_marker, auto_commit_safe_files_if_dirty, auto_commit_worktree,
    branch_changed_files, catchup_and_ff_to_main, commits_in_range, consume_harden_marker,
    consume_plan_marker, default_local_branch, describe_branch_changes, files_have_client_update,
    files_require_restart, git_cmd, has_branch_commits, push_main_in_background,
};
use crate::engine::thread_events::{EventChannel, MessageOrigin};
use crate::engine::{AgentUserInput, LucidosEngine};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

impl LucidosEngine {
    /// Apply Now: keep the existing Claude Code session alive and use it for review/conflict resolution.
    /// Only kills CC after the merge succeeds. Falls back to stale session handling if no live session.
    ///
    /// Runs as a background task (tokio::spawn). Steps:
    /// 1. Auto-commit in worktree
    /// 2. Review if needed (send follow-up to existing CC)
    /// 3. Propose change
    /// 4. Try merge (clean / trivial / CC-assisted conflict resolution)
    /// 5. On success: kill CC + clean up. On failure: CC stays alive for retry.
    pub async fn apply_now(
        self: &Arc<Self>,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Extract session metadata — if no live session, fall back to stale handling
        let (worktree_path, branch_name, repo_root, idle_notify, msg_tx, last_event_at) = {
            let mut guard = self.agent_sessions.lock().await;
            match guard.get_mut(&thread_id) {
                Some(session)
                    if session.worktree_path.is_some()
                        && session.branch_name.is_some()
                        && session.repo_root.is_some() =>
                {
                    if session.apply_now_in_progress {
                        return Err("Apply is already in progress for this thread".into());
                    }
                    session.apply_now_in_progress = true;
                    (
                        session.worktree_path.clone().unwrap(),
                        session.branch_name.clone().unwrap(),
                        session.repo_root.clone().unwrap(),
                        session.idle_notify.clone(),
                        session.msg_tx.clone(),
                        session.last_event_at.clone(),
                    )
                }
                _ => {
                    drop(guard);

                    // Fast path: apply existing pending change directly when
                    // there's no live agent session. Coding-agent sessions
                    // cleanly exit at idle (Phase 5.3), so "no live session" is the
                    // expected post-clean-idle state for any thread the user
                    // returns to apply later — not a signal that the prior
                    // turn died mid-edit. The fast path skips the
                    // worktree-scan + describe + propose_change round-trip
                    // that `end_stale_waiting_session` would otherwise
                    // perform: when a clean pending row already exists, the
                    // recovery sweep would just re-emit a duplicate
                    // `ChangeProposed` with the same payload, producing a
                    // noise event in the timeline. Skipping it keeps the
                    // history clean. (`propose_branch_changes` now derives
                    // `incomplete` from the prior terminal event via
                    // `last_turn_ended_cleanly`, so the recovery path no
                    // longer flips clean rows to incomplete=true — but the
                    // fast path is still worth keeping for the no-op-emit
                    // reason.) See the regression test
                    // `apply_now_no_live_session_fast_path_preserves_clean_pending_change`.
                    let pending = self.changes().pending_for_thread(thread_id).await?;
                    if !pending.is_empty() {
                        log!(
                            "[ApplyNow] No live session for thread {} but {} pending change(s) — applying directly without stale-recovery",
                            thread_id,
                            pending.len()
                        );
                        for change in pending {
                            log!(
                                "[ApplyNow] Applying pending change {} on resumed thread {}",
                                change.id,
                                thread_id
                            );
                            if let Err(e) = self.apply_change(change.id, actor.clone()).await {
                                log!("[ApplyNow] apply_change for {} failed: {}", change.id, e);
                                self.emit_apply_failed(
                                    thread_id,
                                    change.id,
                                    &e.to_string(),
                                    actor.clone(),
                                )
                                .await;
                            }
                        }
                        return Ok(());
                    }

                    log!(
                        "[ApplyNow] No live session and no pending change for thread {} — running stale-session recovery to propose then apply",
                        thread_id
                    );
                    // Stale fallback: no pending change row exists — propose
                    // any uncommitted/committed work on the branch, then
                    // explicitly apply it. The apply is stamped with the
                    // clicking user's actor, so the resulting ChangeApplied
                    // chip reads "You" — not the engine.
                    self.end_stale_waiting_session(thread_id, false, actor.clone())
                        .await?;
                    let pending = self.changes().pending_for_thread(thread_id).await?;
                    // Guarantee a terminal event so the frontend's `applyingNow`
                    // spinner always resolves. Without this, a stale fallback that
                    // proposes nothing (branch had no real commits) would hang
                    // the spinner indefinitely.
                    if pending.is_empty() {
                        self.event_bus
                            .emit_or_log(
                                crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event: crate::engine::thread_events::ThreadEvent::ChangeApplyFailed {
                                        change_id: String::new(),
                                        error: "No changes to apply — branch is already merged or has no commits".to_string(),
                                        actor: actor.clone(),
                                    },
                                    meta: crate::engine::thread_events::EventMeta::NONE,
                                },
                                "[ApplyNow] ChangeApplyFailed (stale fallback, no pending)",
                            )
                            .await;
                        return Ok(());
                    }
                    for change in pending {
                        log!(
                            "[ApplyNow] Applying user-requested change {} on stale-session thread {}",
                            change.id,
                            thread_id
                        );
                        if let Err(e) = self.apply_change(change.id, actor.clone()).await {
                            log!("[ApplyNow] apply_change for {} failed: {}", change.id, e);
                            self.emit_apply_failed(
                                thread_id,
                                change.id,
                                &e.to_string(),
                                actor.clone(),
                            )
                            .await;
                        }
                    }
                    return Ok(());
                }
            }
        };

        let engine = self.clone_arc();
        tokio::spawn(async move {
            // Use std::panic::catch_unwind via FutureExt to guarantee cleanup on panic.
            // tokio::spawn swallows panics — without this, apply_now_in_progress stays
            // stuck forever if apply_now_inner panics.
            let panic_result = std::panic::AssertUnwindSafe(async {
                // Liveness-based timeout: abort only if CC hasn't emitted any
                // events for 10 minutes (not wall-clock — active sessions can
                // run as long as they keep producing output).
                let inactivity_limit_ms: i64 = 600_000;
                let inner_fut = engine.apply_now_inner(
                    thread_id,
                    &worktree_path,
                    &branch_name,
                    &repo_root,
                    &idle_notify,
                    &msg_tx,
                    actor.clone(),
                );
                tokio::pin!(inner_fut);

                loop {
                    match tokio::time::timeout(Duration::from_secs(30), &mut inner_fut).await {
                        Ok(result) => break result,
                        Err(_) => {
                            // Check liveness: has CC emitted an event recently?
                            let last_ms = last_event_at.load(std::sync::atomic::Ordering::Relaxed);
                            let idle_ms = now_epoch_millis() - last_ms;
                            if idle_ms > inactivity_limit_ms {
                                break Err(format!(
                                    "Apply timed out — no CC activity for {} minutes",
                                    idle_ms / 60_000,
                                )
                                .into());
                            }
                            // Still alive — keep waiting
                        }
                    }
                }
            });

            let result: Result<(), Box<dyn std::error::Error + Send + Sync>> =
                match futures::FutureExt::catch_unwind(panic_result).await {
                    Ok(inner) => inner,
                    Err(_) => Err("apply_now_inner panicked".into()),
                };

            // Always clear the in-progress flag — runs after normal completion,
            // timeout, error, or panic.
            {
                let mut guard = engine.agent_sessions.lock().await;
                if let Some(session) = guard.get_mut(&thread_id) {
                    session.apply_now_in_progress = false;
                }
            }

            if let Err(e) = result {
                log!("[ApplyNow] Failed for thread {}: {}", thread_id, e);
                // Emit ChangeApplyFailed so frontend clears the "Applying..." state
                engine
                    .event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ChangeApplyFailed {
                                change_id: String::new(),
                                error: format!("Apply failed: {}", e),
                                actor: actor.clone(),
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        },
                        "[ApplyNow] ChangeApplyFailed",
                    )
                    .await;
            }
        });

        Ok(())
    }

    /// Wait for CC to go idle, polling every 5s for process exit.
    /// Returns Ok when CC fires idle_notify, or Err if the process dies.
    pub(crate) async fn wait_for_idle(
        &self,
        thread_id: Uuid,
        idle_notify: &tokio::sync::Notify,
        context: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        loop {
            // Register waiter BEFORE entering the timeout so that a
            // notify_waiters() call between iterations isn't lost.
            // (notify_waiters doesn't store a permit — it only wakes
            // futures that are already registered.)
            let notified = idle_notify.notified();
            match tokio::time::timeout(std::time::Duration::from_secs(5), notified).await {
                Ok(()) => return Ok(()),
                Err(_) => {
                    let guard = self.agent_sessions.lock().await;
                    if guard
                        .get(&thread_id)
                        .map(|s| s.process_exited)
                        .unwrap_or(true)
                    {
                        return Err(format!("Coding agent session ended while {}", context).into());
                    }
                }
            }
        }
    }

    /// Send a follow-up message to CC, wait for it to go idle, check it's alive,
    /// then auto-commit any changes it made.
    pub(crate) async fn send_and_wait(
        &self,
        thread_id: Uuid,
        msg_tx: &tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
        idle_notify: &tokio::sync::Notify,
        worktree_path: &Path,
        message: &str,
        context: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        log!("[ApplyNow] {} — sending follow-up to CC", context);
        msg_tx
            .send(AgentUserInput {
                text: message.to_string(),
                images: None,
                origin_event_id: None,
                kind: crate::engine::AgentInputKind::User,
            })
            .map_err(|_| -> Box<dyn std::error::Error + Send + Sync> {
                "Session channel closed".into()
            })?;
        self.wait_and_commit(thread_id, idle_notify, worktree_path, context)
            .await
    }

    /// Wait for CC to go idle, then auto-commit any changes it made.
    ///
    /// Propagates `git add` / `git commit` failures via `Err`. A silent failure
    /// here would lose a real CC change — the apply-now caller treats the
    /// returned `Ok` as proof the iteration produced a committed snapshot
    /// before moving on to the next step (hardening, tests, merge).
    pub(crate) async fn wait_and_commit(
        &self,
        thread_id: Uuid,
        idle_notify: &tokio::sync::Notify,
        worktree_path: &Path,
        context: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.wait_for_idle(thread_id, idle_notify, context).await?;

        crate::engine::git_ops::commit_worktree_or_err(
            worktree_path,
            &format!("Coding agent changes ({})", context),
        )
        .await
        .map(|_| ())
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
            format!("wait_and_commit ({}): {}", context, e).into()
        })
    }

    /// Inner implementation for apply_now — runs in a background task.
    #[allow(clippy::too_many_arguments)]
    async fn apply_now_inner(
        self: &Arc<Self>,
        thread_id: Uuid,
        worktree_path: &Path,
        branch_name: &str,
        repo_root: &Path,
        idle_notify: &tokio::sync::Notify,
        msg_tx: &tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::engine::event_bus::BusEvent;
        use crate::engine::thread_events::{EventMeta, ThreadEvent};

        // Step 1: Auto-commit in worktree (preserves harden marker if fresh)
        auto_commit_preserving_marker(
            &self.pool,
            worktree_path,
            repo_root,
            branch_name,
            "Coding agent changes (auto-committed)",
        )
        .await;

        if !branch_is_hardened(&self.pool, self.changes(), repo_root, branch_name).await {
            self.request_hardening_in_session(thread_id, msg_tx)
                .await?;
            self.wait_and_commit(
                thread_id,
                idle_notify,
                worktree_path,
                "waiting for hardening",
            )
            .await?;

            // Step 2b: Run tests after hardening to verify changes didn't break anything
            self.send_and_wait(
                thread_id,
                msg_tx,
                idle_notify,
                worktree_path,
                "Hardening is done. Now run the test suite to verify nothing is broken: \
                `cargo test -p lucidos-engine` and `cd crates/lucidos-app && npm test`. \
                If any tests fail, fix them before proceeding.",
                "waiting for post-hardening tests",
            )
            .await?;

            // A canceled `/harden` reaches `wait_and_commit`'s idle state too,
            // so the marker is the proof — not the wait returning Ok.
            if !branch_is_hardened(&self.pool, self.changes(), repo_root, branch_name).await {
                log!(
                    "[ApplyNow] Hardening session ended without writing marker for branch {} — aborting apply",
                    branch_name
                );
                self.emit_apply_failed_unhardened(
                    thread_id,
                    "",
                    actor.clone(),
                    "[ApplyNow] ChangeApplyFailed (incomplete hardening)",
                )
                .await;
                self.reset_worktree_and_idle(thread_id, worktree_path).await;
                return Ok(());
            }
        }

        // Step 3: Check for commits
        let has_commits = has_branch_commits(repo_root, branch_name).await;

        if !has_commits {
            // No changes to apply — branch is already merged or empty
            log!(
                "[ApplyNow] No commits on branch {} — nothing to apply",
                branch_name
            );
            // Emit ChangeApplyFailed so frontend clears the "Applying..." state
            self.event_bus
                .emit_or_log(
                    BusEvent::Thread {
                        thread_id,
                        event: ThreadEvent::ChangeApplyFailed {
                            change_id: String::new(),
                            error: "No changes to apply — branch is already merged".to_string(),
                            actor: actor.clone(),
                        },
                        meta: EventMeta::NONE,
                    },
                    "[ApplyNow] ChangeApplyFailed",
                )
                .await;
            self.reset_worktree_and_idle(thread_id, worktree_path).await;
            return Ok(());
        }

        // Step 4: Propose change
        let changed_files = branch_changed_files(repo_root, branch_name).await;
        let requires_restart = files_require_restart(&changed_files);
        let base = default_local_branch(repo_root).await;
        let log_range = format!("{}..{}", base, branch_name);
        let description =
            describe_branch_changes(repo_root, &log_range, "Applied changes", None).await;

        // Hardened: we just ran hardening or it was already hardened.
        let repo_root_str = repo_root.to_string_lossy();
        let change_id = self
            .propose_change(crate::engine::change_ops::ProposeChangeInput {
                thread_id,
                branch_name,
                repo_root: &repo_root_str,
                description: &description,
                files: &changed_files,
                requires_restart,
                channel: EventChannel::ClaudeCode,
                hardened: true, // apply_now ensures hardening before this point
                // Apply-now propose is part of a live agent flow — origin is
                // carried by the surrounding MessageReceived.
                origin: None,
                // Apply-now reaches this point only when the user clicked
                // Apply on a clean idle — never from a failed turn.
                incomplete: false,
            })
            .await?;

        // Step 5: Check main repo for uncommitted changes
        self.commit_dirty_logged("Coding agent changes", "apply_now auto-commit")
            .await;
        if auto_commit_safe_files_if_dirty(repo_root).await {
            let msg = "Cannot merge: the repository has uncommitted changes. Commit or stash them first, then try again.";
            self.emit_apply_failed(thread_id, change_id, msg, actor.clone())
                .await;
            // CC stays alive for retry
            return Ok(());
        }

        // Step 6: Merge main into CC worktree and ff main to the branch
        match self
            .merge_via_cc_session(
                thread_id,
                change_id,
                worktree_path,
                branch_name,
                repo_root,
                idle_notify,
                msg_tx,
            )
            .await
        {
            Ok((pre_sha, post_sha)) => {
                let client_update = files_have_client_update(&changed_files);
                self.apply_now_success(
                    thread_id,
                    change_id,
                    requires_restart,
                    client_update,
                    &pre_sha,
                    &post_sha,
                    worktree_path,
                    repo_root,
                    branch_name,
                    actor.clone(),
                )
                .await;
            }
            Err(e) => {
                self.emit_apply_failed(thread_id, change_id, &e.to_string(), actor.clone())
                    .await;
                // CC stays alive for retry
            }
        }

        Ok(())
    }

    /// Merge main into a CC worktree and fast-forward main to the branch.
    /// Fast path: try ff directly. If main diverged, send CC a single prompt
    /// to merge, resolve conflicts, harden, and test — then ff again.
    /// Returns (pre_sha, post_sha) on success.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn merge_via_cc_session(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        worktree_path: &Path,
        branch_name: &str,
        repo_root: &Path,
        idle_notify: &tokio::sync::Notify,
        msg_tx: &tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
    ) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
        // Fast path: if branch already includes main, just ff
        if let Ok(shas) = catchup_and_ff_to_main(repo_root, worktree_path, branch_name).await {
            log!("[MergeViaCC] Fast path succeeded for {}", branch_name);
            return Ok(shas);
        }

        // Main has diverged — CC handles the merge. Files are populated by a
        // probe merge so the panel can list which files actually conflict
        // (empty list = clean merge that just needs a merge commit).
        let conflict_files = probe_merge_conflicts(worktree_path).await;
        log!(
            "[MergeViaCC] Fast path failed — delegating merge to CC for {} ({} conflicting file(s))",
            branch_name,
            conflict_files.len()
        );
        let prompt = self
            .start_merge_and_get_prompt(thread_id, change_id, conflict_files, "main", None, None)
            .await;

        if let Err(e) = msg_tx.send(AgentUserInput {
            text: prompt,
            images: None,
            origin_event_id: None,
            kind: crate::engine::AgentInputKind::User,
        }) {
            return Err(format!(
                "Failed to send merge prompt to the coding-agent session — receiver gone: {}",
                e
            )
            .into());
        }

        if let Err(e) = self
            .wait_for_idle(
                thread_id,
                idle_notify,
                "merging main and resolving conflicts",
            )
            .await
        {
            let _ = git_cmd(&["merge", "--abort"], worktree_path).await;
            return Err(e);
        }

        if !crate::engine::agent_recovery::last_turn_ended_cleanly(&self.pool, thread_id).await
        {
            let _ = git_cmd(&["merge", "--abort"], worktree_path).await;
            return Err(
                "Conflict resolution did not finish cleanly — merge aborted. The change is still pending; try applying again.".into(),
            );
        }

        // Verify CC completed the merge
        let main_merged = git_cmd(
            &["merge-base", "--is-ancestor", "main", branch_name],
            repo_root,
        )
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
        if !main_merged {
            return Err(
                "Coding agent session ended without completing the merge. Try applying again.".into(),
            );
        }

        // Auto-commit any leftover changes
        auto_commit_worktree(
            worktree_path,
            "Coding agent changes (post-merge auto-commit)",
        )
        .await;

        catchup_and_ff_to_main(repo_root, worktree_path, branch_name)
            .await
            .map_err(|e| format!("ff-merge to main failed after CC merge: {}", e).into())
    }

    /// Helper: kill Claude Code session and clean up after a successful apply.
    /// Returns the commit subjects that were merged so the caller can surface
    /// them in the API response without re-running `git log`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn apply_now_success(
        self: &Arc<Self>,
        thread_id: Uuid,
        change_id: Uuid,
        requires_restart: bool,
        client_update: bool,
        pre_sha: &str,
        post_sha: &str,
        worktree_path: &Path,
        repo_root: &Path,
        branch_name: &str,
        actor: Option<MessageOrigin>,
    ) -> Vec<String> {
        let commits = commits_in_range(repo_root, pre_sha, post_sha).await;
        // Title is best-effort metadata for the ChangeApplied event payload —
        // a DB lookup error shouldn't block the apply that just succeeded.
        let thread_title = sqlx::query_scalar::<_, String>(
            "SELECT title FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(&self.pool)
        .await
        .unwrap_or_else(|e| {
            log!(
                "[ApplyNow] Failed to load thread title for {}: {}",
                thread_id,
                e
            );
            None
        });
        self.emit_change_applied(
            thread_id,
            change_id,
            requires_restart,
            client_update,
            commits.clone(),
            thread_title,
            actor.clone(),
            Some(pre_sha.to_string()),
            Some(post_sha.to_string()),
        )
        .await;
        // Refresh entity caches (apps, artifacts) for SSE subscribers — the
        // CC worktree wrote files directly into `data/apps/<id>/...` /
        // `data/artifacts/...`, so the only signal the frontend gets is
        // ChangeApplied unless we ladder up to per-entity events here. See
        // `emit_entity_events_for_change_apply` for the detection rules.
        //
        // A silent miss here resurrects the exact bug this path was added to
        // fix ("no app with id" after Apply when CC created the app), so log
        // any DB failure or missing-row case so it's greppable in
        // `[ApplyNow]` traces.
        match self.changes().get_by_id(change_id).await {
            Ok(Some(change)) => {
                // App coding-agent thread: reload open iframes of this app
                // so the user sees the merged CSS/JS/HTML immediately. The
                // sibling apply paths in `change_ops::apply_change` all
                // pair `maybe_emit_app_ui_refresh` next to `emit_change_applied`
                // — this in-CC in-place merge path used to skip it, leaving
                // the iframe stale after a same-thread Apply.
                let kind_ctx = crate::engine::change_ops::load_apply_kind_context(
                    &self.pool,
                    Some(thread_id),
                )
                .await;
                self.maybe_emit_app_ui_refresh(&kind_ctx, &change.files, actor.as_ref())
                    .await;
                self.emit_entity_events_for_change_apply(
                    &change.files,
                    Some(pre_sha),
                    Some(post_sha),
                    actor,
                )
                .await;
            }
            Ok(None) => {
                log!(
                    "[ApplyNow] entity-event emission skipped — change {} not found post-apply",
                    change_id
                );
            }
            Err(e) => {
                log!(
                    "[ApplyNow] entity-event emission skipped — get_by_id({}) failed: {}",
                    change_id,
                    e
                );
            }
        }

        consume_harden_marker(&self.pool, repo_root, branch_name).await;
        // The worktree/branch is reset for reuse below — clear the Planned
        // marker too so the next round of work on this branch re-triggers the
        // plan gate (a new planning decision for new work), mirroring the
        // harden-marker reset.
        consume_plan_marker(&self.pool, repo_root, branch_name).await;
        self.reset_worktree_and_idle(thread_id, worktree_path).await;
        push_main_in_background(repo_root);
        self.broadcast_changes_updated().await;

        commits
    }

    /// Reset worktree to main and re-enter idle state.
    /// Used after apply, discard, and no-commits to keep the session alive.
    pub(crate) async fn reset_worktree_and_idle(&self, thread_id: Uuid, worktree_path: &Path) {
        use crate::engine::event_bus::BusEvent;
        use crate::engine::thread_events::{EventMeta, ThreadEvent};

        let _ = git_cmd(&["reset", "--hard", "main"], worktree_path).await;
        let _ = git_cmd(&["clean", "-fd"], worktree_path).await;

        let cc_sid = {
            let mut sessions = self.agent_sessions.lock().await;
            if let Some(s) = sessions.get_mut(&thread_id) {
                s.is_waiting = true;
                s.has_changes = false;
                s.requires_restart = false;
                s.idle_notify.notify_waiters();
            }
            sessions
                .get(&thread_id)
                .and_then(|s| s.cc_session_id.clone())
        };

        let coding_agent = self.thread_coding_agent(thread_id).await;
        self.event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::CodingAgentIdled {
                        has_changes: false,
                        requires_restart: false,
                        is_external_repo: false,
                        cc_session_id: cc_sid,
                        coding_agent,
                        reason: None,
                        worktree_path: Some(worktree_path.to_string_lossy().into_owned()),
                        // Apply-now exits the loop with the worktree at the
                        // post-apply state (branch reset to main HEAD).
                        // Recording the SHA on the next real idle is enough;
                        // here we leave it None so legacy-deserialize stays
                        // the canonical "no recorded SHA" sentinel.
                        worktree_head_sha: None,
                        bg_bash_pending: false,
                    },
                    meta: EventMeta::NONE,
                },
                "[ApplyNow] CodingAgentIdled",
            )
            .await;
    }
}

/// Probe `main` against HEAD to detect merge conflicts without touching the
/// worktree. Returns the list of conflicting paths (empty for clean merges).
/// Uses `git merge-tree` (read-only, no index/working-tree mutation) so it's
/// safe to run alongside other operations in the same worktree.
pub(crate) async fn probe_merge_conflicts(worktree_path: &Path) -> Vec<String> {
    // `--name-only` (Git 2.40+) prints one conflicting path per line on stdout;
    // exit code 1 = conflicts present, 0 = clean. Anything else is unexpected.
    let out = match git_cmd(
        &["merge-tree", "--name-only", "HEAD", "main"],
        worktree_path,
    )
    .await
    {
        Ok(o) => o,
        Err(e) => {
            log!(
                "[MergeViaCC] probe_merge_conflicts: merge-tree failed: {}",
                e
            );
            return Vec::new();
        }
    };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Liveness-based timeout: a future that keeps emitting events should not
    /// be killed, but one that goes silent should time out.
    #[tokio::test]
    async fn liveness_timeout_does_not_fire_while_events_arrive() {
        use std::sync::atomic::{AtomicI64, Ordering};

        let last_event_at = Arc::new(AtomicI64::new(now_epoch_millis()));
        let last_event_clone = last_event_at.clone();

        // Simulate a long-running inner future that emits events every 50ms
        let inner = async move {
            for _ in 0..10 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                last_event_clone.store(now_epoch_millis(), Ordering::Relaxed);
            }
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        };
        tokio::pin!(inner);

        // Use a very short inactivity limit (200ms) — the future takes ~500ms
        // but events keep arriving so it should NOT time out
        let inactivity_limit_ms: i64 = 200;
        let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = loop {
            match tokio::time::timeout(std::time::Duration::from_millis(30), &mut inner).await {
                Ok(r) => break r,
                Err(_) => {
                    let last_ms = last_event_at.load(Ordering::Relaxed);
                    if now_epoch_millis() - last_ms > inactivity_limit_ms {
                        break Err("timed out".into());
                    }
                }
            }
        };

        assert!(
            result.is_ok(),
            "Should not time out when events keep arriving"
        );
    }

    /// Liveness-based timeout fires when CC stops emitting events.
    #[tokio::test]
    async fn liveness_timeout_fires_when_events_stop() {
        use std::sync::atomic::{AtomicI64, Ordering};

        // Set last_event_at to 1 second ago — already stale
        let last_event_at = Arc::new(AtomicI64::new(now_epoch_millis() - 1000));

        // Inner future that never completes (simulates stuck CC)
        let inner = async {
            tokio::time::sleep(std::time::Duration::from_secs(999)).await;
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        };
        tokio::pin!(inner);

        // Inactivity limit: 100ms (already exceeded since last_event_at is 1s ago)
        let inactivity_limit_ms: i64 = 100;
        let result: Result<(), Box<dyn std::error::Error + Send + Sync>> = loop {
            match tokio::time::timeout(std::time::Duration::from_millis(30), &mut inner).await {
                Ok(r) => break r,
                Err(_) => {
                    let last_ms = last_event_at.load(Ordering::Relaxed);
                    if now_epoch_millis() - last_ms > inactivity_limit_ms {
                        break Err("timed out".into());
                    }
                }
            }
        };

        assert!(
            result.is_err(),
            "Should time out when no events are arriving"
        );
        assert!(result.unwrap_err().to_string().contains("timed out"));
    }
}
