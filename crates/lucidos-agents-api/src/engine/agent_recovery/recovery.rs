//! The recovery `impl LucidosEngine` block: stale-waiting-session
//! settlement and orphaned-worktree recovery.

use super::super::change_ops::{branch_is_hardened, ProposeOutcome};
use super::super::git_ops::{
    add_paths_to_worktree_exclude, commit_worktree_or_err, default_local_branch,
    describe_branch_changes, files_require_restart, find_worktree_for_branch, git_cmd, git_ran_ok,
    main_worktree, proposal_files_for_branch, worktrees_dir, WorktreeLookup,
};
use super::super::thread_events::{EngineReason, EventChannel, MessageOrigin};
use super::super::thread_lifecycle::ThreadStatus;
use super::super::LucidosEngine;
use super::*;
use crate::engine::branch_recovery_queries::{
    restored_recovery_branches, BRANCH_CLASSIFICATION_SQL,
};
use crate::engine::question_park_sql::thread_has_unanswered_question;
use crate::engine::session_queries::change_description_fallback;
use crate::engine::switch_recovery_sql::after_latest_thread_start_sql;
use crate::engine::switch_recovery_sql::switch_abort_unsuperseded_sql;
use crate::engine::switch_recovery_sql::SWITCH_TEARDOWN_ABORT_SQL;
use crate::engine::undelivered_input::agent_never_engaged_sql;
use crate::engine::worktree_artifacts::{WORKTREE_EXCLUDE_PATHS, WORKTREE_WORKSPACE_MARKER};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

/// What a proposal says about a branch's work beyond its files.
pub(super) struct BranchProposalDetails {
    pub description: String,
    pub hardened: bool,
    pub requires_restart: bool,
}

/// What a proposal says about a branch's work beyond its files.
pub(super) async fn branch_proposal_details(
    pool: &sqlx::PgPool,
    changes: &crate::core::changes_projection::ChangesProjection,
    thread_id: Uuid,
    branch_name: &str,
    repo_root: &Path,
    changed_files: &[String],
) -> BranchProposalDetails {
    let fallback = change_description_fallback(pool, thread_id, branch_name).await;
    let base = default_local_branch(repo_root).await;
    let log_range = format!("{}..{}", base, branch_name);
    let description = describe_branch_changes(repo_root, &log_range, &fallback, None).await;
    // The marker is keyed by repo root plus branch name, so it survives the
    // cleanup worker removing the worktree. Without this lookup,
    // `propose_change` downgrades hardened to false and Apply re-runs
    // `/harden` on already-hardened work.
    let hardened = branch_is_hardened(pool, changes, repo_root, branch_name).await;
    BranchProposalDetails {
        description,
        hardened,
        requires_restart: files_require_restart(changed_files),
    }
}

pub async fn recover_orphaned_worktrees(engine: &std::sync::Arc<LucidosEngine>) -> Vec<uuid::Uuid> {
    engine.recover_orphaned_worktrees().await
}

pub async fn settle_agent_switch_threads(
    engine: &LucidosEngine,
    resumed: &std::collections::HashSet<uuid::Uuid>,
) {
    engine.settle_unresumed_switch_threads(resumed).await;
}

#[easy_ext::ext(AgentRecoveryExt)]
pub(crate) impl LucidosEngine {
    /// Gather branch metadata and propose a Change record. Callers must pass a
    /// non-empty `changed_files` list, so this never creates a phantom `changes`
    /// row with `file_count=0`.
    ///
    /// `origin` reaches the emitted `ChangeProposed`, so the route popover can
    /// name which engine path proposed the change.
    async fn propose_branch_changes(
        &self,
        thread_id: Uuid,
        branch_name: &str,
        repo_root: &Path,
        changed_files: &[String],
        origin: Option<MessageOrigin>,
    ) -> Result<ProposeOutcome, Box<dyn std::error::Error + Send + Sync>> {
        let finished = last_turn_ended_cleanly(self.pool(), thread_id).await;
        self.propose_branch_work(
            thread_id,
            branch_name,
            repo_root,
            changed_files,
            origin,
            finished,
        )
        .await
    }

    /// Settle a branch's committed work as its turn ended: propose it if the
    /// turn finished, withhold it if not (ADR 0400).
    async fn propose_branch_work(
        &self,
        thread_id: Uuid,
        branch_name: &str,
        repo_root: &Path,
        changed_files: &[String],
        origin: Option<MessageOrigin>,
        finished: bool,
    ) -> Result<ProposeOutcome, Box<dyn std::error::Error + Send + Sync>> {
        debug_assert!(
            !changed_files.is_empty(),
            "propose_branch_work called with empty file list; caller must filter"
        );
        let details = branch_proposal_details(
            self.pool(),
            self.changes(),
            thread_id,
            branch_name,
            repo_root,
            changed_files,
        )
        .await;
        self.propose_turn_work(
            crate::engine::change_ops::ProposeChangeInput {
                thread_id,
                branch_name,
                repo_root: &repo_root.to_string_lossy(),
                description: &details.description,
                files: changed_files,
                requires_restart: details.requires_restart,
                channel: EventChannel::ClaudeCode,
                hardened: details.hardened,
                origin,
            },
            finished,
            crate::engine::change_ops::HoldReentry::default(),
        )
        .await
    }

    /// Decide again what a branch's work is: re-read its files, then propose
    /// or withhold it as `finished` says. `Ok(None)` when the branch holds no
    /// proposable work.
    async fn redecide_branch_work(
        &self,
        thread_id: Uuid,
        branch_name: &str,
        repo_root: &Path,
        origin: Option<MessageOrigin>,
        finished: bool,
    ) -> Result<Option<ProposeOutcome>, Box<dyn std::error::Error + Send + Sync>> {
        let Some(files) = proposal_files_for_branch(repo_root, branch_name).await else {
            return Ok(None);
        };
        self.propose_branch_work(thread_id, branch_name, repo_root, &files, origin, finished)
            .await
            .map(Some)
    }

    /// `thread_summaries.coding_agent_is_external_repo`, failing open to
    /// `false` on a read error, as this read always has.
    ///
    /// Fail-open only mis-routes a legacy external thread whose event carries
    /// no kind. That lands on the Lucidos repo: today's behavior, where the
    /// branch is absent and every command fails harmlessly.
    async fn projection_says_external_repo(&self, thread_id: Uuid) -> bool {
        match self.is_external_repo_thread(thread_id).await {
            Ok(v) => v,
            Err(e) => {
                log_legacy!(
                    "[Recovery] Failed to check external repo status for thread {}: {}",
                    thread_id,
                    e
                );
                false
            }
        }
    }

    /// Settle the stale branch's committed work. Thin wrapper so the settle's
    /// own body stays one decision per line; `propose_branch_changes` above
    /// does the work.
    async fn propose_stale_branch(
        &self,
        thread_id: Uuid,
        branch_name: &str,
        repo_root: &Path,
        changed_files: &[String],
    ) {
        match self
            .propose_branch_changes(
                thread_id,
                branch_name,
                repo_root,
                changed_files,
                Some(MessageOrigin::engine(EngineReason::StaleSession)),
            )
            .await
        {
            Ok(ProposeOutcome::Proposed(_)) => {
                log_legacy!(
                    "[Recovery] Proposed change from stale session (branch {})",
                    branch_name
                );
            }
            Ok(
                ProposeOutcome::Held(_)
                | ProposeOutcome::Unfinished
                | ProposeOutcome::AwaitingLanding(_),
            ) => {}
            Err(e) => {
                log_legacy!(
                    "[Recovery] Failed to propose change from stale session: {}",
                    e
                );
            }
        }
    }

