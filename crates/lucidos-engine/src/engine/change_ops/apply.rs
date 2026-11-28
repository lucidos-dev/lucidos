use super::apply_claim::{ApplyClaim, APPLY_IN_PROGRESS_MESSAGE};
use super::*;
use crate::engine::git_ops::{
    auto_commit_safe_files_if_dirty, bounded_fix_refusal_for, branch_changed_files_checked,
    branch_head_sha, catchup_and_ff_to_main, clear_temp_merge_state, commit_worktree_or_err,
    commits_in_range, ff_main_to_branch, files_have_client_update, find_branch_merge_in_main,
    find_worktree_for_branch, git_ran_ok, has_branch_commits, has_full_harden_marker,
    push_main_in_background, recover_no_commits_branch, worktree_add, worktree_dirty_files,
    worktrees_dir, BoundedFixInputs, NoCommitsRecovery, PlanMarkerState, WorktreeLookup,
    MAIN_LOCKS,
};
use crate::engine::in_place_merge::InPlaceMergeStart;
use crate::engine::landing_lane::{queued_snapshot, takes_landing_lane, Submission};
use crate::engine::{ApplyResult, ApplyStatus};

const THREAD_AGENT_STARTING_MESSAGE: &str =
    "A coding agent is starting on this thread. Apply again once it reaches a stopping point.";

