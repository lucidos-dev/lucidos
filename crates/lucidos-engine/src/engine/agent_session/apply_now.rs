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

/// Outcome of `begin_in_place_merge`'s atomic claim on a thread's live session.
pub(crate) enum InPlaceMergeStart {
    /// No live, non-exited session with a worktree — caller falls through to the
    /// dead-session / temp-worktree merge tiers.
    NoLiveSession,
    /// A merge is already running for this thread (`apply_now_in_progress`).
    AlreadyInProgress,
    /// Session claimed; `apply_now_in_progress` is now set. Caller owns clearing it.
    Ready(crate::engine::change_ops::LiveSessionInfo),
}

/// Pure decision core of `begin_in_place_merge` — split out so the claim state
/// machine is unit-testable without standing up a full engine. A session is
/// claimable for an in-place merge only when it exists, its process is alive,
/// it has a worktree, and no apply is already running on it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InPlaceMergeClaim {
    NoLiveSession,
    AlreadyInProgress,
    Claim,
}

pub(crate) fn decide_in_place_merge_claim(
    session: Option<&crate::engine::AgentSession>,
) -> InPlaceMergeClaim {
    match session {
        None => InPlaceMergeClaim::NoLiveSession,
        Some(s) if !s.is_live() || s.worktree_path.is_none() => InPlaceMergeClaim::NoLiveSession,
        Some(s) if s.apply_now_in_progress => InPlaceMergeClaim::AlreadyInProgress,
        Some(_) => InPlaceMergeClaim::Claim,
    }
}

/// Decide whether the apply-now CC-inactivity timeout should fire, and if so
/// how many whole minutes of silence to report.
///
/// The inactivity clock is anchored to the LATER of CC's last event
/// (`last_event_at_ms`) and apply-start (`apply_started_ms`). `last_event_at`
/// only advances when CC emits an event, so it can be many minutes stale if the
/// thread sat idle before the user clicked Apply (CC alive but quiet). Measuring
/// `now - last_event_at` directly therefore counts that *pre-apply* idle gap as
/// if it were apply inactivity, firing an instant false "no CC activity for N
/// minutes" on the very first poll for any thread idle longer than the limit —
/// and machine load worsens it, letting the first 30s tick beat CC's first
/// post-prompt event. Anchoring to apply-start makes the timer measure silence
/// *since apply began*: a freshly-started apply always gets the full limit
/// before it can trip, while genuine mid-apply silence (both timestamps stale)
/// still fires correctly.
///
/// Returns `Some(minutes_of_silence)` when the timeout should fire, else `None`.
pub(crate) fn apply_inactivity_timeout_minutes(
    now_ms: i64,
    last_event_at_ms: i64,
    apply_started_ms: i64,
    inactivity_limit_ms: i64,
) -> Option<i64> {
    let idle_since = last_event_at_ms.max(apply_started_ms);
    let idle_ms = now_ms - idle_since;
    if idle_ms > inactivity_limit_ms {
        Some(idle_ms / 60_000)
    } else {
        None
    }
}

