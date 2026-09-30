//! Work on a coding-agent branch that no change carries (ADR 0328).
//!
//! Three moments can leave it: a Stop with no live session to propose, an
//! archive, and anything an engine death cut short. The first proposes the
//! work as incomplete. The other two set it aside, because the user has
//! already put the thread out of the way.
//!
//! The set-aside `ChangeProposed` is a first record, never a re-sync. Once a
//! change row names the branch, neither the archive nor the boot pass emits
//! for it again. So a trigger on `ChangeProposed` hears each branch once.

use super::super::event_bus::{BusEvent, EventBus};
use super::super::git_ops::{git_cmd, main_worktree, proposal_files_for_branch};
use super::super::thread_events::{
    EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use super::super::LucidosEngine;
use super::recovery::branch_proposal_details;
use super::*;
use crate::engine::change_ops::thread_is_external_repo;
use sqlx::PgPool;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

/// A thread's branch, the repo that holds it, and its net files.
pub(super) struct BranchWork {
    pub branch_name: String,
    pub repo_root: PathBuf,
    pub files: Vec<String>,
}

/// What branch work is read from and recorded in: the store, and the two
/// repos a coding-agent branch can live in. The engine builds one per call.
pub(super) struct BranchWorkScope<'a> {
    pub pool: &'a PgPool,
    pub event_bus: &'a EventBus,
    pub lucidos_repo_root: &'a Path,
    pub workspace_root: &'a Path,
}

impl BranchWorkScope<'_> {
    /// The net work on the branch of the thread's latest session. `None` when
    /// there is none, the repo is external, or git could not answer.
    async fn thread_branch_work(&self, thread_id: Uuid) -> Option<BranchWork> {
        let session: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT payload->>'branch', payload->>'coding_agent_kind' FROM events \
             WHERE event_type = 'SessionStarted' AND thread_id = $1 \
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(thread_id)
        .fetch_optional(self.pool)
        .await
        .unwrap_or_else(|e| {
            log!("[BranchWork] session lookup for {}: {}", thread_id, e);
            None
        });
        let (Some(branch_name), kind) = session? else {
            return None;
        };
        if branch_name.is_empty() {
            return None;
        }
        // An unanswered read counts as external: that side touches nothing.
        let repo = stale_session_repo(
            kind.as_deref(),
            thread_is_external_repo(self.pool, thread_id)
                .await
                .unwrap_or(true),
            self.lucidos_repo_root,
            self.workspace_root,
        );
        let StaleSessionRepo::Owned(repo_root) = repo else {
            return None;
        };
        let files = proposal_files_for_branch(&repo_root, &branch_name).await?;
        Some(BranchWork {
            branch_name,
            repo_root,
            files,
        })
    }

    /// Set aside the net work on an archived thread's branch, when no change
    /// row names that branch. Any row, in any status, means the user already
    /// decided, so this never resurrects a discarded change. Returns whether
    /// it recorded a change.
    async fn set_aside_archived_branch_work(
        &self,
        thread_id: Uuid,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let archived: Option<bool> = sqlx::query_scalar(
            "SELECT is_coding_agent AND archive_state = 'archived' \
             FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(self.pool)
        .await?;
        if archived != Some(true) {
            return Ok(false);
        }
        let Some(work) = self.thread_branch_work(thread_id).await else {
            return Ok(false);
        };
        let changes = self.event_bus.changes_projection();
        if changes.any_for_branch(&work.branch_name).await? {
            return Ok(false);
        }
        let details = branch_proposal_details(
            self.pool,
            changes,
            thread_id,
            &work.branch_name,
            &work.repo_root,
            &work.files,
        )
        .await;
        self.event_bus
            .emit(BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ChangeProposed {
                    change_id: Uuid::new_v4().to_string(),
                    description: Some(details.description),
                    requires_restart: details.requires_restart,
                    files: work.files,
                    origin: Some(MessageOrigin::engine(EngineReason::ArchivedBranchWork)),
                    commit_sha: None,
                    branch_name: work.branch_name.clone(),
                    repo_root: work.repo_root.to_string_lossy().to_string(),
                    hardened: details.hardened,
                    incomplete: true,
                    set_aside: true,
                    path: String::new(),
                    diff: String::new(),
                },
                meta: EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    ..EventMeta::NONE
                },
            })
            .await?;
        log!(
            "[BranchWork] Set aside unproposed work on archived thread {} (branch {})",
            thread_id,
            work.branch_name
        );
        Ok(true)
    }

    /// The boot half of the archive net: archived threads whose branch still
    /// holds work no change carries. Git runs only for a candidate whose branch
    /// still exists, so the pass stays cheap on a long history. Returns how
    /// many it set aside.
    async fn set_aside_archived_branch_work_on_startup(&self) -> usize {
        let candidates: Vec<(Uuid, String)> = match sqlx::query_as(
            "SELECT t.thread_id, s.branch FROM thread_summaries t \
             JOIN LATERAL ( \
                 SELECT payload->>'branch' AS branch FROM events e \
                 WHERE e.event_type = 'SessionStarted' AND e.thread_id = t.thread_id \
                 ORDER BY e.sequence DESC LIMIT 1 \
             ) s ON TRUE \
             WHERE t.is_coding_agent AND t.archive_state = 'archived' \
               AND NOT t.coding_agent_is_external_repo \
               AND s.branch IS NOT NULL AND s.branch <> '' \
               AND NOT EXISTS (SELECT 1 FROM changes c WHERE c.branch_name = s.branch)",
        )
        .fetch_all(self.pool)
        .await
        {
            Ok(rows) => rows,
            Err(e) => {
                log!("[BranchWork] Startup candidates: {}", e);
                return 0;
            }
        };
        if candidates.is_empty() {
            return 0;
        }
        let mut branches = local_branches(self.lucidos_repo_root).await;
        branches.extend(local_branches(self.workspace_root).await);
        let mut set_aside = 0usize;
        for (thread_id, branch) in candidates {
            if !branches.contains(&branch) {
                continue;
            }
            match self.set_aside_archived_branch_work(thread_id).await {
                Ok(true) => set_aside += 1,
                Ok(false) => {}
                Err(e) => log!(
                    "[BranchWork] Failed to set aside work on thread {}: {}",
                    thread_id,
                    e
                ),
            }
        }
        if set_aside > 0 {
            log!(
                "[BranchWork] Set aside unproposed work on {} archived thread(s)",
                set_aside
            );
        }
        set_aside
    }
}

