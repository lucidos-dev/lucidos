use super::event_bus::SystemEvent;
use super::git_ops::{
    any_iframe_bundled_file_changed, full_harden_marker_state, git_cmd, git_ran_ok,
    HardenMarkerState,
};
use super::thread_events::{MessageOrigin, UnproposedReason};
use super::LucidosEngine;
use crate::core::changes::{ChangeStatus, ChangeStatusData};
use crate::engine::thread_events::CodingAgentKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

mod apply;
pub mod apply_claim;
mod discard;
#[cfg(test)]
pub(crate) use discard::{revert_on_main, RevertTarget};
mod proposal_hold;
mod propose;
mod set_aside;
pub use proposal_hold::{change_plan_context, PlanHold};
#[cfg(test)]
pub(crate) use propose::{
    branch_moved_since_last_idle, withhold_for_landing, withhold_unfinished_work, withhold_unready,
    HoldOutcome, OpenChangeAfterHold,
};
#[cfg(test)]
pub(crate) use set_aside::withdraw_for_redecision;
pub use set_aside::SET_ASIDE_APPLY_REFUSAL;
pub use {
    proposal_hold::apply_must_harden, proposal_hold::held_plan, proposal_hold::read_proposal_hold,
    proposal_hold::PlanFloor, proposal_hold::ProposalHold,
};
pub use {
    propose::emit_change_withdrawn, propose::emit_proposal_withheld,
    propose::withhold_handed_off_work, propose::HoldReentry, propose::ProposeOutcome,
};

#[path = "../change_ops_emitters.rs"]
mod emitters;
pub(crate) use emitters::APPLIED_IN_BROADCAST;
#[cfg(test)]
pub(crate) use emitters::CHANGE_APPLIED_ONCE_INDEX;

#[cfg(test)]
pub(crate) use super::git_ops::catchup_and_ff_to_main;
#[cfg(test)]
pub(crate) use super::{ApplyResult, ApplyStatus};

pub struct LiveSessionInfo {
    pub worktree_path: PathBuf,
    pub msg_tx: tokio::sync::mpsc::UnboundedSender<crate::engine::AgentUserInput>,
    /// INVARIANT: Last-activity clock for the session (epoch millis), used by the async
    /// in-place conflict-merge task as its liveness source — the same signal
    /// `apply_now` polls to abort a wedged (alive-but-silent) CC merge.
    pub last_event_at: Arc<std::sync::atomic::AtomicI64>,
}

pub struct ProposeChangeInput<'a> {
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

