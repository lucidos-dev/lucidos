use super::git_ops::{
    auto_commit_safe_files_if_dirty, auto_commit_worktree, catchup_and_ff_to_main,
    commits_in_range, ff_main_to, files_have_client_update, find_branch_merge_in_main, git_cmd,
    harden_marker_state, has_branch_commits, is_harden_marker_present,
    is_merge_of_branch_into_main, parse_worktree_list, push_main_in_background,
    recover_no_commits_branch, worktrees_dir, HardenMarkerState, NoCommitsRecovery, MERGE_MUTEX,
};
use super::thread_events::{MessageOrigin, SessionEndReason};
use super::{ApplyResult, ApplyStatus, CognosEngine};
use crate::core::changes;
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
/// `request_id` is a chat-request UUID, not a persisted event id.
/// `hardened` is set atomically — callers declare hardening status up front.
/// `origin` flows onto the emitted `ChangeProposed` event so engine-internal
/// recovery paths (stale-session, orphan-recovery) can stamp themselves; live
/// agent callers pass `None` (the surrounding `MessageReceived` carries the
/// user/agent origin).
pub(crate) struct ProposeChangeInput<'a> {
    pub request_id: Uuid,
    pub thread_id: Uuid,
    pub branch_name: &'a str,
    pub repo_root: &'a str,
    pub description: &'a str,
    pub files: &'a [String],
    pub requires_restart: bool,
    pub channel: crate::engine::thread_events::EventChannel,
    pub hardened: bool,
    pub origin: Option<MessageOrigin>,
}

/// Trait for coding agents that can interact with change management.
pub(crate) trait CodingAgent: Send + Sync {
    async fn is_running_for(&self, thread_id: Uuid) -> bool;
    fn spawn_hardening(
        &self,
        thread_id: Uuid,
        worktree_path: PathBuf,
        branch_name: String,
        auto_apply_change_id: Option<Uuid>,
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
    repo_root: &Path,
    branch_name: &str,
) -> bool {
    match harden_marker_state(pool, repo_root, branch_name).await {
        HardenMarkerState::Fresh | HardenMarkerState::Stale => true,
        HardenMarkerState::Missing => crate::core::changes::get_pending_by_branch(pool, branch_name)
            .await
            .ok()
            .flatten()
            .map(|c| c.hardened)
            .unwrap_or(false),
    }
}

impl CognosEngine {
    /// Broadcast the current changes state (pending/applied/restart) to all SSE clients.
    pub(crate) async fn broadcast_changes_updated(&self) {
        let pending = crate::core::changes::list_pending(self.pool())
            .await
            .unwrap_or_default();
        let applied = crate::core::changes::list_recently_applied(self.pool(), 15, None)
            .await
            .unwrap_or_default();
        let restart = crate::core::changes::requires_restart_since(
            self.pool(),
            chrono::DateTime::<chrono::Utc>::MIN_UTC,
        )
        .await;
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::System(
                    crate::engine::event_bus::SystemEvent::ChangesUpdated {
                        total_pending: pending.len(),
                        pending,
                        applied,
                        restart_required: restart,
                    },
                ),
                "[Changes] ChangesUpdated",
            )
            .await;
    }

    /// Emit a ChangeApplied event and mark the change as applied in the database.
    /// `commits` and `thread_title` are surfaced in the restart-required toast,
    /// grouped by thread. `actor` identifies who initiated the apply — HTTP
    /// callers should pass `Some` (built via `api::actor::build_message_origin`);
    /// engine-internal applies pass `None`.
    pub(crate) async fn emit_change_applied(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        requires_restart: bool,
        client_update: bool,
        commits: Vec<String>,
        thread_title: Option<String>,
        actor: Option<MessageOrigin>,
    ) {
        if let Err(e) = changes::apply_change_applied(&self.pool, change_id, &commits).await {
            log!(
                "[Changes] Failed to mark change {} as applied: {}",
                change_id,
                e
            );
        }
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::ChangeApplied {
                        change_id: change_id.to_string(),
                        requires_restart,
                        client_update,
                        commits,
                        thread_title,
                        actor,
                        path: String::new(),
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                "[Changes] ChangeApplied",
            )
            .await;
    }

