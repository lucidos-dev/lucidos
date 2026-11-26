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

pub(super) struct BranchProposalDetails {
    pub description: String,
    pub hardened: bool,
    pub requires_restart: bool,
}

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
    let hardened = branch_is_hardened(pool, changes, repo_root, branch_name).await;
    BranchProposalDetails {
        description,
        hardened,
        requires_restart: files_require_restart(changed_files),
    }
}

fn worktrees_scan_prefix(workspace: &Path) -> String {
    let mut prefix = worktrees_dir(workspace).to_string_lossy().to_string();
    if !prefix.ends_with(std::path::MAIN_SEPARATOR) {
        prefix.push(std::path::MAIN_SEPARATOR);
    }
    prefix
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
                | ProposeOutcome::AwaitingLanding(_)
                | ProposeOutcome::Refused(_),
            ) => {}
            Err(e) => {
                log_legacy!(
                    "[Recovery] Failed to propose change from stale session: {}",
                    e
                );
            }
        }
    }

    async fn end_stale_waiting_session(
        self: &Arc<Self>,
        thread_id: Uuid,
        discard: bool,
        actor: Option<MessageOrigin>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
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
                return Err("No Claude Code session found for this thread".into());
            }
            Some(_) => {
                let coding_agent = self.thread_coding_agent(thread_id).await;
                self.event_bus
                    .emit_or_log(
                        crate::engine::event_bus::BusEvent::Thread {
                            thread_id,
                            event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
                                has_changes: false,
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

        let repo = stale_session_repo(
            session_kind.as_deref(),
            self.projection_says_external_repo(thread_id).await,
            &main_worktree().await,
            self.workspace_path(),
        );
        let is_external = matches!(repo, StaleSessionRepo::External);

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
                    match proposal_files_for_branch(repo_root, &branch_name).await {
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
                        None => log_legacy!(
                            "[Recovery] Branch {} has no proposable diff, keeping branch (discard=false)",
                            branch_name
                        ),
                    }
                }
            }
        }

        if discard {
            log_legacy!(
                "[Recovery] Discarding stale session changes (branch {})",
                branch_name
            );
            self.discard_open_changes_for_thread(thread_id, actor.clone())
                .await;
        }

        if let Some(delete_root) = stale_discard_branch_delete_root(discard, &repo, &branch_name) {
            if let Err(e) = git_ran_ok(&["branch", "-D", &branch_name], delete_root).await {
                log_legacy!("[Recovery] Failed to delete branch {}: {}", branch_name, e);
            }
        }

        let coding_agent = self.thread_coding_agent(thread_id).await;
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::Thread {
                    thread_id,
                    event: crate::engine::thread_events::ThreadEvent::CodingAgentIdled {
                        has_changes: branch_has_work,
                        is_external_repo: is_external,
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
                "[Recovery] CodingAgentIdled",
            )
            .await;

        self.broadcast_changes_updated().await;

        Ok(())
    }

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

    async fn settle_unresumed_switch_threads(&self, resumed: &std::collections::HashSet<Uuid>) {
        settle_unresumed_switch_threads(self.pool(), &self.event_bus, resumed).await;
    }

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
        let ws_worktrees_prefix = worktrees_scan_prefix(self.workspace_path());

        let external_repos = crate::core::repositories::RepositoryStore::list(self.pool())
            .await
            .unwrap_or_else(|e| {
                log_legacy!("[Recovery] Failed to list external repos: {}", e);
                Vec::new()
            });
        let repos_to_scan =
            recovery_repo_roots(&lucidos_repo_root, self.workspace_path(), &external_repos);

        let mut to_recover: Vec<(PathBuf, String, Option<String>, PathBuf)> = Vec::new();

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
                        if (crate::engine::git_ops::is_coding_agent_branch(br)
                            || crate::engine::git_ops::is_lane_branch(br))
                            && wt.starts_with(&ws_worktrees_prefix)
                        {
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
                    worktree_path = None;
                    branch = None;
                }
            }
        }

        let t_worktree_scan = t0.elapsed();

        let pool = self.pool();
        let proj = self.changes();
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

        let discovered_branches: std::collections::HashSet<String> =
            to_recover.iter().map(|(_, br, _, _)| br.clone()).collect();

        let end_stuck_session = |engine: &Arc<Self>, thread_id: Uuid| {
            let bus = engine.event_bus.clone();
            let engine = engine.clone();
            async move {
                if thread_has_unanswered_question(engine.pool(), thread_id).await {
                    log_legacy!(
                        "[Recovery] Preserving stuck thread {} — parked on an unanswered question (no idle emitted)",
                        thread_id
                    );
                    return;
                }
                let coding_agent = engine.thread_coding_agent(thread_id).await;
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
                            worktree_path: None,
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
            if discovered_branches.contains(branch)
                || crate::engine::git_ops::is_lane_branch(branch)
            {
                continue;
            }
            if idle_branches.contains(branch) {
                continue;
            }

            let found_repo = first_root_holding_branch(&repos_to_scan, branch)
                .await
                .cloned();

            match found_repo {
                Some((repo_root, repo_id)) => {
                    let wt_path = lost_session_worktree_path(
                        self.workspace_path(),
                        branch_to_thread.get(branch).copied(),
                    );
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
                        crate::engine::git_ops::clear_stranded_worktree_dir(&repo_root, &wt_path)
                            .await;
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
                    log_legacy!("[Recovery] Resuming active session with pending change {} for branch {} — withdrawing it and withholding its unfinished work", pending_change.id, branch_name);
                    let tid = branch_to_thread.get(&branch_name).copied();
                    if let Some(tid) = tid {
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

            if thread_has_unanswered_question(self.pool(), thread_id).await {
                log_legacy!(
                    "[Recovery] Preserving thread {} — parked on an unanswered question (branch {})",
                    thread_id,
                    branch_name
                );
                continue;
            }

            if !recovering_threads.insert(thread_id) {
                log_legacy!("[Recovery] Skipping duplicate recovery for thread {} (branch {}) — already recovering", thread_id, branch_name);
                cleanup_stale_worktree(&wt_path).await;
                continue;
            }

            let cc_session_id: Option<String> =
                crate::engine::session_queries::lookup_latest_cc_session_id(self.pool(), thread_id)
                    .await;

            let is_external_repo = recovery_branch_is_external_repo(
                &repo_root,
                &lucidos_repo_root,
                self.workspace_path(),
            );
            log_legacy!("[Recovery] Surfacing interrupted Claude Code session for user-driven continue: {} (branch {}, thread {}{}, cc_session: {})",
                wt_path.display(), branch_name, thread_id,
                marker_repo_id.as_ref().map(|r| format!(", repo {}", r)).unwrap_or_default(),
                cc_session_id.as_deref().unwrap_or("none"));

            let requires_restart = proposal_files_for_branch(&repo_root, &branch_name)
                .await
                .map(|files| files_require_restart(&files))
                .unwrap_or(false);
            let has_changes = pending_by_branch.contains_key(&branch_name);

            let meta = crate::engine::thread_events::EventMeta {
                channel: Some(EventChannel::ClaudeCode),
                ..crate::engine::thread_events::EventMeta::NONE
            };
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

            if switch_was_user_initiated(self.pool(), thread_id).await
                && crate::core::store::thread_hold::may_start_turn(self.pool(), thread_id).await
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
            if !crate::core::store::thread_hold::may_start_turn(self.pool(), thread_id).await {
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

pub(crate) async fn rescue_stale_worktree(wt: &Path) {
    if let Err(e) = commit_worktree_or_err(wt, "Coding agent changes (auto-committed)").await {
        log_legacy!(
            "[Recovery] Could not auto-commit {} before ending the stale session: {}",
            wt.display(),
            e
        );
    }
}

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

pub(crate) fn branch_awaits_recovery(
    branch: &str,
    idle_branches: &std::collections::HashSet<String>,
    has_pending_change: bool,
) -> bool {
    has_pending_change || !idle_branches.contains(branch)
}

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
    .unwrap_or(false)
}

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

macro_rules! turn_start_events {
    () => {
        "'MessageReceived','TriggerStarted','ChildThreadCompleted',\
         'ContinuationRequested','ContinuationStarted','CodingAgentUserMessageSent'"
    };
}

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

#[cfg(test)]
mod worktrees_scan_prefix_tests {
    use super::*;

    #[test]
    fn the_scan_prefix_rejects_a_sibling_directory_sharing_its_name() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path();
        let prefix = worktrees_scan_prefix(workspace);
        let own = worktrees_dir(workspace).join("thread-1");
        let sibling =
            PathBuf::from(format!("{}-old", worktrees_dir(workspace).display())).join("thread-1");
        assert!(prefix.ends_with(std::path::MAIN_SEPARATOR));
        assert!(own.to_string_lossy().starts_with(&prefix));
        assert!(
            !sibling.to_string_lossy().starts_with(&prefix),
            "a sibling like worktrees-old must not match the prefix"
        );
    }
}