    /// End a stale waiting Claude Code session (no live process) after an engine
    /// restart.
    ///
    /// `actor` is who initiated it. HTTP entry points plumb the user's device
    /// through, so any resulting change event stamps the real actor rather than
    /// the "Lucidos Engine" chip. Engine-internal recovery callers pass `None`.
    async fn end_stale_waiting_session(
        self: &Arc<Self>,
        thread_id: Uuid,
        discard: bool,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Branch and kind come from ONE `SessionStarted` row, so the branch and
        // the repo that owns it can never name different sessions.
        let session: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT payload->>'branch', payload->>'coding_agent_kind' FROM events \
             WHERE event_type = 'SessionStarted' AND thread_id = $1 \
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(thread_id)
        .fetch_optional(self.pool())
        .await?;
        let session_branch = session.as_ref().and_then(|(branch, _)| branch.as_deref());
        if let Some(batch_id) = crate::engine::landing_lane::lane_batch_of_stale_session(
            self.pool(),
            self.workspace_path(),
            thread_id,
            session_branch,
            discard,
        )
        .await?
        {
            self.end_stale_lane_thread(thread_id, batch_id, discard, session_branch)
                .await;
            return Ok(());
        }

        let (branch_name, session_kind) = match session {
            Some((Some(b), kind)) if !b.is_empty() => (b, kind),
            None => {
                // This thread never had a Claude Code session. Erroring beats
                // emitting `SessionEnded`, which would pollute regular chat
                // threads with coding-agent events.
                return Err("No Claude Code session found for this thread".into());
            }
            Some(_) => {
                // A session with no branch. Idle it at the turn boundary rather
                // than terminating it: the thread stays alive, so a follow-up
                // can re-spawn the agent through `--resume`.
                let coding_agent = self.thread_coding_agent(thread_id).await;
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
                                has_changes: false,
                                // Read back rather than hardcoded: this field
                                // is authoritative for the projection column,
                                // so a literal `false` would clear an external
                                // thread's flag instead of preserving it.
                                is_external_repo: self
                                    .projection_says_external_repo(thread_id)
                                    .await,
                                requires_restart: false,
                                cc_session_id: None,
                                coding_agent,
                                reason: None,
                                worktree_path: None,
                                worktree_head_sha: None,
                                bg_bash_pending: false,
                                turn_end: None,
                            },
                            meta: crate::engine::thread_events::EventMeta::NONE,
                        },
                        "[Recovery] CodingAgentIdled (no branch)",
                    )
                    .await;
                return Ok(());
            }
        };

        log_legacy!(
            "[Recovery] Ending stale waiting session for thread {} (branch {})",
            thread_id,
            branch_name
        );

        // Route by the kind this thread's own session recorded. An app thread's
        // worktree and branch live in the WORKSPACE git. Asking the Lucidos repo
        // about them left every app thread settling with no Apply card, for work
        // already committed on its branch.
        let repo = stale_session_repo(
            session_kind.as_deref(),
            self.projection_says_external_repo(thread_id).await,
            &main_worktree().await,
            self.workspace_path(),
        );
        let is_external = matches!(repo, StaleSessionRepo::External);

        // The idle below reports the branch's work whether a change carries
        // it or a hold withheld it, so withheld work stays visible.
        let mut branch_has_work = false;
        match &repo {
            StaleSessionRepo::External => {
                log_legacy!(
                    "[Recovery] External repo branch {}: keeping branch, no change proposed",
                    branch_name
                );
            }
            StaleSessionRepo::Owned(repo_root) => {
                let lookup = find_worktree_for_branch(repo_root, &branch_name).await;
                settle_stale_worktree(repo_root, &branch_name, thread_id, discard, lookup).await;

                if !discard {
                    // `Some(files)` only when the branch has commits AND a
                    // non-empty net diff, so commits that cancel out read as a
                    // no-op branch.
                    match proposal_files_for_branch(repo_root, &branch_name).await {
                        // The successor session carries the work and decides it.
                        Some(_) if crate::engine::pending_handoff::handoff_pending(self.pool(), thread_id).await => {
                            branch_has_work = true;
                            match crate::engine::change_ops::withhold_handed_off_work(
                                self.pool(),
                                &self.event_bus,
                                thread_id,
                                repo_root,
                                &branch_name,
                            )
                            .await
                            {
                                Ok(true) => self.broadcast_changes_updated().await,
                                Ok(false) => {}
                                Err(e) => log_legacy!(
                                    "[Recovery] Could not withdraw the handed-off change on {}: {}",
                                    branch_name,
                                    e
                                ),
                            }
                        }
                        Some(changed_files) => {
                            branch_has_work = true;
                            self.propose_stale_branch(
                                thread_id,
                                &branch_name,
                                repo_root,
                                &changed_files,
                            )
                            .await;
                        }
                        // Keep the branch. An unexplained `None` (a transient
                        // git failure, a projection gap) would otherwise strand
                        // work the user still wants. An orphaned empty branch is
                        // just a ref, and the cleanup sweep collects it once it
                        // is fully merged.
                        None => log_legacy!(
                            "[Recovery] Branch {} has no proposable diff, keeping branch (discard=false)",
                            branch_name
                        ),
                    }
                }
            }
        }

        // AFTER the worktree removal above, not before. `discard_change` resets
        // and cleans the tree it finds, and skips that when the tree is already
        // gone. Pending rows are DB state, so they clear whichever repo owns the
        // branch, an external one included.
        if discard {
            log_legacy!(
                "[Recovery] Discarding stale session changes (branch {})",
                branch_name
            );
            self.discard_open_changes_for_thread(thread_id, actor.clone())
                .await;
        }

        if let Some(delete_root) = stale_discard_branch_delete_root(discard, &repo, &branch_name) {
            // `git_ran_ok`, not `git_cmd`: a non-zero exit is an `Ok` there, so
            // a refused delete logged nothing. Git refuses while a worktree
            // still holds the branch, which is what an Unknown lookup leaves
            // behind: the removal above is skipped, and the branch survives the
            // Discard with its commits.
            if let Err(e) = git_ran_ok(&["branch", "-D", &branch_name], delete_root).await {
                log_legacy!("[Recovery] Failed to delete branch {}: {}", branch_name, e);
            }
        }

        // `SessionEnded` is terminal-only, so mark the orphaned turn as ended
        // with `CodingAgentIdled` instead. The change events emitted above
        // already drive the panel state.
        let coding_agent = self.thread_coding_agent(thread_id).await;
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
                        has_changes: branch_has_work,
                        // Authoritative for
                        // `thread_summaries.coding_agent_is_external_repo`, so a
                        // hardcoded `false` here cleared the flag on an external
                        // thread's Stop. Derived from the resolved repo instead.
                        is_external_repo: is_external,
                        requires_restart: false,
                        cc_session_id: None,
                        coding_agent,
                        reason: None,
                        // Discard removes the tree above, so this path may or
                        // may not still exist. Recording it either way misleads
                        // the resolver, so leave it None and let the next spawn
                        // look the worktree up itself.
                        worktree_path: None,
                        // No worktree, so no SHA. External-edit detection stays
                        // off until a real turn populates the field.
                        worktree_head_sha: None,
                        bg_bash_pending: false,
                        turn_end: None,
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                "[Recovery] CodingAgentIdled",
            )
            .await;

        // No auto-apply tail: a pending change is the user's to resolve from
        // Review. Apply Now chains its own apply call once the proposal lands
        // over SSE.

        self.broadcast_changes_updated().await;

        Ok(())
    }

    /// End lane duty without proposing work. Preserve historical ordinary
    /// sessions separately from the integration tree the lane cleans up.
    async fn end_stale_lane_thread(
        &self,
        thread_id: Uuid,
        batch_id: Uuid,
        discard: bool,
        session_branch: Option<&str>,
    ) {
        let historical_branch = preserve_historical_lane_session(
            &self.event_bus,
            &main_worktree().await,
            thread_id,
            batch_id,
            discard,
            session_branch,
        )
        .await;
        use crate::engine::landing_lane::{LaneThreadEnd, LANE_THREAD_CLOSED};
        let end = if discard {
            LaneThreadEnd::Failed(LANE_THREAD_CLOSED.to_string())
        } else {
            LaneThreadEnd::Stopped
        };
        log_legacy!(
            "[Recovery] Ending stale lane thread {} of batch {}: {:?}",
            thread_id,
            batch_id,
            end
        );
        self.report_lane_thread_end(batch_id, thread_id, end);
        let coding_agent = self.thread_coding_agent(thread_id).await;
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
                        has_changes: historical_branch.is_some(),
                        is_external_repo: false,
                        requires_restart: false,
                        cc_session_id: None,
                        coding_agent,
                        reason: historical_branch.map(|branch| format!(
                            "This landing batch has ended. Historical work is preserved on branch {branch}. Start a new coding-agent thread to recover it; this closed lane cannot run again."
                        )),
                        worktree_path: None,
                        worktree_head_sha: None,
                        bg_bash_pending: false,
                        turn_end: None,
                    },
                    meta: crate::engine::thread_events::EventMeta::NONE,
                },
                "[Recovery] CodingAgentIdled (lane thread)",
            )
            .await;
    }

    /// Boot floor: withdraw every *Switch to new version* resume promise this
    /// boot did not keep. Thin wrapper over
    /// [`settle_unresumed_switch_threads`], which documents the whole contract;
    /// `main.rs` calls this after both resume drains with the union of the ids
    /// they actuated.
    async fn settle_unresumed_switch_threads(&self, resumed: &std::collections::HashSet<Uuid>) {
        settle_unresumed_switch_threads(self.pool(), &self.event_bus, resumed).await;
    }

    /// Detect orphaned coding-agent worktrees from a previous engine run and
    /// start new coding-agent sessions on them instead of proposing pending changes.
    async fn recover_orphaned_worktrees(self: &Arc<Self>) -> Vec<uuid::Uuid> {
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
        let restored_branches = match restored_recovery_branches(self.pool()).await {
            Ok(branches) => branches,
            Err(e) => {
                log_legacy!(
                    "[Recovery] Restore boundary unavailable, refusing worktree recovery: {}",
                    e
                );
                return Vec::new();
            }
        };
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

        // A registry read that failed degrades to "no external repos". The two
        // engine-owned roots are still scanned, so a Lucidos or app thread
        // recovers even when the registry is unreadable.
        let external_repos = crate::core::repositories::RepositoryStore::list(self.pool())
            .await
            .unwrap_or_else(|e| {
                log_legacy!("[Recovery] Failed to list external repos: {}", e);
                Vec::new()
            });
        let repos_to_scan =
            recovery_repo_roots(&lucidos_repo_root, self.workspace_path(), &external_repos);

        // (worktree_path, branch_name, repo_id, repo_root)
        let mut to_recover: Vec<(PathBuf, String, Option<String>, PathBuf)> = Vec::new();

        // Listed concurrently, parsed in root order, so `to_recover` keeps the
        // order a serial scan produced.
        let worktree_lists: Vec<_> = {
            use futures::StreamExt;
            futures::stream::iter(repos_to_scan.iter().map(|(repo_root, _)| async move {
                (
                    repo_root,
                    git_cmd(&["worktree", "list", "--porcelain"], repo_root).await,
                )
            }))
            .buffered(RECOVERY_GIT_CONCURRENCY)
            .collect()
            .await
        };

        for (repo_root, listed) in worktree_lists {
            let wt_output = match listed {
                Ok(o) => o,
                Err(e) => {
                    log_legacy!(
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
                        // A lane thread works on its batch's integration
                        // branch, and recovers like any coding-agent thread.
                        if crate::engine::git_ops::is_coding_agent_branch(br)
                            || crate::engine::git_ops::is_lane_branch(br)
                        {
                            // A worktree outside this workspace's worktrees dir
                            // cannot be ours, so skip the marker read. That read
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
                                            log_legacy!("[Recovery] Skipping worktree {} — owned by workspace {}", wt, owner);
                                        }
                                    }
                                    Err(_) => {
                                        log_legacy!(
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
        // Best-effort: a DB error degrades to "nothing pending" rather than
        // blocking boot, and is logged so the partial sweep is visible.
        let pending_changes_list = proj.list_pending().await.unwrap_or_else(|e| {
            log_legacy!(
                "[Recovery] list_pending: {} — recovery proceeds without pending change context",
                e
            );
            Vec::new()
        });
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
            sqlx::query_as::<_, BranchStatus>(&BRANCH_CLASSIFICATION_SQL).fetch_all(pool),
            sqlx::query_as::<_, BranchThread>(
                "SELECT DISTINCT ON (payload->>'branch') thread_id, payload->>'branch' AS branch FROM events \
                 WHERE event_type = 'SessionStarted' AND payload->>'branch' IS NOT NULL \
                   AND payload->>'branch' != '' AND thread_id IS NOT NULL \
                 ORDER BY payload->>'branch', sequence DESC"
            ).fetch_all(pool),
        );

        // An empty set from a failed query silently misclassifies every branch,
        // so log the error even though recovery proceeds with degraded data.
        fn unwrap_logged<T: Default, E: std::fmt::Display>(label: &str, r: Result<T, E>) -> T {
            r.unwrap_or_else(|e| {
                log_legacy!("[Recovery] {} query failed: {}", label, e);
                T::default()
            })
        }

        let pending_by_branch: std::collections::HashMap<String, crate::core::changes::Change> =
            pending_changes_list
                .into_iter()
                .map(|c| (c.branch_name.clone(), c))
                .collect();

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

        // Before any missing-worktree recreation, pending-change withdrawal or
        // synthetic recovery event. Keep every archived worktree on disk.
        if let Some(eligible) = &restored_branches {
            actively_running_branches.retain(|branch| eligible.contains_key(branch));
            to_recover.retain(|(_, branch, _, _)| eligible.contains_key(branch));
            branch_to_thread = eligible.clone();
        }

        let t_classified = t0.elapsed();
        log_legacy!("[Recovery] Worktree scan: {}ms, DB classification: {}ms (worktrees={}, idle={}, running={})",
            t_worktree_scan.as_millis(),
            (t_classified - t_worktree_scan).as_millis(),
            to_recover.len(),
            idle_branches.len(),
            actively_running_branches.len());

        // DB-based discovery for lost worktrees. The scan above sees only
        // branches whose worktree directory still exists, so a running session
        // whose directory was cleaned up is invisible to it. Find those in the
        // DB and create fresh worktrees, so they enter the normal pipeline.
        // Owned set, because the loop below pushes to `to_recover`.
        let discovered_branches: std::collections::HashSet<String> =
            to_recover.iter().map(|(_, br, _, _)| br.clone()).collect();

        // Unstick a thread whose session cannot be recovered (worktree gone,
        // branch missing, git error). The thread stays alive, and the
        // `engine_restart_interrupt` reason tells the UI to offer Continue
        // rather than treat the idle as natural.
        let end_stuck_session = |engine: &Arc<Self>, thread_id: Uuid| {
            let bus = engine.event_bus.clone();
            let engine = engine.clone();
            async move {
                // Same preserve rule as the loop below. The `CodingAgentIdled`
                // this closure emits is park-ending, so idling here would expire
                // a live question card. Answering still resumes, even with the
                // worktree unrecoverable.
                if thread_has_unanswered_question(engine.pool(), thread_id).await {
                    log_legacy!(
                        "[Recovery] Preserving stuck thread {} — parked on an unanswered question (no idle emitted)",
                        thread_id
                    );
                    return;
                }
                let coding_agent = engine.thread_coding_agent(thread_id).await;
                // Read back, as the no-branch idle above does: this field
                // overwrites the projection column.
                let is_external_repo = engine.projection_says_external_repo(thread_id).await;
                bus.emit_or_log(
                    crate::engine::event_bus::BusEvent::Thread {
                        thread_id,
                        event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
                            has_changes: false,
                            is_external_repo,
                            requires_restart: false,
                            cc_session_id: None,
                            coding_agent,
                            reason: Some(ENGINE_RESTART_INTERRUPT_REASON.to_string()),
                            // No worktree to record: this path fires when
                            // recovery cannot locate one at all. The next spawn
                            // resolves a path itself.
                            worktree_path: None,
                            // No worktree, so no SHA to snapshot.
                            worktree_head_sha: None,
                            bg_bash_pending: false,
                            turn_end: None,
                        },
                        meta: crate::engine::thread_events::EventMeta::NONE,
                    },
                    &format!("[Recovery] CodingAgentIdled for stuck thread {}", thread_id),
                )
                .await;
            }
        };

        for branch in &actively_running_branches {
            // A lane worktree lives at its batch's own path, and the lane
            // thread's next run recreates it there.
            if discovered_branches.contains(branch)
                || crate::engine::git_ops::is_lane_branch(branch)
            {
                continue;
            }
            // `idle_branches` can still hold this branch. The classifier emits
            // one row per THREAD, so two threads sharing a branch name can
            // disagree, and a disagreement is no mandate to recreate anything.
            //
            // `already_recovered` is deliberately NOT consulted here. Every
            // branch in this loop is classified `running`, so the set can only
            // drop a live turn whose worktree is also gone. That would leave a
            // lost worktree behaving differently from a surviving one.
            if idle_branches.contains(branch) {
                continue;
            }

            // An unanswered probe never picks a repo. A miss puts a real
            // recovery attempt in front of the destructive step, where a wrong
            // pick goes straight to the wrong repo with none.
            let found_repo = first_root_holding_branch(&repos_to_scan, branch)
                .await
                .cloned();

            match found_repo {
                Some((repo_root, repo_id)) => {
                    // No prune here, deliberately. `worktree_add` prunes under
                    // the admin lock right before it adds. That is the only
                    // sweep which cannot delete a concurrent spawn's half-built
                    // admin dir.
                    let wt_path = lost_session_worktree_path(
                        self.workspace_path(),
                        branch_to_thread.get(branch).copied(),
                    );
                    // NEVER delete a valid worktree here: reclamation belongs to
                    // the cleanup worker alone (ADR 0035). A deterministic
                    // `thread-<short>` dir on disk is one of four things:
                    //
                    // * a live worktree ON THIS BRANCH, or on a side branch the
                    //   agent cut from it: reuse it as-is. A partial-setup
                    //   leftover is repaired below, not deleted.
                    // * a live worktree on a DIFFERENT branch: skip it. Reusing
                    //   it resumes against the wrong checkout, and deleting it
                    //   destroys the other branch's work.
                    // * a stranded dir whose git admin is gone: cleared by
                    //   `clear_stranded_worktree_dir` so the add can recreate it.
                    // * absent: created by the add.
                    let is_live_worktree =
                        matches!(tokio::fs::try_exists(&wt_path).await, Ok(true))
                            && crate::engine::git_ops::is_live_worktree_at(&wt_path).await;
                    let on_our_branch = is_live_worktree
                        && {
                            let anchor = match branch_to_thread.get(branch) {
                            Some(&tid) => {
                                crate::engine::agent_session::resume::lookup_latest_worktree_head_sha(
                                    self.pool(),
                                    tid,
                                )
                                .await
                            }
                            None => crate::engine::agent_session::resume::IdleAnchor::Unknown,
                        };
                            crate::engine::agent_session::worktree_serves_branch(
                                &repo_root, &wt_path, branch, &anchor,
                            )
                            .await
                        };

                    if is_live_worktree && !on_our_branch {
                        // The shared path is occupied by another branch's live
                        // worktree, so skip it. The occupant recovers on its own
                        // pass.
                        log_legacy!(
                            "[Recovery] Skipping lost branch {} — shared worktree {} is live on a different branch (not reused, not deleted)",
                            branch,
                            wt_path.display()
                        );
                        continue;
                    }

                    let prepared = if on_our_branch {
                        log_legacy!(
                            "[Recovery] Reusing existing valid worktree for lost session: {} (branch {})",
                            wt_path.display(),
                            branch
                        );
                        true
                    } else {
                        // Absent, or present but stranded.
                        // `clear_stranded_worktree_dir` removes the dir only
                        // when the git admin is gone.
                        crate::engine::git_ops::clear_stranded_worktree_dir(&repo_root, &wt_path)
                            .await;
                        // The rebuilt worktree must have the shape the spawn
                        // path gave it. `Some` is an app thread, and takes the
                        // sparse cone over its own app folder.
                        let app_spawn_id = match branch_to_thread.get(branch) {
                            Some(&tid) => lookup_app_spawn_id(self.pool(), tid).await,
                            None => None,
                        };
                        match recreate_lost_worktree(
                            &repo_root,
                            branch,
                            &wt_path,
                            app_spawn_id.as_deref(),
                        )
                        .await
                        {
                            Ok(()) => {
                                log_legacy!(
                                    "[Recovery] Created fresh {} worktree for lost session: {} (branch {})",
                                    if app_spawn_id.is_some() { "sparse app" } else { "full" },
                                    wt_path.display(),
                                    branch
                                );
                                true
                            }
                            Err(e) => {
                                log_legacy!(
                                    "[Recovery] Failed to create worktree for branch {}: {}",
                                    branch,
                                    e
                                );
                                false
                            }
                        }
                    };

                    if prepared {
                        let marker = wt_path.join(WORKTREE_WORKSPACE_MARKER);
                        let marker_content = if let Some(ref rid) = repo_id {
                            format!("{}\n{}", ws_id, rid)
                        } else {
                            ws_id.clone()
                        };
                        if let Err(e) = tokio::fs::write(&marker, &marker_content).await {
                            log_legacy!(
                                "[Recovery] Failed to write workspace marker for {}: {}",
                                branch,
                                e
                            );
                        }
                        // Exclude engine-injected paths, so an external repo does
                        // not see them as untracked and commit them.
                        add_paths_to_worktree_exclude(&wt_path, WORKTREE_EXCLUDE_PATHS).await;
                        to_recover.push((wt_path, branch.clone(), repo_id, repo_root));
                    } else if let Some(&thread_id) = branch_to_thread.get(branch) {
                        log_legacy!(
                            "[Recovery] Ending stuck session for thread {}: worktree unavailable",
                            thread_id
                        );
                        end_stuck_session(self, thread_id).await;
                    }
                }
                None => {
                    if let Some(&thread_id) = branch_to_thread.get(branch) {
                        // The branch ref vanished from every repo. Try to
                        // recreate it from a surviving worktree's HEAD first, so
                        // the recorded `cc_session_id` can still `--resume` on
                        // the original branch. Ending the session drops that id
                        // and forces a fresh branch from main, discarding the
                        // conversation, so it is the last resort.
                        match recover_branch_ref_from_worktree(
                            self.workspace_path(),
                            thread_id,
                            branch,
                        )
                        .await
                        {
                            Some((repo_root, wt_path)) => {
                                log_legacy!(
                                    "[Recovery] Recovered branch {} for thread {} from surviving worktree — routing into resume instead of ending session",
                                    branch,
                                    thread_id
                                );
                                // `marker_repo_id` is only logged; repo selection
                                // uses `repo_root`.
                                to_recover.push((wt_path, branch.clone(), None, repo_root));
                            }
                            None => {
                                log_legacy!("[Recovery] Ending stuck session for thread {} — branch {} not found in any repo and no recoverable worktree", thread_id, branch);
                                end_stuck_session(self, thread_id).await;
                            }
                        }
                    }
                }
            }
        }

        let t_lost_branches = t0.elapsed();

        let mut recovering_threads: std::collections::HashSet<uuid::Uuid> =
            std::collections::HashSet::new();

        for (wt_path, branch_name, marker_repo_id, repo_root) in to_recover {
            if let Some(pending_change) = pending_by_branch.get(&branch_name) {
                if actively_running_branches.contains(&branch_name) {
                    // The session was running at shutdown, so the pending row
                    // may not cover the commits its turn made. Withdraw it
                    // without asking git, which a busy boot can time out, then
                    // withhold the branch's work (ADR 0400).
                    log_legacy!("[Recovery] Resuming active session with pending change {} for branch {} — withdrawing it and withholding its unfinished work", pending_change.id, branch_name);
                    let tid = branch_to_thread.get(&branch_name).copied();
                    if let Some(tid) = tid {
                        // A queued change is frozen (ADR 0463), and an
                        // unanswered read keeps it too. The re-decide still
                        // runs: it withholds the newer work as awaiting landing.
                        let queued = crate::engine::landing_lane::queued_snapshot(
                            self.pool(),
                            tid,
                            pending_change.id,
                        )
                        .await
                        .map_or(true, |snapshot| snapshot.is_some());
                        if !queued {
                            let origin = Some(MessageOrigin::engine(EngineReason::OrphanRecovery));
                            if let Err(e) = crate::engine::change_ops::emit_change_withdrawn(
                                &self.event_bus,
                                tid,
                                pending_change.id,
                                origin,
                            )
                            .await
                            {
                                log_legacy!("[Recovery] {}", e);
                            }
                        }
                        if let Err(e) = self
                            .redecide_branch_work(
                                tid,
                                &branch_name,
                                &repo_root,
                                Some(MessageOrigin::engine(EngineReason::OrphanRecovery)),
                                false,
                            )
                            .await
                        {
                            log_legacy!(
                                "[Recovery] Failed to withhold unfinished work on {}: {}",
                                branch_name,
                                e
                            );
                        }
                    }
                } else {
                    log_legacy!(
                        "[Recovery] Skipping worktree {} — already has pending change",
                        wt_path.display()
                    );
                    continue;
                }
            }
            // A completed prior recovery settles only the turn it recovered, and
            // the set stays true forever once a thread has been resumed once. So
            // a live classification outranks it: an in-flight turn at boot has no
            // live subprocess, whoever recovered the last one.
            if !actively_running_branches.contains(&branch_name)
                && already_recovered.contains(&branch_name)
            {
                log_legacy!(
                    "[Recovery] Skipping worktree {} — recovery thread already exists",
                    wt_path.display()
                );
                continue;
            }
            let has_pending_change = pending_by_branch.contains_key(&branch_name);
            if !branch_awaits_recovery(&branch_name, &idle_branches, has_pending_change) {
                log_legacy!(
                    "[Recovery] Skipping clean worktree {} — branch {} has no in-flight signal; cleanup worker will reclaim",
                    wt_path.display(),
                    branch_name
                );
                continue;
            }
            let Some(thread_id) = orphan_recovery_target(&branch_to_thread, &branch_name) else {
                log_legacy!(
                    "[Recovery] No originating thread for orphaned worktree {} (branch {}) — skipping; cleanup worker will reclaim",
                    wt_path.display(),
                    branch_name
                );
                continue;
            };
            log_legacy!(
                "[Recovery] Reusing original thread {} for branch {}",
                thread_id,
                branch_name
            );

            // Preserve a thread parked on an unanswered question: a stable
            // checkpoint, not an interrupted turn. No abort, no idle, worktree
            // intact, so the card stays answerable. Deliberately not added to
            // `recovering_threads`: the catch-all settle only touches `running`.
            if thread_has_unanswered_question(self.pool(), thread_id).await {
                log_legacy!(
                    "[Recovery] Preserving thread {} — parked on an unanswered question (branch {})",
                    thread_id,
                    branch_name
                );
                continue;
            }

            // Prevent duplicate recovery for the same thread (e.g., two branches
            // mapping to the same thread_id from stale resume retries).
            if !recovering_threads.insert(thread_id) {
                log_legacy!("[Recovery] Skipping duplicate recovery for thread {} (branch {}) — already recovering", thread_id, branch_name);
                cleanup_stale_worktree(&wt_path).await;
                continue;
            }

            // Carry the prior session id onto the synthetic `CodingAgentIdled`,
            // so a later Continue resumes it. The shared lookup also reads the
            // `Init`-time `CodingAgentSettingsChanged`, so a turn interrupted
            // before its first idle still resumes.
            let cc_session_id: Option<String> =
                crate::engine::session_queries::lookup_latest_cc_session_id(self.pool(), thread_id)
                    .await;

            let is_external_repo = recovery_branch_is_external_repo(
                &repo_root,
                &lucidos_repo_root,
                self.workspace_path(),
            );
            // Never auto-spawn the agent for a mid-turn crash. Surface the
            // interruption as a synthetic `CodingAgentIdled` carrying
            // `engine_restart_interrupt`, and let the user's Continue re-enter
            // through `--resume`. The worktree stays on disk: Tier 0 of the
            // cleanup worker leaves it until the thread reaches a terminal idle.
            log_legacy!("[Recovery] Surfacing interrupted Claude Code session for user-driven continue: {} (branch {}, thread {}{}, cc_session: {})",
                wt_path.display(), branch_name, thread_id,
                marker_repo_id.as_ref().map(|r| format!(", repo {}", r)).unwrap_or_default(),
                cc_session_id.as_deref().unwrap_or("none"));

            // Compute requires_restart from the branch's actual files so the
            // Apply button shows the correct label even before CC re-enters.
            let requires_restart = proposal_files_for_branch(&repo_root, &branch_name)
                .await
                .map(|files| files_require_restart(&files))
                .unwrap_or(false);
            let has_changes = pending_by_branch.contains_key(&branch_name);

            let meta = crate::engine::thread_events::EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..crate::engine::thread_events::EventMeta::NONE
            };
            // Emit the boundary `ResponseAborted` FIRST, so the UI shows the
            // "Response interrupted" panel above the synthetic idle. The
            // dispatcher classifies on `CodingAgentIdled.reason`, so the order
            // does not affect spawn decisions.
            if !boundary_abort_already_emitted(self.pool(), thread_id).await {
                let originating_event_id =
                    crate::engine::session_queries::latest_originating_event_id(
                        self.pool(),
                        thread_id,
                        crate::engine::thread_events::CC_ORIGINATING_EVENT_TYPES,
                    )
                    .await;
                let abort_meta = crate::engine::thread_events::EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    request_event_id: originating_event_id,
                    // The host killed the previous turn; recovery only marks it.
                    // Engine-deliberate work uses `Engine { .. }` instead.
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

            // Resume or manual Continue, by cause. A user switch left a
            // device-attributed teardown boundary, so auto-resume it. The resume
            // is queued: `main.rs` emits `ContinuationRequested` later in boot,
            // after the apply recoveries that read the queue. A crash left
            // no boundary, so offer Continue instead and never auto-resume: work
            // that crashed the engine must not loop.
            if switch_was_user_initiated(self.pool(), thread_id).await
                && !crate::engine::thread_stop::thread_is_stopped(self.pool(), thread_id).await
            {
                self.enqueue_switch_resume(thread_id);
                log_legacy!(
                    "[Recovery] Queued auto-resume after user switch for thread {} (branch {})",
                    thread_id,
                    branch_name
                );
            } else {
                let coding_agent = self.thread_coding_agent(thread_id).await;
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
                                has_changes,
                                is_external_repo,
                                requires_restart,
                                cc_session_id,
                                coding_agent,
                                reason: Some(ENGINE_RESTART_INTERRUPT_REASON.to_string()),
                                worktree_path: Some(wt_path.to_string_lossy().into_owned()),
                                // Snapshot HEAD, so the next spawn can detect
                                // edits made while the engine was down.
                                worktree_head_sha:
                                    crate::engine::agent_session::external_edits_for_recovery_head_sha(&wt_path).await,
                                bg_bash_pending: false,
                                turn_end: None,
                            },
                            meta,
                        },
                        "[Recovery] CodingAgentIdled (engine_restart_interrupt)",
                    )
                    .await;
            }
        }

        for thread_id in unengaged_switch_resume_candidates(self.pool()).await {
            if crate::engine::thread_stop::thread_is_stopped(self.pool(), thread_id).await {
                continue;
            }
            self.enqueue_switch_resume(thread_id);
            log_legacy!(
                "[Recovery] Queued auto-resume after user switch for thread {} (agent read nothing)",
                thread_id
            );
        }

        log_legacy!(
            "[Recovery] Lost-branch pass: {}ms, worktree pass: {}ms ({} recovering)",
            (t_lost_branches - t_classified).as_millis(),
            (t0.elapsed() - t_lost_branches).as_millis(),
            recovering_threads.len()
        );

        recovering_threads.into_iter().collect()
    }
}