/// Whether the in-session apply (`apply_now_inner`) must run the Lucidos-source
/// `/harden` + test flow before merging.
///
/// App coding-agent threads own their own hardening and never run the
/// Lucidos-source `/harden` (their worktree has no `cargo`/`tsc`/`scripts` and
/// their session prompt explicitly opts out) — so they ALWAYS skip, regardless
/// of marker state. This mirrors the `is_app()` harden-gate skip in
/// `change_ops::apply_change`; without it, clicking Apply on an app thread with
/// a live session wrongly injected "Run /harden now." + `cargo test -p
/// lucidos-engine` into the app session and then failed the apply with
/// "Hardening did not complete". Non-app (Lucidos-source / external) threads
/// gate on the harden marker as before.
pub(super) fn should_run_in_session_hardening(is_app: bool, branch_hardened: bool) -> bool {
    !is_app && !branch_hardened
}

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
                // `is_live` gates the claim for the same reason
                // `decide_in_place_merge_claim` does: driving a phantom (or an
                // exited) session means sending the review/merge prompt into a
                // dead `msg_tx` and failing the apply with "Session channel
                // closed", when falling through to the no-live-session path
                // below would have applied the pending change cleanly.
                Some(session)
                    if session.is_live()
                        && session.worktree_path.is_some()
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
                // run as long as they keep producing output). The clock is
                // anchored to apply-start (not just CC's last event) so a long
                // pre-apply idle gap can't trip an instant false timeout — see
                // `apply_inactivity_timeout_minutes`.
                let inactivity_limit_ms: i64 = 600_000;
                let apply_started_ms = now_epoch_millis();
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
                            if let Some(minutes) = apply_inactivity_timeout_minutes(
                                now_epoch_millis(),
                                last_ms,
                                apply_started_ms,
                                inactivity_limit_ms,
                            ) {
                                break Err(format!(
                                    "Apply timed out — no CC activity for {} minutes",
                                    minutes,
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
                    if guard.get(&thread_id).map(|s| !s.is_live()).unwrap_or(true) {
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

        // App threads skip the /harden + test gate entirely (apps own their
        // hardening). Probe the marker only for non-app threads — mirrors the
        // `is_app()` skip in `change_ops::apply_change`.
        let is_app =
            crate::engine::change_ops::load_apply_kind_context(&self.pool, Some(thread_id))
                .await
                .is_app();
        let branch_hardened =
            !is_app && branch_is_hardened(&self.pool, self.changes(), repo_root, branch_name).await;
        if should_run_in_session_hardening(is_app, branch_hardened) {
            self.request_hardening_in_session(thread_id, msg_tx).await?;
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
                // Do NOT reset the worktree here. The apply was refused, so the
                // change stays pending and the branch still holds every commit
                // the agent made. `reset_worktree_and_idle` would move that
                // branch ref to main and clean the tree, destroying the work
                // and leaving a pending change pointing at an empty branch
                // (recoverable only via reflog). Just return the session to
                // idle so the user can harden and retry.
                self.mark_session_idle(thread_id, worktree_path).await;
                return Ok(());
            }
        }

        // Step 3: Check for commits
        let has_commits = has_branch_commits(repo_root, branch_name).await;

        if !has_commits {
            // The branch has nothing ahead of main. That's NOT automatically a
            // failure: the work may already be on main out-of-band (a prior
            // apply fast-forwarded main, then was abandoned — exactly what the
            // old stale-baseline timeout caused, stranding the change as
            // `pending` while main already had the commits). Distinguish
            // already-merged from a genuinely-empty branch the same way
            // `apply_change`'s `recover_no_commits_branch` does (empty files =>
            // legit no-op; main's history touches the change's files => already
            // merged), and mark such a pending change APPLIED (truthful success)
            // instead of emitting a confusing red "Change failed". We use the
            // READ-ONLY `main_history_touches_files` rather than
            // `recover_no_commits_branch` because the latter auto-commits the
            // worktree — and the failure fallthrough below resets the worktree
            // (`reset --hard main`), which would destroy any such commit. Unlike
            // `finalize_change_as_noop` we must NOT delete the branch (the live
            // session's worktree is checked out on it); mirror
            // `apply_now_success`: emit `ChangeApplied`, reset the worktree to
            // main, keep the session alive.
            let pending_change = self
                .changes()
                .pending_for_thread(thread_id)
                .await
                .unwrap_or_default()
                .into_iter()
                .next();
            if let Some(change) = pending_change {
                let already_on_main = change.files.is_empty()
                    || crate::engine::git_ops::main_history_touches_files(repo_root, &change.files)
                        .await;
                if already_on_main {
                    log!(
                        "[ApplyNow] Branch {} already present on main — marking pending change {} applied (no-op)",
                        branch_name,
                        change.id
                    );
                    self.emit_change_applied(
                        thread_id,
                        change.id,
                        false, // no-op: the work is already on main, nothing to restart now
                        false, // client_update
                        Vec::new(),
                        change.thread_title.clone(),
                        actor.clone(),
                        None,
                        None,
                    )
                    .await;
                    // The branch is reset for reuse below, so mirror
                    // `apply_now_success`: clear the harden + plan markers (the
                    // next round of work on this branch must re-trigger both
                    // gates) and make sure the already-merged main reaches the
                    // remote (the abandoned apply that merged it may never have
                    // pushed).
                    consume_harden_marker(&self.pool, repo_root, branch_name).await;
                    consume_plan_marker(&self.pool, repo_root, branch_name).await;
                    self.reset_worktree_and_idle(thread_id, worktree_path).await;
                    push_main_in_background(repo_root);
                    self.broadcast_changes_updated().await;
                    return Ok(());
                }
            }

            // Genuinely nothing to apply (empty branch, no pending change, or a
            // declared-files-but-no-commits mismatch that recovery refused).
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

        self.cc_assisted_merge_then_ff(
            thread_id,
            change_id,
            worktree_path,
            branch_name,
            repo_root,
            idle_notify,
            msg_tx,
        )
        .await
    }

    /// The CC-assisted (slow) half of `merge_via_cc_session`: `main` has
    /// diverged, so drive the live session through a merge + conflict
    /// resolution, then ff main to the branch. Split out so the `apply_change`
    /// Tier-1 path can run the fast ff inline (synchronous, sub-second) and
    /// hand only this slow half to a background task — keeping the parent
    /// thread's turn free instead of blocking it for the whole resolution.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn cc_assisted_merge_then_ff(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        worktree_path: &Path,
        branch_name: &str,
        repo_root: &Path,
        idle_notify: &tokio::sync::Notify,
        msg_tx: &tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
    ) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
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

        if !crate::engine::agent_recovery::last_turn_ended_cleanly(&self.pool, thread_id).await {
            let _ = git_cmd(&["merge", "--abort"], worktree_path).await;
            return Err(
                "Conflict resolution did not finish cleanly — merge aborted. The change is still pending; try applying again.".into(),
            );
        }

        // Verify CC completed the merge. `or_unknown(false)`: a probe that
        // could not run must not authorize the fast-forward below. A `false`
        // costs the user one retry, while an unverified `true` would ff main
        // onto a branch that never took the merge (`.claude/rules/rust.md`).
        let main_merged = crate::engine::git_ops::git_answer(
            &["merge-base", "--is-ancestor", "main", branch_name],
            repo_root,
        )
        .await
        .or_unknown(false);
        if !main_merged {
            return Err(
                "Coding agent session ended without completing the merge. Try applying again."
                    .into(),
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

    /// Atomically claim a thread's live session for an in-place merge.
    ///
    /// Locks `agent_sessions` once so the "is there a live session?" check and
    /// the `apply_now_in_progress` claim can't race a concurrent apply — the
    /// `apply_change` Tier-1 path used to do `is_running_for` then a separate
    /// `live_session_info`, a TOCTOU window where two apply calls (e.g. the LLM
    /// calling `apply_change` twice in quick succession) could both start an
    /// in-place merge on the same session and corrupt it by sending two merge
    /// prompts down `msg_tx`. Returns `Ready` with the flag already set; the
    /// caller MUST clear `apply_now_in_progress` when the merge finishes
    /// (`spawn_in_place_conflict_recovery` does this in all arms).
    pub(crate) async fn begin_in_place_merge(&self, thread_id: Uuid) -> InPlaceMergeStart {
        let mut guard = self.agent_sessions.lock().await;
        match decide_in_place_merge_claim(guard.get(&thread_id)) {
            InPlaceMergeClaim::NoLiveSession => InPlaceMergeStart::NoLiveSession,
            InPlaceMergeClaim::AlreadyInProgress => InPlaceMergeStart::AlreadyInProgress,
            InPlaceMergeClaim::Claim => {
                // Re-fetch mutably to set the claim flag; presence + worktree
                // were validated by `decide_in_place_merge_claim` above under the
                // same lock, so the unwraps cannot fire.
                let s = guard
                    .get_mut(&thread_id)
                    .expect("session present (checked under lock)");
                s.apply_now_in_progress = true;
                InPlaceMergeStart::Ready(crate::engine::change_ops::LiveSessionInfo {
                    worktree_path: s
                        .worktree_path
                        .clone()
                        .expect("worktree present (checked under lock)"),
                    idle_notify: s.idle_notify.clone(),
                    msg_tx: s.msg_tx.clone(),
                    last_event_at: s.last_event_at.clone(),
                })
            }
        }
    }

    /// Clear the `apply_now_in_progress` claim for a thread. Used by the
    /// `apply_change` Tier-1 fast path, which claims the session via
    /// `begin_in_place_merge` then finalizes a clean fast-forward inline (no
    /// background task to clear it). Idempotent — a missing session is a no-op.
    pub(crate) async fn clear_apply_now_in_progress(&self, thread_id: Uuid) {
        let mut guard = self.agent_sessions.lock().await;
        if let Some(s) = guard.get_mut(&thread_id) {
            s.apply_now_in_progress = false;
        }
    }

    /// Run the CC-assisted conflict merge as a guarded background task and
    /// finalize, then return immediately. This is the async counterpart of the
    /// `apply_now` spawn: same liveness-timeout (abort if CC goes silent for 10
    /// minutes), panic guard, and always-clear of `apply_now_in_progress`.
    ///
    /// The caller (`apply_change` Tier 1) has already claimed the session via
    /// `begin_in_place_merge` and run the fast-forward inline; this handles only
    /// the slow divergent-`main` path. On success it emits `ChangeApplied` (via
    /// `apply_now_success`); on failure it emits `ChangeApplyFailed` and leaves
    /// the session alive for retry. Either way, when the session reaches its
    /// terminal event the EventBus parent-callback fan-out wakes the parent
    /// thread with a `ChildThreadCompleted` — so the parent gets a fresh
    /// follow-up turn instead of a turn frozen for the whole resolution.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn spawn_in_place_conflict_recovery(
        self: &Arc<Self>,
        thread_id: Uuid,
        change_id: Uuid,
        session: crate::engine::change_ops::LiveSessionInfo,
        branch_name: String,
        repo_root: std::path::PathBuf,
        requires_restart: bool,
        client_update: bool,
        actor: Option<MessageOrigin>,
    ) {
        use std::sync::atomic::Ordering;
        let engine = self.clone_arc();
        tokio::spawn(async move {
            let crate::engine::change_ops::LiveSessionInfo {
                worktree_path,
                idle_notify,
                msg_tx,
                last_event_at,
            } = session;

            // Liveness-based timeout mirrors `apply_now`: abort only if CC has
            // emitted no events for 10 minutes (not wall-clock — an active
            // conflict resolution can run as long as it keeps producing output).
            let panic_result = std::panic::AssertUnwindSafe(async {
                let inactivity_limit_ms: i64 = 600_000;
                // Anchored to recovery-start so a long pre-apply idle gap can't
                // trip an instant false timeout — see `apply_now`'s spawn loop
                // and `apply_inactivity_timeout_minutes`.
                let recovery_started_ms = now_epoch_millis();
                let inner_fut = engine.cc_assisted_merge_then_ff(
                    thread_id,
                    change_id,
                    &worktree_path,
                    &branch_name,
                    &repo_root,
                    &idle_notify,
                    &msg_tx,
                );
                tokio::pin!(inner_fut);
                loop {
                    match tokio::time::timeout(Duration::from_secs(30), &mut inner_fut).await {
                        Ok(result) => break result,
                        Err(_) => {
                            let last_ms = last_event_at.load(Ordering::Relaxed);
                            if let Some(minutes) = apply_inactivity_timeout_minutes(
                                now_epoch_millis(),
                                last_ms,
                                recovery_started_ms,
                                inactivity_limit_ms,
                            ) {
                                break Err(format!(
                                    "Conflict resolution timed out — no CC activity for {} minutes",
                                    minutes,
                                )
                                .into());
                            }
                        }
                    }
                }
            });

            let result: Result<(String, String), Box<dyn std::error::Error + Send + Sync>> =
                match futures::FutureExt::catch_unwind(panic_result).await {
                    Ok(inner) => inner,
                    Err(_) => Err("cc_assisted_merge_then_ff panicked".into()),
                };

            // Always clear the in-progress claim — normal, timeout, error, panic.
            {
                let mut guard = engine.agent_sessions.lock().await;
                if let Some(s) = guard.get_mut(&thread_id) {
                    s.apply_now_in_progress = false;
                }
            }

            match result {
                Ok((pre_sha, post_sha)) => {
                    engine
                        .apply_now_success(
                            thread_id,
                            change_id,
                            requires_restart,
                            client_update,
                            &pre_sha,
                            &post_sha,
                            &worktree_path,
                            &repo_root,
                            &branch_name,
                            actor,
                        )
                        .await;
                }
                Err(e) => {
                    log!(
                        "[Changes] Async in-place merge failed for {}: {} — session stays alive for retry",
                        change_id,
                        e
                    );
                    engine
                        .emit_apply_failed(thread_id, change_id, &e.to_string(), actor)
                        .await;
                }
            }
        });
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
        // Apply-time net for the "≤1 pending change per thread" invariant: now
        // that this change landed, drop any stale pending change the thread
        // still holds on another branch so it can't keep blocking Archive.
        // `propose_change` is the primary guard; this is the apply-side backstop.
        // See docs/plans/2026-07-01-orphaned-pending-change-blocks-archive.md.
        self.discard_orphaned_pending_siblings(thread_id, change_id, actor.clone())
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
                let kind_ctx =
                    crate::engine::change_ops::load_apply_kind_context(&self.pool, Some(thread_id))
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
                // The dev post-apply refresh (background engine rebuild /
                // served-dist re-snapshot) is NOT done here — `emit_change_applied`
                // above owns it for every merge path. See
                // `change_ops_emitters::post_apply_dev_refresh`.
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
    ///
    /// Used after apply, discard, and no-commits to keep the session alive.
    /// ONLY safe when the branch's work is already on main (or there is no
    /// work): the worktree's HEAD is attached to the session branch, so
    /// `reset --hard main` moves that BRANCH REF and `clean -fd` wipes the
    /// tree. On a path where the change is still pending, call
    /// [`Self::mark_session_idle`] instead.
    pub(crate) async fn reset_worktree_and_idle(&self, thread_id: Uuid, worktree_path: &Path) {
        let _ = git_cmd(&["reset", "--hard", "main"], worktree_path).await;
        let _ = git_cmd(&["clean", "-fd"], worktree_path).await;
        self.mark_session_idle(thread_id, worktree_path).await;
    }

    /// Re-enter the idle state WITHOUT touching the worktree or the branch.
    ///
    /// The half of [`Self::reset_worktree_and_idle`] that is always safe. Use it
    /// when the session must go back to idle but the branch still holds commits
    /// the user owns.
    pub(crate) async fn mark_session_idle(&self, thread_id: Uuid, worktree_path: &Path) {
        use crate::engine::event_bus::BusEvent;
        use crate::engine::thread_events::{EventMeta, ThreadEvent};

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
    use crate::engine::AgentSession;
    use tokio::sync::mpsc;

    /// Build a minimal `AgentSession` for claim-decision tests. `worktree` and
    /// `process_exited` / `apply_now_in_progress` are the fields the claim state
    /// machine reads; everything else is inert defaults. The receiver comes back
    /// with it — drop it and the session is a phantom, which the claim treats as
    /// no live session (see `AgentSession::is_live`).
    fn claim_test_session(
        process_exited: bool,
        worktree: Option<&str>,
        apply_now_in_progress: bool,
    ) -> (AgentSession, mpsc::UnboundedReceiver<AgentUserInput>) {
        let (mut session, msg_rx) = AgentSession::for_test();
        session.is_waiting = !process_exited;
        session.process_exited = process_exited;
        session.apply_now_in_progress = apply_now_in_progress;
        session.worktree_path = worktree.map(std::path::PathBuf::from);
        (session, msg_rx)
    }

    #[test]
    fn claim_none_when_no_session() {
        assert_eq!(
            decide_in_place_merge_claim(None),
            InPlaceMergeClaim::NoLiveSession
        );
    }

    #[test]
    fn claim_none_when_process_exited() {
        let (s, _msg_rx) = claim_test_session(true, Some("/wt"), false);
        assert_eq!(
            decide_in_place_merge_claim(Some(&s)),
            InPlaceMergeClaim::NoLiveSession
        );
    }

    #[test]
    fn claim_none_when_no_worktree() {
        let (s, _msg_rx) = claim_test_session(false, None, false);
        assert_eq!(
            decide_in_place_merge_claim(Some(&s)),
            InPlaceMergeClaim::NoLiveSession
        );
    }

    #[test]
    fn claim_already_in_progress_when_flag_set() {
        // A live session already mid-apply must not be claimed again — this is
        // the guard against the LLM calling `apply_change` twice and starting
        // two in-place merges on one session.
        let (s, _msg_rx) = claim_test_session(false, Some("/wt"), true);
        assert_eq!(
            decide_in_place_merge_claim(Some(&s)),
            InPlaceMergeClaim::AlreadyInProgress
        );
    }

    #[test]
    fn claim_ok_when_live_idle_with_worktree() {
        let (s, _msg_rx) = claim_test_session(false, Some("/wt"), false);
        assert_eq!(
            decide_in_place_merge_claim(Some(&s)),
            InPlaceMergeClaim::Claim
        );
    }

    /// Regression: clicking Apply on an *app* coding-agent thread with a live
    /// session must NOT run the Lucidos-source `/harden` + test flow, even when
    /// the branch has no harden marker. Before the fix, `apply_now_inner`
    /// injected "Run /harden now." + `cargo test -p lucidos-engine` into the app
    /// session and failed the apply with "Hardening did not complete" — exactly
    /// the reported bug on the "Create App Demo Video" (`demo-director`) thread.
    #[test]
    fn app_thread_never_runs_in_session_hardening() {
        // App thread, no marker — must still skip.
        assert!(!should_run_in_session_hardening(true, false));
        // App thread, marker present — skip.
        assert!(!should_run_in_session_hardening(true, true));
    }

    /// Non-app (Lucidos-source / external) threads still gate on the marker: run
    /// hardening iff the branch isn't already hardened.
    #[test]
    fn non_app_thread_gates_in_session_hardening_on_marker() {
        assert!(
            should_run_in_session_hardening(false, false),
            "unhardened non-app branch must run /harden before merging"
        );
        assert!(
            !should_run_in_session_hardening(false, true),
            "already-hardened non-app branch skips the redundant /harden"
        );
    }

    const TEN_MIN_MS: i64 = 600_000;

    /// Regression: a thread that sat idle longer than the inactivity limit
    /// BEFORE the user clicked Apply must NOT trip an instant false timeout on
    /// the first poll. `last_event_at` is 11 minutes stale (CC alive but quiet),
    /// but the apply only just started — so the clock anchored to apply-start
    /// reports no timeout. Against the old `now - last_event_at` math this
    /// returned `Some(11)`, which is exactly the "Apply timed out — no CC
    /// activity for 10 minutes" the user saw without 10 minutes having elapsed.
    #[test]
    fn inactivity_timeout_ignores_pre_apply_idle_gap() {
        let now = 100 * 60_000; // t = 100 min
        let last_event = now - 11 * 60_000; // CC last spoke 11 min ago
        let apply_started = now; // apply just kicked off
        assert_eq!(
            apply_inactivity_timeout_minutes(now, last_event, apply_started, TEN_MIN_MS),
            None,
            "a fresh apply must get the full limit regardless of how long the thread was idle first"
        );
    }

    /// One poll-interval after a fresh apply on a long-idle thread is still well
    /// under the limit — no timeout.
    #[test]
    fn inactivity_timeout_holds_through_early_polls() {
        let apply_started = 100 * 60_000;
        let last_event = apply_started - 30 * 60_000; // 30 min stale baseline
        let now = apply_started + 30_000; // 30s into the apply
        assert_eq!(
            apply_inactivity_timeout_minutes(now, last_event, apply_started, TEN_MIN_MS),
            None
        );
    }

    /// Genuine mid-apply silence still fires: both CC's last event and
    /// apply-start are now older than the limit, so the timeout reports the
    /// real minutes of silence.
    #[test]
    fn inactivity_timeout_fires_on_genuine_silence() {
        let apply_started = 100 * 60_000;
        let last_event = apply_started + 60_000; // CC spoke 1 min into the apply
        let now = last_event + 12 * 60_000; // then went silent for 12 min
        assert_eq!(
            apply_inactivity_timeout_minutes(now, last_event, apply_started, TEN_MIN_MS),
            Some(12)
        );
    }

    /// CC activity advances the clock past apply-start: a recent event keeps the
    /// timeout from firing even when apply-start is ancient.
    #[test]
    fn inactivity_timeout_respects_recent_cc_event() {
        let apply_started = 100 * 60_000;
        let now = apply_started + 60 * 60_000; // an hour-long active apply
        let last_event = now - 60_000; // but CC spoke 1 min ago
        assert_eq!(
            apply_inactivity_timeout_minutes(now, last_event, apply_started, TEN_MIN_MS),
            None
        );
    }

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
