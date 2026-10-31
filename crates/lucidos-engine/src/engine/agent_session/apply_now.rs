use super::lifecycle::foreign_worktree_reason;
use crate::engine::change_ops::{
    apply_must_harden, branch_is_hardened, now_epoch_millis, MergeOwnership, PlanFloor,
    ProposalHold, ProposeOutcome, MERGE_OWNED_BY_RESOLVER_MESSAGE,
};
use crate::engine::git_ops::{
    auto_commit_preserving_marker, auto_commit_safe_files_if_dirty, branch_changed_files,
    branch_head_sha, catchup_and_ff_to_main, commit_worktree_or_err, commits_in_range,
    default_local_branch, describe_branch_changes, files_have_client_update, files_require_restart,
    git_answer, git_answer_when_ok, git_cmd, git_ran_ok, has_branch_commits, lands_plan_files_only,
    push_main_in_background, resolution_merged_main, worktree_current_branch,
};
use crate::engine::thread_events::{ApplyNowOutcome, EventChannel, MessageOrigin};
use crate::engine::{AgentUserInput, InputAnswered, LucidosEngine};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Outcome of `begin_in_place_merge`'s atomic claim on a thread's live session.
pub(crate) enum InPlaceMergeStart {
    /// No live, non-exited session with a worktree — caller falls through to the
    /// dead-session / temp-worktree merge tiers.
    NoLiveSession,
    /// Another operation holds the session's change claim, named here.
    Claimed(crate::engine::types::ChangeClaim),
    /// A Stop, Discard or Archive is ending the session (`pending_stop`).
    SessionStopping,
    /// Session claimed; `change_claim` is now `Apply`. Caller owns clearing it.
    Ready(crate::engine::change_ops::LiveSessionInfo),
}

/// Pure decision core of `begin_in_place_merge` and `apply_now`, split out so
/// the claim state machine is unit-testable without a full engine. A session is
/// claimable when it is live and has a worktree. No apply may already run on
/// it, and no stop may be ending it.
///
/// A pending stop still reads live until the loop breaks out, and a Discard
/// deletes the branch the merge would publish. `claude_code::stop_refusal`
/// refuses the other direction.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum InPlaceMergeClaim {
    NoLiveSession,
    Claimed(crate::engine::types::ChangeClaim),
    SessionStopping,
    Claim,
}

pub(crate) fn decide_in_place_merge_claim(
    session: Option<&crate::engine::AgentSession>,
) -> InPlaceMergeClaim {
    let Some(s) = session.filter(|s| s.is_live() && s.worktree_path.is_some()) else {
        return InPlaceMergeClaim::NoLiveSession;
    };
    if let Some(holder) = s.change_claim {
        return InPlaceMergeClaim::Claimed(holder);
    }
    if s.pending_stop.is_some() {
        return InPlaceMergeClaim::SessionStopping;
    }
    InPlaceMergeClaim::Claim
}

/// Release the apply claim that the session behind `claimant` took.
///
/// An apply task can outlive the session it claimed, and a replacement on the
/// same thread takes its own claim. Clearing by thread id alone would release
/// that one, and a second apply could then merge beside it. `msg_tx` is the
/// session's identity, as in `SessionEntryGuard`.
pub(crate) fn release_change_claim(
    sessions: &mut std::collections::HashMap<Uuid, crate::engine::AgentSession>,
    thread_id: Uuid,
    claimant: &tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
) {
    if let Some(s) = sessions
        .get_mut(&thread_id)
        .filter(|s| s.msg_tx.same_channel(claimant))
    {
        s.change_claim = None;
    }
}

/// Holds a change claim for work that awaits inside a request future, and
/// releases it however that work ends. axum drops the future when the client
/// disconnects. A panic unwinds past any release written after it. Either
/// would otherwise leave the claim set until the session ends.
///
/// `release` frees it under the lock, so the next request never meets a stale
/// claim. `Drop` covers every other exit: it cannot await the async lock, so it
/// releases on a detached task, as `SessionEntryGuard` does.
pub(crate) struct ChangeClaimGuard {
    sessions: Option<
        Arc<tokio::sync::Mutex<std::collections::HashMap<Uuid, crate::engine::AgentSession>>>,
    >,
    thread_id: Uuid,
    claimant: tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
}

impl ChangeClaimGuard {
    pub(crate) fn new(
        sessions: Arc<
            tokio::sync::Mutex<std::collections::HashMap<Uuid, crate::engine::AgentSession>>,
        >,
        thread_id: Uuid,
        claimant: tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
    ) -> Self {
        Self {
            sessions: Some(sessions),
            thread_id,
            claimant,
        }
    }

    pub(crate) async fn release(mut self) {
        let Some(sessions) = self.sessions.clone() else {
            return;
        };
        // Disarm only once the lock is held. A cancel while waiting for it
        // must still leave `Drop` armed to release.
        let mut map = sessions.lock().await;
        self.sessions = None;
        release_change_claim(&mut map, self.thread_id, &self.claimant);
    }
}