/// Commit whatever the dead session left uncommitted, so it reaches the Apply
/// card. `proposal_files_for_branch` reads committed state only, so an
/// un-rescued edit is invisible to the proposal.
///
/// **The worktree stays.** Removing it here would be a session teardown
/// reclaiming a worktree, which has exactly one owner (ADR 0035), and nothing
/// downstream needs it gone. A failed commit is logged and nothing else: the
/// branch is still proposed from whatever did land.
pub(crate) async fn rescue_stale_worktree(wt: &Path) {
    if let Err(e) = commit_worktree_or_err(wt, "Coding agent changes (auto-committed)").await {
        log_legacy!(
            "[Recovery] Could not auto-commit {} before ending the stale session: {}",
            wt.display(),
            e
        );
    }
}

/// Rescue historical ordinary work without offering a second landing.
async fn preserve_historical_lane_session<'a>(
    bus: &crate::engine::event_bus::EventBus,
    repo: &Path,
    thread_id: Uuid,
    batch_id: Uuid,
    discard: bool,
    session_branch: Option<&'a str>,
) -> Option<&'a str> {
    let branch = session_branch.filter(|branch| {
        !discard && !branch.is_empty() && *branch != crate::engine::git_ops::lane_branch(batch_id)
    })?;
    let lookup = find_worktree_for_branch(repo, branch).await;
    settle_stale_worktree(repo, branch, thread_id, false, lookup).await;
    if let Some(files) = proposal_files_for_branch(repo, branch).await {
        crate::engine::change_ops::emit_proposal_withheld(
            bus,
            thread_id,
            branch,
            &files,
            crate::engine::thread_events::UnproposedReason::TurnIncomplete,
            false,
        )
        .await;
    }
    Some(branch)
}