    /// If a pending change already exists for this branch, returns its ID instead of creating a duplicate.
    pub(crate) async fn propose_change(
        &self,
        input: ProposeChangeInput<'_>,
    ) -> Result<Uuid, Box<dyn std::error::Error + Send + Sync>> {
        let ProposeChangeInput {
            request_id,
            thread_id,
            branch_name,
            repo_root,
            description,
            files,
            requires_restart,
            channel,
            hardened,
            origin,
        } = input;

        // If a pending change already exists for this branch, update it with fresh metadata
        // instead of creating a duplicate. CC may continue working after the initial proposal,
        // changing the file list, description, requires_restart, and hardened status.
        if let Some(existing) = changes::get_pending_by_branch(&self.pool, branch_name).await? {
            if existing.description != description
                || existing.files != files
                || existing.requires_restart != requires_restart
                || existing.hardened != hardened
            {
                log!("[Changes] Updating pending change {} for branch {} (files: {} → {}, restart: {} → {}, hardened: {} → {})",
                    existing.id, branch_name, existing.file_count, files.len(),
                    existing.requires_restart, requires_restart,
                    existing.hardened, hardened);
                changes::update_pending(
                    &self.pool,
                    existing.id,
                    description,
                    files,
                    requires_restart,
                    hardened,
                )
                .await?;
                self.broadcast_changes_updated().await;
            }
            return Ok(existing.id);
        }

        let change_id = Uuid::new_v4();
        // Don't set request_event_id — the request_id is a chat request UUID,
        // not a persisted event ID. Setting it causes "orphaned event references"
        // warnings and breaks event grouping.
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
        // DB has a unique partial index on (branch_name) WHERE status='pending'.
        // If a concurrent insert wins the race, fall back to the existing record.
        match changes::apply_change_proposed(
            &self.pool,
            change_id,
            request_id,
            Some(thread_id),
            branch_name,
            repo_root,
            description,
            files,
            requires_restart,
            hardened,
        )
        .await
        {
            Ok(()) => Ok(change_id),
            Err(e)
                if e.as_database_error().is_some_and(|db| {
                    db.constraint() == Some("idx_changes_unique_pending_branch")
                }) =>
            {
                let existing = changes::get_pending_by_branch(&self.pool, branch_name)
                    .await?
                    .ok_or("Concurrent insert race — duplicate vanished")?;
                log!(
                    "Concurrent duplicate for branch {} resolved to existing change {}",
                    branch_name,
                    existing.id
                );
                Ok(existing.id)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Emit a ChangeApplyFailed event so the frontend knows the apply didn't succeed
    /// and can keep the thread in "waiting" state with the Apply/Discard panel visible.
    /// `actor` carries the same value passed to the originating apply call.
    pub(crate) async fn emit_apply_failed(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        error: &str,
        actor: Option<MessageOrigin>,
    ) {
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::ChangeApplyFailed {
                        change_id: change_id.to_string(),
                        error: error.to_string(),
                        actor,
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                "[Changes] ChangeApplyFailed",
            )
            .await;
    }

    /// Emit the boundary event that opens a fresh exchange panel for the
    /// hardening run (so its steps don't attach to the previous CC turn).
    pub(crate) async fn emit_missing_hardening_detected(
        &self,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) {
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::MissingHardeningDetected,
                    meta: crate::engine::thread_events::EventMeta {
                        channel: Some(crate::engine::thread_events::EventChannel::CodingAgent),
                        actor,
                        ..crate::engine::thread_events::EventMeta::NONE
                    },
                },
                "[Changes] MissingHardeningDetected",
            )
            .await;
    }

    /// Emit the boundary event that opens a fresh panel for a merge run.
    pub(crate) async fn emit_merge_conflict_detected(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
        files: Vec<String>,
    ) {
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::MergeConflictDetected {
                        change_id: change_id.to_string(),
                        files,
                    },
                    meta: crate::engine::thread_events::EventMeta {
                        channel: Some(crate::engine::thread_events::EventChannel::CodingAgent),
                        ..crate::engine::thread_events::EventMeta::NONE
                    },
                },
                "[Changes] MergeConflictDetected",
            )
            .await;
    }

    /// Emit the harden boundary event then queue AUTO_HARDEN_MESSAGE on a live
    /// agent session. Emit-before-send guarantees the panel sits above any
    /// CodingAgentTextStreamed events CC produces in response.
    pub(crate) async fn request_hardening_in_session(
        &self,
        thread_id: Uuid,
        msg_tx: &tokio::sync::mpsc::UnboundedSender<crate::engine::AgentUserInput>,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.emit_missing_hardening_detected(thread_id, actor).await;
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
        let change = changes::get_by_id(&self.pool, change_id)
            .await?
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

        // External repo fast path: no hardening, no merge. Just mark applied,
        // delete worktree, keep branch. The dev handles push/PR inside the session.
        if let Some(thread_id) = change.thread_id {
            let is_external = self.is_external_repo_thread(thread_id).await?;

            if is_external {
                let repo_root = std::path::PathBuf::from(&change.repo_root);

                // Delete worktree if it exists, keep the branch
                if let Ok(output) = git_cmd(&["worktree", "list", "--porcelain"], &repo_root).await
                {
                    let text = String::from_utf8_lossy(&output.stdout);
                    if let Some(wt_path) = parse_worktree_list(&text).get(&*change.branch_name) {
                        let wt_str = wt_path.to_str().unwrap_or_default();
                        let _ =
                            git_cmd(&["worktree", "remove", "--force", wt_str], &repo_root).await;
                    }
                }

                self.emit_change_applied(
                    thread_id,
                    change_id,
                    false,
                    false,
                    Vec::new(),
                    change.thread_title.clone(),
                    actor.clone(),
                )
                .await;
                self.broadcast_changes_updated().await;
                return Ok(ApplyResult {
                    status: ApplyStatus::Applied,
                    change_id,
                    thread_id: Some(thread_id),
                    message: format!("Done. Branch '{}' kept in repo.", change.branch_name),
                    files_changed: change.files.len(),
                    ..ApplyResult::default()
                });
            }
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
                    if let Err(e) =
                        changes::set_merge_shas(&self.pool, change_id, &pre_sha, &post_sha).await
                    {
                        log!(
                            "[Changes] Failed to store merge SHAs for {}: {}",
                            change_id,
                            e
                        );
                    }
                    // Worktree from the deleted branch may still be on disk
                    if let Ok(output) =
                        git_cmd(&["worktree", "list", "--porcelain"], &repo_root).await
                    {
                        let text = String::from_utf8_lossy(&output.stdout);
                        if let Some(wt) = parse_worktree_list(&text).get(&*change.branch_name) {
                            let _ = git_cmd(
                                &[
                                    "worktree",
                                    "remove",
                                    "--force",
                                    wt.to_str().unwrap_or_default(),
                                ],
                                &repo_root,
                            )
                            .await;
                        }
                    }
                    let thread_id = change.thread_id.unwrap_or(change_id);
                    self.emit_change_applied(
                        thread_id,
                        change_id,
                        change.requires_restart,
                        files_have_client_update(&change.files),
                        commits.clone(),
                        change.thread_title.clone(),
                        actor.clone(),
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
                let _ = changes::clear_merge_worktree(&self.pool, change_id).await;
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
            if let Err(e) = changes::set_hardened(&self.pool, change_id).await {
                log!(
                    "[Changes] Failed to set hardened flag for change {}: {}",
                    change_id,
                    e
                );
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
                    .request_hardening_in_session(thread_id, &session.msg_tx, actor.clone())
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
                self.emit_missing_hardening_detected(thread_id, actor.clone())
                    .await;
            }

            let repo_root = std::path::PathBuf::from(&change.repo_root);

            // Create a worktree from the branch so the hardening session can inspect it
            let wt_path = worktrees_dir(self.workspace_path())
                .join(format!("harden-{}", change_id.as_simple()));
            let wt_str = wt_path.to_str().unwrap().to_string();
            let _ = git_cmd(&["worktree", "remove", "--force", &wt_str], &repo_root).await;
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
                    &change.branch_name,
                ],
                &repo_root,
            )
            .await
            {
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
                    let _ = git_cmd(&["branch", "-D", &change.branch_name], &repo_root).await;
                    self.emit_change_applied(
                        change.thread_id.unwrap_or(change_id),
                        change_id,
                        false,
                        false,
                        Vec::new(),
                        change.thread_title.clone(),
                        actor.clone(),
                    )
                    .await;
                    self.broadcast_changes_updated().await;
                    return Ok(ApplyResult::noop(
                        change_id,
                        change.thread_id,
                        change.files.len(),
                        "Change applied (no commits to merge).",
                    ));
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

        // Auto-commit safe files (docs) if they're the only dirty files
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
            let wt_path = {
                if let Ok(output) = git_cmd(&["worktree", "list", "--porcelain"], &repo_root).await
                {
                    let text = String::from_utf8_lossy(&output.stdout);
                    parse_worktree_list(&text)
                        .get(&*change.branch_name)
                        .cloned()
                } else {
                    None
                }
            };

            if let Some(wt_path) = wt_path {
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
                        let wt_str = wt_path.to_str().unwrap();
                        let _ =
                            git_cmd(&["worktree", "remove", "--force", wt_str], &repo_root).await;
                        if let Err(e) =
                            changes::set_merge_shas(&self.pool, change_id, &pre_sha, &post_sha)
                                .await
                        {
                            log!(
                                "[Changes] Failed to store merge SHAs for {}: {}",
                                change_id,
                                e
                            );
                        }
                        self.event_bus
                            .emit_or_log(
                                crate::engine::event_bus::BusEvent::Thread {
                                    thread_id,
                                    event:
                                        crate::engine::thread_events::ThreadEvent::SessionEnded {
                                            reason: SessionEndReason::ChangesApplied,
                                        },
                                    meta: crate::engine::thread_events::EventMeta::NONE,
                                },
                                "[Changes] SessionEnded",
                            )
                            .await;
                        self.emit_change_applied(
                            thread_id,
                            change_id,
                            change.requires_restart,
                            files_have_client_update(&change.files),
                            commits.clone(),
                            change.thread_title.clone(),
                            actor.clone(),
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
                                    let wt_str = wt_path.to_str().unwrap();
                                    let _ = git_cmd(
                                        &["worktree", "remove", "--force", wt_str],
                                        &repo_root,
                                    )
                                    .await;
                                    if let Err(e) = changes::set_merge_shas(
                                        &self.pool, change_id, &pre_sha, &post_sha,
                                    )
                                    .await
                                    {
                                        log!(
                                            "[Changes] Failed to store merge SHAs for {}: {}",
                                            change_id,
                                            e
                                        );
                                    }
                                    self.emit_change_applied(
                                        thread_id,
                                        change_id,
                                        change.requires_restart,
                                        files_have_client_update(&change.files),
                                        commits.clone(),
                                        change.thread_title.clone(),
                                        actor.clone(),
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
                let _ = git_cmd(&["branch", "-D", &change.branch_name], &repo_root).await;
                push_main_in_background(&repo_root);
                if let Err(e) =
                    changes::set_merge_shas(&self.pool, change_id, &shas.0, &shas.1).await
                {
                    log!(
                        "[Changes] Failed to store merge SHAs for {}: {}",
                        change_id,
                        e
                    );
                }
                self.emit_change_applied(
                    change.thread_id.unwrap_or(change_id),
                    change_id,
                    change.requires_restart,
                    files_have_client_update(&change.files),
                    commits.clone(),
                    change.thread_title.clone(),
                    actor.clone(),
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
                git_cmd(&["worktree", "add", &temp_wt_str, &change.branch_name], &repo_root).await,
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
                    if let Err(e) =
                        changes::set_merge_shas(&self.pool, change_id, &pre_sha, &post_sha).await
                    {
                        log!(
                            "[Changes] Failed to store merge SHAs for {}: {}",
                            change_id,
                            e
                        );
                    }
                    self.emit_change_applied(
                        change.thread_id.unwrap_or(change_id),
                        change_id,
                        change.requires_restart,
                        files_have_client_update(&change.files),
                        commits.clone(),
                        change.thread_title.clone(),
                        actor.clone(),
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
                &wt_path_str,
                "-b",
                &temp_branch,
            ],
            &repo_root,
        )
        .await
        {
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

        if let Err(e) =
            changes::set_merge_worktree(&self.pool, change_id, &wt_path_str, &temp_branch).await
        {
            let _ = git_cmd(&["worktree", "remove", "--force", &wt_path_str], &repo_root).await;
            let _ = git_cmd(&["branch", "-D", &temp_branch], &repo_root).await;
            let msg = format!("Failed to store merge worktree info: {}", e);
            self.emit_apply_failed(thread_id, change_id, &msg, actor.clone())
                .await;
            return Err(msg.into());
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

    /// Discard all pending changes for a thread. Reuses `discard_change` for each,
    /// which handles event emission, DB update, and safe branch deletion.
    pub async fn discard_pending_for_thread(&self, thread_id: Uuid) {
        let pending = match changes::pending_for_thread(&self.pool, thread_id).await {
            Ok(p) => p,
            Err(e) => {
                log!(
                    "[Changes] Failed to list pending changes for thread {}: {}",
                    thread_id,
                    e
                );
                return;
            }
        };
        for change in &pending {
            if let Err(e) = self.discard_change(change.id, None).await {
                log!(
                    "[Changes] Failed to discard change {} for thread {}: {}",
                    change.id,
                    thread_id,
                    e
                );
            }
        }
    }

    /// Discard a single pending change: delete its branch (only if no other pending change uses it).
    pub async fn discard_change(
        &self,
        change_id: Uuid,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let change = changes::get_by_id(&self.pool, change_id)
            .await?
            .ok_or("Change not found")?;
        if change.status == "discarded" {
            // Idempotent: already discarded, return success
            return Ok(());
        }
        if change.status != "pending" {
            return Err(format!("Change is already {}", change.status).into());
        }

        // Mark as discarded FIRST, before touching git — DB state is the source of truth
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
        changes::apply_change_discarded(&self.pool, change_id).await?;

        // Only delete the branch if no OTHER pending change references it
        let others = match changes::other_pending_for_branch(
            &self.pool,
            &change.branch_name,
            change_id,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                log!("Failed to check for other pending changes on branch {} — keeping branch as precaution: {}", change.branch_name, e);
                true
            }
        };
        if !others {
            let repo_root = std::path::PathBuf::from(&change.repo_root);
            let _ = git_cmd(&["branch", "-D", &change.branch_name], &repo_root).await;
            log!("Discarded pending change branch {}", change.branch_name);
        } else {
            log!(
                "Discarded change {} but kept branch {} — other pending changes reference it",
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
        let change = changes::get_by_id(&self.pool, change_id)
            .await?
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
                    "Reverted change {} (branch {})",
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
                changes::apply_change_reverted(&self.pool, change_id).await?;
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
mod tests {
    use super::*;

    async fn make_test_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().to_path_buf();
        let _ = git_cmd(&["init"], &repo).await;
        let _ = git_cmd(&["checkout", "-b", "main"], &repo).await;
        tokio::fs::write(repo.join("init.txt"), "initial")
            .await
            .unwrap();
        let _ = git_cmd(&["add", "."], &repo).await;
        let _ = git_cmd(&["commit", "-m", "initial commit"], &repo).await;
        (tmp, repo)
    }

    async fn rev_parse(repo: &Path, refname: &str) -> String {
        String::from_utf8_lossy(&git_cmd(&["rev-parse", refname], repo).await.unwrap().stdout)
            .trim()
            .to_string()
    }

    /// Reverting a fast-forwarded branch undoes its commits.
    #[tokio::test]
    async fn revert_fast_forward_commits() {
        let (_tmp, repo) = make_test_repo().await;

        // Create feature branch with a commit
        let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
        tokio::fs::write(repo.join("feature.txt"), "feature work")
            .await
            .unwrap();
        let _ = git_cmd(&["add", "."], &repo).await;
        let _ = git_cmd(&["commit", "-m", "add feature"], &repo).await;

        let pre_sha = rev_parse(&repo, "main").await;
        let post_sha = rev_parse(&repo, "feature").await;

        // Fast-forward main to feature
        let _ = git_cmd(&["checkout", "main"], &repo).await;
        let _ = git_cmd(&["merge", "--ff-only", "feature"], &repo).await;

        // feature.txt should exist
        assert!(repo.join("feature.txt").exists());

        // Revert the fast-forward
        let result = revert_with_shas(&repo, &pre_sha, &post_sha, "feature").await;
        assert!(result.is_ok(), "revert should succeed: {:?}", result.err());

        // feature.txt should be gone
        assert!(
            !repo.join("feature.txt").exists(),
            "feature.txt should be removed after revert"
        );
    }

    /// Reverting a catchup merge (main merged INTO branch, then ff'd to main)
    /// correctly uses -m 2 to undo the branch changes, not main's changes.
    #[tokio::test]
    async fn revert_catchup_merge_uses_correct_parent() {
        let (_tmp, repo) = make_test_repo().await;

        // Create feature branch with a commit
        let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
        tokio::fs::write(repo.join("feature.txt"), "feature work")
            .await
            .unwrap();
        let _ = git_cmd(&["add", "."], &repo).await;
        let _ = git_cmd(&["commit", "-m", "add feature"], &repo).await;

        // Go back to main and make a commit (simulates other work landing on main)
        let _ = git_cmd(&["checkout", "main"], &repo).await;
        tokio::fs::write(repo.join("main-work.txt"), "main work")
            .await
            .unwrap();
        let _ = git_cmd(&["add", "."], &repo).await;
        let _ = git_cmd(&["commit", "-m", "main work"], &repo).await;

        let pre_sha = rev_parse(&repo, "main").await;

        // Catchup: merge main INTO feature (creates "Merge branch 'main' into feature")
        let _ = git_cmd(&["checkout", "feature"], &repo).await;
        let _ = git_cmd(&["merge", "main", "--no-edit"], &repo).await;
        let post_sha = rev_parse(&repo, "feature").await;

        // Fast-forward main to the merge commit
        let _ = git_cmd(&["checkout", "main"], &repo).await;
        let _ = git_cmd(&["merge", "--ff-only", "feature"], &repo).await;

        // Both files should exist
        assert!(repo.join("feature.txt").exists());
        assert!(repo.join("main-work.txt").exists());

        // Revert should undo feature changes but keep main's work
        let result = revert_with_shas(&repo, &pre_sha, &post_sha, "feature").await;
        assert!(result.is_ok(), "revert should succeed: {:?}", result.err());

        // feature.txt should be gone (branch changes reverted)
        assert!(
            !repo.join("feature.txt").exists(),
            "feature.txt should be removed — branch changes should be reverted"
        );
        // main-work.txt should still exist (main's changes preserved)
        assert!(
            repo.join("main-work.txt").exists(),
            "main-work.txt should remain — main's changes should be preserved"
        );
    }

    /// Reverting a regular merge ("Merge branch 'feature' into main")
    /// correctly uses -m 1 to undo the branch changes.
    #[tokio::test]
    async fn revert_regular_merge_uses_correct_parent() {
        let (_tmp, repo) = make_test_repo().await;

        // Create feature branch with a commit
        let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
        tokio::fs::write(repo.join("feature.txt"), "feature work")
            .await
            .unwrap();
        let _ = git_cmd(&["add", "."], &repo).await;
        let _ = git_cmd(&["commit", "-m", "add feature"], &repo).await;

        // Go back to main and make a commit so merge is non-ff
        let _ = git_cmd(&["checkout", "main"], &repo).await;
        tokio::fs::write(repo.join("main-work.txt"), "main work")
            .await
            .unwrap();
        let _ = git_cmd(&["add", "."], &repo).await;
        let _ = git_cmd(&["commit", "-m", "main work"], &repo).await;

        let pre_sha = rev_parse(&repo, "main").await;

        // Regular merge: merge feature INTO main
        let _ = git_cmd(&["merge", "feature", "--no-edit"], &repo).await;
        let post_sha = rev_parse(&repo, "main").await;

        // Both files should exist
        assert!(repo.join("feature.txt").exists());
        assert!(repo.join("main-work.txt").exists());

        // Revert should undo feature changes but keep main's work
        let result = revert_with_shas(&repo, &pre_sha, &post_sha, "feature").await;
        assert!(result.is_ok(), "revert should succeed: {:?}", result.err());

        assert!(
            !repo.join("feature.txt").exists(),
            "feature.txt should be removed — branch changes should be reverted"
        );
        assert!(
            repo.join("main-work.txt").exists(),
            "main-work.txt should remain — main's changes should be preserved"
        );
    }

    /// Multiple fast-forwarded commits are all reverted.
    #[tokio::test]
    async fn revert_multiple_fast_forward_commits() {
        let (_tmp, repo) = make_test_repo().await;

        let pre_sha = rev_parse(&repo, "main").await;

        let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
        tokio::fs::write(repo.join("a.txt"), "a").await.unwrap();
        let _ = git_cmd(&["add", "."], &repo).await;
        let _ = git_cmd(&["commit", "-m", "add a"], &repo).await;
        tokio::fs::write(repo.join("b.txt"), "b").await.unwrap();
        let _ = git_cmd(&["add", "."], &repo).await;
        let _ = git_cmd(&["commit", "-m", "add b"], &repo).await;

        let post_sha = rev_parse(&repo, "feature").await;

        let _ = git_cmd(&["checkout", "main"], &repo).await;
        let _ = git_cmd(&["merge", "--ff-only", "feature"], &repo).await;

        assert!(repo.join("a.txt").exists());
        assert!(repo.join("b.txt").exists());

        let result = revert_with_shas(&repo, &pre_sha, &post_sha, "feature").await;
        assert!(result.is_ok(), "revert should succeed: {:?}", result.err());

        assert!(
            !repo.join("a.txt").exists(),
            "a.txt should be removed after revert"
        );
        assert!(
            !repo.join("b.txt").exists(),
            "b.txt should be removed after revert"
        );
    }

    // ── Phase 0: Pre-refactor safety net ──

    #[test]
    fn apply_result_applied_without_restart() {
        let cid = Uuid::new_v4();
        let tid = Uuid::new_v4();
        let result = ApplyResult::applied(cid, Some(tid), false);
        assert_eq!(result.status, ApplyStatus::Applied);
        assert_eq!(result.change_id, cid);
        assert_eq!(result.thread_id, Some(tid));
        assert!(!result.restart_required);
        assert!(result.conflict_thread_id.is_none());
        assert!(result.review_thread_id.is_none());
        assert!(!result.message.contains("restart"));
    }

    #[test]
    fn apply_result_applied_with_restart() {
        let cid = Uuid::new_v4();
        let result = ApplyResult::applied(cid, None, true);
        assert_eq!(result.status, ApplyStatus::Applied);
        assert!(result.restart_required);
        assert!(
            result.message.contains("restart"),
            "message should mention restart: {}",
            result.message
        );
        assert!(result.conflict_thread_id.is_none());
        assert!(result.review_thread_id.is_none());
    }

    #[test]
    fn apply_result_applied_with_merge_carries_shas_and_counts() {
        let cid = Uuid::new_v4();
        let tid = Uuid::new_v4();
        let pre = "0".repeat(40);
        let post = "1".repeat(40);
        let commits = vec!["fix: a".to_string(), "fix: b".to_string()];
        let result = ApplyResult::applied_with_merge(
            cid,
            Some(tid),
            false,
            pre.clone(),
            post.clone(),
            &commits,
            5,
        );
        assert_eq!(result.status, ApplyStatus::Applied);
        assert_eq!(result.change_id, cid);
        assert_eq!(result.thread_id, Some(tid));
        assert_eq!(result.previous_commit.as_deref(), Some(pre.as_str()));
        assert_eq!(result.applied_commit.as_deref(), Some(post.as_str()));
        assert_eq!(result.commits_applied, 2);
        assert_eq!(result.files_changed, 5);
    }

    #[test]
    fn now_epoch_millis_is_reasonable() {
        let ms = now_epoch_millis();
        // Should be after 2026-01-01 and before 2100-01-01
        assert!(
            ms > 1_767_225_600_000,
            "epoch millis {} is too small (before 2026)",
            ms
        );
        assert!(
            ms < 4_102_444_800_000,
            "epoch millis {} is too large (after 2100)",
            ms
        );
    }

    /// Reverting a merge with a mismatched pre_sha fails explicitly
    /// instead of silently reverting the wrong side.
    #[tokio::test]
    async fn revert_merge_with_wrong_pre_sha_fails() {
        let (_tmp, repo) = make_test_repo().await;

        let _ = git_cmd(&["checkout", "-b", "feature"], &repo).await;
        tokio::fs::write(repo.join("feature.txt"), "feature")
            .await
            .unwrap();
        let _ = git_cmd(&["add", "."], &repo).await;
        let _ = git_cmd(&["commit", "-m", "add feature"], &repo).await;

        let _ = git_cmd(&["checkout", "main"], &repo).await;
        tokio::fs::write(repo.join("main-work.txt"), "main")
            .await
            .unwrap();
        let _ = git_cmd(&["add", "."], &repo).await;
        let _ = git_cmd(&["commit", "-m", "main work"], &repo).await;

        // Merge feature into main
        let _ = git_cmd(&["merge", "feature", "--no-edit"], &repo).await;
        let post_sha = rev_parse(&repo, "main").await;

        // Pass a bogus pre_sha that doesn't match either parent
        let result = revert_with_shas(
            &repo,
            "0000000000000000000000000000000000000000",
            &post_sha,
            "feature",
        )
        .await;
        assert!(
            result.is_err(),
            "should fail when pre_sha doesn't match any parent"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("does not match any parent"),
            "error should explain the mismatch: {}",
            err
        );
    }
}