impl Drop for ChangeClaimGuard {
    fn drop(&mut self) {
        let Some(sessions) = self.sessions.take() else {
            return;
        };
        // Outside a runtime the map dies with the process, so nothing is left
        // to release.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (thread_id, claimant) = (self.thread_id, self.claimant.clone());
        handle.spawn(async move {
            release_change_claim(&mut *sessions.lock().await, thread_id, &claimant);
            log!(
                "[AgentSession] A change claim on thread {} outlived its work and was released on drop",
                thread_id
            );
        });
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

/// Send `text` to a live session as a prompt the caller waits on. The receiver
/// is answered when the turn that reads this prompt ends.
pub(crate) fn send_awaited_prompt(
    msg_tx: &tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
    text: String,
) -> Result<tokio::sync::oneshot::Receiver<InputAnswered>, Box<dyn std::error::Error + Send + Sync>>
{
    let (completion, answered) = tokio::sync::oneshot::channel();
    msg_tx
        .send(AgentUserInput {
            text,
            images: None,
            origin_event_id: None,
            kind: crate::engine::AgentInputKind::User,
            completion: Some(completion),
        })
        .map_err(|_| "Session channel closed")?;
    Ok(answered)
}

/// Wait until the turn that read a [`send_awaited_prompt`] prompt ends.
///
/// A turn already running when the prompt was sent ends first, and does not
/// answer it (ADR 0268). A session that ends first drops the sender.
pub(crate) async fn wait_for_prompt_turn(
    answered: tokio::sync::oneshot::Receiver<InputAnswered>,
    context: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match answered.await {
        Ok(InputAnswered::TurnEnded) => Ok(()),
        Ok(InputAnswered::Rejected) => {
            Err(format!("The coding agent rejected the prompt while {}", context).into())
        }
        Err(_) => Err(format!("Coding agent session ended while {}", context).into()),
    }
}

/// What an Apply with nothing pending and no proposal hold answers (ADR 0400).
pub(crate) const NOTHING_PROPOSED_REFUSAL: &str =
    "Nothing is proposed on this thread, so there is nothing to apply. A turn that \
     finishes proposes its work.";

impl LucidosEngine {
    /// Apply Now: keep the existing Claude Code session alive and use it for review/conflict resolution.
    /// Only kills CC after the merge succeeds. With no live session it applies
    /// the pending change directly. Either way it needs a proposal: with
    /// nothing pending it refuses (ADR 0400).
    ///
    /// Runs as a background task (tokio::spawn). Steps:
    /// 1. Auto-commit in worktree
    /// 2. Review if needed (send follow-up to existing CC)
    /// 3. Refresh the pending change to the branch head
    /// 4. Try merge (clean / trivial / CC-assisted conflict resolution)
    /// 5. On success: kill CC + clean up. On failure: CC stays alive for retry.
    ///
    /// An apply that passes its refusals is bracketed by `ApplyNowStarted` and
    /// `ApplyNowEnded`, which project `apply_in_flight` (ADR 0437). Returns the
    /// thread's summary version once the call's own events landed, so the
    /// caller can hold its "requesting" state until the row reaches it.
    pub async fn apply_now(
        self: &Arc<Self>,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<Option<i64>, Box<dyn std::error::Error + Send + Sync>> {
        // Thread-scoped twin of the guard in `apply_change_inner`: while a
        // conflict resolution is in flight on this thread, it owns the merge.
        // Checked before the session is claimed, because the live-session
        // branch below would otherwise send a SECOND merge prompt down the
        // resolver's own `msg_tx` and then ff `main` under it. The no-live
        // branch needs no separate cover: it delegates to `apply_change`, which
        // carries the change-scoped guard.
        if self.merge_ownership_for_thread(thread_id).await == MergeOwnership::ResolverOwnsIt {
            log!(
                "[ApplyNow] Refused for thread {}: a conflict resolution is in flight and owns the merge",
                thread_id
            );
            return Err(MERGE_OWNED_BY_RESOLVER_MESSAGE.into());
        }

        // Both branches below prompt the agent: a live session gets the review
        // or merge prompt, and a missing harden marker starts a hardening run.
        // Either prompt overtakes a question the user can still answer, and
        // the typed answer then reaches the wrong session. So refuse first.
        match crate::engine::agent_recovery::thread_parked_on_question(self.pool(), thread_id).await
        {
            Ok(false) => {}
            Ok(true) => {
                log!(
                    "[ApplyNow] Refused for thread {}: it is parked on a question",
                    thread_id
                );
                return Err(crate::engine::claude_code::QUESTION_OPEN_MESSAGE.into());
            }
            Err(e) => {
                log!(
                    "[ApplyNow] Refused for thread {}: could not check for a parked question: {}",
                    thread_id,
                    e
                );
                return Err(crate::engine::claude_code::QUESTION_UNKNOWN_MESSAGE.into());
            }
        }

        // Extract session metadata — if no live session, fall back to stale handling
        let (worktree_path, branch_name, repo_root, msg_tx, last_event_at) = {
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
                    match decide_in_place_merge_claim(Some(session)) {
                        InPlaceMergeClaim::Claimed(holder) => {
                            return Err(
                                crate::engine::claude_code::claim_refusal_message(holder).into()
                            );
                        }
                        InPlaceMergeClaim::SessionStopping => {
                            return Err(crate::engine::claude_code::SESSION_STOPPING_MESSAGE.into());
                        }
                        InPlaceMergeClaim::NoLiveSession | InPlaceMergeClaim::Claim => {}
                    }
                    session.change_claim = Some(crate::engine::types::ChangeClaim::Apply);
                    (
                        session.worktree_path.clone().unwrap(),
                        session.branch_name.clone().unwrap(),
                        session.repo_root.clone().unwrap(),
                        session.msg_tx.clone(),
                        session.last_event_at.clone(),
                    )
                }
                _ => {
                    drop(guard);

                    // No live session: apply the pending change directly.
                    // Coding-agent sessions exit at a clean idle, so this is
                    // the usual state of a thread the user returns to apply.
                    // A pending row needs no re-proposal, which would only
                    // add a duplicate `ChangeProposed` to the timeline. See
                    // `apply_now_no_live_session_fast_path_preserves_clean_pending_change`.
                    let pending = self.changes().pending_for_thread(thread_id).await?;
                    if !pending.is_empty() {
                        // A task of its own, so a dropped request cannot cut
                        // the bracket between its start and its end.
                        let engine = self.clone_arc();
                        let actor = actor.clone();
                        let bracket = tokio::spawn(async move {
                            engine
                                .emit_apply_now_started(thread_id, actor.clone())
                                .await?;
                            let applying = std::panic::AssertUnwindSafe(
                                engine.apply_pending_now(thread_id, pending, actor),
                            );
                            if futures::FutureExt::catch_unwind(applying).await.is_err() {
                                log!(
                                    "[ApplyNow] Applying pending changes panicked for {}",
                                    thread_id
                                );
                            }
                            Ok(engine
                                .emit_apply_now_ended(thread_id, ApplyNowOutcome::Finished)
                                .await)
                        });
                        return bracket
                            .await
                            .map_err(|e| format!("Apply task for {thread_id} failed: {e}"))?;
                    }

                    // Nothing is pending, so nothing is Apply's to land
                    // (ADR 0400). A stale session's work is proposed by its
                    // own recovery, never by an Apply.
                    let error = self.nothing_proposed_refusal(thread_id).await;
                    log!("[ApplyNow] Refused for thread {}: {}", thread_id, error);
                    let refused = self
                        .event_bus
                        .emit(crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ChangeApplyFailed {
                                change_id: String::new(),
                                error: error.to_string(),
                                actor: actor.clone(),
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        })
                        .await;
                    return Ok(match refused {
                        Ok(emitted) => emitted.and_then(|r| r.summary_version),
                        Err(e) => {
                            log!(
                                "[ApplyNow] ChangeApplyFailed (no live session, nothing pending) emit failed: {}",
                                e
                            );
                            None
                        }
                    });
                }
            }
        };

        // The claim is held, so this apply is now the thread's. The task
        // announces it first, so a dropped request cannot leave the claim or
        // the flag with no task left to release them.
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let engine = self.clone_arc();
        tokio::spawn(async move {
            let started = engine
                .emit_apply_now_started(thread_id, actor.clone())
                .await;
            let refused = started.is_err();
            // The receiver is gone only when the request was dropped, and the
            // apply goes on without it.
            let _ = started_tx.send(started);
            if refused {
                release_change_claim(&mut *engine.agent_sessions.lock().await, thread_id, &msg_tx);
                return;
            }
            // Use std::panic::catch_unwind via FutureExt to guarantee cleanup on panic.
            // tokio::spawn swallows panics. Without this, change_claim stays
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
            release_change_claim(&mut *engine.agent_sessions.lock().await, thread_id, &msg_tx);

            if let Err(e) = result {
                log!("[ApplyNow] Failed for thread {}: {}", thread_id, e);
                // Tell the user why. `ApplyNowEnded` below ends the apply.
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
            // Every exit reaches here, panic and timeout included, and only
            // after the claim is released: a client that sees the apply end
            // can start the next one.
            engine
                .emit_apply_now_ended(thread_id, ApplyNowOutcome::Finished)
                .await;
        });

        started_rx
            .await
            .map_err(|_| format!("The apply task for {thread_id} ended before it started"))?
    }

    /// Apply each pending change in turn, for an Apply Now with no live
    /// session.
    async fn apply_pending_now(
        self: &Arc<Self>,
        thread_id: Uuid,
        pending: Vec<crate::core::changes::Change>,
        actor: Option<MessageOrigin>,
    ) {
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
            // Log only: `apply_change` announces this failure itself, and a
            // second emit draws the same card twice. The two errors it leaves
            // unannounced mean the row it was just handed no longer reads.
            if let Err(e) = self.apply_change(change.id, actor.clone()).await {
                log!("[ApplyNow] apply_change for {} failed: {}", change.id, e);
            }
        }
    }

    /// Announce that an Apply Now began, and return the row's version after it.
    async fn emit_apply_now_started(
        &self,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<Option<i64>, Box<dyn std::error::Error + Send + Sync>> {
        let emitted = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: crate::engine::thread_events::ThreadEvent::ApplyNowStarted,
                meta: crate::engine::thread_events::EventMeta::with_actor(actor),
            })
            .await?;
        Ok(emitted.and_then(|r| r.summary_version))
    }

    /// Announce that the Apply Now on this thread is over, and return the row's
    /// version after it. A failed emit leaves the flag for the next startup's
    /// `end_interrupted_applies`.
    async fn emit_apply_now_ended(&self, thread_id: Uuid, outcome: ApplyNowOutcome) -> Option<i64> {
        emit_apply_now_ended(&self.event_bus, thread_id, outcome).await
    }