/// Act on a stale session's worktree, given what git said about its branch.
///
/// Discard removes the tree, and that removal is what lets the `git branch -D`
/// downstream run. Otherwise the tree is rescued with a commit and left to the
/// cleanup worker.
///
/// The lookup is a parameter rather than an inner call so a test can drive the
/// `Unknown` arm against a real repo. Unknown skips both arms: neither a
/// force-remove nor an auto-commit may run against a tree we could not locate.
/// The branch-delete gate downstream stays untouched. That delete is authorized
/// by the user's Discard, and git refuses to delete a branch a worktree holds.
pub(crate) async fn settle_stale_worktree(
    repo_root: &Path,
    branch_name: &str,
    thread_id: Uuid,
    discard: bool,
    lookup: WorktreeLookup,
) {
    match lookup {
        WorktreeLookup::Found(wt) => {
            if discard {
                remove_discarded_stale_worktree(repo_root, &wt).await;
            } else {
                rescue_stale_worktree(&wt).await;
            }
        }
        WorktreeLookup::NotFound => {}
        WorktreeLookup::Unknown => log_legacy!(
            "[Recovery] Thread {}: git worktree list gave no answer for branch {}; leaving its \
             worktree alone rather than removing or committing on a guess",
            thread_id,
            branch_name
        ),
    }
}

