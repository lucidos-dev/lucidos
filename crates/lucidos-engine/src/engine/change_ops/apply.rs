use super::*;
use crate::engine::agent_session::InPlaceMergeStart;
use crate::engine::git_ops::{
    auto_commit_safe_files_if_dirty, auto_commit_worktree, catchup_and_ff_to_main,
    commits_in_range, ff_main_to, files_have_client_update, find_branch_merge_in_main,
    find_worktree_for_branch, has_branch_commits, is_harden_marker_present,
    push_main_in_background, recover_no_commits_branch, worktree_add, worktrees_dir,
    NoCommitsRecovery, WorktreeLookup, MERGE_MUTEX,
};
use crate::engine::{ApplyResult, ApplyStatus};

/// Pick the worktree the hardening session runs in, given what git said about
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
            log!(
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

impl LucidosEngine {
    /// Mark a change applied without merging anything: delete the branch ref,
    /// emit `ChangeApplied`, broadcast the projection, return `ApplyResult::noop`.
    /// Shared by the two `apply_change` recovery paths that resolve to a no-op
    /// (`LegitimateNoOp`, `AlreadyApplied`).
    async fn finalize_change_as_noop(
        &self,
        change: &crate::core::changes::Change,
        change_id: Uuid,
        repo_root: &Path,
        actor: Option<MessageOrigin>,
        result_message: &'static str,
    ) -> ApplyResult {
        let _ = git_cmd(&["branch", "-D", &change.branch_name], repo_root).await;
        self.emit_change_applied(
            change.thread_id.unwrap_or(change_id),
            change_id,
            false,
            false,
            Vec::new(),
            change.thread_title.clone(),
            actor,
            None,
            None,
        )
        .await;
        self.broadcast_changes_updated().await;
        ApplyResult::noop(
            change_id,
            change.thread_id,
            change.files.len(),
            result_message,
        )
    }

    /// Apply a single pending change: merge its branch into main.
    /// `actor` identifies who initiated the apply. HTTP callers construct via
    /// `api::actor::build_message_origin`; engine-internal callers pass `None`.
    ///
    /// Thin wrapper over `apply_change_inner` that runs the apply-time
    /// **"≤1 pending change per thread" reconcile** — discard any other pending
    /// change the thread still holds on a stale branch, so a pre-existing orphan
    /// can't keep blocking Archive. Gated on the change ACTUALLY merging
    /// (`ApplyStatus::Applied` — not `Noop`/`Hardening`/`Conflict`, which also
    /// return `Ok`); that gate is load-bearing, since reconciling on
    /// `Noop`/`Hardening`/`Conflict` would discard a *newer* sibling (data loss)
    /// or drop siblings before the merge lands. See
    /// docs/plans/2026-07-01-orphaned-pending-change-blocks-archive.md.
    ///
    /// This single point covers every `apply_change` caller (HTTP handler, the
    /// no-live `apply_now` fast/stale paths, the Apply-All driver, the
    /// post-hardening auto-apply re-entry) whose merge finishes *before* the
    /// return. Two paths finish after it and reconcile themselves, for the same
    /// reason: their result is `Conflict` at return time, so this gate can never
    /// see the eventual `Applied`. The live in-place merge reconciles in
    /// `apply_now_success`; the detached Tier-2 merge reconciles in its spawned
    /// task, re-reading the change row and gating on `status == "applied"` so the
    /// data-loss trap above still holds.
    ///
    /// The *other* apply-time follow-up — kicking the background engine rebuild
    /// (or re-snapshotting the served `dist/`) — deliberately does NOT live here:
    /// it hangs off `emit_change_applied`, the one emit every merge path performs
    /// exactly once. See `change_ops_emitters::post_apply_dev_refresh`.
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

    /// Inner apply implementation — see `apply_change` for the apply-time orphan
    /// reconcile. `actor` identifies who initiated the apply. HTTP callers
    /// construct via `api::actor::build_message_origin`; engine-internal callers
    /// pass `None`.
    async fn apply_change_inner(
        self: &Arc<Self>,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<ApplyResult, Box<dyn std::error::Error + Send + Sync>> {
        let change = self
            .changes()
            .get_by_id(change_id)
            .await?
            .ok_or("Change not found")?;

        // Resolve coding-agent-kind context up front so every branch below
        // (hardening gate, restart gating, post-success refresh) reads the
        // same value. App threads:
        //   - skip the /harden gate (apps own their hardening),
        //   - force requires_restart=false (data-tree changes never
        //     restart the engine),
        //   - emit AppUiRefreshRequested when iframe-bundled files change
        //     so open iframes reload.
        let kind_ctx = load_apply_kind_context(&self.pool, change.thread_id).await;
        let effective_requires_restart = change.requires_restart && !kind_ctx.is_app();

        if change.status == "applied" {
            // Echo the stored merge SHAs so a re-apply still gives callers
            // a verifiable reference to the original merge. Use the
            // effective restart flag here too — an app re-apply must never
            // claim it needs a restart even if a stale row says otherwise.
            return Ok(ApplyResult {
                status: ApplyStatus::Noop,
                change_id,
                thread_id: change.thread_id,
                restart_required: effective_requires_restart,
                message: "Change already applied.".to_string(),
                applied_commit: change.post_merge_sha.clone(),
                previous_commit: change.pre_merge_sha.clone(),
                commits_applied: change.commits.len(),
                files_changed: change.files.len(),
                ..ApplyResult::default()
            });
        }
        if change.status != "pending" {
            return Err(format!("Change is already {}", change.status).into());
        }

        // Idempotency fast-path: when the branch ref is gone, check if it was
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
                    log!("[Changes] Branch {} already merged into main as {} — marking change {} applied",
                        change.branch_name,
                        &post_sha[..post_sha.floor_char_boundary(8)],
                        change_id);
                    let commits = commits_in_range(&repo_root, &pre_sha, &post_sha).await;
                    // Phase 6.2: branch was already deleted out-of-band, but
                    // the worktree may still be on disk. Keep it — the thread
                    // is still alive and the next user message will reuse it.
                    // We can't reset to main here because the branch ref is
                    // gone, so the worktree is effectively detached at the
                    // pre-merge SHA; the spawn dispatcher handles that case.
                    let thread_id = change.thread_id.unwrap_or(change_id);
                    self.emit_change_applied(
                        thread_id,
                        change_id,
                        effective_requires_restart,
                        files_have_client_update(&change.files),
                        commits.clone(),
                        change.thread_title.clone(),
                        actor.clone(),
                        Some(pre_sha.clone()),
                        Some(post_sha.clone()),
                    )
                    .await;
                    self.maybe_emit_app_ui_refresh(&kind_ctx, &change.files, actor.as_ref())
                        .await;
                    self.emit_entity_events_for_change_apply(
                        &change.files,
                        Some(&pre_sha),
                        Some(&post_sha),
                        actor.clone(),
                    )
                    .await;
                    self.broadcast_changes_updated().await;
                    return Ok(ApplyResult::applied_with_merge(
                        change_id,
                        change.thread_id,
                        effective_requires_restart,
                        pre_sha,
                        post_sha,
                        &commits,
                        change.files.len(),
                    ));
                }
            }
        }

        // Clear stale merge worktree metadata if the directory no longer exists
        if let Some(ref wt) = change.merge_worktree_path {
            if !std::path::Path::new(wt).exists() {
                log!(
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

        // An in-flight conflict resolution owns this change's merge.
        //
        // Placement is load-bearing, and this is the FIRST thing after the
        // stale-merge-worktree self-heal above (which closes a dead Tier-3
        // pairing by emitting `MergeResolutionCleared`, so a pruned temp
        // worktree cannot refuse forever). Everything below this point has a
        // side effect a refused apply must not have: the plan gate and the
        // harden gate emit events and can spawn a session, the dirty-tree check
        // auto-commits the workspace repo, and the tiers move `main`.
        //
        // 2026-08-11: without this, a second `apply_change` for a change whose
        // Tier-2 resolution was mid-turn took the Tier-1 path (the resolver's
        // own session is a live session), fast-forwarded `main` at step 2 of
        // the resolver's 5-step merge prompt, and then `apply_now_success` ran
        // `reset --hard main` + `clean -fd` inside the resolver's worktree
        // while it was still working. `apply_now_in_progress` did not catch it:
        // only `apply_now` and the Tier-1 path set that flag, never the
        // detached Tier-2 / Tier-3 merge spawns.
        //
        // `Conflict`, not `Err`: the frontend, the Apply-All driver and the
        // `apply_change` LLM tool all already read `Conflict` as "an agent is
        // resolving it", and the change really does apply on its own when the
        // resolver's completion (`finalize_direct_agent`) lands its terminal.
        //
        // Deliberately NOT covered: the Tier-2 / Tier-3 startup window, where
        // the spawned task has opened the pairing but its session is not
        // registered yet, so no resolver is named. `main` cannot move there.
        // Reaching a resolution at all means `catchup_and_ff_to_main` already
        // failed on this branch, and in that window nothing has merged yet, so
        // a second apply's ff fails identically. Closing the window would mean
        // an in-memory claim spanning a spawn, which is the wedging shape
        // ADR 0060 rejects. Tier 1 has no such window: it binds the session
        // before it opens the pairing.
        if let Some(tid) = change.thread_id {
            if self.merge_ownership_for_change(tid, change_id).await
                == MergeOwnership::ResolverOwnsIt
            {
                log!(
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

        // Implementation-plan floor (Lucidos-source only). A gate-satisfying
        // marker — an APPROVED plan (`planned`), or an explicit
        // `lucidos planned mark --simple` acknowledgment — MUST exist before a
        // Lucidos-source change can apply. A `proposed` (awaiting-approval)
        // marker does NOT satisfy the floor: the human hasn't approved the plan
        // yet. The Claude-Code `cc-plan-gate` PreToolUse hook and the prompt
        // rule are meant to make a non-satisfying branch unreachable; this is
        // the hard backstop (and the ONLY enforcement for Codex, which has no
        // PreToolUse hook). App and external-repo changes are exempt — neither
        // uses the `docs/plans/` convention or the marker. Per the resolved
        // design decision: if the marker is Missing/Proposed here, refuse the
        // apply (no auto-recovery).
        // App and external-repo changes are exempt — neither uses the
        // `docs/plans/` convention or the marker, so don't even query.
        if kind_ctx.is_lucidos_source() {
            let plan_state = self
                .plan_marker_state(
                    &std::path::PathBuf::from(&change.repo_root),
                    &change.branch_name,
                )
                .await;
            if !plan_state.satisfies_gate() {
                let msg = if plan_state.is_present() {
                    // Present but not satisfying => Proposed (awaiting approval).
                    "The implementation plan on this branch is awaiting approval. The user must \
                     approve the plan, after which the coding agent runs `lucidos planned approve` \
                     to unblock implementation. Approve the plan, then apply."
                } else {
                    "No implementation-plan marker on this branch. Before applying, the coding \
                     agent must run the `implementation-plan` skill (records a plan for the user to \
                     approve) or `lucidos planned mark --simple \"<reason>\"` (acknowledges a local \
                     fix). Re-run the session to set the marker, then apply."
                };
                log!(
                    "[Changes] Apply blocked — plan marker not satisfying ({:?}) for branch {} (change {})",
                    plan_state,
                    change.branch_name,
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
        }

        // App threads skip the /harden gate entirely — apps own their own
        // hardening (per the app's `.claude/commands/harden.md` if it ships
        // one); the engine never invokes `/harden` for `data/apps/<id>/`
        // changes and the changes.hardened flag is ignored for them.
        let marker_hardened = if kind_ctx.is_app() {
            true
        } else {
            is_harden_marker_present(
                &self.pool,
                &std::path::PathBuf::from(&change.repo_root),
                &change.branch_name,
            )
            .await
        };
        if marker_hardened && !change.hardened && !kind_ctx.is_app() {
            log!(
                "[Changes] Marker found but DB not set — marking change {} as hardened",
                change_id
            );
            if let Some(tid) = change.thread_id {
                self.emit_change_hardened(tid, change_id, "[Changes] ChangeHardened")
                    .await;
            }
        }
        if !change.hardened && !marker_hardened {
            let thread_id = match change.thread_id {
                Some(tid) => tid,
                None => {
                    let msg = "Change has no associated thread — cannot run hardening automatically. Discard and re-create with a Claude Code session.";
                    self.emit_apply_failed(change_id, change_id, msg, actor.clone())
                        .await;
                    return Err(msg.into());
                }
            };

            // Live session: route harden through it (emit + send). If send fails
            // the boundary is already up — fall through to the worktree spawn
            // without re-emitting. No live session: emit boundary now.
            if let Some(session) =
                CodingAgentChangeOps::live_session_info(self.as_ref(), thread_id).await
            {
                match self
                    .request_hardening_in_session(thread_id, &session.msg_tx)
                    .await
                {
                    Ok(()) => {
                        log!(
                            "[Changes] Reusing existing session on thread {} for hardening",
                            thread_id
                        );
                        return Ok(ApplyResult::hardening(
                            change_id,
                            thread_id,
                            change.files.len(),
                            "Hardening sent to existing session — please wait.",
                        ));
                    }
                    Err(e) => {
                        log!(
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

            log!(
                "[Changes] Not hardened — spawning hardening recovery for change {} (thread {})",
                change_id,
                thread_id
            );
            CodingAgentChangeOps::spawn_hardening(
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

        // Validate branch exists — don't auto-discard, let the user decide
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

        // When the branch ref looks empty, the worktree may still hold uncommitted
        // CC work (multi-draft compose, never auto-committed). Rescue it before
        // taking any destructive action; refuse to silently apply when files are
        // declared but the branch has nothing to merge.
        if !has_branch_commits(&repo_root, &change.branch_name).await {
            match recover_no_commits_branch(&repo_root, &change.branch_name, &change.files).await {
                Ok(NoCommitsRecovery::AutoCommitted) => {
                    log!(
                        "[Changes] Auto-committed worktree for branch {} — proceeding with merge",
                        change.branch_name
                    );
                }
                Ok(NoCommitsRecovery::LegitimateNoOp) => {
                    return Ok(self
                        .finalize_change_as_noop(
                            &change,
                            change_id,
                            &repo_root,
                            actor.clone(),
                            "Change applied (no commits to merge).",
                        )
                        .await);
                }
                Ok(NoCommitsRecovery::AlreadyApplied) => {
                    log!(
                        "[Changes] Branch {} already merged into main — marking change {} as applied (no-op)",
                        change.branch_name,
                        change_id
                    );
                    return Ok(self
                        .finalize_change_as_noop(
                            &change,
                            change_id,
                            &repo_root,
                            actor.clone(),
                            "Change already present on main — marked applied.",
                        )
                        .await);
                }
                Err(e) => {
                    let msg = e.to_string();
                    log!(
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

        // Auto-commit safe files (docs) if they're the only dirty files, then
        // reject if the workspace tree still has genuine uncommitted changes.
        //
        // MERGE_MUTEX is held alongside workspace_repo_lock here: a concurrent
        // apply's merge advances `refs/heads/main` via `ff_main_to`'s
        // `update-ref` and only afterwards resets the working tree via
        // `checkout -f main`. Between those two git calls the tree transiently
        // looks dirty (`main` moved, the working tree hasn't caught up). Without
        // MERGE_MUTEX this gate would observe that mid-merge window and reject a
        // perfectly valid concurrent apply with a spurious "uncommitted changes"
        // error (regression caught by app_coding_agent_concurrent_apply). Taking
        // MERGE_MUTEX guarantees the gate only ever sees a settled tree — the
        // other apply's merge has either not started or finished its checkout.
        // Lock order is always MERGE_MUTEX → workspace_repo_lock; no other site
        // acquires both (merges take only MERGE_MUTEX, data writes take only
        // workspace_repo_lock), so this can't deadlock.
        //
        // Both guards drop at the end of this block so neither is held across a
        // tier's Claude Code subprocess await — those merges happen in separate
        // worktrees and would otherwise block every data API write for minutes.
        {
            let _merge_guard = MERGE_MUTEX.lock().await;
            let _repo_guard = self.lock_workspace_repo().await;
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

        // Tier 1: If a live agent session exists for this thread, merge main
        // into its worktree instead of creating a temp worktree. The original
        // agent has full context and can resolve conflicts intelligently.
        //
        // A clean fast-forward merge runs inline (sub-second) and returns
        // `applied`. When `main` has diverged and CC-assisted conflict
        // resolution is needed, that runs in a background task and we return
        // `Conflict` immediately — the caller's turn (the parent chat thread's
        // `apply_change` tool call) is not held open for the potentially
        // many-minute resolution. The eventual `ChangeApplied` plus the EventBus
        // parent-callback fan-out's `ChildThreadCompleted` give the parent a
        // fresh follow-up turn instead of one turn frozen the whole time.
        if let Some(thread_id) = change.thread_id {
            match self.begin_in_place_merge(thread_id).await {
                InPlaceMergeStart::Ready(session) => {
                    log!(
                        "[Changes] Live agent session found for thread {} — merging in-place",
                        thread_id
                    );

                    // Auto-commit any uncommitted work before merging.
                    auto_commit_worktree(
                        &session.worktree_path,
                        "Coding agent changes (pre-merge auto-commit)",
                    )
                    .await;

                    // Fast path: clean fast-forward — finalize synchronously.
                    if let Ok((pre_sha, post_sha)) = catchup_and_ff_to_main(
                        &repo_root,
                        &session.worktree_path,
                        &change.branch_name,
                    )
                    .await
                    {
                        log!(
                            "[Changes] Fast-forward merge succeeded for {} (Tier 1)",
                            change.branch_name
                        );
                        // We claimed the in-progress flag in `begin_in_place_merge`;
                        // clear it since we're finalizing inline (no spawned task).
                        self.clear_apply_now_in_progress(thread_id).await;
                        // apply_now_success emits ChangeApplied, the entity-cache
                        // events (App*/Artifact*), AND AppUiRefreshRequested (for
                        // app threads) internally — no sibling emit needed here.
                        let commits = self
                            .apply_now_success(
                                thread_id,
                                change_id,
                                effective_requires_restart,
                                files_have_client_update(&change.files),
                                &pre_sha,
                                &post_sha,
                                &session.worktree_path,
                                &repo_root,
                                &change.branch_name,
                                actor.clone(),
                            )
                            .await;
                        return Ok(ApplyResult::applied_with_merge(
                            change_id,
                            Some(thread_id),
                            effective_requires_restart,
                            pre_sha,
                            post_sha,
                            &commits,
                            change.files.len(),
                        ));
                    }

                    // `main` diverged — hand the CC-assisted resolution to a
                    // background task and return immediately. The task clears
                    // the in-progress flag and emits the terminal event.
                    log!(
                        "[Changes] main diverged for {} — spawning async in-place conflict recovery",
                        change.branch_name
                    );
                    self.spawn_in_place_conflict_recovery(
                        thread_id,
                        change_id,
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
                InPlaceMergeStart::AlreadyInProgress => {
                    log!(
                        "[Changes] Apply already in progress for thread {} — returning conflict",
                        thread_id
                    );
                    return Ok(ApplyResult::conflict(
                        change_id,
                        thread_id,
                        change.files.len(),
                        "An apply is already in progress for this thread — it will finish on its own.",
                    ));
                }
                // No live session — fall through to the dead-session tiers below.
                InPlaceMergeStart::NoLiveSession => {}
            }
        }

        // Tier 2: dead session with worktree on disk — try ff, else resume CC for merge
        if let Some(thread_id) = change.thread_id {
            let lookup = find_worktree_for_branch(&repo_root, &change.branch_name).await;
            // An unknown lookup must not silently downgrade the tier. Tier 3
            // force-removes its temp tree and can fast-forward main without ever
            // running the auto-commit below over the real worktree.
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
                // Auto-commit any uncommitted CC work before merging
                auto_commit_worktree(&wt_path, "Coding agent changes (pre-merge auto-commit)")
                    .await;

                // Fast path: try ff directly
                match catchup_and_ff_to_main(&repo_root, &wt_path, &change.branch_name).await {
                    Ok((pre_sha, post_sha)) => {
                        log!(
                            "[Changes] Fast path succeeded for {} (Tier 2)",
                            change.branch_name
                        );
                        let commits = commits_in_range(&repo_root, &pre_sha, &post_sha).await;
                        // Phase 6.2: keep worktree on disk; reset to main HEAD
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
                        // Phase 4: Apply no longer terminates the thread —
                        // SessionEnded is terminal-only. Phase 6.2: the
                        // worktree and branch are preserved above so the
                        // next user message resumes the same Claude Code session.
                        // ChangeApplied below is the user-visible signal.
                        self.emit_change_applied(
                            thread_id,
                            change_id,
                            effective_requires_restart,
                            files_have_client_update(&change.files),
                            commits.clone(),
                            change.thread_title.clone(),
                            actor.clone(),
                            Some(pre_sha.clone()),
                            Some(post_sha.clone()),
                        )
                        .await;
                        self.maybe_emit_app_ui_refresh(&kind_ctx, &change.files, actor.as_ref())
                            .await;
                        self.emit_entity_events_for_change_apply(
                            &change.files,
                            Some(&pre_sha),
                            Some(&post_sha),
                            actor.clone(),
                        )
                        .await;
                        self.broadcast_changes_updated().await;
                        return Ok(ApplyResult::applied_with_merge(
                            change_id,
                            Some(thread_id),
                            effective_requires_restart,
                            pre_sha,
                            post_sha,
                            &commits,
                            change.files.len(),
                        ));
                    }
                    Err(_) => {
                        // ff failed — CC needs to merge main into the branch,
                        // in a detached task (see the spawn below).
                        log!(
                            "[Changes] Fast path failed for {} — spawning a detached CC merge (Tier 2)",
                            change.branch_name
                        );
                    }
                }

                // Look up resume token for potential resume
                let resume_token =
                    CodingAgentChangeOps::lookup_session_id_for_resume(self.as_ref(), thread_id)
                        .await;

                // Park the actor by change_id so the conflict-recovery cleanup
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

                // Detach. `run_merge_session_tier2` drives an entire
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
                // `spawn_in_place_conflict_recovery`. That one waits on
                // `idle_notify` for a session it does not own, so a silent
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
                    match CodingAgentChangeOps::run_merge_session_tier2(
                        engine.as_ref(),
                        thread_id,
                        change_id,
                        &merge_wt,
                        &branch_name,
                        &description,
                        resume_token,
                    )
                    .await
                    {
                        Ok(_) => {
                            // The cleanup already emitted the terminal and, on
                            // success, ff'd main.
                            //
                            // Reconcile orphaned sibling pending changes here,
                            // not in `apply_change`: that call site is gated on
                            // the returned `ApplyStatus::Applied`, and this path
                            // now returns `Conflict` long before the merge
                            // lands. Re-read the row for the same reason the
                            // old inline code did — the cleanup, not this task,
                            // decides whether the change actually applied — and
                            // gate on `"applied"` so a failed or handed-off
                            // merge never discards a newer sibling's work (the
                            // data-loss trap `apply_change`'s `Applied` gate
                            // exists to avoid).
                            match engine.changes().get_by_id(change_id).await {
                                Ok(Some(c)) if c.status == "applied" => {
                                    engine
                                        .discard_orphaned_pending_siblings(
                                            thread_id,
                                            change_id,
                                            task_actor.clone(),
                                        )
                                        .await;
                                }
                                Ok(_) => {}
                                Err(e) => log!(
                                    "[Changes] post-merge re-read of {} failed: {} — \
                                     skipping the orphan-sibling reconcile",
                                    change_id,
                                    e
                                ),
                            }
                            engine.broadcast_changes_updated().await;
                        }
                        Err(e) => {
                            log!(
                                "[Changes] CC merge failed for {}: {} — falling back to Tier 3",
                                change_id,
                                e
                            );
                            // Tier 3 spawns its own merge session and re-stashes
                            // the actor with its own scope; clear this Tier-2
                            // stash so the fallback doesn't double-park.
                            engine.pending_apply_actors.take(change_id);
                            let _ = git_cmd(&["merge", "--abort"], &merge_wt).await;
                            if let Err(e) = engine
                                .apply_change_tier3(
                                    &tier3_change,
                                    change_id,
                                    task_actor,
                                    &tier3_kind_ctx,
                                    &tier3_repo_root,
                                    effective_requires_restart,
                                )
                                .await
                            {
                                log!(
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
            change_id,
            actor,
            &kind_ctx,
            &repo_root,
            effective_requires_restart,
        )
        .await
    }

    /// Tier 3: no live session and no worktree for the branch — fast-forward
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
        change_id: Uuid,
        actor: Option<MessageOrigin>,
        kind_ctx: &ApplyKindContext,
        repo_root: &std::path::Path,
        effective_requires_restart: bool,
    ) -> Result<ApplyResult, Box<dyn std::error::Error + Send + Sync>> {
        // Tier 3: No worktree — try ff directly, spawn CC in temp worktree if needed

        // Fast path: branch may already be a descendant of main
        {
            let _merge_guard = MERGE_MUTEX.lock().await;
            let main_sha = git_cmd(&["rev-parse", "main"], repo_root)
                .await
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();
            let branch_sha = git_cmd(&["rev-parse", &change.branch_name], repo_root)
                .await
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();

            if let Ok(shas) = ff_main_to(repo_root, &branch_sha, &main_sha).await {
                log!(
                    "[Changes] Fast path succeeded for {} (Tier 3)",
                    change.branch_name
                );
                let commits = commits_in_range(repo_root, &shas.0, &shas.1).await;
                // Phase 6.2: keep the branch alive — it's at the same SHA as
                // main now, and the thread may resume CC against it later. No
                // worktree exists for this branch in Tier 3, so there's
                // nothing to reset.
                // Lucidos-source threads push main to remote so origin tracks
                // the merged work. App threads merge to the workspace git's
                // main, which is local-only by design — no remote push.
                if !kind_ctx.is_app() {
                    push_main_in_background(repo_root);
                }
                self.emit_change_applied(
                    change.thread_id.unwrap_or(change_id),
                    change_id,
                    effective_requires_restart,
                    files_have_client_update(&change.files),
                    commits.clone(),
                    change.thread_title.clone(),
                    actor.clone(),
                    Some(shas.0.clone()),
                    Some(shas.1.clone()),
                )
                .await;
                self.maybe_emit_app_ui_refresh(kind_ctx, &change.files, actor.as_ref())
                    .await;
                self.emit_entity_events_for_change_apply(
                    &change.files,
                    Some(&shas.0),
                    Some(&shas.1),
                    actor.clone(),
                )
                .await;
                self.broadcast_changes_updated().await;
                return Ok(ApplyResult::applied_with_merge(
                    change_id,
                    change.thread_id,
                    effective_requires_restart,
                    shas.0,
                    shas.1,
                    &commits,
                    change.files.len(),
                ));
            }
        }

        // Auto-merge path: main moved forward — try a worktree-based catchup-and-ff
        // before falling back to spawning CC. Handles the common race where a parallel
        // commit on main left this branch behind, but the merge is conflict-free.
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
                    // `catchup_and_ff_to_main` deletes change.branch_name on success.
                    // On failure, leave it intact so the CC slow path below can still merge from it.
                    log!(
                        "[Changes] Auto-merge path succeeded for {} (Tier 3)",
                        change.branch_name
                    );
                    let commits = commits_in_range(repo_root, &pre_sha, &post_sha).await;
                    self.emit_change_applied(
                        change.thread_id.unwrap_or(change_id),
                        change_id,
                        effective_requires_restart,
                        files_have_client_update(&change.files),
                        commits.clone(),
                        change.thread_title.clone(),
                        actor.clone(),
                        Some(pre_sha.clone()),
                        Some(post_sha.clone()),
                    )
                    .await;
                    self.maybe_emit_app_ui_refresh(kind_ctx, &change.files, actor.as_ref())
                        .await;
                    self.emit_entity_events_for_change_apply(
                        &change.files,
                        Some(&pre_sha),
                        Some(&post_sha),
                        actor.clone(),
                    )
                    .await;
                    self.broadcast_changes_updated().await;
                    return Ok(ApplyResult::applied_with_merge(
                        change_id,
                        change.thread_id,
                        effective_requires_restart,
                        pre_sha,
                        post_sha,
                        &commits,
                        change.files.len(),
                    ));
                }
            }
        }

        // Slow path: create temp worktree and spawn CC to handle the merge
        let thread_id = change.thread_id.unwrap_or_else(Uuid::new_v4);
        let temp_branch = format!("merge-tmp/{}", change_id.as_simple());
        let wt_path =
            worktrees_dir(self.workspace_path()).join(format!("merge-{}", change_id.as_simple()));
        let wt_path_str = wt_path.to_string_lossy().into_owned();

        let _ = git_cmd(&["worktree", "remove", "--force", &wt_path_str], repo_root).await;
        let _ = git_cmd(&["branch", "-D", &temp_branch], repo_root).await;

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

        // Park the actor by change_id so the cleanup in `run_session` can
        // stamp ChangeApplied / ChangeApplyFailed with the device that clicked
        // Apply, instead of falling through to the "Lucidos Engine" chip.
        // The HTTP call returns immediately below — by the time the spawned
        // CC merges and idles, this scope is long gone, so the stash is the
        // only way to keep the actor available for the eventual emit.
        if let Some(a) = actor.as_ref() {
            self.pending_apply_actors.stash(change_id, a.clone());
        }
        CodingAgentChangeOps::spawn_merge_session(
            self.as_ref(),
            thread_id,
            change_id,
            &change.description,
        );
        Ok(ApplyResult::conflict(
            change_id,
            thread_id,
            change.files.len(),
            "Branch needs merging — agent is handling it.",
        ))
    }
}
