use super::git_ops::{
    auto_commit_safe_files_if_dirty, auto_commit_worktree, catchup_and_ff_to_main,
    commits_in_range, ff_main_to, files_have_client_update, find_branch_merge_in_main,
    find_worktree_for_branch, git_cmd, harden_marker_state, has_branch_commits,
    is_harden_marker_present, is_merge_of_branch_into_main, push_main_in_background,
    recover_no_commits_branch, worktree_add, worktrees_dir, HardenMarkerState, NoCommitsRecovery,
    MERGE_MUTEX,
};
use super::thread_events::MessageOrigin;
use super::{ApplyResult, ApplyStatus, LucidosEngine};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

/// Information about a live agent session, sufficient for in-place merge.
pub(crate) struct LiveSessionInfo {
    pub worktree_path: PathBuf,
    pub idle_notify: Arc<tokio::sync::Notify>,
    pub msg_tx: tokio::sync::mpsc::UnboundedSender<crate::engine::AgentUserInput>,
}

/// Inputs for proposing (or updating) a pending change.
///
/// `hardened` is set atomically — callers declare hardening status up front.
/// `origin` flows onto the emitted `ChangeProposed` event so engine-internal
/// recovery paths (stale-session, orphan-recovery) can stamp themselves; live
/// agent callers pass `None` (the surrounding `MessageReceived` carries the
/// user/agent origin).
pub(crate) struct ProposeChangeInput<'a> {
    pub thread_id: Uuid,
    pub branch_name: &'a str,
    pub repo_root: &'a str,
    pub description: &'a str,
    pub files: &'a [String],
    pub requires_restart: bool,
    pub channel: crate::engine::thread_events::EventChannel,
    pub hardened: bool,
    pub origin: Option<MessageOrigin>,
    /// `true` when the proposing CC turn ended in `ResponseFailed` — the
    /// worktree contents reflect partial work, not a deliberate completion.
    /// Flows onto `ChangeProposed.incomplete` and the `changes.incomplete`
    /// projection column so the apply UI can confirm before landing.
    pub incomplete: bool,
}