/// Remove the worktree of a session the user has just DISCARDED.
///
/// Load-bearing rather than reclamation: git refuses to delete a branch that a
/// worktree still has checked out, so the `git branch -D` that follows cannot
/// run until this does. ADR 0035 names this removal as one the session path
/// keeps.
pub(crate) async fn remove_discarded_stale_worktree(repo_root: &Path, wt: &Path) {
    let Some(wt_str) = wt.to_str() else {
        log_legacy!(
            "[Recovery] skipped worktree remove (non-UTF8 path): {}",
            wt.display()
        );
        return;
    };
    match git_cmd(&["worktree", "remove", "--force", wt_str], repo_root).await {
        Ok(o) if o.status.success() => {}
        Ok(o) => log_legacy!(
            "[Recovery] git worktree remove failed for {}: {}",
            wt.display(),
            String::from_utf8_lossy(&o.stderr).trim()
        ),
        Err(e) => log_legacy!(
            "[Recovery] git worktree remove errored for {}: {}",
            wt.display(),
            e
        ),
    }
}

/// True when a discovered worktree's branch still owes the user a recovery: the
/// turn on it was open when the engine died, or a change on it is still waiting
/// to be applied. False means the branch is settled, so the cleanup worker can
/// have the disk.
///
/// **Every input must be a fact about the branch's CURRENT turn.** That is why
/// this exists as a named predicate rather than an inline `&&`. `idle_branches`
/// reads the newest lifecycle event, and a pending change is by definition
/// unresolved. A *historical* fact is not admissible however settled it sounds.
/// A coding-agent thread works one branch across many turns, so anything true of
/// an earlier turn stays true forever and silently retires the thread.
pub(crate) fn branch_awaits_recovery(
    branch: &str,
    idle_branches: &std::collections::HashSet<String>,
    has_pending_change: bool,
) -> bool {
    // Pending change wins over an idle classification: the agent reached its
    // idle and then waited for Apply, so the branch is settled only once the
    // user resolves the change.
    has_pending_change || !idle_branches.contains(branch)
}

