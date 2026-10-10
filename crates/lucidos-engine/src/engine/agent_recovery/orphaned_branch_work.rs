//! Work on a coding-agent branch that no change carries (ADR 0328).
//!
//! Four moments can leave it:
//! - A Stop with no live session to propose proposes the work as incomplete,
//!   unless the proposal hold keeps it (ADR 0397, ADR 0416).
//! - A canceled event wait that held the idle proposal (ADR 0395) proposes it
//!   as the idle would have.
//! - An archive, and anything an engine death cut short, set it aside, because
//!   the user has already put the thread out of the way.
//!
//! The set-aside `ChangeProposed` is a first record, never a re-sync. Once a
//! change row names the branch, neither the archive nor the boot pass emits
//! for it again. So a trigger on `ChangeProposed` hears each branch once.

use super::super::change_ops::{read_proposal_hold, ProposalHold, ProposeOutcome};
use super::super::event_bus::{BusEvent, EventBus};
use super::super::git_ops::{git_cmd, main_worktree, proposal_files_for_branch};
use super::super::thread_events::{
    EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent, UnproposedReason,
};
use super::super::LucidosEngine;
use super::recovery::branch_proposal_details;
use super::*;
use crate::core::changes::ChangeStatus;
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

    /// The work a canceled event wait held back from the idle proposal, and
    /// how its turn ended (ADR 0395). `None` unless the thread is an idle,
    /// unarchived coding-agent thread with no wait left, whose last turn ended
    /// in a way a sweep settles.
    async fn work_a_canceled_wait_held(&self, thread_id: Uuid) -> Option<(BranchWork, TurnEnd)> {
        let released: Option<bool> = sqlx::query_scalar(
            "SELECT COALESCE(is_coding_agent AND archive_state <> 'archived' \
                    AND status = 'idle' AND live_event_wait_count = 0, FALSE) \
             FROM thread_summaries WHERE thread_id = $1",
        )
        .bind(thread_id)
        .fetch_optional(self.pool)
        .await
        .unwrap_or_else(|e| {
            log!("[BranchWork] wait-release lookup for {}: {}", thread_id, e);
            None
        });
        if released != Some(true) {
            return None;
        }
        let turn_end = last_turn_end(self.pool, thread_id).await?;
        Some((self.thread_branch_work(thread_id).await?, turn_end))
    }

    /// Set aside the net work on an archived thread's branch, when no change
    /// row names that branch. Any row but a withdrawn one means the user
    /// already decided, so this never resurrects a discarded change. Returns
    /// whether it recorded a change.
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
        if changes.branch_has_decided_change(&work.branch_name).await? {
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
               AND NOT EXISTS (SELECT 1 FROM changes c \
                               WHERE c.branch_name = s.branch AND c.status <> $1)",
        )
        .bind(ChangeStatus::Withdrawn)
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

    /// Withhold what a user Stop left on the branch when no live session was
    /// there to do it. A Stop cut the turn short, so it proposes nothing
    /// (ADR 0400).
    pub(crate) async fn withhold_stopped_work(
        &self,
        thread_id: Uuid,
        actor: Option<MessageOrigin>,
    ) {
        let lucidos_repo_root = main_worktree().await;
        let Some(work) = self
            .branch_work_scope(&lucidos_repo_root)
            .thread_branch_work(thread_id)
            .await
        else {
            return;
        };
        self.propose_thread_work(thread_id, work, actor, false)
            .await;
    }

    /// Propose the work a canceled event wait held back from the idle
    /// proposal, credited to whoever canceled it. A delivery or an expiry
    /// re-opens the thread instead, and that turn's idle proposes.
    ///
    /// A waiting child's card for its parent defers to this, so the card can
    /// list the change. It is sent after the proposal, whether that landed or
    /// not.
    pub(crate) async fn propose_work_a_canceled_wait_held(
        &self,
        thread_id: Uuid,
        canceled_by: Option<MessageOrigin>,
    ) {
        let card_awaits = self.event_bus.held_card_awaits_proposal(thread_id).await;
        let lucidos_repo_root = main_worktree().await;
        if let Some((work, turn_end)) = self
            .branch_work_scope(&lucidos_repo_root)
            .work_a_canceled_wait_held(thread_id)
            .await
        {
            self.propose_thread_work(thread_id, work, canceled_by, turn_end == TurnEnd::Finished)
                .await;
        }
        if card_awaits {
            self.event_bus
                .send_held_card_after_proposal(thread_id)
                .await;
        }
    }

    async fn propose_thread_work(
        &self,
        thread_id: Uuid,
        work: BranchWork,
        origin: Option<MessageOrigin>,
        finished: bool,
    ) {
        match self
            .propose_branch_work(
                thread_id,
                &work.branch_name,
                &work.repo_root,
                &work.files,
                origin,
                finished,
            )
            .await
        {
            Ok(ProposeOutcome::Proposed(_)) => self.broadcast_changes_updated().await,
            Ok(ProposeOutcome::Held(_) | ProposeOutcome::Unfinished) => {}
            Err(e) => log!(
                "[BranchWork] Failed to propose work on {}: {}",
                work.branch_name,
                e
            ),
        }
    }

    /// Why nothing was proposed for the thread's branch work, when the
    /// proposal hold is the reason. `None` when there is no work or nothing
    /// holds it.
    pub(crate) async fn branch_work_hold(&self, thread_id: Uuid) -> Option<ProposalHold> {
        let lucidos_repo_root = main_worktree().await;
        let work = self
            .branch_work_scope(&lucidos_repo_root)
            .thread_branch_work(thread_id)
            .await?;
        read_proposal_hold(
            self.pool(),
            thread_id,
            &work.repo_root,
            &work.branch_name,
            &work.files,
        )
        .await
    }

    /// Whether the thread rests with work a turn end held for
    /// `hardening_missing`: exactly what the Not ready strip offers **Harden**
    /// for. It reads the recorded reason, never a fresh verdict, so a stopped
    /// turn is never resumed as a hardening run. An unanswered read is a no.
    pub(crate) async fn work_awaits_hardening(&self, thread_id: Uuid) -> bool {
        let held: Result<bool, sqlx::Error> = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM thread_summaries WHERE thread_id = $1 \
               AND coding_agent_change_state = $2 \
               AND coding_agent_unproposed_reason = $3)",
        )
        .bind(thread_id)
        .bind(crate::engine::thread_lifecycle::ChangeStateKind::Unproposed.as_str())
        .bind(UnproposedReason::HardeningMissing.as_str())
        .fetch_one(self.pool())
        .await;
        let unsettled =
            crate::core::changes::unsettled_thread_ids(self.pool(), std::iter::once(thread_id))
                .await;
        match (held, unsettled) {
            (Ok(held), Ok(unsettled)) => held && unsettled.is_empty(),
            (Err(e), _) => {
                log!("[BranchWork] change-state read for {}: {}", thread_id, e);
                false
            }
            (_, Err(e)) => {
                log!("[BranchWork] unsettled read for {}: {}", thread_id, e);
                false
            }
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

/// The bus moments after which a thread's branch work may need recording.
#[derive(Debug, Clone, PartialEq)]
enum BranchWorkMoment {
    Archived,
    /// Carries who canceled the wait.
    WaitCanceled(Option<MessageOrigin>),
}

impl BranchWorkMoment {
    fn of(event: &ThreadEvent, meta: &EventMeta) -> Option<Self> {
        match event {
            ThreadEvent::ThreadArchived => Some(Self::Archived),
            ThreadEvent::EventWaitCanceled { cause, .. } if cause.leaves_thread_open() => {
                Some(Self::WaitCanceled(meta.actor.clone()))
            }
            _ => None,
        }
    }
}

/// Run the branch-work net, outside the request that caused each moment so it
/// stays fast: set aside unproposed work after every `ThreadArchived`, and
/// propose held work after an `EventWaitCanceled` that left the thread open.
pub fn spawn_branch_work_net(engine: Arc<LucidosEngine>) -> tokio::task::JoinHandle<()> {
    let rx = engine.event_bus.subscribe();
    tokio::spawn(async move {
        let stream = BroadcastStream::new(rx);
        tokio::pin!(stream);
        // A lag skips those moments here. The boot passes catch them later.
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
                event,
                meta,
            } = &emitted.typed
            else {
                continue;
            };
            let Some(moment) = BranchWorkMoment::of(event, meta) else {
                continue;
            };
            let (engine, thread_id) = (engine.clone(), *thread_id);
            tokio::spawn(async move {
                match moment {
                    BranchWorkMoment::Archived => {
                        if let Err(e) = engine.set_aside_archived_branch_work(thread_id).await {
                            log!(
                                "[BranchWork] Failed to set aside work on thread {}: {}",
                                thread_id,
                                e
                            );
                        }
                    }
                    BranchWorkMoment::WaitCanceled(canceled_by) => {
                        engine
                            .propose_work_a_canceled_wait_held(thread_id, canceled_by)
                            .await;
                    }
                }
            });
        }
    })
}

#[cfg(test)]
#[path = "orphaned_branch_work_tests.rs"]
mod tests;
