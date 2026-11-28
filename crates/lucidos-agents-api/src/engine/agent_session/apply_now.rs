use super::lifecycle::foreign_worktree_reason;
use crate::engine::agent_recovery::orphaned_branch_work::OrphanedBranchWorkExt;
use crate::engine::change_ops::{
    apply_must_harden, branch_is_hardened, now_epoch_millis, MergeOwnership, PlanFloor,
    ProposalHold, ProposeOutcome, MERGE_OWNED_BY_RESOLVER_MESSAGE,
};
use crate::engine::coding_turn_end::send_awaited_prompt;
use crate::engine::git_ops::{
    auto_commit_preserving_marker, auto_commit_safe_files_if_dirty, branch_changed_files,
    branch_head_sha, catchup_and_ff_to_main, commit_worktree_or_err, commits_in_range,
    default_local_branch, describe_branch_changes, files_have_client_update, files_require_restart,
    git_answer, git_answer_when_ok, git_cmd, git_ran_ok, has_branch_commits, lands_plan_files_only,
    push_main_in_background, resolution_merged_main, worktree_current_branch,
};
use crate::engine::in_place_merge::emit_apply_now_ended;
use crate::engine::in_place_merge::{decide_in_place_merge_claim, InPlaceMergeClaim};
use crate::engine::release_change_claim;
use crate::engine::thread_events::{ApplyNowOutcome, EventChannel, MessageOrigin};
use crate::engine::{AgentUserInput, InputAnswered, LucidosEngine};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

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
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (thread_id, claimant) = (self.thread_id, self.claimant.clone());
        handle.spawn(async move {
            release_change_claim(&mut *sessions.lock().await, thread_id, &claimant);
            log_legacy!(
                "[AgentSession] A change claim on thread {} outlived its work and was released on drop",
                thread_id
            );
        });
    }
}

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