/// True when the newest `ResponseAborted` after the thread's last start is an
/// **engine-shutdown teardown carrying a device actor**: the fingerprint of a
/// user-initiated *Switch to new version*. A crash emits no teardown boundary,
/// so this is false and the thread keeps the manual Continue affordance instead
/// of auto-resuming (ADR 0045).
///
/// **Both halves of the fingerprint are load-bearing.** The device actor alone
/// is not enough: `AbortCause::StaleSettle` deliberately carries the actor of
/// the user button that exposed a stuck row. An actor-only predicate would read
/// a user *Stop* as a *Switch* and resume work the user just abandoned.
///
/// The start set includes the resume starts, so once a switch abort has been
/// consumed by a resume it stops counting. That is the loop-breaker: an
/// auto-resume that crashes the engine again before emitting anything leaves the
/// resume start newer than the abort, so the next boot offers manual Continue.
///
/// Shared with the chat resume gate, so the two definitions cannot drift.
pub(crate) async fn switch_was_user_initiated(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(&format!(
        "SELECT EXISTS ( \
            SELECT 1 FROM events WHERE aggregate_id = $1 \
              AND {SWITCH_TEARDOWN_ABORT_SQL} \
              AND {unsuperseded})",
        unsuperseded = switch_abort_unsuperseded_sql("$1", "sequence"),
    ))
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

/// True when a `ResponseAborted` already covers the thread's **current** turn, so
/// the recovery pass must not emit a second boundary over the top of it.
///
/// `/api/v1/restart` pre-emits a `ResponseAborted { actor: device }` for
/// in-flight coding-agent threads BEFORE shutdown, so the post-restart timeline
/// reads "Paused by restart". Emitting again here would double-render the abort
/// panel and bury that device attribution under the system actor.
///
/// "Current turn" is the load-bearing half. It is why this shares
/// [`after_latest_thread_start_sql`] with the switch fingerprint rather than
/// spelling out a start set of its own. An abort older than the thread's newest
/// start belongs to a turn a later resume superseded. It says nothing about
/// whether THIS turn was interrupted.
pub(crate) async fn boundary_abort_already_emitted(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
    sqlx::query_scalar::<_, bool>(&format!(
        "SELECT EXISTS ( \
            SELECT 1 FROM events WHERE aggregate_id = $1 \
              AND event_type = 'ResponseAborted' \
              AND {current_turn})",
        current_turn = after_latest_thread_start_sql("$1", "sequence"),
    ))
    .bind(thread_id.to_string())
    .fetch_one(pool)
    .await
    // A probe that could not run is UNKNOWN. Fall back to emitting: a duplicate
    // boundary is cosmetic noise, a missing one hides a real interruption.
    .unwrap_or(false)
}

/// [`boundary_abort_already_emitted`], narrowed to a boundary that **names this
/// turn**: same current-turn window, plus the abort's `request_event_id` must
/// equal the turn's own anchor.
///
/// The extra clause is what makes the answer safe for a caller that will SKIP
/// its own terminal on a `true`. The recovery pass can rely on the window alone,
/// because a spurious `true` costs it only a duplicate panel. An in-loop
/// terminal is the turn's ONLY terminator, so there a spurious `true` costs the
/// turn its terminator.
///
/// The window alone is turn-exact only for turns carrying one of
/// [`THREAD_START_EVENTS_SQL`], and two ordinary shapes carry none: a parent
/// woken by `ChildThreadCompleted`, and an `answered_after_idle` continuation
/// that deliberately withholds its `ContinuationStarted`. For those, a previous
/// turn's abort stays inside the window forever.
///
/// A `None` anchor proves nothing, so the caller treats it as "not covered" and
/// emits. Same fail-open direction as the sibling: a duplicate boundary is
/// cosmetic, a missing terminator is not.
pub(crate) async fn boundary_abort_covers_turn(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
    request_event_id: Uuid,
) -> bool {
    sqlx::query_scalar::<_, bool>(&format!(
        "SELECT EXISTS ( \
            SELECT 1 FROM events WHERE aggregate_id = $1 \
              AND event_type = 'ResponseAborted' \
              AND payload->>'request_event_id' = $2 \
              AND {current_turn})",
        current_turn = after_latest_thread_start_sql("$1", "sequence"),
    ))
    .bind(thread_id.to_string())
    .bind(request_event_id.to_string())
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

/// The coding-agent threads a user-initiated *Switch to new version* interrupted
/// before their agent read anything, and that nothing has resumed since.
///
/// The worktree scan in `recover_orphaned_worktrees` cannot see a thread with no
/// `SessionStarted`: it maps a branch to a thread through that event. Every
/// other clause is the chat drain's, from the same shared fragments, so the
/// definitions of "a live switch abort" cannot drift.
///
/// A thread whose agent started but read nothing is also selected, though the
/// scan queues it too: `enqueue_switch_resume` keeps it queued once.
fn unengaged_switch_resume_candidates_sql() -> String {
    format!(
        "SELECT e.aggregate_id::uuid AS thread_id \
         FROM events e \
         JOIN thread_summaries t ON t.thread_id = e.aggregate_id::uuid \
         WHERE e.aggregate = 'thread' \
           AND t.source = 'claude_code' \
           AND t.state = 'active' \
           AND t.archive_state != 'archived' \
           AND {abort} \
           AND {unsuperseded} \
           AND {unengaged} \
         ORDER BY e.sequence ASC",
        abort = SWITCH_TEARDOWN_ABORT_SQL,
        unengaged = agent_never_engaged_sql("e.aggregate_id"),
        unsuperseded = switch_abort_unsuperseded_sql("e.aggregate_id", "e.sequence"),
    )
}

/// Run [`unengaged_switch_resume_candidates_sql`]. An error yields an empty
/// list (logged): the affected threads keep their manual Continue.
pub(crate) async fn unengaged_switch_resume_candidates(pool: &sqlx::PgPool) -> Vec<Uuid> {
    match sqlx::query_scalar::<_, Uuid>(&unengaged_switch_resume_candidates_sql())
        .fetch_all(pool)
        .await
    {
        Ok(ids) => ids,
        Err(e) => {
            log_legacy!(
                "[Recovery] unengaged switch-resume scan failed: {}. \
                 Affected threads keep their manual Continue",
                e
            );
            Vec::new()
        }
    }
}

/// Threads still holding an UNKEPT resume promise: a switch-teardown abort that
/// is the thread's newest `ResponseAborted`, with no start event after it, on a
/// thread the projection still shows `paused`. ADR 0045 records why the engine
/// discharges its own promise, and which paths leave one unkept.
///
/// Three clauses, each load-bearing:
///
/// * `t.status = 'paused'` scopes the sweep to threads still on the interruption.
/// * `t.state = 'active'` is the compose lifecycle, NOT the archive curtain. A
///   composing row is a draft and a discarded one a tombstone.
/// * [`switch_abort_unsuperseded_sql`], shared with both resume gates. Its
///   newest-abort half is the idempotency guard: the withdrawal emits a
///   `RecoveryAfterRestart` abort, so the thread stops matching on the next boot.
///
/// **Archived threads are deliberately INCLUDED**, unlike the resume drains:
/// this revives nothing, it corrects a promise the engine could not keep. The
/// withdrawal inherits the abort's `request_event_id` AND its `actor`, the only
/// surviving record of who clicked switch.
fn unresumed_switch_threads_sql() -> String {
    format!(
        "SELECT e.aggregate_id::uuid AS thread_id, \
                e.payload->>'request_event_id' AS request_event_id, \
                e.payload->'actor' AS actor \
         FROM events e \
         JOIN thread_summaries t ON t.thread_id = e.aggregate_id::uuid \
         WHERE e.aggregate = 'thread' \
           AND t.state = 'active' \
           AND t.status = {paused} \
           AND {abort} \
           AND {unsuperseded} \
         ORDER BY e.sequence ASC",
        paused = ThreadStatus::Paused.sql_literal(),
        abort = SWITCH_TEARDOWN_ABORT_SQL,
        unsuperseded = switch_abort_unsuperseded_sql("e.aggregate_id", "e.sequence"),
    )
}

/// Withdraw every resume promise this boot did not keep. It emits the
/// crash-shaped `ResponseAborted { RecoveryAfterRestart }` boundary that
/// `chat::recovery::recover_orphaned_threads` already uses for an interrupted
/// turn nobody is resuming. The frontend's newest-abort scan then re-arms
/// Continue on its own, because that boundary is not a switch abort (ADR 0045).
///
/// `resumed` is the union of what the two resume drains actuated, passed BY ID
/// rather than re-derived from the events table. A coding-agent resume has only
/// emitted `ContinuationRequested` by then, and that type is deliberately absent
/// from [`THREAD_START_EVENTS_SQL`]. A query-only exclusion would therefore
/// re-abort a thread that is resuming perfectly well.
///
/// Best-effort, like every boot sweep: a DB error degrades to "nothing to
/// withdraw" rather than blocking boot. Every withdrawal is logged by id, so the
/// sweep can never read as "resumed everything" when it did not.
///
/// Runs LAST in the boot sequence, after both drains: only then can it tell a
/// broken promise from a kept one. The boundary is crash-SHAPED, not
/// crash-ATTRIBUTED. The actor is carried over from the switch abort by
/// [`unresumed_switch_threads_sql`].
pub(crate) async fn settle_unresumed_switch_threads(
    pool: &sqlx::PgPool,
    bus: &crate::engine::event_bus::EventBus,
    resumed: &std::collections::HashSet<Uuid>,
) {
    type UnresumedSwitchRow = (Uuid, Option<String>, Option<serde_json::Value>);
    let rows: Vec<UnresumedSwitchRow> = match sqlx::query_as(&unresumed_switch_threads_sql())
        .fetch_all(pool)
        .await
    {
        Ok(rows) => rows,
        Err(e) => {
            log_legacy!(
                "[Recovery] unresumed-switch sweep query failed: {}. \
                 Affected threads keep their paused status with no Continue",
                e
            );
            return;
        }
    };

    for (thread_id, request_event_id, switch_actor) in rows {
        if resumed.contains(&thread_id) {
            continue;
        }
        log_legacy!(
            "[Recovery] Switch-interrupted thread {} was not resumed by this boot. \
             Withdrawing the resume promise so its Continue affordance returns",
            thread_id
        );
        // Both fields are inherited from the switch abort rather than invented.
        // The id keeps the two panels in one exchange, and the actor keeps them
        // naming the same person.
        //
        // The fallback is unreachable, since the selection matches only a device
        // actor. Log it rather than silently attributing the user's restart to
        // the host.
        let actor = switch_actor
            .and_then(|v| match serde_json::from_value::<MessageOrigin>(v) {
                Ok(origin) => Some(origin),
                Err(e) => {
                    log_legacy!(
                        "[Recovery] Switch abort on thread {} has an unreadable actor: {}. \
                         Withdrawal falls back to the system actor",
                        thread_id,
                        e
                    );
                    None
                }
            })
            .unwrap_or_else(MessageOrigin::system);
        let meta = crate::engine::thread_events::EventMeta {
            // `.ok()` is safe: an unparseable value only costs grouping, and the
            // engine wrote the field itself.
            request_event_id: request_event_id.as_deref().and_then(|s| s.parse().ok()),
            actor: Some(actor),
            ..crate::engine::thread_events::EventMeta::NONE
        };
        crate::engine::thread_events::emit_response_aborted(
            bus,
            thread_id,
            crate::engine::thread_events::AbortCause::RecoveryAfterRestart,
            "This response was interrupted by an engine restart and did not resume.".to_string(),
            vec![],
            None,
            None,
            meta,
            "[Recovery] ResponseAborted (switch resume not kept)",
        )
        .await;
    }
}

/// The events that start a turn, as an SQL list, for the two boot settles.
macro_rules! turn_start_events {
    () => {
        "'MessageReceived','TriggerStarted','ChildThreadCompleted',\
         'ContinuationRequested','ContinuationStarted','CodingAgentUserMessageSent'"
    };
}

/// The start of a thread's latest turn, when that turn was interrupted: no
/// terminator came after it, and either it did something or it was never
/// going to run on its own. A bare `ChildThreadCompleted` DOES run on its own
/// (the parent-resume refire), so it is pending rather than interrupted.
/// Ordered by `sequence`, which is total.
///
/// A bare `ContinuationRequested` is interrupted. The spawn dispatcher's
/// orphan scan reads only `running` threads, and a request the previous
/// process never started must not auto-resume after a crash.
///
/// The terminator and activity lists are the chat sweep's own. The start list
/// is wider than the chat sweep's: a coding-agent turn or a Continue rerun can
/// also die before the settle runs.
const INTERRUPTED_TURN_START_SQL: &str = concat!(
    "WITH start AS ( \
        SELECT id, event_type, payload->>'channel' AS channel, sequence FROM events \
        WHERE aggregate = 'thread' AND aggregate_id = $1::text \
          AND event_type IN (",
    turn_start_events!(),
    ") \
        ORDER BY sequence DESC LIMIT 1 \
    ), marks AS ( \
        SELECT MAX(sequence) FILTER (WHERE event_type IN (",
    crate::engine::restore_boot::turn_terminal_events!(),
    ")) AS last_terminal, \
               MAX(sequence) FILTER (WHERE event_type IN (",
    crate::engine::restore_boot::turn_activity_events!(),
    ")) AS last_activity \
        FROM events WHERE aggregate = 'thread' AND aggregate_id = $1::text \
    ) \
    SELECT s.id, s.event_type, s.channel FROM start s, marks m \
    WHERE s.sequence > COALESCE(m.last_terminal, 0) \
      AND (m.last_activity > s.sequence \
           OR s.event_type <> 'ChildThreadCompleted')"
);

/// The boot step that settles every thread still `running`, once recovery has
/// had its turn. Only [`settle_stranded_trigger_runs`] runs after it. No turn
/// runs at boot, so each one is either a turn the restart interrupted or a
/// status write no event backs.
///
/// - **An interrupted turn** gets `ResponseAborted { RecoveryAfterRestart }`,
///   what the chat sweep emits, so it reads "Response interrupted" with a
///   Continue. That includes a message the crash dropped before its first
///   token, and a `ContinuationRequested` the previous process never started.
/// - **Anything else** goes straight to `idle`. An ended turn's `running` came
///   from the event-less parent wake, and a pending child completion runs
///   later in boot. An abort for either would record one that never happened.
///
/// `recovering` holds threads the coding-agent pass is about to resume.
pub async fn settle_orphaned_running_threads(
    pool: &sqlx::PgPool,
    bus: &crate::engine::event_bus::EventBus,
    recovering: &std::collections::HashSet<Uuid>,
) {
    let running: Vec<Uuid> = match sqlx::query_scalar::<_, Uuid>(&format!(
        "SELECT thread_id FROM thread_summaries WHERE status = {}",
        ThreadStatus::Running.sql_literal(),
    ))
    .fetch_all(pool)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            log_legacy!(
                "[Recovery] orphaned-running settle sweep query failed: {}",
                e
            );
            return;
        }
    };
    for tid in running {
        if recovering.contains(&tid) {
            continue;
        }
        if let Err(e) = settle_one_orphaned_running_thread(pool, bus, tid).await {
            log_legacy!(
                "[Recovery] Failed to settle orphaned running thread {}: {}",
                tid,
                e
            );
        }
    }
}