impl LucidosEngine {
    pub(crate) async fn live_session_info(&self, thread_id: Uuid) -> Option<LiveSessionInfo> {
        let guard = self.agent_sessions.lock().await;
        guard.get(&thread_id).and_then(|s| {
            if !s.is_live() {
                return None;
            }
            Some(LiveSessionInfo {
                worktree_path: s.worktree_path.clone()?,
                msg_tx: s.msg_tx.clone(),
                last_event_at: s.last_event_at.clone(),
            })
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedMerge {
    pub pre: String,
    pub post: String,
    pub landing: Option<String>,
}

impl AppliedMerge {
    pub fn new(pre: &str, post: &str) -> Self {
        Self {
            pre: pre.to_string(),
            post: post.to_string(),
            landing: None,
        }
    }

    pub(crate) fn landing(first_parent: &str, merge: &str) -> Self {
        Self {
            landing: Some(merge.to_string()),
            ..Self::new(first_parent, merge)
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ApplyKindContext {
    pub kind: CodingAgentKind,
    pub app_id: Option<String>,
}

impl ApplyKindContext {
    pub fn is_app(&self) -> bool {
        matches!(self.kind, CodingAgentKind::App)
    }

    /// INVARIANT: True for Lucidos-source changes (not app, not external). The
    /// implementation-plan floor ([`PlanFloor`]) applies only here: app threads
    /// don't plan, and external repos have no `docs/plans/` convention or
    /// `lucidos planned` marker. Legacy NULL `coding_agent_kind` parses to
    /// `Lucidos`, so old rows are gated, which is right: they ARE Lucidos source.
    pub fn is_lucidos_source(&self) -> bool {
        matches!(self.kind, CodingAgentKind::Lucidos)
    }
}

impl super::LucidosEngine {
    pub async fn maybe_emit_app_ui_refresh(
        &self,
        ctx: &ApplyKindContext,
        files: &[String],
        actor: Option<&MessageOrigin>,
    ) {
        if !ctx.is_app() {
            return;
        }
        let Some(app_id) = ctx.app_id.as_ref() else {
            return;
        };
        if !any_iframe_bundled_file_changed(files, app_id) {
            return;
        }
        self.event_bus
            .emit_or_log(
                crate::engine::event_bus::BusEvent::System(SystemEvent::AppUiRefreshRequested {
                    app_id: app_id.clone(),
                    actor: actor.cloned(),
                }),
                "[Changes] AppUiRefreshRequested",
            )
            .await;
    }
}

/// INVARIANT: Read `coding_agent_kind` + `app_id` for a thread. App threads encode the
/// app id as the last segment of `coding_agent_folder` (`<ws>/data/apps/<id>/`)
/// — kept out of `thread_summaries` as its own column per §4.6 of the design
/// plan, so we recover it from the folder string here.
pub async fn load_apply_kind_context(
    pool: &sqlx::PgPool,
    thread_id: Option<Uuid>,
) -> ApplyKindContext {
    let Some(tid) = thread_id else {
        return ApplyKindContext::default();
    };
    let row = sqlx::query_as::<_, (Option<String>, Option<String>)>(
        "SELECT coding_agent_kind, coding_agent_folder \
         FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(tid)
    .fetch_optional(pool)
    .await;
    let (kind_str, folder) = match row {
        Ok(Some((k, f))) => (k, f),
        _ => return ApplyKindContext::default(),
    };
    let kind = match kind_str.as_deref() {
        Some(s) => CodingAgentKind::parse(s),
        None => CodingAgentKind::Lucidos,
    };
    let app_id = if matches!(kind, CodingAgentKind::App) {
        folder
            .as_deref()
            .and_then(|f| f.trim_end_matches('/').rsplit('/').next())
            .map(str::to_string)
    } else {
        None
    };
    ApplyKindContext { kind, app_id }
}

pub fn now_epoch_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub const MERGE_OWNED_BY_RESOLVER_MESSAGE: &str =
    "A merge resolution is already in flight for this change. The coding-agent \
     session owns the merge until it finishes; the change applies automatically \
     when it does.";

#[derive(Debug, PartialEq, Eq)]
pub enum MergeOwnership {
    /// INVARIANT: A conflict-resolution session holds the merge. Every other apply path
    /// must refuse: no fast-forward, no worktree reset, no `ChangeApplied`.
    ResolverOwnsIt,
    CallerMayMerge,
}

/// INVARIANT: Decide whether an in-flight merge-conflict resolution owns a change's merge.
///
/// The durable half is the `MergeConflictDetected` pairing: every tier opens it
/// through `start_merge_and_get_prompt`, and it closes on `ChangeApplied` /
/// `ChangeApplyFailed` / `MergeResolutionCleared`. That is what was missing on
/// 2026-08-11, when a second `apply_change` for the same change found the
/// resolver's own live session, fast-forwarded `main` at step 2 of the
/// resolver's 5-step prompt, and then reset the resolver's worktree under it.
/// The only re-entrancy guard at the time was the in-memory
/// `change_claim` flag, which the detached Tier-2 and Tier-3 merge
/// spawns never set.
///
/// `resolver_present` is the second half, and it is deliberately about THE
/// resolver, not about any live session. A live session that happens to share
/// the thread is not evidence that anyone is resolving: a pairing stranded by a
/// crash plus a later, unrelated turn on the same thread would otherwise refuse
/// every Apply for the length of that turn, over and over. So the term is
/// the change id of `AgentSession::conflict`, set where each tier binds its
/// resolution to a session.
///
/// It is also what keeps this from wedging Apply outright. An engine restart
/// empties `agent_sessions`, so a stranded pairing names no resolver and does
/// NOT refuse: the apply falls through to the ordinary tiers, which is exactly
/// the recovery that exists today.
///
/// `pairing_open` is `None` when the projection query itself failed. That is an
/// UNKNOWN, and per `.claude/rules/rust.md` an unknown must not authorize the
/// destructive direction: merging under a working resolver is what destroys
/// something, refusing costs a retry. So an unknown refuses, but only while a
/// resolver is actually present, since with nobody resolving there is nobody to
/// own the merge whatever the query would have said.
pub(crate) fn decide_merge_ownership(
    pairing_open: Option<bool>,
    resolver_present: bool,
) -> MergeOwnership {
    if resolver_present && pairing_open != Some(false) {
        MergeOwnership::ResolverOwnsIt
    } else {
        MergeOwnership::CallerMayMerge
    }
}

impl super::LucidosEngine {
    pub(crate) async fn merge_ownership_for_change(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
    ) -> MergeOwnership {
        let resolver_present = self.live_resolver_change(thread_id).await == Some(change_id);
        if !resolver_present {
            return MergeOwnership::CallerMayMerge;
        }
        decide_merge_ownership(
            self.conflict_pairing_open_or_unknown(thread_id, change_id)
                .await,
            true,
        )
    }

    pub async fn merge_ownership_for_thread(&self, thread_id: Uuid) -> MergeOwnership {
        let Some(change_id) = self.live_resolver_change(thread_id).await else {
            return MergeOwnership::CallerMayMerge;
        };
        decide_merge_ownership(
            self.conflict_pairing_open_or_unknown(thread_id, change_id)
                .await,
            true,
        )
    }

    pub(crate) async fn conflict_pairing_open_or_unknown(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
    ) -> Option<bool> {
        match self
            .changes()
            .conflict_pairing_open(thread_id, change_id)
            .await
        {
            Ok(open) => Some(open),
            Err(e) => {
                log_legacy!(
                    "[Changes] conflict_pairing_open({}, {}) failed: {}. Treating the resolution as in flight",
                    thread_id,
                    change_id,
                    e
                );
                None
            }
        }
    }

    async fn live_resolver_change(&self, thread_id: Uuid) -> Option<Uuid> {
        let guard = self.agent_sessions.lock().await;
        guard
            .get(&thread_id)
            .filter(|s| s.is_live())
            .and_then(|s| s.conflict)
            .and_then(crate::engine::types::ConflictBinding::change_id)
    }

    pub async fn bind_session_to_conflict_resolution(&self, thread_id: Uuid, change_id: Uuid) {
        bind_in_place_conflict_resolution(
            &mut *self.agent_sessions.lock().await,
            thread_id,
            change_id,
        );
    }
}

pub fn bind_in_place_conflict_resolution(
    sessions: &mut std::collections::HashMap<Uuid, crate::engine::AgentSession>,
    thread_id: Uuid,
    change_id: Uuid,
) {
    if let Some(s) = sessions.get_mut(&thread_id) {
        s.conflict = Some(crate::engine::types::ConflictBinding::InPlace { change_id });
    }
}

pub async fn branch_is_hardened(
    pool: &sqlx::PgPool,
    changes: &crate::core::changes_projection::ChangesProjection,
    repo_root: &Path,
    branch_name: &str,
) -> bool {
    match full_harden_marker_state(pool, repo_root, branch_name).await {
        HardenMarkerState::Fresh | HardenMarkerState::Stale => true,
        HardenMarkerState::Missing => match changes.get_open_by_branch(branch_name).await {
            Ok(Some(c)) => c.hardened,
            Ok(None) => false,
            Err(e) => {
                log_legacy!(
                    "[Changes] branch_is_hardened: get_open_by_branch({}): {}; \
                     treating as not hardened (safer than letting a stale check pass)",
                    branch_name,
                    e
                );
                false
            }
        },
    }
}

pub async fn thread_is_external_repo(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar::<_, bool>(
        "SELECT coding_agent_is_external_repo FROM thread_summaries WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .map(|opt| opt.unwrap_or(false))
}

pub(crate) async fn reset_worktree_to_main_after_apply(
    worktree_path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let status = git_cmd(&["status", "--porcelain"], worktree_path)
        .await
        .map_err(|e| {
            format!(
                "git status failed in worktree {}: {}",
                worktree_path.display(),
                e
            )
        })?;
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

    reset_hard_to_main_and_clean(worktree_path).await
}

pub(crate) async fn reset_worktree_to_main_after_discard(
    worktree_path: &Path,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    reset_hard_to_main_and_clean(worktree_path).await
}

async fn reset_hard_to_main_and_clean(
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

    // INVARIANT: `git clean -fd` removes untracked files. Untracked-but-gitignored build
    // outputs (`target/`, `node_modules/`) are preserved — `clean -fdx` would
    // touch them, which we don't want.
    let clean = git_cmd(&["clean", "-fd"], worktree_path)
        .await
        .map_err(|e| {
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

pub(crate) async fn revert_with_shas(
    repo_root: &Path,
    pre_sha: &str,
    post_sha: &str,
    branch_name: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let log_out = git_cmd(&["log", "--pretty=%P", "-1", post_sha], repo_root)
        .await
        .map_err(|e| format!("cannot read the parents of {}: {}", post_sha, e))?;
    if !log_out.status.success() {
        return Err(format!(
            "cannot read the parents of {}: {}",
            post_sha,
            String::from_utf8_lossy(&log_out.stderr).trim()
        )
        .into());
    }
    let parents: Vec<String> = String::from_utf8_lossy(&log_out.stdout)
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();

    if parents.len() > 1 {
        let parent_num = match parents.iter().position(|p| p == pre_sha) {
            Some(i) => i + 1,
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
        revert_merge(repo_root, post_sha, parent_num).await
    } else {
        let squashed = squash_range(repo_root, pre_sha, post_sha).await?;
        match git_cmd(&["revert", "--no-commit", &squashed], repo_root).await {
            Ok(o) if o.status.success() => {
                let msg = format!("Revert changes from {}", branch_name);
                git_ran_ok(&["commit", "-m", &msg], repo_root)
                    .await
                    .map_err(|e| format!("Failed to commit revert: {}", e).into())
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

/// INVARIANT: Revert exactly one landing merge: the change it landed, and nothing that
/// landed beside it. Its first parent is `main` before the landing.
pub(crate) async fn revert_landing_merge(
    repo_root: &Path,
    landing_merge_sha: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    revert_merge(repo_root, landing_merge_sha, 1).await
}

async fn revert_merge(
    repo_root: &Path,
    merge_sha: &str,
    parent_num: usize,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let parent = parent_num.to_string();
    match git_cmd(
        &["revert", merge_sha, "-m", &parent, "--no-edit"],
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
}

async fn squash_range(
    repo_root: &Path,
    pre_sha: &str,
    post_sha: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let tree = format!("{post_sha}^{{tree}}");
    let msg = format!("Squash of {pre_sha}..{post_sha}");
    let out = git_cmd(
        &["commit-tree", &tree, "-p", pre_sha, "-m", &msg],
        repo_root,
    )
    .await
    .map_err(|e| format!("Revert error: cannot squash {pre_sha}..{post_sha}: {e}"))?;
    let sha = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || sha.is_empty() {
        return Err(format!(
            "Revert error: cannot squash {pre_sha}..{post_sha}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )
        .into());
    }
    Ok(sha)
}

#[cfg(test)]
#[path = "../change_ops_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../change_ops_engine_origin_stamping_tests.rs"]
mod engine_origin_stamping;

#[cfg(test)]
#[path = "../change_ops_withheld_work_tests.rs"]
mod withheld_work_tests;