async fn commit_after_prompt_turn(
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

pub(crate) const NOTHING_PROPOSED_REFUSAL: &str =
    "Nothing is proposed on this thread, so there is nothing to apply. A turn that \
     finishes proposes its work.";

#[easy_ext::ext(ApplyNowExt)]
pub(crate) impl LucidosEngine {
    async fn apply_now(
        self: &Arc<Self>,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<Option<i64>, Box<dyn std::error::Error + Send + Sync>> {
        if self.merge_ownership_for_thread(thread_id).await == MergeOwnership::ResolverOwnsIt {
            log_legacy!(
                "[ApplyNow] Refused for thread {}: a conflict resolution is in flight and owns the merge",
                thread_id
            );
            return Err(MERGE_OWNED_BY_RESOLVER_MESSAGE.into());
        }

        match crate::engine::question_park_sql::thread_parked_on_question(self.pool(), thread_id)
            .await
        {
            Ok(false) => {}
            Ok(true) => {
                log_legacy!(
                    "[ApplyNow] Refused for thread {}: it is parked on a question",
                    thread_id
                );
                return Err(crate::engine::claude_code::QUESTION_OPEN_MESSAGE.into());
            }
            Err(e) => {
                log_legacy!(
                    "[ApplyNow] Refused for thread {}: could not check for a parked question: {}",
                    thread_id,
                    e
                );
                return Err(crate::engine::claude_code::QUESTION_UNKNOWN_MESSAGE.into());
            }
        }

        let (worktree_path, branch_name, repo_root, msg_tx, last_event_at) = {
            let mut guard = self.agent_sessions.lock().await;
            match guard.get_mut(&thread_id) {
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
                            return Err(
                                crate::engine::session_claim_refusal::SESSION_STOPPING_MESSAGE
                                    .into(),
                            );
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

                    let pending = self.changes().pending_for_thread(thread_id).await?;
                    if !pending.is_empty() {
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
                                log_legacy!(
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

                    let error = self.nothing_proposed_refusal(thread_id).await;
                    log_legacy!("[ApplyNow] Refused for thread {}: {}", thread_id, error);
                    let refused = self
                        .event_bus
                        .emit(crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ChangeApplyFailed {
                                pre_merge_sha: None,
                                post_merge_sha: None,
                                landing_merge_sha: None,
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
                            log_legacy!(
                                "[ApplyNow] ChangeApplyFailed (no live session, nothing pending) emit failed: {}",
                                e
                            );
                            None
                        }
                    });
                }
            }
        };

        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let engine = self.clone_arc();
        tokio::spawn(async move {
            let started = engine
                .emit_apply_now_started(thread_id, actor.clone())
                .await;
            let refused = started.is_err();
            let _ = started_tx.send(started);
            if refused {
                release_change_claim(&mut *engine.agent_sessions.lock().await, thread_id, &msg_tx);
                return;
            }
            let panic_result = std::panic::AssertUnwindSafe(async {
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
                        }
                    }
                }
            });

            let result: Result<(), Box<dyn std::error::Error + Send + Sync>> =
                match futures::FutureExt::catch_unwind(panic_result).await {
                    Ok(inner) => inner,
                    Err(_) => Err("apply_now_inner panicked".into()),
                };

            release_change_claim(&mut *engine.agent_sessions.lock().await, thread_id, &msg_tx);

            if let Err(e) = result {
                log_legacy!("[ApplyNow] Failed for thread {}: {}", thread_id, e);
                engine
                    .event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::ChangeApplyFailed {
                                pre_merge_sha: None,
                                post_merge_sha: None,
                                landing_merge_sha: None,
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
            engine
                .emit_apply_now_ended(thread_id, ApplyNowOutcome::Finished)
                .await;
        });

        started_rx
            .await
            .map_err(|_| format!("The apply task for {thread_id} ended before it started"))?
    }

    async fn apply_pending_now(
        self: &Arc<Self>,
        thread_id: Uuid,
        pending: Vec<crate::core::changes::Change>,
        actor: Option<MessageOrigin>,
    ) {
        log_legacy!(
            "[ApplyNow] No live session for thread {} but {} pending change(s) — applying directly without stale-recovery",
            thread_id,
            pending.len()
        );
        for change in pending {
            log_legacy!(
                "[ApplyNow] Applying pending change {} on resumed thread {}",
                change.id,
                thread_id
            );
            if let Err(e) = self.apply_change(change.id, actor.clone()).await {
                log_legacy!("[ApplyNow] apply_change for {} failed: {}", change.id, e);
            }
        }
    }

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

    async fn emit_apply_now_ended(&self, thread_id: Uuid, outcome: ApplyNowOutcome) -> Option<i64> {
        emit_apply_now_ended(&self.event_bus, thread_id, outcome).await
    }

    async fn wait_and_commit(
        &self,
        answered: tokio::sync::oneshot::Receiver<InputAnswered>,
        worktree_path: &Path,
        context: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        commit_after_prompt_turn(answered, worktree_path, context).await
    }

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

        let pending = match self.changes().get_pending_by_branch(branch_name).await {
            Ok(row) => row.is_some(),
            Err(e) => {
                log_legacy!("[ApplyNow] pending lookup for {}: {}", branch_name, e);
                false
            }
        };
        if !pending {
            let error = self.nothing_proposed_refusal(thread_id).await;
            self.refuse_apply_now(thread_id, worktree_path, error, actor)
                .await;
            return Ok(());
        }

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
        let landing = self.landing_files(repo_root, branch_name).await;
        let (floor, marker) = self.plan_floor(&kind_ctx, repo_root, branch_name).await;
        let floor = floor.with_landing(landing.as_deref().unwrap_or_default());
        if !matches!(floor, PlanFloor::Clear(kind) if kind.is_file_bounded()) {
            self.observe_apply_plan(
                thread_id,
                kind_ctx.kind,
                &marker,
                landing.as_deref(),
                matches!(floor, PlanFloor::Held(_)),
            )
            .await;
        }
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

        let mut hardened = !kind_ctx.is_app()
            && branch_is_hardened(&self.pool, self.changes(), repo_root, branch_name).await;
        if apply_must_harden(&kind_ctx, hardened, landing.as_deref()) {
            let answered = self.request_hardening_in_session(thread_id, msg_tx).await?;
            self.wait_and_commit(answered, worktree_path, "waiting for hardening")
                .await?;

            if !branch_is_hardened(&self.pool, self.changes(), repo_root, branch_name).await {
                log_legacy!(
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
                self.mark_session_idle(thread_id, worktree_path).await;
                return Ok(());
            }
            hardened = true;
        }

        let still_dirty = git_answer_when_ok(
            &["status", "--porcelain", "--ignore-submodules=all"],
            worktree_path,
            |o| !o.stdout.is_empty(),
        )
        .await;
        if still_dirty.or_unknown(true) {
            log_legacy!(
                "[ApplyNow] Branch {}'s worktree is still dirty (or unreadable) after the \
                 auto-commit. Leaving the worktree untouched",
                branch_name
            );
            self.event_bus
                .emit_or_log(
                    BusEvent::Thread {
                        thread_id,
                        event: ThreadEvent::ChangeApplyFailed {
                            pre_merge_sha: None,
                            post_merge_sha: None,
                            landing_merge_sha: None,
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

        let has_commits = has_branch_commits(repo_root, branch_name).await;

        if !has_commits {
            let pending_change = self
                .changes()
                .pending_for_thread(thread_id)
                .await
                .unwrap_or_else(|e| {
                    log_legacy!(
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
                        log_legacy!(
                            "[ApplyNow] Change {} is being applied elsewhere or was resolved; leaving it to that path",
                            change.id
                        );
                        return Ok(());
                    }
                },
                None => None,
            };
            if let Some((change, claim)) = pending_change {
                if let Err(refusal) = crate::engine::thread_events::emit_change_apply_requested(
                    &self.event_bus,
                    thread_id,
                    change.id,
                    &change.branch_name,
                    &change.repo_root,
                    actor.clone(),
                )
                .await
                {
                    self.emit_apply_failed(thread_id, change.id, &refusal.to_string(), actor)
                        .await;
                    return Ok(());
                }
                let already_on_main = change.files.is_empty()
                    || crate::engine::git_ops::main_history_touches_files(repo_root, &change.files)
                        .await;
                if already_on_main {
                    log_legacy!(
                        "[ApplyNow] Branch {} already present on main — marking pending change {} applied (no-op)",
                        branch_name,
                        change.id
                    );
                    if let Err(error) = self
                        .emit_change_applied(
                            &claim,
                            thread_id,
                            false,
                            false,
                            Vec::new(),
                            change.thread_title.clone(),
                            actor.clone(),
                            None,
                        )
                        .await
                    {
                        log_legacy!("[ApplyNow] Could not record merged change: {}", error);
                        return Ok(());
                    }
                    self.reset_worktree_and_idle(thread_id, worktree_path, branch_name)
                        .await;
                    push_main_in_background(repo_root, &self.event_bus);
                    self.broadcast_changes_updated().await;
                    return Ok(());
                }
            }

            log_legacy!(
                "[ApplyNow] No commits on branch {} — nothing to apply",
                branch_name
            );
            self.event_bus
                .emit_or_log(
                    BusEvent::Thread {
                        thread_id,
                        event: ThreadEvent::ChangeApplyFailed {
                            pre_merge_sha: None,
                            post_merge_sha: None,
                            landing_merge_sha: None,
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

        // INVARIANT: Recovery tests use this boundary: // Step 4: Propose change
        let changed_files = branch_changed_files(repo_root, branch_name).await;
        let requires_restart = files_require_restart(&changed_files);
        let base = default_local_branch(repo_root).await;
        let log_range = format!("{}..{}", base, branch_name);
        let description =
            describe_branch_changes(repo_root, &log_range, "Applied changes", None).await;

        let repo_root_str = repo_root.to_string_lossy();
        let outcome = self
            .propose_change(
                crate::engine::change_ops::ProposeChangeInput {
                    thread_id,
                    branch_name,
                    repo_root: &repo_root_str,
                    description: &description,
                    files: &changed_files,
                    requires_restart,
                    channel: EventChannel::ClaudeCode,
                    hardened,
                    origin: None,
                },
                crate::engine::change_ops::HoldReentry::default(),
            )
            .await?;
        let change_id = match outcome {
            ProposeOutcome::Proposed(id) => id,
            ProposeOutcome::Held(hold) => {
                self.refuse_apply_now_for_hold(thread_id, worktree_path, hold, actor)
                    .await;
                return Ok(());
            }
            ProposeOutcome::Unfinished => {
                return Err("propose_change never withholds a finished turn".into());
            }
            ProposeOutcome::Refused(refusal) => {
                self.refuse_apply_now(thread_id, worktree_path, &refusal.to_string(), actor)
                    .await;
                return Ok(());
            }
            ProposeOutcome::AwaitingLanding(id) => id,
        };
        let Some(claim) = self.claim_pending_change(change_id).await? else {
            log_legacy!(
                "[ApplyNow] Change {} is being applied elsewhere or was resolved; leaving it to that path",
                change_id
            );
            return Ok(());
        };
        if let Err(refusal) = crate::engine::thread_events::emit_change_apply_requested(
            &self.event_bus,
            thread_id,
            change_id,
            branch_name,
            &repo_root_str,
            actor.clone(),
        )
        .await
        {
            self.emit_apply_failed(thread_id, change_id, &refusal.to_string(), actor)
                .await;
            return Ok(());
        }

        self.commit_dirty_logged("Coding agent changes", "apply_now auto-commit")
            .await;
        let dirty = {
            let _main = crate::engine::git_ops::MAIN_LOCKS.lock_main().await;
            auto_commit_safe_files_if_dirty(repo_root).await
        };
        if dirty {
            let msg = "Cannot merge: the repository has uncommitted changes. Commit or stash them first, then try again.";
            self.emit_apply_failed(thread_id, change_id, msg, actor.clone())
                .await;
            return Ok(());
        }

        let marker = self.plan_marker_snapshot(repo_root, branch_name).await;
        if let crate::engine::git_ops::PlanMarkerState::Present(kind) = marker.state {
            if kind.is_file_bounded() {
                let dirty = crate::engine::git_ops::worktree_dirty_files(worktree_path).await;
                let (landing, refusal) = self
                    .bounded_plan_candidate(
                        repo_root,
                        branch_name,
                        marker.file_bound.as_deref(),
                        dirty,
                    )
                    .await;
                if kind_ctx.is_lucidos_source() {
                    self.observe_apply_plan(
                        thread_id,
                        kind_ctx.kind,
                        &marker,
                        landing.as_deref(),
                        refusal.is_some(),
                    )
                    .await;
                }
                if let Some(msg) = refusal {
                    log_legacy!(
                        "[ApplyNow] Apply blocked: bounded security fix left its bound on branch {}",
                        branch_name
                    );
                    self.emit_apply_failed(thread_id, change_id, &msg, actor.clone())
                        .await;
                    return Ok(());
                }
            }
        }

        if crate::engine::landing_lane::takes_landing_lane(&kind_ctx, repo_root, self.repo_root()) {
            self.apply_now_through_lane(thread_id, claim, worktree_path, branch_name, actor)
                .await;
            return Ok(());
        }

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
                if let Err(error) = self
                    .apply_now_success(
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
                    .await
                {
                    log_legacy!("[ApplyNow] Could not record merged change: {}", error);
                    return Ok(());
                }
            }
            Err(e) => {
                self.emit_apply_failed(thread_id, change_id, &e.to_string(), actor.clone())
                    .await;
            }
        }

        Ok(())
    }

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
                log_legacy!("[ApplyNow] change {} vanished before queueing", change_id);
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
                log_legacy!("[ApplyNow] landing lane refused {}: {}", change_id, e);
                false
            }
        };
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

    async fn merge_via_cc_session(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        worktree_path: &Path,
        branch_name: &str,
        repo_root: &Path,
        msg_tx: &tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
    ) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
        if let Ok(shas) = catchup_and_ff_to_main(repo_root, worktree_path, branch_name).await {
            log_legacy!("[MergeViaCC] Fast path succeeded for {}", branch_name);
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

    async fn cc_assisted_merge_then_ff(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        worktree_path: &Path,
        branch_name: &str,
        repo_root: &Path,
        msg_tx: &tokio::sync::mpsc::UnboundedSender<AgentUserInput>,
    ) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>> {
        let conflict_files = probe_merge_conflicts(worktree_path).await;
        log_legacy!(
            "[MergeViaCC] Fast path failed — delegating merge to CC for {} ({} conflicting file(s))",
            branch_name,
            conflict_files.len()
        );
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

        if !resolution_merged_main(repo_root, main_at_start.as_deref(), branch_name).await {
            return Err(
                "Coding agent session ended without completing the merge. Try applying again."
                    .into(),
            );
        }

        commit_worktree_or_err(
            worktree_path,
            "Coding agent changes (post-merge auto-commit)",
        )
        .await
        .map_err(|e| format!("Could not commit the leftover merge edits: {e}"))?;

        // INVARIANT: The resolving session edited after the apply-time bound check, so
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

    #[allow(clippy::too_many_arguments)]
    fn spawn_in_place_conflict_recovery(
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

            let panic_result = std::panic::AssertUnwindSafe(async {
                let inactivity_limit_ms: i64 = 600_000;
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

            release_change_claim(&mut *engine.agent_sessions.lock().await, thread_id, &msg_tx);

            match result {
                Ok((pre_sha, post_sha)) => {
                    if let Err(error) = engine
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
                        .await
                    {
                        log_legacy!(
                            "[Changes] Could not record merged change {}: {}",
                            change_id,
                            error
                        );
                    }
                }
                Err(e) => {
                    log_legacy!(
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

    #[allow(clippy::too_many_arguments)]
    async fn apply_now_success(
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
    ) -> Result<(Vec<String>, crate::engine::BuildRequest), Box<dyn std::error::Error + Send + Sync>>
    {
        let change_id = claim.change_id();
        let commits = commits_in_range(repo_root, pre_sha, post_sha).await;
        let thread_title = sqlx::query_scalar::<_, String>(
            "SELECT title FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(&self.pool)
        .await
        .unwrap_or_else(|e| {
            log_legacy!(
                "[ApplyNow] Failed to load thread title for {}: {}",
                thread_id,
                e
            );
            None
        });
        let build_request = self
            .emit_change_applied(
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
            .await?;
        self.discard_orphaned_pending_siblings(thread_id, change_id, actor.clone())
            .await;
        match self.changes().get_by_id(change_id).await {
            Ok(Some(change)) => {
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
            }
            Ok(None) => {
                log_legacy!(
                    "[ApplyNow] entity-event emission skipped — change {} not found post-apply",
                    change_id
                );
            }
            Err(e) => {
                log_legacy!(
                    "[ApplyNow] entity-event emission skipped — get_by_id({}) failed: {}",
                    change_id,
                    e
                );
            }
        }

        self.reset_worktree_and_idle(thread_id, worktree_path, branch_name)
            .await;
        push_main_in_background(repo_root, &self.event_bus);
        self.broadcast_changes_updated().await;

        Ok((commits, build_request))
    }

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

    async fn refuse_apply_now(
        &self,
        thread_id: Uuid,
        worktree_path: &Path,
        error: &str,
        actor: Option<MessageOrigin>,
    ) {
        use crate::engine::event_bus::BusEvent;
        use crate::engine::thread_events::{EventMeta, ThreadEvent};

        log_legacy!("[ApplyNow] Refused for thread {}: {}", thread_id, error);
        self.event_bus
            .emit_or_log(
                BusEvent::Thread {
                    thread_id,
                    event: ThreadEvent::ChangeApplyFailed {
                        pre_merge_sha: None,
                        post_merge_sha: None,
                        landing_merge_sha: None,
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

    async fn nothing_proposed_refusal(&self, thread_id: Uuid) -> &'static str {
        match self.branch_work_hold(thread_id).await {
            Some(hold) => hold.apply_refusal(),
            None => NOTHING_PROPOSED_REFUSAL,
        }
    }

    async fn reset_worktree_and_idle(
        &self,
        thread_id: Uuid,
        worktree_path: &Path,
        session_branch: &str,
    ) {
        reset_session_worktree_to_main(worktree_path, session_branch).await;
        self.mark_session_idle(thread_id, worktree_path).await;
    }

    async fn mark_session_idle(&self, thread_id: Uuid, worktree_path: &Path) {
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
                        worktree_head_sha: None,
                        bg_bash_pending: false,
                        turn_end: None,
                    },
                    meta: EventMeta::NONE,
                },
                "[ApplyNow] CodingAgentIdled",
            )
            .await;
    }
}

async fn reset_session_worktree_to_main(worktree_path: &Path, session_branch: &str) {
    let worktree_branch = worktree_current_branch(worktree_path).await;
    if let Some(reason) = foreign_worktree_reason(worktree_branch.as_deref(), session_branch) {
        log_legacy!(
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
                log_legacy!(
                    "[ApplyNow] git clean -fd failed in {}: {}",
                    worktree_path.display(),
                    e
                );
            }
        }
        Err(e) => log_legacy!(
            "[ApplyNow] git reset --hard main failed in {}: {}. Leaving the tree alone, so the worktree still holds whatever was there",
            worktree_path.display(),
            e
        ),
    }
}

pub(crate) async fn probe_merge_conflicts(worktree_path: &Path) -> Vec<String> {
    crate::engine::git_ops::probe_merge(worktree_path, "HEAD", "main")
        .await
        .into_conflicts()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::in_place_merge::end_interrupted_applies;
    use crate::engine::types::ChangeClaim;
    use crate::engine::AgentSession;
    use tokio::sync::mpsc;

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

    #[test]
    fn apply_now_refuses_a_parked_question_before_it_prompts_anything() {
        let src = include_str!("apply_now.rs");
        let production = &src[..src.find("\n#[cfg(test)]").unwrap()];
        let body = &production[production.find("async fn apply_now(").unwrap()..];
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

    #[test]
    fn every_apply_now_is_bracketed_by_its_start_and_end() {
        let src = include_str!("apply_now.rs");
        let production = &src[..src.find("\n#[cfg(test)]").unwrap()];
        let body = &production[production.find("async fn apply_now(").unwrap()..];
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

    #[test]
    fn apply_now_refuses_rather_than_proposes_when_nothing_is_pending() {
        let src = include_str!("apply_now.rs");
        let production = &src[..src.find("\n#[cfg(test)]").unwrap()];
        let start = production.find("async fn apply_now(").unwrap();
        let end = production.find("async fn wait_and_commit(").unwrap();
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

    fn production_source() -> &'static str {
        let src = include_str!("apply_now.rs");
        &src[..src.find("\n#[cfg(test)]").unwrap()]
    }

    fn source_between(start: &str, end: &str) -> &'static str {
        let production = production_source();
        let from = production
            .find(start)
            .unwrap_or_else(|| panic!("apply_now.rs no longer contains {start:?}"));
        let rest = &production[from..];
        &rest[..rest
            .find(end)
            .unwrap_or_else(|| panic!("apply_now.rs no longer contains {end:?}"))]
    }

    fn position(body: &str, needle: &str) -> usize {
        body.find(needle)
            .unwrap_or_else(|| panic!("the checked function no longer contains {needle:?}"))
    }

    #[test]
    fn apply_now_refuses_while_a_resolver_owns_the_merge_before_it_claims_anything() {
        let body = source_between("async fn apply_now(", "async fn apply_pending_now(");
        let refusal = position(body, "MERGE_OWNED_BY_RESOLVER_MESSAGE");
        for later in [
            "session.change_claim = Some(",
            "pending_for_thread(thread_id)",
            "self.nothing_proposed_refusal(thread_id)",
        ] {
            assert!(
                refusal < position(body, later),
                "the resolver refusal must come before {later:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_commit_failure_after_the_prompt_turn_is_an_err_not_an_ok() {
        let not_a_repo = tempfile::tempdir().unwrap();
        let (answer, answered) = tokio::sync::oneshot::channel();
        answer.send(InputAnswered::TurnEnded).unwrap();

        let err = commit_after_prompt_turn(answered, not_a_repo.path(), "waiting for hardening")
            .await
            .expect_err("a worktree git cannot read is not a committed snapshot");
        assert!(
            err.to_string()
                .contains("wait_and_commit (waiting for hardening)"),
            "{err}"
        );
    }

    #[test]
    fn a_handed_off_thread_is_refused_before_its_branch_is_auto_committed() {
        let body = source_between(
            "async fn apply_now_inner(",
            "async fn apply_now_through_lane(",
        );
        let handoff = position(
            body,
            "if crate::engine::successor::handed_off(&self.pool, thread_id).await {",
        );
        assert!(position(body, "ProposalHold::CarriedBySuccessor") > handoff);
        assert!(
            handoff < position(body, "auto_commit_preserving_marker("),
            "the handoff check must come before the auto-commit"
        );
    }

    #[test]
    fn a_hardening_turn_without_a_marker_refuses_and_keeps_the_branch() {
        let body = source_between(
            "async fn apply_now_inner(",
            "async fn apply_now_through_lane(",
        );
        let after_wait = &body[position(body, "\"waiting for hardening\"")..];
        let refusal = &after_wait[..position(after_wait, "hardened = true;")];
        assert!(
            refusal.contains("if !branch_is_hardened("),
            "the marker, not the wait, decides whether hardening happened"
        );
        assert!(refusal.contains("self.mark_session_idle("));
        assert!(
            !refusal.contains("reset_worktree_and_idle("),
            "a refused apply must not reset a branch that still holds the work"
        );
    }

    #[test]
    fn the_no_commits_recovery_is_read_only_and_keeps_the_live_branch() {
        let body = source_between(
            "async fn apply_now_inner(",
            "async fn apply_now_through_lane(",
        );
        let from = position(body, "let has_commits");
        let recovery = &body[from..from + position(&body[from..], "// Step 4: Propose change")];
        assert!(recovery.contains("main_history_touches_files("));
        for forbidden in [
            "recover_no_commits_branch(",
            "finalize_change_as_noop(",
            "delete_branch(",
            ".unwrap_or_default()",
        ] {
            assert!(
                !recovery.contains(forbidden),
                "the no-commits recovery must not use {forbidden:?}"
            );
        }
        assert!(recovery.contains(".unwrap_or_else(|e| {"));
    }

    #[test]
    fn the_main_dirty_check_runs_under_both_main_locks() {
        let body = source_between(
            "async fn apply_now_inner(",
            "async fn apply_now_through_lane(",
        );
        let lock = position(
            body,
            "let _main = crate::engine::git_ops::MAIN_LOCKS.lock_main().await;",
        );
        assert!(lock < position(body, "auto_commit_safe_files_if_dirty(repo_root)"));
    }

    #[test]
    fn apply_now_checks_the_security_bound_before_main_moves_and_refuses_cleanly() {
        let body = source_between(
            "async fn apply_now_inner(",
            "async fn apply_now_through_lane(",
        );
        let marker = position(body, "self.plan_marker_snapshot(repo_root, branch_name)");
        let check = position(body, ".bounded_plan_candidate(");
        let lane = position(body, "takes_landing_lane(");
        let merge = position(body, "merge_via_cc_session(");
        assert!(marker < check && check < lane && lane < merge);
        let refusal = &body[check..lane];
        assert!(refusal.contains("emit_apply_failed("));
        assert!(
            !refusal.contains("return Err"),
            "an Err out of this check would read as a diverged main"
        );
    }

    #[test]
    fn apply_now_asks_the_hooks_before_main_moves_and_refuses_cleanly() {
        let body = source_between(
            "async fn apply_now_inner(",
            "async fn apply_now_through_lane(",
        );
        let ask =
            "if let Err(refusal) = crate::engine::thread_events::emit_change_apply_requested(";
        let no_commits = position(body, ask);
        assert!(no_commits < position(body, ".emit_change_applied("));
        let proposed = position(
            body,
            "let Some(claim) = self.claim_pending_change(change_id)",
        );
        let after_claim = &body[proposed..];
        let asked = position(after_claim, ask);
        for mover in [
            "commit_dirty_logged(",
            "takes_landing_lane(",
            "merge_via_cc_session(",
        ] {
            assert!(
                asked < position(after_claim, mover),
                "{mover} runs before the ask"
            );
        }
        for at in [&body[no_commits..], &after_claim[asked..]] {
            let refusal = &at[..position(at, "return Ok(());")];
            assert!(refusal.contains("self.emit_apply_failed("), "{refusal}");
            assert!(!refusal.contains("return Err"), "{refusal}");
        }
    }

    #[test]
    fn an_unanswered_ancestor_probe_does_not_authorize_the_lane_reset() {
        let body = source_between(
            "async fn apply_now_through_lane(",
            "async fn merge_via_cc_session(",
        );
        let probe = position(body, "\"merge-base\", \"--is-ancestor\"");
        assert!(position(body, ".or_unknown(false)") > probe);
        assert!(!body.contains(".or_unknown(true)"));
    }

    #[test]
    fn the_conflict_recovery_anchors_its_inactivity_clock_to_its_own_start() {
        let body = source_between(
            "fn spawn_in_place_conflict_recovery(",
            "async fn apply_now_success(",
        );
        position(body, "let recovery_started_ms = now_epoch_millis();");
        let call = &body[position(body, "apply_inactivity_timeout_minutes(")..];
        assert!(call[..200].contains("recovery_started_ms"));
    }

    #[tokio::test]
    async fn marking_a_mid_turn_session_idle_leaves_its_phase_alone() {
        let (pool, db) = crate::test_support::setup_test_db().await;
        let workspace = tempfile::tempdir().unwrap();
        let engine = LucidosEngine::new(
            workspace.path().to_path_buf(),
            &crate::test_support::test_db_url(&db),
            crate::llm::ProviderBuildContext {
                default_model: "mock".into(),
                model_is_mock: true,
                vertex_project_id: String::new(),
                vertex_location: crate::llm::vertex::location_handle(String::new()),
                vertex_token_cache: None,
                model_registry: Default::default(),
                not_served: Default::default(),
                claude_gateway_access: None,
                chatgpt_plan_access: None,
            },
            Arc::new(crate::engine::agent_recovery::EngineAgentHost),
            Box::new(crate::engine::provider_adapters::ProviderAdapters::default()),
        )
        .await
        .unwrap();
        let thread_id = crate::test_support::seed_chat_thread(&engine.event_bus, "mid turn").await;
        let (mut session, _rx) = AgentSession::for_test();
        session.is_waiting = false;
        engine
            .agent_sessions
            .lock()
            .await
            .insert(thread_id, session);

        engine
            .mark_session_idle(thread_id, Path::new("/tmp/mid-turn-worktree"))
            .await;

        assert!(
            !engine.agent_sessions.lock().await[&thread_id].is_waiting,
            "only the run loop writes is_waiting"
        );
        pool.close().await;
        crate::test_support::teardown_test_db(&db).await;
    }

    #[test]
    fn the_session_reset_base_is_the_literal_main() {
        let body = source_between(
            "async fn reset_session_worktree_to_main(",
            "pub(crate) async fn probe_merge_conflicts(",
        );
        assert!(body.contains("&[\"reset\", \"--hard\", \"main\"]"));
        assert!(!body.contains("default_local_branch"));
    }

    const SESSION_BRANCH: &str = "lucidos-claude-code-repo-lucidos-reset-gate";

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

    #[test]
    fn inactivity_timeout_ignores_pre_apply_idle_gap() {
        let now = 100 * 60_000;
        let last_event = now - 11 * 60_000;
        let apply_started = now;
        assert_eq!(
            apply_inactivity_timeout_minutes(now, last_event, apply_started, TEN_MIN_MS),
            None,
            "a fresh apply must get the full limit regardless of how long the thread was idle first"
        );
    }

    #[test]
    fn inactivity_timeout_holds_through_early_polls() {
        let apply_started = 100 * 60_000;
        let last_event = apply_started - 30 * 60_000;
        let now = apply_started + 30_000;
        assert_eq!(
            apply_inactivity_timeout_minutes(now, last_event, apply_started, TEN_MIN_MS),
            None
        );
    }

    #[test]
    fn inactivity_timeout_fires_on_genuine_silence() {
        let apply_started = 100 * 60_000;
        let last_event = apply_started + 60_000;
        let now = last_event + 12 * 60_000;
        assert_eq!(
            apply_inactivity_timeout_minutes(now, last_event, apply_started, TEN_MIN_MS),
            Some(12)
        );
    }

    #[test]
    fn inactivity_timeout_respects_recent_cc_event() {
        let apply_started = 100 * 60_000;
        let now = apply_started + 60 * 60_000;
        let last_event = now - 60_000;
        assert_eq!(
            apply_inactivity_timeout_minutes(now, last_event, apply_started, TEN_MIN_MS),
            None
        );
    }

    #[tokio::test]
    async fn liveness_timeout_does_not_fire_while_events_arrive() {
        use std::sync::atomic::{AtomicI64, Ordering};

        let last_event_at = Arc::new(AtomicI64::new(now_epoch_millis()));
        let last_event_clone = last_event_at.clone();

        let inner = async move {
            for _ in 0..10 {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                last_event_clone.store(now_epoch_millis(), Ordering::Relaxed);
            }
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        };
        tokio::pin!(inner);

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

    #[tokio::test]
    async fn liveness_timeout_fires_when_events_stop() {
        use std::sync::atomic::{AtomicI64, Ordering};

        let last_event_at = Arc::new(AtomicI64::new(now_epoch_millis() - 1000));

        let inner = async {
            tokio::time::sleep(std::time::Duration::from_secs(999)).await;
            Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
        };
        tokio::pin!(inner);

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

    mod guard_tests {
        #[test]
        fn the_apply_claim_is_released_in_one_place() {
            let writers: Vec<(String, usize)> =
                crate::test_support::source_scan::production_sources()
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
                vec![("engine/engine_impl/accessors.rs".to_string(), 1)],
                "only `release_change_claim` may clear the claim"
            );
        }
    }
}