async fn settle_one_orphaned_running_thread(
    pool: &sqlx::PgPool,
    bus: &crate::engine::event_bus::EventBus,
    tid: Uuid,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crate::engine::chat::recovery::{
        emit_restart_abort, start_channel, RESTART_INTERRUPTED_TEXT,
    };

    // An ended turn with an eligible successor is intentionally still
    // running. Startup replays that handoff after this sweep; no process was
    // interrupted in the gap. An active turn still follows crash recovery.
    if super::super::successor::between_successor_sessions(pool, tid).await? {
        return Ok(());
    }

    let interrupted: Option<(Uuid, String, Option<String>)> =
        sqlx::query_as(INTERRUPTED_TURN_START_SQL)
            .bind(tid)
            .fetch_optional(pool)
            .await?;
    let Some((start_id, start_type, stamped_channel)) = interrupted else {
        sqlx::query(&format!(
            "UPDATE thread_summaries SET status = {} WHERE thread_id = $1 AND status = {}",
            ThreadStatus::Idle.sql_literal(),
            ThreadStatus::Running.sql_literal(),
        ))
        .bind(tid)
        .execute(pool)
        .await?;
        return Ok(());
    };
    emit_restart_abort(
        bus,
        tid,
        RESTART_INTERRUPTED_TEXT.to_string(),
        Some(start_id),
        start_channel(stamped_channel.as_deref(), Some(start_type.as_str())),
    )
    .await?;
    log_legacy!(
        "[Recovery] Settled orphaned `running` thread {}: the restart interrupted its turn",
        tid
    );
    Ok(())
}