    /// Wait for the turn that read the prompt behind `answered`, then
    /// auto-commit any changes it made.
    ///
    /// Propagates `git add` / `git commit` failures via `Err`. A silent failure
    /// here would lose a real CC change — the apply-now caller treats the
    /// returned `Ok` as proof the iteration produced a committed snapshot
    /// before moving on to the next step (hardening, merge).
    pub(crate) async fn wait_and_commit(
        &self,
        answered: tokio::sync::oneshot::Receiver<InputAnswered>,
        worktree_path: &Path,
        context: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        wait_for_prompt_turn(answered, context).await?;

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
    async fn apply_now_inner(
        self: &Arc<Self>,
        thread_id: Uuid,
        worktree_path: &Path,
        branch_name: &str,
        repo_root: &Path,
        msg_tx: &tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use crate::engine::event_bus::BusEvent;
        use crate::engine::thread_events::{EventMeta, ThreadEvent};

        // Apply acts only on a proposal (ADR 0400). Work no change carries is
        // never Apply's to propose. An unanswered read refuses too.
        let pending = match self.changes().get_pending_by_branch(branch_name).await {
            Ok(row) => row.is_some(),
            Err(e) => {
                log!("[ApplyNow] pending lookup for {}: {}", branch_name, e);
                false
            }
        };
        if !pending {
            let error = self.nothing_proposed_refusal(thread_id).await;
            self.refuse_apply_now(thread_id, worktree_path, error, actor)
                .await;
            return Ok(());
        }

        // A thread that handed off lands nothing itself: its successor's
        // change carries the work. Before the auto-commit, so the old branch
        // stays at the commit the successor carried.
        if crate::engine::successor::handed_off(&self.pool, thread_id).await {
            self.refuse_apply_now_for_hold(
                thread_id,
                worktree_path,
                ProposalHold::CarriedBySuccessor,
                actor,
            )
            .await;
            return Ok(());
        }

        // Step 1: Auto-commit in worktree (preserves harden marker if fresh)
        auto_commit_preserving_marker(
            &self.pool,
            worktree_path,
            repo_root,
            branch_name,
            "Coding agent changes (auto-committed)",
        )
        .await;

        let kind_ctx =
            crate::engine::change_ops::load_apply_kind_context(&self.pool, Some(thread_id)).await;
        // Before hardening, so a held branch never spends a suite run on a
        // change that cannot land. Both gates judge what will land.
        let landing = self.landing_files(repo_root, branch_name).await;
        let floor = self
            .plan_floor(&kind_ctx, repo_root, branch_name)
            .await
            .with_landing(landing.as_deref().unwrap_or_default());
        if let PlanFloor::Held(hold) = floor {
            self.refuse_apply_now_for_hold(
                thread_id,
                worktree_path,
                ProposalHold::Plan(hold),
                actor,
            )
            .await;
            return Ok(());
        }
        // Before hardening too: an integrated sub-thread's work never lands
        // on its own, so a full suite run here would buy nothing. A plan
        // alone is the exception, as in the proposal hold.
        if !lands_plan_files_only(landing.as_deref().unwrap_or_default())
            && crate::engine::integrated::thread_is_integrated(&self.pool, thread_id).await
        {
            self.refuse_apply_now_for_hold(
                thread_id,
                worktree_path,
                ProposalHold::IntegratedIntoParent,
                actor,
            )
            .await;
            return Ok(());
        }

        // The same hardening gate as `change_ops::apply_change`. Apps own
        // their hardening, so their marker is never probed.
        let mut hardened = !kind_ctx.is_app()
            && branch_is_hardened(&self.pool, self.changes(), repo_root, branch_name).await;
        if apply_must_harden(&kind_ctx, hardened, landing.as_deref()) {
            let answered = self.request_hardening_in_session(thread_id, msg_tx).await?;
            self.wait_and_commit(answered, worktree_path, "waiting for hardening")
                .await?;

            // `/harden` ran the suites this change needs. A canceled `/harden`
            // ends its turn too, so the marker is the proof, not the wait
            // returning Ok.
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
            hardened = true;
        }

        // Step 1 only logs a failed auto-commit. A tree still dirty here holds
        // work that never reached the branch. Every path below ends in a reset
        // that would wipe it, including a merge of the commits that did land.
        // An unanswered probe counts as dirty. Submodules are left out: the
        // auto-commit cannot stage inside one, and the reset never touches one.
        let still_dirty = git_answer_when_ok(
            &["status", "--porcelain", "--ignore-submodules=all"],
            worktree_path,
            |o| !o.stdout.is_empty(),
        )
        .await;
        if still_dirty.or_unknown(true) {
            log!(
                "[ApplyNow] Branch {}'s worktree is still dirty (or unreadable) after the \
                 auto-commit. Leaving the worktree untouched",
                branch_name
            );
            self.event_bus
                .emit_or_log(
                    BusEvent::Thread {
                        thread_id,
                        event: ThreadEvent::ChangeApplyFailed {
                            change_id: String::new(),
                            error: "Could not confirm the coding agent's changes were \
                                    committed, so nothing was applied. The files are still in \
                                    the worktree."
                                .to_string(),
                            actor: actor.clone(),
                        },
                        meta: EventMeta::NONE,
                    },
                    "[ApplyNow] ChangeApplyFailed (auto-commit failed)",
                )
                .await;
            self.mark_session_idle(thread_id, worktree_path).await;
            return Ok(());
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
            // A read that failed is not "no pending change". It falls through
            // to the branch below, which resets the worktree. So the reason has
            // to reach the log rather than vanish into an empty list.
            let pending_change = self
                .changes()
                .pending_for_thread(thread_id)
                .await
                .unwrap_or_else(|e| {
                    log!(
                        "[ApplyNow] pending_for_thread({}): {}. Treating the branch as having nothing to apply",
                        thread_id,
                        e
                    );
                    Vec::new()
                })
                .into_iter()
                .next();
            let pending_change = match pending_change {
                Some(change) => match self.claim_pending_change(change.id).await? {
                    Some(claim) => Some((change, claim)),
                    None => {
                        log!(
                            "[ApplyNow] Change {} is being applied elsewhere or was resolved; leaving it to that path",
                            change.id
                        );
                        return Ok(());
                    }
                },
                None => None,
            };
            if let Some((change, claim)) = pending_change {
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
                        &claim,
                        thread_id,
                        false, // no-op: the work is already on main, nothing to restart now
                        false, // client_update
                        Vec::new(),
                        change.thread_title.clone(),
                        actor.clone(),
                        None,
                    )
                    .await;
                    // The branch is reset for reuse below. `emit_change_applied`
                    // cleared the applied change's branch markers. Make sure the
                    // already-merged main reaches the remote: the abandoned
                    // apply that merged it may never have pushed.
                    self.reset_worktree_and_idle(thread_id, worktree_path, branch_name)
                        .await;
                    push_main_in_background(repo_root, &self.event_bus);
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
            self.reset_worktree_and_idle(thread_id, worktree_path, branch_name)
                .await;
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
        let outcome = self
            .propose_change(crate::engine::change_ops::ProposeChangeInput {
                thread_id,
                branch_name,
                repo_root: &repo_root_str,
                description: &description,
                files: &changed_files,
                requires_restart,
                channel: EventChannel::ClaudeCode,
                hardened,
                // Apply-now propose is part of a live agent flow — origin is
                // carried by the surrounding MessageReceived.
                origin: None,
            })
            .await?;
        let change_id = match outcome {
            ProposeOutcome::Proposed(id) => id,
            // The marker went away after the check above.
            ProposeOutcome::Held(hold) => {
                self.refuse_apply_now_for_hold(thread_id, worktree_path, hold, actor)
                    .await;
                return Ok(());
            }
            ProposeOutcome::Unfinished => {
                return Err("propose_change never withholds a finished turn".into());
            }
            // Already in the landing lane: the lane answers for it below.
            ProposeOutcome::AwaitingLanding(id) => id,
        };
        let Some(claim) = self.claim_pending_change(change_id).await? else {
            log!(
                "[ApplyNow] Change {} is being applied elsewhere or was resolved; leaving it to that path",
                change_id
            );
            return Ok(());
        };

        // Step 5: Check main repo for uncommitted changes
        self.commit_dirty_logged("Coding agent changes", "apply_now auto-commit")
            .await;
        // Under both of `main`'s locks, as in `apply_change`: a safe-file
        // commit moves `main`, and a concurrent merge's ref move and tree sync
        // land wholly before or after the check.
        let dirty = {
            let _main = crate::engine::git_ops::MAIN_LOCKS.lock_main().await;
            auto_commit_safe_files_if_dirty(repo_root).await
        };
        if dirty {
            let msg = "Cannot merge: the repository has uncommitted changes. Commit or stash them first, then try again.";
            self.emit_apply_failed(thread_id, change_id, msg, actor.clone())
                .await;
            // CC stays alive for retry
            return Ok(());
        }

        // Step 5b: the bounded-security-fix bound, checked on THIS path too.
        //
        // `apply_now` is a second route to main: with a live session it merges
        // below rather than through `change_ops::apply_change`, so the bound
        // enforced there would never run. The lane skips the human plan
        // decision in exchange for this check, and `apply_now_success` consumes
        // the marker afterwards, so nothing later can catch a breach.
        //
        // Here rather than at the merge, for the same reason as in
        // `apply_change_inner`: this returns a clean refusal, while an `Err`
        // out of the merge helpers reads as "main diverged" and escalates.
        // The rest of the plan floor already ran, before hardening.
        if let crate::engine::git_ops::PlanMarkerState::Present(kind) =
            self.plan_marker_state(repo_root, branch_name).await
        {
            if kind.is_file_bounded() {
                let dirty = crate::engine::git_ops::worktree_dirty_files(worktree_path).await;
                if let Some(msg) = self
                    .bounded_fix_refusal(repo_root, dirty, branch_name)
                    .await
                {
                    log!(
                        "[ApplyNow] Apply blocked: bounded security fix left its bound on branch {}",
                        branch_name
                    );
                    self.emit_apply_failed(thread_id, change_id, &msg, actor.clone())
                        .await;
                    // CC stays alive so the session can widen the bound or plan.
                    return Ok(());
                }
            }
        }

        // Only the landing lane moves `main` for Lucidos source (ADR 0463).
        if crate::engine::landing_lane::takes_landing_lane(&kind_ctx, repo_root, self.repo_root()) {
            self.apply_now_through_lane(thread_id, claim, worktree_path, branch_name, actor)
                .await;
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
                msg_tx,
            )
            .await
        {
            Ok((pre_sha, post_sha)) => {
                let client_update = files_have_client_update(&changed_files);
                self.apply_now_success(
                    &claim,
                    thread_id,
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

    /// The landing-lane half of Apply Now. This apply holds the idle session,
    /// so once the lane has landed everything on the branch, the worktree
    /// resets to `main` as after any apply. A change still queued leaves the
    /// worktree alone. Every failure was announced by the lane.
    async fn apply_now_through_lane(
        self: &Arc<Self>,
        thread_id: Uuid,
        claim: crate::engine::change_ops::apply_claim::ApplyClaim,
        worktree_path: &Path,
        branch_name: &str,
        actor: Option<MessageOrigin>,
    ) {
        let change_id = claim.change_id();
        let change = match self.changes().get_by_id(change_id).await {
            Ok(Some(change)) => change,
            Ok(None) => {
                log!("[ApplyNow] change {} vanished before queueing", change_id);
                self.mark_session_idle(thread_id, worktree_path).await;
                return;
            }
            Err(e) => {
                let msg = format!("Could not read the change to queue it: {e}");
                self.emit_apply_failed(thread_id, change_id, &msg, actor)
                    .await;
                self.mark_session_idle(thread_id, worktree_path).await;
                return;
            }
        };
        let repo_root = std::path::PathBuf::from(&change.repo_root);
        let landed = match self.queue_in_landing_lane(change, claim, actor).await {
            Ok(result) => matches!(
                result.status,
                crate::engine::ApplyStatus::Applied | crate::engine::ApplyStatus::Noop
            ),
            Err(e) => {
                log!("[ApplyNow] landing lane refused {}: {}", change_id, e);
                false
            }
        };
        // `or_unknown(false)`: a yes authorizes the reset below.
        let all_on_main = landed
            && git_answer(
                &["merge-base", "--is-ancestor", branch_name, "main"],
                &repo_root,
            )
            .await
            .or_unknown(false);
        if all_on_main {
            self.reset_worktree_and_idle(thread_id, worktree_path, branch_name)
                .await;
        } else {
            self.mark_session_idle(thread_id, worktree_path).await;
        }
    }

    /// Merge main into a CC worktree and fast-forward main to the branch.
    /// Fast path: try ff directly. If main diverged, send CC a single prompt
    /// to merge, resolve conflicts, harden, and test — then ff again.
    /// Returns (pre_sha, post_sha) on success.
    pub(crate) async fn merge_via_cc_session(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        worktree_path: &Path,
        branch_name: &str,
        repo_root: &Path,
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
    pub(crate) async fn cc_assisted_merge_then_ff(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        worktree_path: &Path,
        branch_name: &str,
        repo_root: &Path,
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
        // Bind the session to this resolution at the same moment the pairing
        // opens, so the merge-ownership guard (ADR 0060) can name the resolver.
        // The detached Tier-2 / Tier-3 spawns get their binding at session
        // registration instead, because their session does not exist yet when
        // they open the pairing; Tier 1 injects the prompt into a session that
        // is already running, so the binding belongs here.
        self.bind_session_to_conflict_resolution(thread_id, change_id)
            .await;
        let main_at_start = branch_head_sha(repo_root, "main").await;
        let prompt = self
            .start_merge_and_get_prompt(thread_id, change_id, conflict_files, "main", None, None)
            .await;

        let answered = send_awaited_prompt(msg_tx, prompt)
            .map_err(|e| format!("Failed to send merge prompt to the coding-agent session: {e}"))?;

        if let Err(e) = wait_for_prompt_turn(answered, "merging main and resolving conflicts").await
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

        // Verify CC completed the merge it was asked for. Changes that landed
        // on main since are the catch-up's to merge, below.
        if !resolution_merged_main(repo_root, main_at_start.as_deref(), branch_name).await {
            return Err(
                "Coding agent session ended without completing the merge. Try applying again."
                    .into(),
            );
        }

        // A leftover edit that fails to commit fails the apply before `main`
        // moves. Fast-forwarding anyway publishes only the committed part.
        commit_worktree_or_err(
            worktree_path,
            "Coding agent changes (post-merge auto-commit)",
        )
        .await
        .map_err(|e| format!("Could not commit the leftover merge edits: {e}"))?;

        // The resolving session edited after the apply-time bound check, so
        // this rechecks a bounded security fix, as Tiers 2 and 3 do. Both
        // callers treat an `Err` as a failed apply, never as a cue to escalate.
        if let Some(refusal) = self
            .bounded_fix_refusal_for_resolution(repo_root, worktree_path, branch_name, branch_name)
            .await
        {
            return Err(refusal.into());
        }

        catchup_and_ff_to_main(repo_root, worktree_path, branch_name)
            .await
            .map_err(|e| format!("ff-merge to main failed after CC merge: {}", e).into())
    }

    /// Atomically claim a thread's live session for an in-place merge.
    ///
    /// Locks `agent_sessions` once so the "is there a live session?" check and
    /// the `change_claim` claim can't race a concurrent apply. The
    /// `apply_change` Tier-1 path used to do `is_running_for` then a separate
    /// `live_session_info`, a TOCTOU window where two apply calls (e.g. the LLM
    /// calling `apply_change` twice in quick succession) could both start an
    /// in-place merge on the same session and corrupt it by sending two merge
    /// prompts down `msg_tx`. Returns `Ready` with the flag already set; the
    /// caller MUST clear `change_claim` when the merge finishes
    /// (`spawn_in_place_conflict_recovery` does this in all arms).
    pub(crate) async fn begin_in_place_merge(&self, thread_id: Uuid) -> InPlaceMergeStart {
        let mut guard = self.agent_sessions.lock().await;
        match decide_in_place_merge_claim(guard.get(&thread_id)) {
            InPlaceMergeClaim::NoLiveSession => InPlaceMergeStart::NoLiveSession,
            InPlaceMergeClaim::Claimed(holder) => InPlaceMergeStart::Claimed(holder),
            InPlaceMergeClaim::SessionStopping => InPlaceMergeStart::SessionStopping,
            InPlaceMergeClaim::Claim => {
                // Re-fetch mutably to set the claim flag; presence + worktree
                // were validated by `decide_in_place_merge_claim` above under the
                // same lock, so the unwraps cannot fire.
                let s = guard
                    .get_mut(&thread_id)
                    .expect("session present (checked under lock)");
                s.change_claim = Some(crate::engine::types::ChangeClaim::Apply);
                InPlaceMergeStart::Ready(crate::engine::change_ops::LiveSessionInfo {
                    worktree_path: s
                        .worktree_path
                        .clone()
                        .expect("worktree present (checked under lock)"),
                    msg_tx: s.msg_tx.clone(),
                    last_event_at: s.last_event_at.clone(),
                })
            }
        }
    }

    /// Clear the `change_claim` claim `claimant` took. Used by the
    /// `apply_change` Tier-1 fast path, which claims the session via
    /// `begin_in_place_merge` then finalizes a clean fast-forward inline (no
    /// background task to clear it). Idempotent — a missing session is a no-op.
    pub(crate) async fn clear_change_claim(
        &self,
        thread_id: Uuid,
        claimant: &tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
    ) {
        release_change_claim(&mut *self.agent_sessions.lock().await, thread_id, claimant);
    }

    /// Run the CC-assisted conflict merge as a guarded background task and
    /// finalize, then return immediately. This is the async counterpart of the
    /// `apply_now` spawn: same liveness-timeout (abort if CC goes silent for 10
    /// minutes), panic guard, and always-clear of `change_claim`.
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
        claim: crate::engine::change_ops::apply_claim::ApplyClaim,
        session: crate::engine::change_ops::LiveSessionInfo,
        branch_name: String,
        repo_root: std::path::PathBuf,
        requires_restart: bool,
        client_update: bool,
        actor: Option<MessageOrigin>,
    ) {
        use std::sync::atomic::Ordering;
        let engine = self.clone_arc();
        let change_id = claim.change_id();
        tokio::spawn(async move {
            let crate::engine::change_ops::LiveSessionInfo {
                worktree_path,
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
            release_change_claim(&mut *engine.agent_sessions.lock().await, thread_id, &msg_tx);

            match result {
                Ok((pre_sha, post_sha)) => {
                    engine
                        .apply_now_success(
                            &claim,
                            thread_id,
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
        claim: &crate::engine::change_ops::apply_claim::ApplyClaim,
        thread_id: Uuid,
        requires_restart: bool,
        client_update: bool,
        pre_sha: &str,
        post_sha: &str,
        worktree_path: &Path,
        repo_root: &Path,
        branch_name: &str,
        actor: Option<MessageOrigin>,
    ) -> Vec<String> {
        let change_id = claim.change_id();
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
            claim,
            thread_id,
            requires_restart,
            client_update,
            commits.clone(),
            thread_title,
            actor.clone(),
            Some(crate::engine::change_ops::AppliedMerge::new(
                pre_sha, post_sha,
            )),
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
                    Some(change.thread_id.unwrap_or(thread_id)),
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

        // The branch is reset for reuse below. `emit_change_applied` cleared
        // the applied change's branch markers, so new work re-triggers both.
        self.reset_worktree_and_idle(thread_id, worktree_path, branch_name)
            .await;
        push_main_in_background(repo_root, &self.event_bus);
        self.broadcast_changes_updated().await;

        commits
    }

    /// Refuse an Apply Now the proposal hold keeps. The branch keeps its work
    /// and the session stays alive to clear the hold.
    async fn refuse_apply_now_for_hold(
        &self,
        thread_id: Uuid,
        worktree_path: &Path,
        hold: ProposalHold,
        actor: Option<MessageOrigin>,
    ) {
        self.refuse_apply_now(thread_id, worktree_path, hold.apply_refusal(), actor)
            .await;
    }

    /// Refuse an Apply Now with `error`. The branch keeps its work and the
    /// session stays alive.
    async fn refuse_apply_now(
        &self,
        thread_id: Uuid,
        worktree_path: &Path,
        error: &str,
        actor: Option<MessageOrigin>,
    ) {
        use crate::engine::event_bus::BusEvent;
        use crate::engine::thread_events::{EventMeta, ThreadEvent};

        log!("[ApplyNow] Refused for thread {}: {}", thread_id, error);
        self.event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::ChangeApplyFailed {
                        change_id: String::new(),
                        error: error.to_string(),
                        actor,
                    },
                    meta: EventMeta::NONE,
                },
                "[ApplyNow] ChangeApplyFailed",
            )
            .await;
        self.mark_session_idle(thread_id, worktree_path).await;
    }

    /// What an Apply with nothing pending answers. The proposal hold names
    /// itself; otherwise the work waits on a turn that finishes.
    async fn nothing_proposed_refusal(&self, thread_id: Uuid) -> &'static str {
        match self.branch_work_hold(thread_id).await {
            Some(hold) => hold.apply_refusal(),
            None => NOTHING_PROPOSED_REFUSAL,
        }
    }

    /// Reset worktree to main and re-enter idle state.
    ///
    /// Used after apply, discard, and no-commits to keep the session alive.
    /// ONLY safe when the branch's work is already on main (or there is no
    /// work). On a path where the change is still pending, call
    /// [`Self::mark_session_idle`] instead. See
    /// [`reset_session_worktree_to_main`] for the branch check.
    pub(crate) async fn reset_worktree_and_idle(
        &self,
        thread_id: Uuid,
        worktree_path: &Path,
        session_branch: &str,
    ) {
        reset_session_worktree_to_main(worktree_path, session_branch).await;
        self.mark_session_idle(thread_id, worktree_path).await;
    }

    /// Re-enter the idle state WITHOUT touching the worktree or the branch.
    ///
    /// The half of [`Self::reset_worktree_and_idle`] that is always safe. Use it
    /// when the session must go back to idle but the branch still holds commits
    /// the user owns.
    ///
    /// It announces the idle and leaves the session's phase alone. Only the run
    /// loop writes `is_waiting` and fires `idle_notify`, because only it keeps
    /// its local copy in step. A Discard can land on a mid-turn session, and a
    /// write from here made that turn read idle until its `Result`.
    pub(crate) async fn mark_session_idle(&self, thread_id: Uuid, worktree_path: &Path) {
        use crate::engine::event_bus::BusEvent;
        use crate::engine::thread_events::{EventMeta, ThreadEvent};

        let cc_sid = self
            .agent_sessions
            .lock()
            .await
            .get(&thread_id)
            .and_then(|s| s.backend_session_id.clone());

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

async fn emit_apply_now_ended(
    event_bus: &crate::engine::event_bus::EventBus,
    thread_id: Uuid,
    outcome: ApplyNowOutcome,
) -> Option<i64> {
    match event_bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: crate::engine::thread_events::ThreadEvent::ApplyNowEnded { outcome },
            meta: crate::engine::thread_events::EventMeta::NONE,
        })
        .await
    {
        Ok(emitted) => emitted.and_then(|r| r.summary_version),
        Err(e) => {
            log!(
                "[ApplyNow] ApplyNowEnded emit failed for {}: {}",
                thread_id,
                e
            );
            None
        }
    }
}

/// End every Apply Now a stop cut off. The apply task dies with the engine. So
/// an apply still in flight at startup has nobody left to end it, and the
/// thread would read as applying for good (ADR 0437). A failed read leaves the
/// flags to the next startup.
pub(crate) async fn end_interrupted_applies(
    pool: &sqlx::PgPool,
    event_bus: &crate::engine::event_bus::EventBus,
) {
    let interrupted = sqlx::query_scalar::<_, Uuid>(
        "SELECT thread_id FROM thread_summaries WHERE apply_in_flight",
    )
    .fetch_all(pool)
    .await;
    match interrupted {
        Ok(threads) => {
            for thread_id in threads {
                emit_apply_now_ended(event_bus, thread_id, ApplyNowOutcome::Interrupted).await;
            }
        }
        Err(e) => log!("[Startup] Reading in-flight applies failed: {}", e),
    }
}

/// `git reset --hard main` then `git clean -fd` in the session's worktree, but
/// only while its HEAD is still on `session_branch`.
///
/// The reset moves whatever branch HEAD is attached to. Claude Code can
/// `git checkout` inside its own worktree, and a reset there would rewind that
/// other branch to main and wipe its untracked files. So a worktree on another
/// branch, or one whose branch git cannot name, is left untouched.
///
/// The `clean` waits on the reset landing, because the reset is what makes
/// deleting untracked files safe: run it alone and the branch keeps its
/// commits while the agent's untracked work is gone.
///
/// The base stays the literal `main`, deliberately. Resolving the repo's real
/// default here would make this line disagree with the apply pipeline that
/// runs immediately before it: `catchup_and_ff_to_main` publishes to `main`, so
/// a reset onto anything else rewinds the session branch off the work just
/// applied. The whole family has to move together, which is tracked as
/// `harden-hardcoded-main-branch-in-change-ops`.
async fn reset_session_worktree_to_main(worktree_path: &Path, session_branch: &str) {
    let worktree_branch = worktree_current_branch(worktree_path).await;
    if let Some(reason) = foreign_worktree_reason(worktree_branch.as_deref(), session_branch) {
        log!(
            "[ApplyNow] Not resetting {} to main for session branch {}: {}",
            worktree_path.display(),
            session_branch,
            reason
        );
        return;
    }
    match git_ran_ok(&["reset", "--hard", "main"], worktree_path).await {
        Ok(()) => {
            if let Err(e) = git_ran_ok(&["clean", "-fd"], worktree_path).await {
                log!(
                    "[ApplyNow] git clean -fd failed in {}: {}",
                    worktree_path.display(),
                    e
                );
            }
        }
        Err(e) => log!(
            "[ApplyNow] git reset --hard main failed in {}: {}. Leaving the tree alone, so the worktree still holds whatever was there",
            worktree_path.display(),
            e
        ),
    }
}

/// The paths a merge of `main` into the worktree's HEAD would conflict on, for
/// the conflict prompt and panel. Empty for a clean merge, and for a probe git
/// could not answer: the merge session runs either way and finds the conflicts
/// itself, so an unknown list costs only the preview.
pub(crate) async fn probe_merge_conflicts(worktree_path: &Path) -> Vec<String> {
    crate::engine::git_ops::probe_merge(worktree_path, "HEAD", "main")
        .await
        .into_conflicts()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::types::ChangeClaim;
    use crate::engine::AgentSession;
    use tokio::sync::mpsc;

    /// Build a minimal `AgentSession` for claim-decision tests. `worktree` and
    /// `process_exited` / `change_claim` are the fields the claim state
    /// machine reads; everything else is inert defaults. The receiver comes back
    /// with it — drop it and the session is a phantom, which the claim treats as
    /// no live session (see `AgentSession::is_live`).
    fn claim_test_session(
        process_exited: bool,
        worktree: Option<&str>,
        change_claim: Option<ChangeClaim>,
    ) -> (AgentSession, mpsc::UnboundedReceiver<AgentUserInput>) {
        let (mut session, msg_rx) = AgentSession::for_test();
        session.is_waiting = !process_exited;
        session.process_exited = process_exited;
        session.change_claim = change_claim;
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
        let (s, _msg_rx) = claim_test_session(true, Some("/wt"), None);
        assert_eq!(
            decide_in_place_merge_claim(Some(&s)),
            InPlaceMergeClaim::NoLiveSession
        );
    }

    #[test]
    fn claim_none_when_no_worktree() {
        let (s, _msg_rx) = claim_test_session(false, None, None);
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
        for holder in [ChangeClaim::Apply, ChangeClaim::Discard] {
            let (s, _msg_rx) = claim_test_session(false, Some("/wt"), Some(holder));
            assert_eq!(
                decide_in_place_merge_claim(Some(&s)),
                InPlaceMergeClaim::Claimed(holder),
                "the refusal names who holds the claim"
            );
        }
    }

    #[test]
    fn claim_ok_when_live_idle_with_worktree() {
        let (s, _msg_rx) = claim_test_session(false, Some("/wt"), None);
        assert_eq!(
            decide_in_place_merge_claim(Some(&s)),
            InPlaceMergeClaim::Claim
        );
    }

    /// A Stop has been accepted and the loop has not broken out yet, so the
    /// session still reads live. An apply claiming it now would merge on a
    /// session whose teardown is under way, and a Discard deletes the branch.
    #[test]
    fn claim_refused_while_a_stop_is_pending() {
        use crate::engine::StopReason;
        for reason in [StopReason::Apply, StopReason::Discard, StopReason::Archive] {
            let (mut s, _msg_rx) = claim_test_session(false, Some("/wt"), None);
            s.pending_stop = Some(reason);
            assert_eq!(
                decide_in_place_merge_claim(Some(&s)),
                InPlaceMergeClaim::SessionStopping,
                "{reason:?}"
            );
        }
    }

    /// An apply task outlives the session it claimed: the session ended, and a
    /// replacement registered and took its own claim. The old task's release
    /// must not clear the replacement's, or a second apply merges beside it.
    #[test]
    fn an_old_apply_task_leaves_a_replacements_claim_alone() {
        let thread_id = uuid::Uuid::new_v4();
        let (old, _old_rx) = claim_test_session(false, Some("/wt"), Some(ChangeClaim::Apply));
        let old_claimant = old.msg_tx.clone();
        let (replacement, _rx) = claim_test_session(false, Some("/wt"), Some(ChangeClaim::Apply));
        let mut sessions = std::collections::HashMap::from([(thread_id, replacement)]);

        release_change_claim(&mut sessions, thread_id, &old_claimant);
        assert!(
            sessions[&thread_id].change_claim.is_some(),
            "the replacement's claim belongs to its own apply"
        );

        let own_claimant = sessions[&thread_id].msg_tx.clone();
        release_change_claim(&mut sessions, thread_id, &own_claimant);
        assert!(sessions[&thread_id].change_claim.is_none());
    }

    /// An in-session Discard awaits inside its HTTP request. A client that
    /// disconnects mid-reset drops that future before its release line runs.
    /// The guard must free the claim anyway, or every Apply, Discard and Stop
    /// on the thread answers 409 until the session ends.
    #[tokio::test]
    async fn a_dropped_change_claim_guard_still_releases_the_claim() {
        let thread_id = uuid::Uuid::new_v4();
        let (session, _rx) = claim_test_session(false, Some("/wt"), Some(ChangeClaim::Apply));
        let claimant = session.msg_tx.clone();
        let sessions = Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::from([
            (thread_id, session),
        ])));

        drop(ChangeClaimGuard::new(
            sessions.clone(),
            thread_id,
            claimant.clone(),
        ));
        for _ in 0..100 {
            if sessions.lock().await[&thread_id].change_claim.is_none() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(sessions.lock().await[&thread_id].change_claim.is_none());

        // Cancelled while `release` waits for a busy map: `Drop` must still free it.
        sessions
            .lock()
            .await
            .get_mut(&thread_id)
            .unwrap()
            .change_claim = Some(ChangeClaim::Discard);
        let busy = sessions.lock().await;
        let mut release = Box::pin(
            ChangeClaimGuard::new(sessions.clone(), thread_id, claimant.clone()).release(),
        );
        assert!(futures::FutureExt::now_or_never(&mut release).is_none());
        drop(release);
        drop(busy);
        for _ in 0..100 {
            if sessions.lock().await[&thread_id].change_claim.is_none() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(sessions.lock().await[&thread_id].change_claim.is_none());

        sessions
            .lock()
            .await
            .get_mut(&thread_id)
            .unwrap()
            .change_claim = Some(ChangeClaim::Discard);
        ChangeClaimGuard::new(sessions.clone(), thread_id, claimant)
            .release()
            .await;
        assert!(
            sessions.lock().await[&thread_id].change_claim.is_none(),
            "`release` frees it before returning"
        );
    }

    /// Every production release goes through `release_change_claim`, so none
    /// can clear a replacement's claim by thread id alone.
    #[test]
    fn the_apply_claim_is_released_in_one_place() {
        let writers: Vec<(String, usize)> = crate::test_support::source_scan::production_sources()
            .into_iter()
            .map(|(path, src)| {
                let clears = src.matches("change_claim = None").count()
                    + src.matches("change_claim.take()").count();
                (path, clears)
            })
            .filter(|(_, count)| *count > 0)
            .collect();
        assert_eq!(
            writers,
            vec![("engine/agent_session/apply_now.rs".to_string(), 1)],
            "only `release_change_claim` may clear the claim"
        );
    }

    /// Apply Now asks about a parked question before either branch can prompt
    /// the agent. Checked after the claim or the fast path, a hardening run
    /// could already be on its way. The predicate itself is pinned by
    /// `a_proposal_keeps_the_thread_on_its_open_question`.
    #[test]
    fn apply_now_refuses_a_parked_question_before_it_prompts_anything() {
        let src = include_str!("apply_now.rs");
        let production = &src[..src.find("\n#[cfg(test)]").unwrap()];
        let body = &production[production.find("pub async fn apply_now(").unwrap()..];
        let at = |needle: &str| {
            body.find(needle)
                .unwrap_or_else(|| panic!("apply_now no longer contains {needle:?}"))
        };
        for refusal in ["QUESTION_OPEN_MESSAGE", "QUESTION_UNKNOWN_MESSAGE"] {
            for later in [
                "session.change_claim = Some(",
                "pending_for_thread(thread_id)",
                "self.nothing_proposed_refusal(thread_id)",
            ] {
                assert!(
                    at(refusal) < at(later),
                    "{refusal} must be returned before {later:?}"
                );
            }
        }
    }

    /// `apply_in_flight` follows the bracket: `ApplyNowStarted` sets it and
    /// `ApplyNowEnded` clears it. Startup ends an apply a stop cut off, and
    /// leaves one that already ended alone (ADR 0437).
    #[tokio::test]
    async fn an_apply_cut_off_by_a_stop_is_ended_at_startup() {
        use crate::engine::event_bus::{BusEvent, EventBus};
        use crate::engine::thread_events::{EventMeta, ThreadEvent};

        let (pool, db) = crate::test_support::setup_test_db().await;
        let (bus, _parent_rx) = EventBus::new(pool.clone());
        let in_flight = |id: Uuid| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, bool>(
                    "SELECT apply_in_flight FROM thread_summaries WHERE thread_id = $1",
                )
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
            }
        };
        let emit = |id: Uuid, event: ThreadEvent| {
            let bus = bus.clone();
            async move {
                bus.emit(BusEvent::Thread {
                    thread_id: id,
                    event,
                    meta: EventMeta::NONE,
                })
                .await
                .unwrap();
            }
        };
        let cut_off = crate::test_support::seed_chat_thread(&bus, "apply this").await;
        let finished = crate::test_support::seed_chat_thread(&bus, "apply that").await;

        emit(cut_off, ThreadEvent::ApplyNowStarted).await;
        emit(finished, ThreadEvent::ApplyNowStarted).await;
        assert!(in_flight(cut_off).await, "a started apply is in flight");
        emit(
            finished,
            ThreadEvent::ApplyNowEnded {
                outcome: ApplyNowOutcome::Finished,
            },
        )
        .await;
        assert!(!in_flight(finished).await, "an ended apply is not");

        end_interrupted_applies(&pool, &bus).await;

        assert!(
            !in_flight(cut_off).await,
            "startup ends the apply a stop cut off"
        );
        let ends: Vec<(Uuid, String)> = sqlx::query_as(
            "SELECT thread_id, payload->>'outcome' FROM events WHERE event_type = 'ApplyNowEnded' \
             ORDER BY created",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            ends,
            vec![
                (finished, "finished".to_string()),
                (cut_off, "interrupted".to_string())
            ]
        );

        crate::test_support::teardown_test_db(&db).await;
    }

    /// Every apply that passes its refusals is bracketed, inside a task of its
    /// own so a dropped request cannot cut it. The live task announces the end
    /// on every exit: after `catch_unwind`, so a panic still reaches it, and
    /// after the claim is released. The no-live task brackets its loop.
    #[test]
    fn every_apply_now_is_bracketed_by_its_start_and_end() {
        let src = include_str!("apply_now.rs");
        let production = &src[..src.find("\n#[cfg(test)]").unwrap()];
        let body = &production[production.find("pub async fn apply_now(").unwrap()..];
        let body = &body[..body.find("async fn emit_apply_now_started(").unwrap()];
        let at = |needle: &str, from: usize| {
            body[from..]
                .find(needle)
                .map(|i| i + from)
                .unwrap_or_else(|| panic!("apply_now no longer contains {needle:?}"))
        };

        let fast_path = at("pending_for_thread(thread_id)", 0);
        let fast_spawn = at("tokio::spawn(", fast_path);
        let fast_start = at(".emit_apply_now_started(", fast_path);
        let fast_apply = at("engine.apply_pending_now(", fast_path);
        let fast_end = at(
            "emit_apply_now_ended(thread_id, ApplyNowOutcome::Finished)",
            fast_path,
        );
        assert!(
            fast_spawn < fast_start && fast_start < fast_apply && fast_apply < fast_end,
            "the no-live task brackets its loop"
        );

        let spawn = at("tokio::spawn(", at("let (started_tx, started_rx)", 0));
        let live_start = at(".emit_apply_now_started(", spawn);
        let unwind = at("catch_unwind(", live_start);
        let release = at("release_change_claim(", unwind);
        let live_end = at(
            ".emit_apply_now_ended(thread_id, ApplyNowOutcome::Finished)",
            unwind,
        );
        assert!(
            unwind < release && release < live_end,
            "the end follows a panic and the release"
        );
    }

    /// Apply needs a proposal (ADR 0400). With no live session and nothing
    /// pending, Apply refuses: it never recovers the stale session, which
    /// would propose the branch. The live path checks for the pending change
    /// before it commits, prompts or merges anything.
    #[test]
    fn apply_now_refuses_rather_than_proposes_when_nothing_is_pending() {
        let src = include_str!("apply_now.rs");
        let production = &src[..src.find("\n#[cfg(test)]").unwrap()];
        let start = production.find("pub async fn apply_now(").unwrap();
        let end = production
            .find("pub(crate) async fn wait_and_commit(")
            .unwrap();
        let no_live = &production[start..end];
        assert!(!no_live.contains("end_stale_waiting_session("));
        assert!(!no_live.contains(".propose"), "an Apply never proposes");
        let pending = no_live.find("pending_for_thread(thread_id)").unwrap();
        let refusal = no_live
            .find("self.nothing_proposed_refusal(thread_id)")
            .expect("nothing pending is refused");
        assert!(pending < refusal);

        let inner = &production[production.find("async fn apply_now_inner(").unwrap()..];
        let check = inner
            .find("get_pending_by_branch(branch_name)")
            .expect("the live path reads the pending change");
        let refused = inner
            .find("self.nothing_proposed_refusal(thread_id)")
            .expect("and refuses without one");
        for later in [
            "auto_commit_preserving_marker(",
            "request_hardening_in_session(",
            ".propose_change(",
        ] {
            let at = inner.find(later).unwrap_or_else(|| panic!("{later:?}"));
            assert!(
                check < at && refused < at,
                "{later:?} must follow the check"
            );
        }
    }

    /// Step 1's auto-commit only logs a failure, so a branch can sit under a
    /// worktree still holding uncommitted work, with or without earlier
    /// commits. The dirtiness gate has to run before the first reset on every
    /// path, or Apply wipes that work.
    #[test]
    fn a_failed_auto_commit_is_caught_before_any_merge_or_reset() {
        let src = include_str!("apply_now.rs");
        let body = &src[src.find("async fn apply_now_inner(").unwrap()..];
        let at = |needle: &str| {
            body.find(needle)
                .unwrap_or_else(|| panic!("apply_now_inner no longer contains {needle:?}"))
        };
        let gate = at("still_dirty.or_unknown(true)");
        assert!(at("auto_commit_preserving_marker(") < gate);
        assert!(
            gate < at("let has_commits"),
            "the gate must cover both the commits and the no-commits path"
        );
        assert!(
            gate < at(".merge_via_cc_session("),
            "the gate must precede the merge"
        );
        assert!(
            gate < at("self.reset_worktree_and_idle("),
            "the dirtiness gate must precede every reset in apply_now_inner"
        );
    }

    /// Regression: the Tier 1 resolution session edits after the apply-time
    /// bound check, and its work was fast-forwarded to main unchecked. Tiers
    /// 2 and 3 recheck the bound there, so a bounded fix could widen only here.
    #[test]
    fn the_tier1_resolution_rechecks_the_bound_before_main_moves() {
        let src = include_str!("apply_now.rs");
        let start = src.find("async fn cc_assisted_merge_then_ff(").unwrap();
        let body = &src[start..];
        let body = &body[..body.find("\n    }\n").unwrap()];
        let at = |needle: &str| {
            body.find(needle).unwrap_or_else(|| {
                panic!("cc_assisted_merge_then_ff no longer contains {needle:?}")
            })
        };
        let check = at(".bounded_fix_refusal_for_resolution(");
        assert!(
            at("commit_worktree_or_err(") < check,
            "the check must see the session's last edits"
        );
        assert!(
            check < at("catchup_and_ff_to_main("),
            "the check must come before main moves"
        );
    }

    const SESSION_BRANCH: &str = "lucidos-claude-code-repo-lucidos-reset-gate";

    /// A real repo on `main` with a linked worktree on [`SESSION_BRANCH`]
    /// holding one commit, the way an applied or discarded session finds it.
    async fn session_worktree() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let wt = tmp.path().join("wt");
        tokio::fs::create_dir_all(&repo).await.unwrap();
        let wt_str = wt.to_str().unwrap();
        for args in [
            &["init", "-b", "main"][..],
            &["config", "user.email", "test@example.com"],
            &["config", "user.name", "Test"],
            &["commit", "--allow-empty", "-m", "initial"],
            &["worktree", "add", "-b", SESSION_BRANCH, wt_str],
        ] {
            git_ran_ok(args, &repo).await.unwrap();
        }
        tokio::fs::write(wt.join("session.txt"), "session work")
            .await
            .unwrap();
        git_ran_ok(&["add", "."], &wt).await.unwrap();
        git_ran_ok(&["commit", "-m", "session work"], &wt)
            .await
            .unwrap();
        (tmp, repo, wt)
    }

    async fn rev(repo: &Path, name: &str) -> String {
        let out = git_cmd(&["rev-parse", name], repo).await.unwrap();
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// The reset the session paths depend on still happens on the session's
    /// own branch: the branch is rewound to main and untracked files go.
    #[tokio::test]
    async fn the_reset_rewinds_the_session_branch_it_can_name() {
        let (_tmp, repo, wt) = session_worktree().await;
        tokio::fs::write(wt.join("scratch.txt"), "untracked")
            .await
            .unwrap();

        reset_session_worktree_to_main(&wt, SESSION_BRANCH).await;

        assert_eq!(rev(&repo, SESSION_BRANCH).await, rev(&repo, "main").await);
        assert!(!wt.join("scratch.txt").exists());
    }

    /// **Regression.** The agent ran `git checkout -b` mid-session, then the
    /// user clicked Discard. The reset moved the OTHER branch's ref to main and
    /// `clean -fd` wiped its untracked files. A worktree on another branch is
    /// not the session's to reset.
    #[tokio::test]
    async fn the_reset_leaves_a_worktree_the_agent_moved_to_another_branch() {
        let (_tmp, repo, wt) = session_worktree().await;
        git_ran_ok(&["checkout", "-b", "agent-side-branch"], &wt)
            .await
            .unwrap();
        tokio::fs::write(wt.join("side.txt"), "side work")
            .await
            .unwrap();
        git_ran_ok(&["add", "."], &wt).await.unwrap();
        git_ran_ok(&["commit", "-m", "side work"], &wt)
            .await
            .unwrap();
        tokio::fs::write(wt.join("scratch.txt"), "untracked")
            .await
            .unwrap();
        let side_before = rev(&repo, "agent-side-branch").await;
        let session_before = rev(&repo, SESSION_BRANCH).await;

        reset_session_worktree_to_main(&wt, SESSION_BRANCH).await;

        assert_eq!(
            rev(&repo, "agent-side-branch").await,
            side_before,
            "the other branch's ref must not be rewound to main"
        );
        assert_eq!(rev(&repo, SESSION_BRANCH).await, session_before);
        assert!(
            wt.join("scratch.txt").exists(),
            "untracked files on the other branch must survive"
        );
    }

    /// A detached HEAD names no branch, so it is not a positive match either.
    #[tokio::test]
    async fn the_reset_leaves_a_detached_worktree() {
        let (_tmp, repo, wt) = session_worktree().await;
        git_ran_ok(&["checkout", "--detach"], &wt).await.unwrap();
        tokio::fs::write(wt.join("scratch.txt"), "untracked")
            .await
            .unwrap();
        let head_before = rev(&wt, "HEAD").await;

        reset_session_worktree_to_main(&wt, SESSION_BRANCH).await;

        assert_eq!(rev(&wt, "HEAD").await, head_before);
        assert_ne!(head_before, rev(&repo, "main").await);
        assert!(wt.join("scratch.txt").exists());
    }

    /// The other direction: a Stop, Discard or Archive is refused while an apply
    /// holds the claim. A Discard would otherwise wake the apply task's idle
    /// wait with `Ok`, and the task would merge a branch Discard is deleting.
    #[test]
    fn a_stop_is_refused_while_an_apply_holds_the_claim() {
        use crate::engine::claude_code::{
            stop_refusal, APPLY_IN_PROGRESS_MESSAGE, DISCARD_IN_PROGRESS_MESSAGE,
        };
        let (applying, _rx) = claim_test_session(false, Some("/wt"), Some(ChangeClaim::Apply));
        assert_eq!(stop_refusal(&applying), Some(APPLY_IN_PROGRESS_MESSAGE));
        let (discarding, _rx) = claim_test_session(false, Some("/wt"), Some(ChangeClaim::Discard));
        assert_eq!(
            stop_refusal(&discarding),
            Some(DISCARD_IN_PROGRESS_MESSAGE),
            "a Discard's refusal must never read as an apply in progress"
        );

        let (free, _rx) = claim_test_session(false, Some("/wt"), None);
        assert_eq!(crate::engine::claude_code::stop_refusal(&free), None);
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