/// Trait for coding agents that can interact with change management.
pub(crate) trait CodingAgent: Send + Sync {
    async fn is_running_for(&self, thread_id: Uuid) -> bool;
    /// `actor` carries the user who initiated the apply that triggered hardening.
    /// When `auto_apply_change_id` is `Some` and hardening completes successfully,
    /// the agent re-enters `apply_change(change_id, actor)` so the resulting
    /// `ChangeApplied` event is stamped with the original user instead of
    /// collapsing to the engine fallback.
    fn spawn_hardening(
        &self,
        thread_id: Uuid,
        worktree_path: PathBuf,
        branch_name: String,
        auto_apply_change_id: Option<Uuid>,
        actor: Option<MessageOrigin>,
    );
    async fn live_session_info(&self, thread_id: Uuid) -> Option<LiveSessionInfo>;
    async fn merge_via_session(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        wt_path: &Path,
        branch_name: &str,
        repo_root: &Path,
        session: &LiveSessionInfo,
    ) -> Result<(String, String), Box<dyn std::error::Error + Send + Sync>>;
    fn spawn_merge_session(&self, thread_id: Uuid, change_id: Uuid, description: &str);
    async fn lookup_session_id_for_resume(&self, thread_id: Uuid) -> Option<String>;
    /// Resume an in-progress merge session via this agent.
    /// `resume_token` is opaque to the engine; each implementor decides what it
    /// means (Claude Code: the `session_id` to pass to `claude --resume`).
    async fn run_merge_session_tier2(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        wt_path: &Path,
        branch_name: &str,
        description: &str,
        resume_token: Option<String>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>;
}

/// Current time as epoch milliseconds (i64). Used for liveness tracking.
pub(crate) fn now_epoch_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// True iff the branch counts as hardened for Apply purposes. Marker existence
/// (Fresh or Stale) means CC ran `/harden` at least once and is trusted; if
/// the marker was consumed by a prior apply (Missing), fall back to the DB
/// `hardened` flag on the pending change row.
pub(crate) async fn branch_is_hardened(
    pool: &sqlx::PgPool,
    changes: &crate::core::changes_projection::ChangesProjection,
    repo_root: &Path,
    branch_name: &str,
) -> bool {
    match harden_marker_state(pool, repo_root, branch_name).await {
        HardenMarkerState::Fresh | HardenMarkerState::Stale => true,
        HardenMarkerState::Missing => changes
            .get_pending_by_branch(branch_name)
            .await
            .map(|c| c.hardened)
            .unwrap_or(false),
    }
}

/// Phase 6.2: Reset a thread's worktree to the new main HEAD after a successful
/// Apply, leaving both the worktree and its branch on disk so the next user
/// message in the thread can resume CC at the deterministic worktree path.
///
/// After a successful merge, the branch ref points at the same SHA as main, so
/// `git reset --hard main` is a no-op for clean worktrees and corrects any
/// straggler state otherwise. `git clean -fd` removes untracked files left
/// behind by the merge (build artifacts excluded by .gitignore are preserved).
///
/// If the worktree has uncommitted changes (rare after a successful merge —
/// usually means the user edited files between `auto_commit_worktree` and the
/// reset), refuse to reset and surface the dirty paths so the caller can fail
/// the apply explicitly. Silent reset would discard user work.
///
/// Errors from `git reset --hard main` propagate so the user sees them; this
/// is a structural change to the thread's worktree and silent failure would
/// leave the thread in a confusing post-Apply state on the next spawn.
pub(crate) async fn reset_worktree_to_main_after_apply(
    worktree_path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Refuse to reset if the worktree is dirty — would silently discard work.
    let status = git_cmd(&["status", "--porcelain"], worktree_path)
        .await
        .map_err(|e| format!("git status failed in worktree {}: {}", worktree_path.display(), e))?;
    if !status.status.success() {
        return Err(format!(
            "git status returned non-zero in worktree {}: {}",
            worktree_path.display(),
            String::from_utf8_lossy(&status.stderr).trim()
        )
        .into());
    }
    let dirty = String::from_utf8_lossy(&status.stdout);
    if !dirty.trim().is_empty() {
        return Err(format!(
            "Worktree {} is dirty after merge — refusing to reset to main. Resolve manually:\n{}",
            worktree_path.display(),
            dirty.trim()
        )
        .into());
    }

    let reset = git_cmd(&["reset", "--hard", "main"], worktree_path)
        .await
        .map_err(|e| {
            format!(
                "git reset --hard main failed in worktree {}: {}",
                worktree_path.display(),
                e
            )
        })?;
    if !reset.status.success() {
        return Err(format!(
            "git reset --hard main failed in worktree {}: {}",
            worktree_path.display(),
            String::from_utf8_lossy(&reset.stderr).trim()
        )
        .into());
    }

    // `git clean -fd` removes untracked files (e.g. half-written merge
    // artifacts). Untracked-but-gitignored build outputs (target/, node_modules/)
    // are preserved — only `clean -fdx` would touch them, which we don't want.
    let clean = git_cmd(&["clean", "-fd"], worktree_path).await.map_err(|e| {
        format!(
            "git clean -fd failed in worktree {}: {}",
            worktree_path.display(),
            e
        )
    })?;
    if !clean.status.success() {
        return Err(format!(
            "git clean -fd failed in worktree {}: {}",
            worktree_path.display(),
            String::from_utf8_lossy(&clean.stderr).trim()
        )
        .into());
    }
    Ok(())
}

/// Phase 6.3: Reset a thread's worktree to main HEAD after a Discard, leaving
/// both the worktree and its branch on disk so the thread can continue. The
/// thread is NOT terminated (Phase 4 already removed the `Discarded` reason
/// from `SessionEndReason`); the next user message resumes the same CC
/// session at a clean worktree.
///
/// Discard is the user's explicit "throw away all CC's pending work" signal,
/// so unlike `reset_worktree_to_main_after_apply` this DOES wipe uncommitted
/// state (`reset --hard main` followed by `clean -fd`). The Apply helper
/// refuses on dirty because Apply's user intent is "merge what CC committed"
/// — silently nuking later user edits would surprise. Discard's user intent
/// is the opposite: "drop everything on this branch beyond main".
///
/// Errors from git propagate so callers can surface them — silent failure
/// would leave the branch advanced past main and the next CC spawn would see
/// a "phantom" pending state for a change the user already discarded.
pub(crate) async fn reset_worktree_to_main_after_discard(
    worktree_path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let reset = git_cmd(&["reset", "--hard", "main"], worktree_path)
        .await
        .map_err(|e| {
            format!(
                "git reset --hard main failed in worktree {}: {}",
                worktree_path.display(),
                e
            )
        })?;
    if !reset.status.success() {
        return Err(format!(
            "git reset --hard main failed in worktree {}: {}",
            worktree_path.display(),
            String::from_utf8_lossy(&reset.stderr).trim()
        )
        .into());
    }

    // `git clean -fd` removes untracked files. Untracked-but-gitignored build
    // outputs (`target/`, `node_modules/`) are preserved — `clean -fdx` would
    // touch them, which we don't want.
    let clean = git_cmd(&["clean", "-fd"], worktree_path).await.map_err(|e| {
        format!(
            "git clean -fd failed in worktree {}: {}",
            worktree_path.display(),
            e
        )
    })?;
    if !clean.status.success() {
        return Err(format!(
            "git clean -fd failed in worktree {}: {}",
            worktree_path.display(),
            String::from_utf8_lossy(&clean.stderr).trim()
        )
        .into());
    }
    Ok(())
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

    /// If a pending change already exists for this branch, returns its ID instead of creating a duplicate.
    pub(crate) async fn propose_change(
        &self,
        input: ProposeChangeInput<'_>,
    ) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
        let ProposeChangeInput {
            thread_id,
            branch_name,
            repo_root,
            description,
            files,
            requires_restart,
            channel,
            hardened,
            origin,
            incomplete,
        } = input;

        // If a pending change already exists for this branch, reuse its
        // change_id and re-emit `ChangeProposed`. The `needs_emit` guard
        // short-circuits when no field changed — without it, every CC
        // end-of-turn would re-emit identical events and inflate history.
        // `incomplete` participates in the dedup so a follow-up successful
        // turn against the same branch clears a prior failure tag (the
        // re-emit propagates `incomplete: false` to the projection row).
        let existing = self.changes().get_pending_by_branch(branch_name).await;
        let change_id = existing.as_ref().map(|c| c.id).unwrap_or_else(Uuid::new_v4);
        let needs_emit = existing.as_ref().is_none_or(|e| {
            e.description != description
                || e.files != files
                || e.requires_restart != requires_restart
                || e.hardened != hardened
                || e.incomplete != incomplete
        });

        if needs_emit {
            self.event_bus
                .emit_or_log(
                    crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::ChangeProposed {
                            change_id: change_id.to_string(),
                            description: Some(description.to_string()),
                            files: files.to_vec(),
                            requires_restart,
                            origin,
                            commit_sha: None,
                            branch_name: branch_name.to_string(),
                            repo_root: repo_root.to_string(),
                            hardened,
                            incomplete,
                            path: String::new(),
                            diff: String::new(),
                        },
                        meta: crate::engine::thread_events::EventMeta {
                            channel: Some(channel),
                            ..crate::engine::thread_events::EventMeta::NONE
                        },
                    },
                    "[Changes] ChangeProposed",
                )
                .await;
            if existing.is_some() {
                self.broadcast_changes_updated().await;
            }
        }
        Ok(change_id)
    }




    /// Emit the harden boundary event then queue AUTO_HARDEN_MESSAGE on a live
    /// agent session. Emit-before-send guarantees the panel sits above any
    /// CodingAgentTextStreamed events CC produces in response.
    ///
    /// The boundary event is stamped engine-origin internally (see
    /// `emit_missing_hardening_detected`), so callers don't pass an actor.
    pub(crate) async fn request_hardening_in_session(
        &self,
        thread_id: Uuid,
        msg_tx: &tokio::sync::mpsc::UnboundedSender<crate::engine::AgentUserInput>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.emit_missing_hardening_detected(thread_id).await;
        msg_tx
            .send(crate::engine::AgentUserInput {
                text: crate::engine::claude_code::AUTO_HARDEN_MESSAGE.to_string(),
                images: None,
                origin_event_id: None,
            })
            .map_err(|_| -> Box<dyn std::error::Error + Send + Sync> {
                "Session channel closed".into()
            })?;
        Ok(())
    }

    /// Apply a single pending change: merge its branch into main.
    /// `actor` identifies who initiated the apply. HTTP callers construct via
    /// `api::actor::build_message_origin`; engine-internal callers pass `None`.
    pub async fn apply_change(
        self: &Arc<Self>,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<ApplyResult, Box<dyn std::error::Error + Send + Sync>> {
        let change = self
            .changes()
            .get_by_id(change_id)
            .await
            .ok_or("Change not found")?;
        if change.status == "applied" {
            // Echo the stored merge SHAs so a re-apply still gives callers
            // a verifiable reference to the original merge.
            return Ok(ApplyResult {
                status: ApplyStatus::Noop,
                change_id,
                thread_id: change.thread_id,
                restart_required: change.requires_restart,
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
        {
            let repo_root = std::path::PathBuf::from(&change.repo_root);
            let branch_exists =
                git_cmd(&["rev-parse", "--verify", &change.branch_name], &repo_root)
                    .await
                    .map(|o| o.status.success())
                    .unwrap_or(false);
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
                        change.requires_restart,
                        files_have_client_update(&change.files),
                        commits.clone(),
                        change.thread_title.clone(),
                        actor.clone(),
                        Some(pre_sha.clone()),
                        Some(post_sha.clone()),
                    )
                    .await;
                    self.broadcast_changes_updated().await;
                    return Ok(ApplyResult::applied_with_merge(
                        change_id,
                        change.thread_id,
                        change.requires_restart,
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

        // Check the hardened_branches DB row in case /harden ran but the
        // change row's `hardened` flag wasn't set yet. Marker existence (Fresh
        // or Stale) is enough — once CC has hardened the branch, follow-up
        // commits are presumed minor. The DB-backed marker survives worktree
        // removal during stale-session recovery, which the prior file marker
        // (keyed by worktree path) did not.
        let marker_hardened = is_harden_marker_present(
            &self.pool,
            &std::path::PathBuf::from(&change.repo_root),
            &change.branch_name,
        )
        .await;
        if marker_hardened && !change.hardened {
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
            if let Some(session) = CodingAgent::live_session_info(self.as_ref(), thread_id).await {
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

            // Reuse an existing CC worktree on this branch if one's still on
            // disk (the producing CC session has exited, but its worktree and
            // branch lock survive). Without this, `git worktree add` below
            // would fail with "branch already used by worktree" — which is
            // what broke nightly trigger applies (orchestrator POSTs apply
            // immediately after CC exits, before any cleanup pass).
            let wt_path = if let Some(existing) =
                find_worktree_for_branch(&repo_root, &change.branch_name).await
            {
                log!(
                    "[Changes] Reusing existing worktree {} for hardening of branch {}",
                    existing.display(),
                    change.branch_name
                );
                existing
            } else {
                let wt_path = worktrees_dir(self.workspace_path())
                    .join(format!("harden-{}", change_id.as_simple()));
                let wt_str = wt_path.to_str().unwrap().to_string();
                let _ = git_cmd(&["worktree", "remove", "--force", &wt_str], &repo_root).await;
                match worktree_add(&repo_root, &wt_path, &[&change.branch_name]).await {
                    Ok(o) if o.status.success() => {}
                    Ok(o) => {
                        let msg = format!(
                            "Failed to create worktree for hardening: {}",
                            String::from_utf8_lossy(&o.stderr).trim()
                        );
                        self.emit_apply_failed(thread_id, change_id, &msg, actor.clone())
                            .await;
                        return Err(msg.into());
                    }
                    Err(e) => {
                        let msg = format!("Failed to create worktree for hardening: {}", e);
                        self.emit_apply_failed(thread_id, change_id, &msg, actor.clone())
                            .await;
                        return Err(msg.into());
                    }
                }
                wt_path
            };

            log!(
                "[Changes] Not hardened — spawning hardening recovery for change {} (thread {})",
                change_id,
                thread_id
            );
            CodingAgent::spawn_hardening(
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
        let branch_exists = git_cmd(&["rev-parse", "--verify", &change.branch_name], &repo_root)
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
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

        // Auto-commit safe files (docs) if they're the only dirty files.
        // Scoped so the lock drops before any tier's CC subprocess await —
        // those merges happen in separate worktrees and would otherwise
        // block every data API write for minutes.
        {
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

        // Tier 1: If a live agent session exists for this thread, merge main into its worktree
        // instead of creating a temp worktree. The original agent has full context and
        // can resolve conflicts intelligently.
        if let Some(thread_id) = change.thread_id {
            if CodingAgent::is_running_for(self.as_ref(), thread_id).await {
                if let Some(session) =
                    CodingAgent::live_session_info(self.as_ref(), thread_id).await
                {
                    log!(
                        "[Changes] Live agent session found for thread {} — merging in-place",
                        thread_id
                    );

                    // Auto-commit any uncommitted work before merging
                    auto_commit_worktree(
                        &session.worktree_path,
                        "Claude Code changes (pre-merge auto-commit)",
                    )
                    .await;

                    match CodingAgent::merge_via_session(
                        self.as_ref(),
                        thread_id,
                        change_id,
                        &session.worktree_path,
                        &change.branch_name,
                        &repo_root,
                        &session,
                    )
                    .await
                    {
                        Ok((pre_sha, post_sha)) => {
                            let commits = self
                                .apply_now_success(
                                    thread_id,
                                    change_id,
                                    change.requires_restart,
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
                                change.requires_restart,
                                pre_sha,
                                post_sha,
                                &commits,
                                change.files.len(),
                            ));
                        }
                        Err(e) => {
                            // Agent stays alive for retry — keep the session as the conflict thread.
                            log!("[Changes] In-place merge failed for {}: {} — agent stays alive for retry", change_id, e);
                            self.emit_apply_failed(
                                thread_id,
                                change_id,
                                &e.to_string(),
                                actor.clone(),
                            )
                            .await;
                            return Ok(ApplyResult::conflict(
                                change_id,
                                thread_id,
                                change.files.len(),
                                format!("Merge failed: {} — session preserved, try again.", e),
                            ));
                        }
                    }
                }
            }
        }

        // Tier 2: dead session with worktree on disk — try ff, else resume CC for merge
        if let Some(thread_id) = change.thread_id {
            if let Some(wt_path) = find_worktree_for_branch(&repo_root, &change.branch_name).await {
                // Auto-commit any uncommitted CC work before merging
                auto_commit_worktree(&wt_path, "Claude Code changes (pre-merge auto-commit)").await;

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
                        // next user message resumes the same CC session.
                        // ChangeApplied below is the user-visible signal.
                        self.emit_change_applied(
                            thread_id,
                            change_id,
                            change.requires_restart,
                            files_have_client_update(&change.files),
                            commits.clone(),
                            change.thread_title.clone(),
                            actor.clone(),
                            Some(pre_sha.clone()),
                            Some(post_sha.clone()),
                        )
                        .await;
                        self.broadcast_changes_updated().await;
                        return Ok(ApplyResult::applied_with_merge(
                            change_id,
                            Some(thread_id),
                            change.requires_restart,
                            pre_sha,
                            post_sha,
                            &commits,
                            change.files.len(),
                        ));
                    }
                    Err(_) => {
                        // ff failed — CC needs to merge main into the branch
                        log!(
                            "[Changes] Fast path failed for {} — resuming CC for merge",
                            change.branch_name
                        );
                    }
                }

                // Look up resume token for potential resume
                let resume_token =
                    CodingAgent::lookup_session_id_for_resume(self.as_ref(), thread_id).await;

                match CodingAgent::run_merge_session_tier2(
                    self.as_ref(),
                    thread_id,
                    change_id,
                    &wt_path,
                    &change.branch_name,
                    &change.description,
                    resume_token,
                )
                .await
                {
                    Ok(_) => {
                        let merge_ok = git_cmd(
                            &["merge-base", "--is-ancestor", "main", &change.branch_name],
                            &repo_root,
                        )
                        .await
                        .map(|o| o.status.success())
                        .unwrap_or(false);

                        if merge_ok {
                            match catchup_and_ff_to_main(&repo_root, &wt_path, &change.branch_name)
                                .await
                            {
                                Ok((pre_sha, post_sha)) => {
                                    let commits =
                                        commits_in_range(&repo_root, &pre_sha, &post_sha).await;
                                    // Phase 6.2: preserve worktree + branch
                                    // (CC may continue in this thread after
                                    // resolving the merge); reset to main HEAD.
                                    if let Err(e) =
                                        reset_worktree_to_main_after_apply(&wt_path).await
                                    {
                                        self.emit_apply_failed(
                                            thread_id,
                                            change_id,
                                            &e.to_string(),
                                            actor.clone(),
                                        )
                                        .await;
                                        return Err(e);
                                    }
                                    self.emit_change_applied(
                                        thread_id,
                                        change_id,
                                        change.requires_restart,
                                        files_have_client_update(&change.files),
                                        commits.clone(),
                                        change.thread_title.clone(),
                                        actor.clone(),
                                        Some(pre_sha.clone()),
                                        Some(post_sha.clone()),
                                    )
                                    .await;
                                    self.broadcast_changes_updated().await;
                                    return Ok(ApplyResult::applied_with_merge(
                                        change_id,
                                        Some(thread_id),
                                        change.requires_restart,
                                        pre_sha,
                                        post_sha,
                                        &commits,
                                        change.files.len(),
                                    ));
                                }
                                Err(e) => {
                                    self.emit_apply_failed(
                                        thread_id,
                                        change_id,
                                        &format!("ff-merge failed after CC merge: {}", e),
                                        actor.clone(),
                                    )
                                    .await;
                                    return Ok(ApplyResult::conflict(
                                        change_id,
                                        thread_id,
                                        change.files.len(),
                                        format!("Merge failed: {}", e),
                                    ));
                                }
                            }
                        } else {
                            let msg = "CC session ended without completing the merge — try applying again.";
                            self.emit_apply_failed(thread_id, change_id, msg, actor.clone())
                                .await;
                            return Ok(ApplyResult::conflict(
                                change_id,
                                thread_id,
                                change.files.len(),
                                msg,
                            ));
                        }
                    }
                    Err(e) => {
                        log!(
                            "[Changes] CC merge failed for {}: {} — falling through to Tier 3",
                            change_id,
                            e
                        );
                        let _ = git_cmd(&["merge", "--abort"], &wt_path).await;
                    }
                }
            }
        }

        // Tier 3: No worktree — try ff directly, spawn CC in temp worktree if needed

        // Fast path: branch may already be a descendant of main
        {
            let _merge_guard = MERGE_MUTEX.lock().await;
            let main_sha = git_cmd(&["rev-parse", "main"], &repo_root)
                .await
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();
            let branch_sha = git_cmd(&["rev-parse", &change.branch_name], &repo_root)
                .await
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();

            if let Ok(shas) = ff_main_to(&repo_root, &branch_sha, &main_sha).await {
                log!(
                    "[Changes] Fast path succeeded for {} (Tier 3)",
                    change.branch_name
                );
                let commits = commits_in_range(&repo_root, &shas.0, &shas.1).await;
                // Phase 6.2: keep the branch alive — it's at the same SHA as
                // main now, and the thread may resume CC against it later. No
                // worktree exists for this branch in Tier 3, so there's
                // nothing to reset.
                push_main_in_background(&repo_root);
                self.emit_change_applied(
                    change.thread_id.unwrap_or(change_id),
                    change_id,
                    change.requires_restart,
                    files_have_client_update(&change.files),
                    commits.clone(),
                    change.thread_title.clone(),
                    actor.clone(),
                    Some(shas.0.clone()),
                    Some(shas.1.clone()),
                )
                .await;
                self.broadcast_changes_updated().await;
                return Ok(ApplyResult::applied_with_merge(
                    change_id,
                    change.thread_id,
                    change.requires_restart,
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
            let temp_wt_str = temp_wt.to_str().unwrap().to_string();
            let _ = git_cmd(&["worktree", "remove", "--force", &temp_wt_str], &repo_root).await;

            let add_ok = matches!(
                worktree_add(&repo_root, &temp_wt, &[&change.branch_name]).await,
                Ok(o) if o.status.success()
            );

            if add_ok {
                let result =
                    catchup_and_ff_to_main(&repo_root, &temp_wt, &change.branch_name).await;
                let _ = git_cmd(&["worktree", "remove", "--force", &temp_wt_str], &repo_root).await;

                if let Ok((pre_sha, post_sha)) = result {
                    // `catchup_and_ff_to_main` deletes change.branch_name on success.
                    // On failure, leave it intact so the CC slow path below can still merge from it.
                    log!(
                        "[Changes] Auto-merge path succeeded for {} (Tier 3)",
                        change.branch_name
                    );
                    let commits = commits_in_range(&repo_root, &pre_sha, &post_sha).await;
                    self.emit_change_applied(
                        change.thread_id.unwrap_or(change_id),
                        change_id,
                        change.requires_restart,
                        files_have_client_update(&change.files),
                        commits.clone(),
                        change.thread_title.clone(),
                        actor.clone(),
                        Some(pre_sha.clone()),
                        Some(post_sha.clone()),
                    )
                    .await;
                    self.broadcast_changes_updated().await;
                    return Ok(ApplyResult::applied_with_merge(
                        change_id,
                        change.thread_id,
                        change.requires_restart,
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
        let wt_path_str = wt_path.to_str().unwrap().to_string();

        let _ = git_cmd(&["worktree", "remove", "--force", &wt_path_str], &repo_root).await;
        let _ = git_cmd(&["branch", "-D", &temp_branch], &repo_root).await;

        match worktree_add(&repo_root, &wt_path, &["-b", &temp_branch]).await {
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
        CodingAgent::spawn_merge_session(self.as_ref(), thread_id, change_id, &change.description);
        Ok(ApplyResult::conflict(
            change_id,
            thread_id,
            change.files.len(),
            "Branch needs merging — agent is handling it.",
        ))
    }

    pub async fn is_external_repo_thread(&self, thread_id: Uuid) -> Result<bool, sqlx::Error> {
        sqlx::query_scalar::<_, bool>(
            "SELECT cc_is_external_repo FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(&self.pool)
        .await
        .map(|opt| opt.unwrap_or(false))
    }

    /// Discard all pending changes for a thread. `actor` flows into the
    /// resulting `ChangeDiscarded` events so the chip reads "You" rather than
    /// the engine fallback.
    pub async fn discard_pending_for_thread(
        &self,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) {
        let pending = self.changes().pending_for_thread(thread_id).await;
        for change in &pending {
            if let Err(e) = self.discard_change(change.id, actor.clone()).await {
                log!(
                    "[Changes] Failed to discard change {} for thread {}: {}",
                    change.id,
                    thread_id,
                    e
                );
            }
        }
    }

    /// Discard a single pending change.
    ///
    /// Phase 6.3 of the CC resume architecture: Discard preserves the thread's
    /// worktree directory and the branch ref so the thread stays alive and the
    /// next user message resumes the same CC session. The branch's commits
    /// (the ones the user is discarding) are wiped by resetting the worktree
    /// to main HEAD via `reset_worktree_to_main_after_discard`. The branch is
    /// NOT deleted — keeping it lets the same `cc_session_id` resume on the
    /// same branch ref instead of having to recreate everything.
    ///
    /// If the discarded change leaves OTHER pending changes referencing the
    /// same branch (multi-change-on-one-branch case), skip the worktree reset
    /// — the other changes' commits would be wiped along with this one. The
    /// branch and worktree stay as-is, preserving the still-pending work.
    pub async fn discard_change(
        &self,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let change = self
            .changes()
            .get_by_id(change_id)
            .await
            .ok_or("Change not found")?;
        if change.status == "discarded" {
            // Idempotent: already discarded, return success
            return Ok(());
        }
        if change.status != "pending" {
            return Err(format!("Change is already {}", change.status).into());
        }

        // Mark as discarded FIRST, before touching git — event is the source of truth
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id: change.thread_id.unwrap_or(change_id),
                    event: crate::engine::thread_events::ThreadEvent::ChangeDiscarded {
                        change_id: change_id.to_string(),
                        actor,
                        path: String::new(),
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                "[Changes] ChangeDiscarded",
            )
            .await;

        // Other pending changes on the same branch? If so, leave the branch
        // and worktree untouched — wiping the branch back to main would also
        // discard the still-pending work.
        let others = self
            .changes()
            .other_pending_for_branch(&change.branch_name, change_id)
            .await;
        if others {
            log!(
                "[Changes] Discarded change {} but kept branch {} and worktree — other pending changes reference it",
                change_id,
                change.branch_name
            );
            return Ok(());
        }

        // Phase 6.3: reset the worktree to main HEAD; preserve directory +
        // branch so the next user message resumes the same CC session.
        let repo_root = std::path::PathBuf::from(&change.repo_root);
        if let Some(wt_path) = find_worktree_for_branch(&repo_root, &change.branch_name).await {
            reset_worktree_to_main_after_discard(&wt_path).await?;
            log!(
                "[Changes] Discarded change {}: reset worktree {} on branch {} to main; branch preserved",
                change_id,
                wt_path.display(),
                change.branch_name
            );
        } else {
            // No worktree on disk for this branch (legacy threads, or already
            // cleaned up). Reset the branch ref to main directly so the
            // commits don't linger as a phantom pending state.
            let reset = git_cmd(
                &["branch", "-f", &change.branch_name, "main"],
                &repo_root,
            )
            .await
            .map_err(|e| {
                format!(
                    "git branch -f {} main failed in repo {}: {}",
                    change.branch_name,
                    repo_root.display(),
                    e
                )
            })?;
            if !reset.status.success() {
                return Err(format!(
                    "git branch -f {} main failed in repo {}: {}",
                    change.branch_name,
                    repo_root.display(),
                    String::from_utf8_lossy(&reset.stderr).trim()
                )
                .into());
            }
            log!(
                "[Changes] Discarded change {}: no worktree on branch {} — reset branch ref to main; branch preserved",
                change_id,
                change.branch_name
            );
        }
        Ok(())
    }

    /// Revert a previously applied change by reverting its commits.
    /// Uses stored pre/post merge SHAs when available, falls back to searching merge history.
    pub async fn revert_change(
        &self,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let change = self
            .changes()
            .get_by_id(change_id)
            .await
            .ok_or("Change not found")?;
        if change.status == "reverted" {
            return Ok("Change already reverted.".to_string());
        }
        if change.status != "applied" {
            return Err(format!(
                "Change is '{}', only applied changes can be reverted",
                change.status
            )
            .into());
        }

        let repo_root = std::path::PathBuf::from(&change.repo_root);

        // Auto-commit safe files (docs) if they're the only dirty files
        if auto_commit_safe_files_if_dirty(&repo_root).await {
            return Err("Cannot revert: the repository has uncommitted changes. Commit or stash them first.".into());
        }

        let result = if let (Some(ref pre_sha), Some(ref post_sha)) =
            (&change.pre_merge_sha, &change.post_merge_sha)
        {
            self.revert_with_shas(&repo_root, pre_sha, post_sha, &change.branch_name)
                .await
        } else {
            self.revert_legacy(&repo_root, &change.branch_name).await
        };

        match result {
            Ok(()) => {
                log!(
                    "[Changes] Reverted change {} (branch {})",
                    change_id,
                    change.branch_name
                );
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id: change.thread_id.unwrap_or(change_id),
                            event: crate::engine::thread_events::ThreadEvent::ChangeReverted {
                                change_id: change_id.to_string(),
                                actor,
                                path: String::new(),
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        },
                        "[Changes] ChangeReverted",
                    )
                    .await;
                Ok("Change reverted.".to_string())
            }
            Err(e) => Err(e),
        }
    }

    /// Revert using stored pre/post merge SHAs.
    async fn revert_with_shas(
        &self,
        repo_root: &Path,
        pre_sha: &str,
        post_sha: &str,
        branch_name: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        revert_with_shas(repo_root, pre_sha, post_sha, branch_name).await
    }

    /// Legacy revert: search for merge commit in recent git history,
    /// falling back to branch-ref based revert for fast-forwarded merges.
    async fn revert_legacy(
        &self,
        repo_root: &Path,
        branch_name: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Try 1: find a merge commit that merged this branch INTO main.
        // Must match "Merge branch 'feature'" or "Merge feature:" patterns,
        // NOT "Merge branch 'main' into feature" (which is the reverse direction).
        let log_output = git_cmd(&["log", "--merges", "--oneline", "-50"], repo_root)
            .await
            .map_err(|e| format!("Failed to read git log: {}", e))?;
        if log_output.status.success() {
            let log_text = String::from_utf8_lossy(&log_output.stdout);
            if let Some(merge_hash) = log_text
                .lines()
                .find(|line| is_merge_of_branch_into_main(line, branch_name))
                .and_then(|line| line.split_whitespace().next())
            {
                return match git_cmd(&["revert", merge_hash, "-m", "1", "--no-edit"], repo_root)
                    .await
                {
                    Ok(o) if o.status.success() => Ok(()),
                    Ok(o) => {
                        let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
                        let _ = git_cmd(&["revert", "--abort"], repo_root).await;
                        Err(format!("Revert failed (conflicts): {}", stderr).into())
                    }
                    Err(e) => Err(format!("Revert error: {}", e).into()),
                };
            }
        }

        // Try 2: branch ref still exists — find its commits via merge-base and revert the range
        if let Ok(ref_output) = git_cmd(&["rev-parse", "--verify", branch_name], repo_root).await {
            if ref_output.status.success() {
                let branch_sha = String::from_utf8_lossy(&ref_output.stdout)
                    .trim()
                    .to_string();
                let base_output = git_cmd(&["merge-base", "HEAD", branch_name], repo_root)
                    .await
                    .map_err(|e| format!("Failed to find merge-base: {}", e))?;
                if base_output.status.success() {
                    let base_sha = String::from_utf8_lossy(&base_output.stdout)
                        .trim()
                        .to_string();

                    // If the branch tip is an ancestor of HEAD, its commits were fast-forwarded into main
                    let is_ancestor = git_cmd(
                        &["merge-base", "--is-ancestor", &branch_sha, "HEAD"],
                        repo_root,
                    )
                    .await
                    .map(|o| o.status.success())
                    .unwrap_or(false);

                    if is_ancestor && base_sha != branch_sha {
                        return self
                            .revert_with_shas(repo_root, &base_sha, &branch_sha, branch_name)
                            .await;
                    }
                }
            }
        }

        Err(format!(
            "Could not find commits for branch '{}'. \
             The branch may have been deleted and this change was applied before revert tracking was added.",
            branch_name
        ).into())
    }
}

/// Revert commits identified by pre/post merge SHAs.
/// Handles both merge commits (using correct -m parent) and fast-forwarded ranges.
pub(crate) async fn revert_with_shas(
    repo_root: &Path,
    pre_sha: &str,
    post_sha: &str,
    branch_name: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let parents: Vec<String> = git_cmd(&["log", "--pretty=%P", "-1", post_sha], repo_root)
        .await
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default();

    if parents.len() > 1 {
        // Match pre_sha to determine which parent is old main — catchup
        // merges have main as parent 2, regular merges as parent 1.
        let parent_num = match parents.iter().position(|p| p == pre_sha) {
            Some(i) => i + 1, // git -m is 1-indexed
            None => {
                return Err(format!(
                    "pre_merge_sha {} does not match any parent of merge commit {} — \
                 cannot determine which side to revert. Parents: {:?}",
                    &pre_sha[..pre_sha.floor_char_boundary(8)],
                    &post_sha[..post_sha.floor_char_boundary(8)],
                    parents
                        .iter()
                        .map(|p| &p[..p.floor_char_boundary(8)])
                        .collect::<Vec<_>>(),
                )
                .into())
            }
        };
        match git_cmd(
            &[
                "revert",
                post_sha,
                "-m",
                &parent_num.to_string(),
                "--no-edit",
            ],
            repo_root,
        )
        .await
        {
            Ok(o) if o.status.success() => Ok(()),
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
                let _ = git_cmd(&["revert", "--abort"], repo_root).await;
                Err(format!("Revert failed (conflicts): {}", stderr).into())
            }
            Err(e) => Err(format!("Revert error: {}", e).into()),
        }
    } else {
        match git_cmd(
            &[
                "revert",
                "--no-commit",
                &format!("{}..{}", pre_sha, post_sha),
            ],
            repo_root,
        )
        .await
        {
            Ok(o) if o.status.success() => {
                let msg = format!("Revert changes from {}", branch_name);
                match git_cmd(&["commit", "-m", &msg], repo_root).await {
                    Ok(o) if o.status.success() => Ok(()),
                    _ => Err("Failed to commit revert".into()),
                }
            }
            Ok(o) => {
                let stderr = String::from_utf8_lossy(&o.stderr).trim().to_string();
                let _ = git_cmd(&["revert", "--abort"], repo_root).await;
                Err(format!("Revert failed (conflicts): {}", stderr).into())
            }
            Err(e) => Err(format!("Revert error: {}", e).into()),
        }
    }
}

#[cfg(test)]
#[path = "change_ops_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "change_ops_engine_origin_stamping_tests.rs"]
mod engine_origin_stamping;

#[path = "change_ops_emitters.rs"]
mod emitters;