/// INVARIANT: Pick the worktree the hardening session runs in, given what git said about
/// `branch_name`.
///
/// Reuse an existing worktree when one is on disk: the producing Claude Code
/// session has exited, but its worktree and branch lock survive, and
/// `git worktree add` would fail with "branch already used by worktree".
/// That is what broke nightly trigger applies, which POST apply immediately
/// after the session exits.
///
/// The lookup is a parameter rather than an inner call so a test can drive the
/// `Unknown` arm against a real repo. Unknown refuses, because the `NotFound`
/// arm force-removes `fresh_path`. That path is derived from the change id, so
/// a second Apply aims at the same directory a live hardening session may be
/// working in. Apply is user-initiated and retryable, so refusing costs a click.
pub(crate) async fn resolve_harden_worktree(
    repo_root: &Path,
    branch_name: &str,
    fresh_path: &Path,
    lookup: WorktreeLookup,
) -> Result<PathBuf, String> {
    match lookup {
        WorktreeLookup::Found(existing) => {
            log_legacy!(
                "[Changes] Reusing existing worktree {} for hardening of branch {}",
                existing.display(),
                branch_name
            );
            Ok(existing)
        }
        WorktreeLookup::Unknown => Err(format!(
            "Could not determine which worktree holds branch {} (git worktree list gave no \
             answer). Refusing to clear the hardening worktree on a guess. Try Apply again.",
            branch_name
        )),
        WorktreeLookup::NotFound => {
            let wt_str = fresh_path.to_string_lossy().into_owned();
            let _ = git_cmd(&["worktree", "remove", "--force", &wt_str], repo_root).await;
            match worktree_add(repo_root, fresh_path, &[branch_name]).await {
                Ok(o) if o.status.success() => Ok(fresh_path.to_path_buf()),
                Ok(o) => Err(format!(
                    "Failed to create worktree for hardening: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                )),
                Err(e) => Err(format!("Failed to create worktree for hardening: {}", e)),
            }
        }
    }
}

struct MergedMain<'a> {
    repo_root: &'a Path,
    pre_sha: String,
    post_sha: String,
    commits: Vec<String>,
}

impl LucidosEngine {
    /// INVARIANT: The bound check for a conflict-resolution merge, `None` when the branch
    /// is not in the lane or kept inside it.
    ///
    /// This is the one route the entry-point check cannot cover. A resolution
    /// session edits AFTER that check, running `/harden` and fixing tests, and
    /// its work is auto-committed and published. Rechecking here is safe where
    /// rechecking at the tier fast paths was not. Both callers treat a refusal
    /// as terminal, emitting `ChangeApplyFailed`, rather than as a cue to
    /// escalate to a slower merge: `completion.rs` for Tiers 2 and 3, and
    /// `cc_assisted_merge_then_ff` for Tier 1.
    ///
    /// Two branch names, because they can differ. The marker lives on the
    /// CHANGE branch, while the merge publishes the session's own branch, which
    /// for an original Tier-3 session is a temp one. Tier 1 passes one name
    /// twice, since its session runs on the change branch.
    pub async fn bounded_fix_refusal_for_resolution(
        &self,
        repo_root: &Path,
        worktree_path: &Path,
        marker_branch: &str,
        merge_source_branch: &str,
    ) -> Option<String> {
        match self.plan_marker_state(repo_root, marker_branch).await {
            PlanMarkerState::Present(kind) if kind.is_file_bounded() => {}
            _ => return None,
        }
        bounded_fix_refusal_for(BoundedFixInputs {
            bound: self.plan_marker_files(repo_root, marker_branch).await,
            committed: branch_changed_files_checked(repo_root, merge_source_branch).await,
            dirty: worktree_dirty_files(worktree_path).await,
        })
    }

    /// INVARIANT: What is uncommitted on `branch_name`, for the bound check, when the
    /// caller does not already hold the worktree path.
    ///
    /// A lookup that could not answer is `Err`, never "nothing dirty". Reading
    /// `Unknown` as clean is how a saturated host would wave an out-of-bound
    /// tree through: `git worktree list` times out, the dirty half is skipped,
    /// and Tier 1 then commits and merges that very tree.
    pub(crate) async fn dirty_files_for_bound(
        &self,
        repo_root: &Path,
        branch_name: &str,
    ) -> Result<Vec<String>, String> {
        match find_worktree_for_branch(repo_root, branch_name).await {
            WorktreeLookup::Found(wt) => worktree_dirty_files(&wt).await,
            WorktreeLookup::NotFound => Ok(Vec::new()),
            WorktreeLookup::Unknown => {
                Err("git worktree list gave no answer, so its uncommitted files are unknown".into())
            }
        }
    }

    pub async fn bounded_plan_candidate(
        &self,
        repo_root: &Path,
        branch_name: &str,
        file_bound: Option<&[String]>,
        dirty: Result<Vec<String>, String>,
    ) -> (Option<Vec<String>>, Option<String>) {
        let committed = branch_changed_files_checked(repo_root, branch_name).await;
        let landing = match (&committed, &dirty) {
            (Ok(committed), Ok(dirty)) => Some(committed.iter().chain(dirty).cloned().collect()),
            _ => None,
        };
        let refusal = bounded_fix_refusal_for(BoundedFixInputs {
            bound: Ok(file_bound.unwrap_or_default().to_vec()),
            committed,
            dirty,
        });
        (landing, refusal)
    }

    pub async fn landing_files(&self, repo_root: &Path, branch_name: &str) -> Option<Vec<String>> {
        let committed = branch_changed_files_checked(repo_root, branch_name).await;
        let dirty = self.dirty_files_for_bound(repo_root, branch_name).await;
        match (committed, dirty) {
            (Ok(mut landing), Ok(dirty)) => {
                landing.extend(dirty);
                Some(landing)
            }
            (Err(e), _) | (_, Err(e)) => {
                log_legacy!(
                    "[Changes] landing files of {}: {}. Treating them as unknown",
                    branch_name,
                    e
                );
                None
            }
        }
    }

    async fn finalize_change_as_noop(
        &self,
        change: &crate::core::changes::Change,
        claim: &ApplyClaim,
        repo_root: &Path,
        actor: Option<MessageOrigin>,
        result_message: &'static str,
    ) -> Result<ApplyResult, Box<dyn std::error::Error + Send + Sync>> {
        let change_id = claim.change_id();
        if let Err(e) = git_ran_ok(&["branch", "-D", &change.branch_name], repo_root).await {
            log_legacy!(
                "[Changes] Could not delete branch {} while resolving change {} as a no-op: {}",
                change.branch_name,
                change_id,
                e
            );
        }
        self.emit_change_applied(
            claim,
            change.thread_id.unwrap_or(change_id),
            false,
            false,
            Vec::new(),
            change.thread_title.clone(),
            actor,
            None,
        )
        .await?;
        self.broadcast_changes_updated().await;
        Ok(ApplyResult::noop(
            change_id,
            change.thread_id,
            change.files.len(),
            result_message,
        ))
    }

    async fn finish_merged_apply(
        &self,
        change: &crate::core::changes::Change,
        claim: &ApplyClaim,
        actor: Option<MessageOrigin>,
        kind_ctx: &ApplyKindContext,
        effective_requires_restart: bool,
        merged: MergedMain<'_>,
    ) -> Result<ApplyResult, Box<dyn std::error::Error + Send + Sync>> {
        let change_id = claim.change_id();
        let MergedMain {
            repo_root,
            pre_sha,
            post_sha,
            commits,
        } = merged;
        if !kind_ctx.is_app() {
            push_main_in_background(repo_root, &self.event_bus);
        }
        let build_request = self
            .emit_change_applied(
                claim,
                change.thread_id.unwrap_or(change_id),
                effective_requires_restart,
                files_have_client_update(&change.files),
                commits.clone(),
                change.thread_title.clone(),
                actor.clone(),
                Some(AppliedMerge::new(&pre_sha, &post_sha)),
            )
            .await?;
        self.maybe_emit_app_ui_refresh(kind_ctx, &change.files, actor.as_ref())
            .await;
        self.emit_entity_events_for_change_apply(
            &change.files,
            Some(&pre_sha),
            Some(&post_sha),
            actor,
            change.thread_id,
        )
        .await;
        self.broadcast_changes_updated().await;
        Ok(ApplyResult::applied_with_merge(
            change_id,
            change.thread_id,
            effective_requires_restart,
            pre_sha,
            post_sha,
            &commits,
            change.files.len(),
        )
        .with_version_build(build_request))
    }

    /// INVARIANT: Apply a single pending change: merge its branch into main.
    /// `actor` identifies who initiated the apply. HTTP callers construct via
    /// `api::actor::build_message_origin`; engine-internal callers pass `None`.
    ///
    /// Thin wrapper over `apply_change_inner` that runs the apply-time
    /// **"≤1 pending change per thread" reconcile** — discard any other pending
    /// change the thread still holds on a stale branch, so a pre-existing orphan
    /// can't keep blocking Archive. Gated on the change ACTUALLY merging
    /// (`ApplyStatus::Applied`, not the other statuses, which also return
    /// `Ok`). That gate is load-bearing: reconciling on any other status would
    /// discard a *newer* sibling (data loss) or drop siblings before the merge
    /// lands. See docs/plans/2026-07-01-orphaned-pending-change-blocks-archive.md.
    ///
    /// This single point covers every `apply_change` caller whose merge
    /// finishes *before* the return. Three paths finish after it and reconcile
    /// themselves, because their result at return time is `Conflict` or
    /// `Queued`. The live in-place merge reconciles in `apply_now_success`. The
    /// detached Tier-2 merge reconciles in its spawned task, gating on
    /// `ChangeStatus::Applied`. The landing lane reconciles once a change lands
    /// (`settle_source_after_lane`).
    ///
    /// The *other* apply-time follow-up — kicking the background engine rebuild
    /// (or re-snapshotting the served `dist/`) — deliberately does NOT live here:
    /// it hangs off `emit_change_applied`, the one emit every merge path performs
    /// exactly once. See `change_ops_emitters::post_apply_dev_refresh`.
    ///
    /// # This call announces its own failure
    ///
    /// Every `Err` returned for a change that EXISTS has already been emitted as
    /// a `ChangeApplyFailed` on its thread. A caller must not emit a second one:
    /// the frontend draws one card per event, so a caller that re-announces puts
    /// the same failure on screen twice. Log it and let the event stand, as the
    /// Apply-All driver and the standing-apply resolver do.
    ///
    /// Two errors are raised before a change row is in hand: the row is missing,
    /// or its lookup failed. Neither names a thread, so neither announces. A
    /// caller holding a thread id may announce those itself.
    ///
    /// Pinned by `apply_change_callers_do_not_re_announce_the_failure`.
    pub async fn apply_change(
        self: &Arc<Self>,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<ApplyResult, Box<dyn std::error::Error + Send + Sync>> {
        let result = self.apply_change_inner(change_id, actor.clone()).await?;
        if result.status == ApplyStatus::Applied {
            if let Some(tid) = result.thread_id {
                self.discard_orphaned_pending_siblings(tid, change_id, actor)
                    .await;
            }
        }
        Ok(result)
    }

    async fn apply_change_inner(
        self: &Arc<Self>,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<ApplyResult, Box<dyn std::error::Error + Send + Sync>> {
        // INVARIANT: Claimed before the row is read, so the read and every act below sit
        // inside one claim. The apply that holds it announces the outcome.
        let Some(claim) = self.apply_claims.try_claim(change_id) else {
            return Ok(ApplyResult {
                status: ApplyStatus::Conflict,
                change_id,
                message: APPLY_IN_PROGRESS_MESSAGE.to_string(),
                ..ApplyResult::default()
            });
        };
        let change = self
            .changes()
            .get_by_id(change_id)
            .await?
            .ok_or("Change not found")?;

        if let Some(thread_id) = change.thread_id {
            if change.is_pending() {
                if let Err(refusal) = crate::engine::thread_events::emit_change_apply_requested(
                    &self.event_bus,
                    thread_id,
                    change_id,
                    &change.branch_name,
                    &change.repo_root,
                    actor.clone(),
                )
                .await
                {
                    let message = refusal.to_string();
                    self.emit_apply_failed(thread_id, change_id, &message, actor)
                        .await;
                    return Err(message.into());
                }
            }
        }

        let kind_ctx = load_apply_kind_context(&self.pool, change.thread_id).await;
        let effective_requires_restart = change.requires_restart && !kind_ctx.is_app();

        if let ChangeStatusData::Applied(shas) = &change.state {
            return Ok(ApplyResult {
                status: ApplyStatus::Noop,
                change_id,
                thread_id: change.thread_id,
                restart_required: effective_requires_restart,
                message: "Change already applied.".to_string(),
                applied_commit: shas.post.clone(),
                previous_commit: shas.pre.clone(),
                commits_applied: change.commits.len(),
                files_changed: change.files.len(),
                ..ApplyResult::default()
            });
        }
        // INVARIANT: Announced like every other refusal below, so `apply_change`'s contract
        // holds for the whole function: a caller never has to guess whether this
        // one reached the thread. It also gives the Apply spinner the terminal
        // event it waits on. And it advances an Apply All batch past a member
        // somebody discarded mid-run, which the driver waits on this event for.
        if !change.is_pending() {
            let msg = match change.status() {
                ChangeStatus::SetAside => SET_ASIDE_APPLY_REFUSAL.to_string(),
                status => format!("Change is already {status}"),
            };
            self.emit_apply_failed(
                change.thread_id.unwrap_or(change_id),
                change_id,
                &msg,
                actor.clone(),
            )
            .await;
            return Err(msg.into());
        }

        // INVARIANT: Idempotency fast-path: when the branch ref is gone, check if it was
        // already merged into main out-of-band (e.g. by an agentic loop calling
        // `git merge` directly). Only kicks in when the live branch is missing,
        // so live-branch flows (Tier 1/2/3) keep ownership of the merge.
        //
        // `or_unknown(true)` (branch assumed PRESENT when git could not be
        // asked): entering this arm resolves the change as an already-applied
        // no-op, and an unanswered probe must never authorize that
        // (`.claude/rules/rust.md`). `git_cmd` returns `Err` for a spawn failure
        // AND for its 30s timeout, which a saturated host really does hit on an
        // ordinary `rev-parse`, so the old `.unwrap_or(false)` read "could not
        // ask" as "branch gone" and could mark a pending change applied while
        // its commits were never merged. Assuming the branch is there instead
        // falls through to the normal apply path, which re-checks the ref below
        // and fails loudly if it truly is missing.
        {
            let repo_root = std::path::PathBuf::from(&change.repo_root);
            let branch_exists = crate::engine::git_ops::git_answer(
                &["rev-parse", "--verify", &change.branch_name],
                &repo_root,
            )
            .await
            .or_unknown(true);
            if !branch_exists {
                if let Some((pre_sha, post_sha)) =
                    find_branch_merge_in_main(&repo_root, &change.branch_name).await
                {
                    log_legacy!("[Changes] Branch {} already merged into main as {} — marking change {} applied",
                        change.branch_name,
                        &post_sha[..post_sha.floor_char_boundary(8)],
                        change_id);
                    let commits = commits_in_range(&repo_root, &pre_sha, &post_sha).await;
                    return self
                        .finish_merged_apply(
                            &change,
                            &claim,
                            actor,
                            &kind_ctx,
                            effective_requires_restart,
                            MergedMain {
                                repo_root: &repo_root,
                                pre_sha,
                                post_sha,
                                commits,
                            },
                        )
                        .await;
                }
            }
        }

        if let Some(merge) = change.merge_worktree() {
            let wt = &merge.path;
            if !std::path::Path::new(wt).exists() {
                log_legacy!(
                    "[Changes] Stale merge worktree {} no longer exists, clearing metadata",
                    wt
                );
                if let Some(tid) = change.thread_id {
                    self.emit_merge_resolution_cleared(
                        tid,
                        change_id,
                        "[Changes] MergeResolutionCleared",
                    )
                    .await;
                }
            }
        }

        if let Some(tid) = change.thread_id {
            if self.merge_ownership_for_change(tid, change_id).await
                == MergeOwnership::ResolverOwnsIt
            {
                log_legacy!(
                    "[Changes] Apply refused for {} (thread {}): a conflict resolution is in flight and owns the merge",
                    change_id,
                    tid
                );
                return Ok(ApplyResult::conflict(
                    change_id,
                    tid,
                    change.files.len(),
                    MERGE_OWNED_BY_RESOLVER_MESSAGE,
                ));
            }
        }

        // INVARIANT: The implementation-plan floor. The propose path asks the same
        // question, so a change held there never reaches this point. This
        // stays the backstop for a row proposed before that gate existed.
        // Both it and the hardening gate below judge what will land,
        // re-derived from git as the bound check is. An unanswered read keeps
        // the hold and the hardening.
        let plan_repo_root = std::path::PathBuf::from(&change.repo_root);
        let (floor, marker) = self
            .plan_floor(&kind_ctx, &plan_repo_root, &change.branch_name)
            .await;
        let (landing, bound_refusal) = if matches!(floor, PlanFloor::Clear(kind) if kind.is_file_bounded())
        {
            let dirty = self
                .dirty_files_for_bound(&plan_repo_root, &change.branch_name)
                .await;
            self.bounded_plan_candidate(
                &plan_repo_root,
                &change.branch_name,
                marker.file_bound.as_deref(),
                dirty,
            )
            .await
        } else if kind_ctx.is_app() {
            (None, None)
        } else {
            (
                self.landing_files(&plan_repo_root, &change.branch_name)
                    .await,
                None,
            )
        };
        let floor = floor.with_landing(landing.as_deref().unwrap_or_default());
        self.observe_apply_plan(
            change.thread_id.unwrap_or(change_id),
            kind_ctx.kind,
            &marker,
            landing.as_deref(),
            matches!(floor, PlanFloor::Held(_)) || bound_refusal.is_some(),
        )
        .await;
        match floor {
            PlanFloor::Exempt => {}
            PlanFloor::Held(hold) => {
                let msg = hold.apply_refusal();
                log_legacy!(
                    "[Changes] Apply blocked: plan floor holds branch {} ({:?}, change {})",
                    change.branch_name,
                    hold,
                    change_id
                );
                self.emit_apply_failed(
                    change.thread_id.unwrap_or(change_id),
                    change_id,
                    msg,
                    actor.clone(),
                )
                .await;
                return Err(msg.into());
            }
            PlanFloor::Clear(_) => {}
        }
        if let Some(msg) = bound_refusal {
            log_legacy!(
                        "[Changes] Apply blocked: bounded security fix left its bound on branch {} (change {})",
                        change.branch_name,
                        change_id
                    );
            self.emit_apply_failed(
                change.thread_id.unwrap_or(change_id),
                change_id,
                &msg,
                actor.clone(),
            )
            .await;
            return Err(msg.into());
        }

        let marker_hardened = !kind_ctx.is_app()
            && has_full_harden_marker(&self.pool, &plan_repo_root, &change.branch_name).await;
        if marker_hardened && !change.hardened {
            log_legacy!(
                "[Changes] Marker found but DB not set — marking change {} as hardened",
                change_id
            );
            if let Some(tid) = change.thread_id {
                self.emit_change_hardened(tid, change_id, "[Changes] ChangeHardened")
                    .await;
            }
        }
        let hardened = change.hardened || marker_hardened;
        if apply_must_harden(&kind_ctx, hardened, landing.as_deref()) {
            let thread_id = match change.thread_id {
                Some(tid) => tid,
                None => {
                    let msg = "Change has no associated thread — cannot run hardening automatically. Discard and re-create with a Claude Code session.";
                    self.emit_apply_failed(change_id, change_id, msg, actor.clone())
                        .await;
                    return Err(msg.into());
                }
            };

            if let Some(session) = self.live_session_info(thread_id).await {
                match self
                    .request_hardening_in_session(thread_id, &session.msg_tx)
                    .await
                {
                    Ok(answered) => {
                        log_legacy!(
                            "[Changes] Reusing existing session on thread {} for hardening",
                            thread_id
                        );
                        // INVARIANT: The session's turn end never applies, so this apply
                        // owes its own outcome once hardening settles.
                        self.agent_host().spawn_apply_after_session_harden(
                            self,
                            thread_id,
                            change_id,
                            answered,
                            actor.clone(),
                        );
                        return Ok(ApplyResult::hardening(
                            change_id,
                            thread_id,
                            change.files.len(),
                            "Hardening sent to the live session. The change will be applied after hardening completes.",
                        ));
                    }
                    Err(e) => {
                        log_legacy!(
                            "[Changes] Live session on thread {} did not accept harden message ({}); falling back to fresh worktree",
                            thread_id,
                            e
                        );
                    }
                }
            } else {
                self.emit_missing_hardening_detected(thread_id).await;
            }

            let repo_root = std::path::PathBuf::from(&change.repo_root);

            let lookup = find_worktree_for_branch(&repo_root, &change.branch_name).await;
            let fresh_path = worktrees_dir(self.workspace_path())
                .join(format!("harden-{}", change_id.as_simple()));
            let wt_path =
                match resolve_harden_worktree(&repo_root, &change.branch_name, &fresh_path, lookup)
                    .await
                {
                    Ok(p) => p,
                    Err(msg) => {
                        self.emit_apply_failed(thread_id, change_id, &msg, actor.clone())
                            .await;
                        return Err(msg.into());
                    }
                };

            log_legacy!(
                "[Changes] Not hardened — spawning hardening recovery for change {} (thread {})",
                change_id,
                thread_id
            );
            self.agent_host().spawn_hardening(
                self.as_ref(),
                thread_id,
                wt_path,
                change.branch_name.clone(),
                Some(change_id),
                actor.clone(),
            );

            return Ok(ApplyResult::hardening(
                change_id,
                thread_id,
                change.files.len(),
                "Hardening started — change will be applied after hardening completes.",
            ));
        }

        let repo_root = std::path::PathBuf::from(&change.repo_root);

        // INVARIANT: Validate branch exists — don't auto-discard, let the user decide
        //
        // `or_unknown(true)` (branch assumed PRESENT when git could not be
        // asked): the `!branch_exists` arm tells the user the branch is gone and
        // that the change "may need to be discarded manually", so a `rev-parse`
        // that merely timed out would invite the user to throw away work that is
        // still on disk. Assuming it is there costs a loud, accurate git error
        // from the merge below instead.
        let branch_exists = crate::engine::git_ops::git_answer(
            &["rev-parse", "--verify", &change.branch_name],
            &repo_root,
        )
        .await
        .or_unknown(true);
        if !branch_exists {
            let msg = format!(
                "Branch '{}' no longer exists. The change may need to be discarded manually.",
                change.branch_name
            );
            self.emit_apply_failed(
                change.thread_id.unwrap_or(change_id),
                change_id,
                &msg,
                actor.clone(),
            )
            .await;
            return Err(msg.into());
        }

        if !has_branch_commits(&repo_root, &change.branch_name).await {
            match recover_no_commits_branch(&repo_root, &change.branch_name, &change.files).await {
                Ok(NoCommitsRecovery::AutoCommitted) => {
                    log_legacy!(
                        "[Changes] Auto-committed worktree for branch {} — proceeding with merge",
                        change.branch_name
                    );
                }
                Ok(NoCommitsRecovery::LegitimateNoOp) => {
                    return self
                        .finalize_change_as_noop(
                            &change,
                            &claim,
                            &repo_root,
                            actor.clone(),
                            "Change applied (no commits to merge).",
                        )
                        .await;
                }
                Ok(NoCommitsRecovery::AlreadyApplied) => {
                    log_legacy!(
                        "[Changes] Branch {} already merged into main — marking change {} as applied (no-op)",
                        change.branch_name,
                        change_id
                    );
                    return self
                        .finalize_change_as_noop(
                            &change,
                            &claim,
                            &repo_root,
                            actor.clone(),
                            "Change already present on main — marked applied.",
                        )
                        .await;
                }
                Err(e) => {
                    let msg = e.to_string();
                    log_legacy!(
                        "[Changes] Refusing silent-apply for change {}: {}",
                        change_id,
                        msg
                    );
                    self.emit_apply_failed(
                        change.thread_id.unwrap_or(change_id),
                        change_id,
                        &msg,
                        actor.clone(),
                    )
                    .await;
                    return Err(e);
                }
            }
        }

        // INVARIANT: Auto-commit safe files (docs) if they're the only dirty files, then
        // reject if the workspace tree still has genuine uncommitted changes.
        //
        // Both of `main`'s locks, since a safe-file commit moves `main`. They
        // keep a concurrent merge's ref move and tree sync wholly before or
        // after this check. Between them the tree looks dirty, and the gate
        // would refuse a valid apply (app_coding_agent_concurrent_apply).
        // They drop at the end of this block, so no tier holds them across a
        // coding-agent subprocess, which would stall every data write.
        {
            let _main = MAIN_LOCKS.lock_main().await;
            if auto_commit_safe_files_if_dirty(&repo_root).await {
                let msg = "Cannot merge: the repository has uncommitted changes. Commit or stash them first, then try again.";
                self.emit_apply_failed(
                    change.thread_id.unwrap_or(change_id),
                    change_id,
                    msg,
                    actor.clone(),
                )
                .await;
                return Err(msg.into());
            }
        }

        if takes_landing_lane(&kind_ctx, &repo_root, self.repo_root()) {
            return self.queue_in_landing_lane(change, claim, actor).await;
        }

        if let Some(thread_id) = change.thread_id {
            match self.begin_in_place_merge(thread_id).await {
                InPlaceMergeStart::Ready(session) => {
                    log_legacy!(
                        "[Changes] Live agent session found for thread {} — merging in-place",
                        thread_id
                    );

                    // INVARIANT: A commit that does not land fails the apply before `main`
                    // moves, as in Tier 2. A live agent can hold `index.lock`,
                    // and merging anyway publishes only the committed part.
                    if let Err(e) = commit_worktree_or_err(
                        &session.worktree_path,
                        "Coding agent changes (pre-merge auto-commit)",
                    )
                    .await
                    {
                        self.clear_change_claim(thread_id, &session.msg_tx).await;
                        let msg = format!(
                            "Could not commit the uncommitted work in {} before merging: {e}. \
                             The change is still pending; try applying again.",
                            session.worktree_path.display()
                        );
                        self.emit_apply_failed(thread_id, change_id, &msg, actor.clone())
                            .await;
                        return Err(msg.into());
                    }

                    if let Ok((pre_sha, post_sha)) = catchup_and_ff_to_main(
                        &repo_root,
                        &session.worktree_path,
                        &change.branch_name,
                    )
                    .await
                    {
                        log_legacy!(
                            "[Changes] Fast-forward merge succeeded for {} (Tier 1)",
                            change.branch_name
                        );
                        self.clear_change_claim(thread_id, &session.msg_tx).await;
                        let (commits, build_request) = self
                            .agent_host()
                            .apply_now_success(
                                self,
                                &claim,
                                thread_id,
                                effective_requires_restart,
                                files_have_client_update(&change.files),
                                &pre_sha,
                                &post_sha,
                                &session.worktree_path,
                                &repo_root,
                                &change.branch_name,
                                actor.clone(),
                            )
                            .await?;
                        return Ok(ApplyResult::applied_with_merge(
                            change_id,
                            Some(thread_id),
                            effective_requires_restart,
                            pre_sha,
                            post_sha,
                            &commits,
                            change.files.len(),
                        )
                        .with_version_build(build_request));
                    }

                    log_legacy!(
                        "[Changes] main diverged for {} — spawning async in-place conflict recovery",
                        change.branch_name
                    );
                    self.agent_host().spawn_in_place_conflict_recovery(
                        self,
                        thread_id,
                        claim,
                        session,
                        change.branch_name.clone(),
                        repo_root.clone(),
                        effective_requires_restart,
                        files_have_client_update(&change.files),
                        actor.clone(),
                    );
                    return Ok(ApplyResult::conflict(
                        change_id,
                        thread_id,
                        change.files.len(),
                        "Merge conflict — the coding-agent session is resolving it. \
                         The change will apply automatically when resolution completes.",
                    ));
                }
                InPlaceMergeStart::Claimed(holder) => {
                    log_legacy!(
                        "[Changes] Thread {} is claimed by {:?}; returning conflict",
                        thread_id,
                        holder
                    );
                    return Ok(ApplyResult::conflict(
                        change_id,
                        thread_id,
                        change.files.len(),
                        crate::engine::session_claim_refusal::claim_refusal_message(holder),
                    ));
                }
                InPlaceMergeStart::SessionStopping => {
                    log_legacy!(
                        "[Changes] Apply refused for {} (thread {}): the session is stopping",
                        change_id,
                        thread_id
                    );
                    return Ok(ApplyResult::conflict(
                        change_id,
                        thread_id,
                        change.files.len(),
                        crate::engine::session_claim_refusal::SESSION_STOPPING_MESSAGE,
                    ));
                }
                InPlaceMergeStart::NoLiveSession => {}
            }
        }

        if let Some(thread_id) = change.thread_id {
            let lookup = find_worktree_for_branch(&repo_root, &change.branch_name).await;
            if matches!(lookup, WorktreeLookup::Unknown) {
                let msg = format!(
                    "Could not determine which worktree holds branch {} (git worktree list gave \
                     no answer). Refusing to merge without it. Try Apply again.",
                    change.branch_name
                );
                self.emit_apply_failed(thread_id, change_id, &msg, actor.clone())
                    .await;
                return Err(msg.into());
            }
            if let WorktreeLookup::Found(wt_path) = lookup {
                let thread_hold = match self.hold_thread_for_worktree_reset(thread_id).await {
                    Ok(Some(hold)) => hold,
                    Ok(None) => {
                        self.emit_apply_failed(
                            thread_id,
                            change_id,
                            THREAD_AGENT_STARTING_MESSAGE,
                            actor.clone(),
                        )
                        .await;
                        return Err(THREAD_AGENT_STARTING_MESSAGE.into());
                    }
                    Err(e) => {
                        let msg = format!("Could not hold the thread for the apply: {e}");
                        self.emit_apply_failed(thread_id, change_id, &msg, actor.clone())
                            .await;
                        return Err(msg.into());
                    }
                };
                // INVARIANT: A commit that does not land fails the apply before `main`
                // moves. Merging anyway publishes only the committed part, and
                // the reset below then refuses the dirty tree after the fact.
                if let Err(e) =
                    commit_worktree_or_err(&wt_path, "Coding agent changes (pre-merge auto-commit)")
                        .await
                {
                    let msg = format!(
                        "Could not commit the uncommitted work in {} before merging: {e}",
                        wt_path.display()
                    );
                    self.emit_apply_failed(thread_id, change_id, &msg, actor.clone())
                        .await;
                    return Err(msg.into());
                }

                match catchup_and_ff_to_main(&repo_root, &wt_path, &change.branch_name).await {
                    Ok((pre_sha, post_sha)) => {
                        log_legacy!(
                            "[Changes] Fast path succeeded for {} (Tier 2)",
                            change.branch_name
                        );
                        let commits = commits_in_range(&repo_root, &pre_sha, &post_sha).await;
                        // INVARIANT: Phase 6.2: keep worktree on disk; reset to main HEAD
                        // so the next user message resumes CC at a clean state.
                        // The branch ref is still alive (catchup_and_ff_to_main
                        // tries `branch -D` against repo_root, which fails
                        // silently when a worktree owns the branch).
                        if let Err(e) = reset_worktree_to_main_after_apply(&wt_path).await {
                            self.emit_apply_failed(
                                thread_id,
                                change_id,
                                &e.to_string(),
                                actor.clone(),
                            )
                            .await;
                            return Err(e);
                        }
                        // INVARIANT: Phase 4: Apply no longer terminates the thread —
                        // SessionEnded is terminal-only. Phase 6.2: the
                        // worktree and branch are preserved above so the
                        // next user message resumes the same Claude Code session.
                        // ChangeApplied is the user-visible signal.
                        return self
                            .finish_merged_apply(
                                &change,
                                &claim,
                                actor,
                                &kind_ctx,
                                effective_requires_restart,
                                MergedMain {
                                    repo_root: &repo_root,
                                    pre_sha,
                                    post_sha,
                                    commits,
                                },
                            )
                            .await;
                    }
                    Err(_) => {
                        log_legacy!(
                            "[Changes] Fast path failed for {} — spawning a detached CC merge (Tier 2)",
                            change.branch_name
                        );
                    }
                }

                let resume_token = crate::engine::session_queries::lookup_latest_cc_session_id(
                    self.pool(),
                    thread_id,
                )
                .await;

                // INVARIANT: Park the actor by change_id so the conflict-recovery cleanup
                // in `run_session.rs` (the `if let Some(change) = conflict_change`
                // branch — grep for `pending_apply_actors.take`) stamps the
                // resulting ChangeApplied / ChangeApplyFailed with the device
                // that clicked Apply. Without this the cleanup falls through to
                // None, which renders as "Lucidos Engine" in the chat chip. The
                // stash is load-bearing precisely because the merge is detached:
                // by the time it finishes, this scope is long gone. Mirrors the
                // Tier-3 stash.
                if let Some(a) = actor.as_ref() {
                    self.pending_apply_actors.stash(change_id, a.clone());
                }

                // INVARIANT: Detach. `run_merge_session_tier2` drives an entire
                // coding-agent session, and awaiting it here would tie that
                // session's lifetime to the caller's future — for the HTTP
                // handlers that means an iOS PWA backgrounding itself kills a
                // merge mid-conflict-resolution (2026-07-28, thread 293f96d5:
                // the subprocess died 72 s in with `interruptedByShutdown`, and
                // the entry it left behind wedged the thread). Every other
                // CC-assisted merge path already spawns — Tier 1 via
                // `spawn_in_place_conflict_recovery`, Tier 3 via
                // `spawn_merge_session`; Tier 2 was the last inline one.
                //
                // Nothing is lost by returning early: the outcome was never
                // carried by this return value. The conflict-recovery cleanup in
                // `run_session.rs` owns the terminal (`ChangeApplied` /
                // `ChangeApplyFailed`) and the post-await block here only ever
                // re-read the change row to shape an `ApplyResult` for the
                // caller. Callers cope with `Conflict` already: the frontend
                // resolves its spinner on the events, and the Apply-All driver
                // is fed by the same events.
                // No apply-level liveness timeout here, unlike Tier 1's
                // `spawn_in_place_conflict_recovery`. That one waits on its
                // prompt's turn in a session it does not own, so a silent
                // agent would hang it forever. This task OWNS the session
                // through `run_direct_agent`, which carries the in-loop and
                // external watchdogs — a second timer on top would just race
                // them. (Tier 2 had no such timeout while it was inline either.)
                let engine = self.clone_arc();
                let branch_name = change.branch_name.clone();
                let description = change.description.clone();
                let merge_wt = wt_path.clone();
                let tier3_change = change.clone();
                let tier3_kind_ctx = kind_ctx.clone();
                let tier3_repo_root = repo_root.clone();
                let task_actor = actor.clone();
                Self::spawn_cc_task_guarded(engine.clone(), thread_id, async move {
                    match engine
                        .agent_host()
                        .run_merge_session_tier2(
                            engine.as_ref(),
                            thread_id,
                            &claim,
                            thread_hold,
                            &merge_wt,
                            &branch_name,
                            &description,
                            resume_token,
                        )
                        .await
                    {
                        Ok(_) => {
                            // INVARIANT: The cleanup already emitted the terminal and, on
                            // success, ff'd main.
                            //
                            // Reconcile orphaned sibling pending changes here,
                            // not in `apply_change`: that call site is gated on
                            // the returned `ApplyStatus::Applied`, and this path
                            // now returns `Conflict` long before the merge
                            // lands. Re-read the row for the same reason the
                            // old inline code did — the cleanup, not this task,
                            // decides whether the change actually applied — and
                            // gate on `Applied` so a failed or handed-off
                            // merge never discards a newer sibling's work (the
                            // data-loss trap `apply_change`'s `Applied` gate
                            // exists to avoid).
                            match engine.changes().get_by_id(change_id).await {
                                Ok(Some(c)) if c.status() == ChangeStatus::Applied => {
                                    engine
                                        .discard_orphaned_pending_siblings(
                                            thread_id,
                                            change_id,
                                            task_actor.clone(),
                                        )
                                        .await;
                                }
                                Ok(_) => {}
                                Err(e) => log_legacy!(
                                    "[Changes] post-merge re-read of {} failed: {} — \
                                     skipping the orphan-sibling reconcile",
                                    change_id,
                                    e
                                ),
                            }
                            engine.broadcast_changes_updated().await;
                        }
                        Err(e) => {
                            log_legacy!(
                                "[Changes] CC merge failed for {}: {} — falling back to Tier 3",
                                change_id,
                                e
                            );
                            engine.pending_apply_actors.take(change_id);
                            let _ = git_cmd(&["merge", "--abort"], &merge_wt).await;
                            if let Err(e) = engine
                                .apply_change_tier3(
                                    &tier3_change,
                                    claim,
                                    task_actor,
                                    &tier3_kind_ctx,
                                    &tier3_repo_root,
                                    effective_requires_restart,
                                )
                                .await
                            {
                                log_legacy!(
                                    "[Changes] Tier-3 fallback after the Tier-2 merge failure also failed for {}: {}",
                                    change_id,
                                    e
                                );
                            }
                        }
                    }
                });

                return Ok(ApplyResult::conflict(
                    change_id,
                    thread_id,
                    change.files.len(),
                    "Merge conflict — the coding-agent session is resolving it. \
                     The change will apply automatically when resolution completes.",
                ));
            }
        }

        self.apply_change_tier3(
            &change,
            claim,
            actor,
            &kind_ctx,
            &repo_root,
            effective_requires_restart,
        )
        .await
    }

    pub async fn queue_in_landing_lane(
        &self,
        change: crate::core::changes::Change,
        claim: ApplyClaim,
        actor: Option<MessageOrigin>,
    ) -> Result<ApplyResult, Box<dyn std::error::Error + Send + Sync>> {
        let thread_id = change.thread_id.unwrap_or(change.id);
        let snapshot_sha = match self.landing_snapshot(&change).await {
            Ok(sha) => sha,
            Err(msg) => {
                self.emit_apply_failed(thread_id, change.id, &msg, actor)
                    .await;
                return Err(msg.into());
            }
        };
        self.submit_to_landing_lane(Submission {
            change,
            claim,
            actor,
            snapshot_sha,
        })
        .await
        .map_err(Into::into)
    }

    async fn landing_snapshot(
        &self,
        change: &crate::core::changes::Change,
    ) -> Result<String, String> {
        let thread_id = change.thread_id.unwrap_or(change.id);
        match queued_snapshot(self.pool(), thread_id, change.id).await {
            Ok(Some(sha)) => return Ok(sha),
            Ok(None) => {}
            Err(e) => {
                return Err(format!(
                    "Could not read whether the change is already queued: {e}. Try Apply again."
                ))
            }
        }
        let repo_root = Path::new(&change.repo_root);
        match find_worktree_for_branch(repo_root, &change.branch_name).await {
            WorktreeLookup::Unknown => {
                return Err(format!(
                    "Could not determine which worktree holds branch {} (git worktree list gave \
                     no answer). Refusing to queue it without committing its work. Try Apply again.",
                    change.branch_name
                ))
            }
            WorktreeLookup::NotFound => {}
            WorktreeLookup::Found(worktree) => {
                let hold = match change.thread_id {
                    Some(tid) => self
                        .hold_thread_for_worktree_reset(tid)
                        .await
                        .map_err(|e| format!("Could not hold the thread for the apply: {e}"))?,
                    None => None,
                };
                if hold.is_some() {
                    commit_worktree_or_err(&worktree, "Coding agent changes (pre-landing auto-commit)")
                        .await
                        .map_err(|e| {
                            format!(
                                "Could not commit the uncommitted work in {} before queueing it: {e}",
                                worktree.display()
                            )
                        })?;
                }
            }
        }
        branch_head_sha(repo_root, &change.branch_name)
            .await
            .ok_or_else(|| {
                format!(
                    "Could not read the head of branch {}. Try Apply again.",
                    change.branch_name
                )
            })
    }

    /// INVARIANT: Tier 3: no live session and no worktree for the branch — fast-forward
    /// `main` directly when the branch is already a descendant, else try a
    /// throwaway catchup worktree, else hand the merge to a coding agent in a
    /// temp worktree and return `Conflict` so the caller stops waiting.
    ///
    /// Split out of `apply_change_inner` so the *detached* Tier-2 merge task can
    /// reach it too: Tier 2 used to fall through to this code inline when its
    /// merge session failed, and that fallthrough has to survive the merge
    /// moving off the caller's future.
    async fn apply_change_tier3(
        self: &Arc<Self>,
        change: &crate::core::changes::Change,
        claim: ApplyClaim,
        actor: Option<MessageOrigin>,
        kind_ctx: &ApplyKindContext,
        repo_root: &std::path::Path,
        effective_requires_restart: bool,
    ) -> Result<ApplyResult, Box<dyn std::error::Error + Send + Sync>> {
        let change_id = claim.change_id();

        // INVARIANT: The next heading separates Tier 2 from Tier 3; a test uses it to delimit the source region.
        // Tier 3: No worktree — try ff directly, spawn CC in temp worktree if needed

        {
            let _merge_guard = MAIN_LOCKS.lock_merge().await;
            let held = self.lock_workspace_repo().await;
            let fast_forward = ff_main_to_branch(&held, repo_root, &change.branch_name).await;
            drop(held);
            if let Ok(shas) = fast_forward {
                log_legacy!(
                    "[Changes] Fast path succeeded for {} (Tier 3)",
                    change.branch_name
                );
                let commits = commits_in_range(repo_root, &shas.0, &shas.1).await;
                return self
                    .finish_merged_apply(
                        change,
                        &claim,
                        actor,
                        kind_ctx,
                        effective_requires_restart,
                        MergedMain {
                            repo_root,
                            pre_sha: shas.0,
                            post_sha: shas.1,
                            commits,
                        },
                    )
                    .await;
            }
        }

        {
            let temp_wt = worktrees_dir(self.workspace_path())
                .join(format!("apply-{}", change_id.as_simple()));
            let temp_wt_str = temp_wt.to_string_lossy().into_owned();
            let _ = git_cmd(&["worktree", "remove", "--force", &temp_wt_str], repo_root).await;

            let add_ok = matches!(
                worktree_add(repo_root, &temp_wt, &[&change.branch_name]).await,
                Ok(o) if o.status.success()
            );

            if add_ok {
                let result = catchup_and_ff_to_main(repo_root, &temp_wt, &change.branch_name).await;
                let _ = git_cmd(&["worktree", "remove", "--force", &temp_wt_str], repo_root).await;

                if let Ok((pre_sha, post_sha)) = result {
                    // INVARIANT: `catchup_and_ff_to_main` deletes change.branch_name on success.
                    // On failure, leave it intact so the CC slow path below can still merge from it.
                    log_legacy!(
                        "[Changes] Auto-merge path succeeded for {} (Tier 3)",
                        change.branch_name
                    );
                    let commits = commits_in_range(repo_root, &pre_sha, &post_sha).await;
                    return self
                        .finish_merged_apply(
                            change,
                            &claim,
                            actor,
                            kind_ctx,
                            effective_requires_restart,
                            MergedMain {
                                repo_root,
                                pre_sha,
                                post_sha,
                                commits,
                            },
                        )
                        .await;
                }
            }
        }

        let thread_id = change.thread_id.unwrap_or_else(Uuid::new_v4);
        let temp_branch = format!("merge-tmp/{}", change_id.as_simple());
        let wt_path =
            worktrees_dir(self.workspace_path()).join(format!("merge-{}", change_id.as_simple()));
        let wt_path_str = wt_path.to_string_lossy().into_owned();

        // INVARIANT: The in-flight guard sees only a LIVE resolver, so one whose turn
        // ended can leave its half-done merge in this pair.
        if let Err(msg) = clear_temp_merge_state(
            repo_root,
            &wt_path,
            &temp_branch,
            &change.branch_name,
            "[Changes]",
        )
        .await
        {
            log_legacy!("[Changes] Apply refused for {}: {}", change_id, msg);
            self.emit_apply_failed(
                change.thread_id.unwrap_or(change_id),
                change_id,
                &msg,
                actor.clone(),
            )
            .await;
            return Err(msg.into());
        }

        match worktree_add(repo_root, &wt_path, &["-b", &temp_branch]).await {
            Ok(o) if o.status.success() => {}
            Ok(o) => {
                let msg = format!(
                    "Failed to create merge worktree: {}",
                    String::from_utf8_lossy(&o.stderr).trim()
                );
                self.emit_apply_failed(
                    change.thread_id.unwrap_or(change_id),
                    change_id,
                    &msg,
                    actor.clone(),
                )
                .await;
                return Err(msg.into());
            }
            Err(e) => {
                let msg = format!("Failed to create merge worktree: {}", e);
                self.emit_apply_failed(
                    change.thread_id.unwrap_or(change_id),
                    change_id,
                    &msg,
                    actor.clone(),
                )
                .await;
                return Err(msg.into());
            }
        }

        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::MergeResolutionStarted {
                        change_id: change_id.to_string(),
                        worktree_path: wt_path_str.clone(),
                        temp_branch: temp_branch.clone(),
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                "[Changes] MergeResolutionStarted",
            )
            .await;

        if let Some(a) = actor.as_ref() {
            self.pending_apply_actors.stash(change_id, a.clone());
        }
        self.agent_host()
            .spawn_merge_session(self.as_ref(), thread_id, claim, &change.description);
        Ok(ApplyResult::conflict(
            change_id,
            thread_id,
            change.files.len(),
            "Branch needs merging — agent is handling it.",
        ))
    }
}