impl LucidosEngine {
    fn branch_work_scope<'a>(&'a self, lucidos_repo_root: &'a Path) -> BranchWorkScope<'a> {
        BranchWorkScope {
            pool: self.pool(),
            event_bus: &self.event_bus,
            lucidos_repo_root,
            workspace_root: self.workspace_path(),
        }
    }

    /// Propose what a user Stop left on the branch when no live session was
    /// there to do it. A Stop cut the turn short, so the change is incomplete.
    pub(crate) async fn propose_stopped_work(&self, thread_id: Uuid, actor: Option<MessageOrigin>) {
        let lucidos_repo_root = main_worktree().await;
        let Some(work) = self
            .branch_work_scope(&lucidos_repo_root)
            .thread_branch_work(thread_id)
            .await
        else {
            return;
        };
        match self
            .propose_branch_work(
                thread_id,
                &work.branch_name,
                &work.repo_root,
                &work.files,
                actor,
                true,
            )
            .await
        {
            Ok(_) => self.broadcast_changes_updated().await,
            Err(e) => log!(
                "[BranchWork] Failed to propose stopped work on {}: {}",
                work.branch_name,
                e
            ),
        }
    }

    /// The archive half of the net, for one thread. See
    /// [`BranchWorkScope::set_aside_archived_branch_work`].
    pub(crate) async fn set_aside_archived_branch_work(
        &self,
        thread_id: Uuid,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let lucidos_repo_root = main_worktree().await;
        let set_aside = self
            .branch_work_scope(&lucidos_repo_root)
            .set_aside_archived_branch_work(thread_id)
            .await?;
        if set_aside {
            self.broadcast_changes_updated().await;
        }
        Ok(set_aside)
    }

    /// The boot half of the net. One `ChangesUpdated` covers the whole pass.
    pub async fn set_aside_archived_branch_work_on_startup(&self) {
        let lucidos_repo_root = main_worktree().await;
        let set_aside = self
            .branch_work_scope(&lucidos_repo_root)
            .set_aside_archived_branch_work_on_startup()
            .await;
        if set_aside > 0 {
            self.broadcast_changes_updated().await;
        }
    }
}

/// The repo's local branch names. An unanswered git call yields none, which
/// only skips candidates: this pass never deletes anything.
async fn local_branches(repo_root: &Path) -> HashSet<String> {
    match git_cmd(
        &["for-each-ref", "--format=%(refname:short)", "refs/heads/"],
        repo_root,
    )
    .await
    {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
        _ => HashSet::new(),
    }
}

/// Run the archive net: set aside unproposed branch work after every
/// `ThreadArchived`, outside the archive request so it stays fast.
pub fn spawn_archive_net(engine: Arc<LucidosEngine>) -> tokio::task::JoinHandle<()> {
    let rx = engine.event_bus.subscribe();
    tokio::spawn(async move {
        let stream = BroadcastStream::new(rx);
        tokio::pin!(stream);
        // A lag skips those archives here. The boot pass catches them later.
        while let Some(result) = stream.next().await {
            let emitted = match result {
                Ok(e) => e,
                Err(BroadcastStreamRecvError::Lagged(n)) => {
                    log!("[BranchWork] Broadcast lagged by {} events", n);
                    continue;
                }
            };
            let BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ThreadArchived,
                ..
            } = &emitted.typed
            else {
                continue;
            };
            let (engine, thread_id) = (engine.clone(), *thread_id);
            tokio::spawn(async move {
                if let Err(e) = engine.set_aside_archived_branch_work(thread_id).await {
                    log!(
                        "[BranchWork] Failed to set aside work on thread {}: {}",
                        thread_id,
                        e
                    );
                }
            });
        }
    })
}

#[cfg(test)]
#[path = "orphaned_branch_work_tests.rs"]
mod tests;