/// Trigger roots left `idle` in the inbox over a bare `TriggerStarted`: the
/// latest turn start, with nothing after it that ends the run. `$1` is the
/// idle status and `$2` the inbox state.
///
/// Every filter keeps a decision out of the sweep's hands:
///
/// - `idle` skips a thread parked on the user (ADR 0259). The running settle
///   has already ended every `running` row.
/// - The inbox skips a thread the user archived, since an abort moves a
///   thread to the inbox.
/// - `TriggerCompleted` counts as an end: the run recorded its completion.
const STRANDED_TRIGGER_RUNS_SQL: &str = concat!(
    "SELECT ts.thread_id, s.id, s.channel FROM thread_summaries ts \
     CROSS JOIN LATERAL ( \
        SELECT id, event_type, payload->>'channel' AS channel, sequence FROM events \
        WHERE aggregate = 'thread' AND aggregate_id = ts.thread_id::text \
          AND event_type IN (",
    turn_start_events!(),
    ") \
        ORDER BY sequence DESC LIMIT 1 \
     ) s \
     WHERE ts.source = 'trigger' AND ts.parent_thread_id IS NULL \
       AND ts.status = $1 AND ts.archive_state = $2 \
       AND s.event_type = 'TriggerStarted' \
       AND NOT EXISTS ( \
        SELECT 1 FROM events t \
        WHERE t.aggregate = 'thread' AND t.aggregate_id = ts.thread_id::text \
          AND t.sequence > s.sequence \
          AND t.event_type IN (",
    crate::engine::restore_boot::turn_terminal_events!(),
    ",'TriggerCompleted'))"
);

/// Settle the trigger runs a restart cut off before any activity, whose
/// `running` an event-less write already turned to `idle`. The running settle
/// never sees them, and the chat sweep needs activity after the start.
///
/// Each gets the running settle's `ResponseAborted { RecoveryAfterRestart }`.
/// The lifecycle contract then picks the section: an unattended run archives,
/// while a review run or a pinned one stays in the inbox (ADR 0312). Nothing
/// resumes the run, so a crash cannot loop.
pub async fn settle_stranded_trigger_runs(
    pool: &sqlx::PgPool,
    bus: &crate::engine::event_bus::EventBus,
) {
    use crate::engine::chat::recovery::{
        emit_restart_abort, start_channel, RESTART_INTERRUPTED_TEXT,
    };
    use crate::engine::thread_lifecycle::ArchiveState;

    let stranded: Vec<(Uuid, Uuid, Option<String>)> =
        match sqlx::query_as(STRANDED_TRIGGER_RUNS_SQL)
            .bind(ThreadStatus::Idle.as_str())
            .bind(ArchiveState::Inbox.as_str())
            .fetch_all(pool)
            .await
        {
            Ok(rows) => rows,
            Err(e) => {
                log_legacy!("[Recovery] stranded trigger run sweep query failed: {}", e);
                return;
            }
        };
    for (tid, start_id, stamped_channel) in stranded {
        match emit_restart_abort(
            bus,
            tid,
            RESTART_INTERRUPTED_TEXT.to_string(),
            Some(start_id),
            start_channel(stamped_channel.as_deref(), Some("TriggerStarted")),
        )
        .await
        {
            Ok(()) => log_legacy!(
                "[Recovery] Settled stranded trigger run {}: a restart cut it off",
                tid
            ),
            Err(e) => log_legacy!(
                "[Recovery] Failed to settle stranded trigger run {}: {}",
                tid,
                e
            ),
        }
    }
}

#[cfg(test)]
mod open_request_tests {
    use crate::engine::open_question::OpenRequest;

    /// An agent's prepared request parks its question tool. One the engine
    /// raised parks nobody, and a plain question is no request. Words answer
    /// only an engine-raised request that offers choices and that no
    /// unattested caller waits on.
    #[test]
    fn a_request_is_awaited_only_when_an_agent_prepared_it() {
        let card = |request: serde_json::Value| {
            OpenRequest::of(&serde_json::json!({ "question": "q", "permission_request": request }))
        };
        assert_eq!(
            card(serde_json::json!({ "action": "create-workspace", "prepared": "req-1" })),
            Some(OpenRequest {
                awaited: true,
                words_answer: false
            })
        );
        assert_eq!(
            card(serde_json::json!({
                "action": "run-batch",
                "choices": { "deny": { "kind": "refuse" } }
            })),
            Some(OpenRequest {
                awaited: false,
                words_answer: true
            })
        );
        assert_eq!(
            card(serde_json::json!({ "action": "run-batch" })),
            Some(OpenRequest {
                awaited: false,
                words_answer: false
            })
        );
        assert_eq!(
            card(serde_json::json!({
                "action": "act-for-unattested-caller",
                "unattested_act": { "verb": "start" }
            })),
            Some(OpenRequest {
                awaited: false,
                words_answer: false
            })
        );
        assert_eq!(card(serde_json::Value::Null), None);
        assert_eq!(
            OpenRequest::of(&serde_json::json!({ "question": "q" })),
            None
        );
    }
}

#[cfg(test)]
mod historical_lane_tests {
    use super::*;
    use crate::engine::event_bus::EventBus;
    use crate::test_support::{setup_test_db, teardown_test_db};

    #[tokio::test]
    async fn historical_lane_cleanup_rescues_dirty_work_and_withholds_it() {
        let (pool, db) = setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let repo = tempfile::tempdir().unwrap();
        let wt = tempfile::tempdir().unwrap();
        git_ran_ok(&["init", "-b", "main"], repo.path())
            .await
            .unwrap();
        git_ran_ok(&["config", "user.name", "Test User"], repo.path())
            .await
            .unwrap();
        git_ran_ok(&["config", "user.email", "test@example.com"], repo.path())
            .await
            .unwrap();
        git_ran_ok(&["commit", "--allow-empty", "-m", "base"], repo.path())
            .await
            .unwrap();
        git_ran_ok(
            &[
                "worktree",
                "add",
                "-b",
                "historical-work",
                wt.path().to_str().unwrap(),
            ],
            repo.path(),
        )
        .await
        .unwrap();
        std::fs::write(wt.path().join("work.txt"), "recover this work").unwrap();
        let thread = Uuid::new_v4();
        crate::test_support::start_cc_session(&bus, thread, "historical-work", None).await;

        let batch = Uuid::new_v4();
        for (discard, branch) in [
            (true, Some("historical-work")),
            (false, None),
            (false, Some("")),
        ] {
            assert_eq!(preserve_historical_lane_session(
                &bus, repo.path(), thread, batch, discard, branch,
            ).await, None);
        }
        let lane = crate::engine::git_ops::lane_branch(batch);
        assert_eq!(
            preserve_historical_lane_session(&bus, repo.path(), thread, batch, false, Some(&lane),)
                .await,
            None
        );
        let dirty = git_cmd(&["status", "--porcelain"], wt.path())
            .await
            .unwrap();
        assert!(
            !dirty.stdout.is_empty(),
            "excluded sessions must not be committed"
        );
        assert_eq!(
            preserve_historical_lane_session(
                &bus,
                repo.path(),
                thread,
                batch,
                false,
                Some("historical-work"),
            )
            .await,
            Some("historical-work")
        );

        assert_eq!(
            std::fs::read_to_string(wt.path().join("work.txt")).unwrap(),
            "recover this work"
        );
        let status = git_cmd(&["status", "--porcelain"], wt.path())
            .await
            .unwrap();
        assert!(status.status.success());
        assert!(status.stdout.is_empty(), "uncommitted work must be rescued");
        let files = proposal_files_for_branch(repo.path(), "historical-work")
            .await
            .unwrap();
        assert_eq!(files, vec!["work.txt"]);
        let events: Vec<String> = sqlx::query_scalar(
            "SELECT event_type FROM events WHERE thread_id = $1 \
             AND event_type IN ('ProposalWithheld', 'ChangeProposed') ORDER BY sequence",
        )
        .bind(thread)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            events,
            vec!["ProposalWithheld"],
            "work is visible without a duplicate proposal"
        );
        teardown_test_db(&db).await;
    }
}
