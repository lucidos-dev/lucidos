use super::agent_session::CodingAgentKind;
use super::event_bus::SystemEvent;
use super::git_ops::{
    any_iframe_bundled_file_changed, git_cmd, git_ran_ok, harden_marker_state, HardenMarkerState,
};
use super::thread_events::MessageOrigin;
use super::LucidosEngine;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use uuid::Uuid;

mod apply;
mod discard;
mod propose;

#[path = "../change_ops_emitters.rs"]
mod emitters;

// Test-only re-exports — `change_ops_tests.rs` reaches these via `use super::*`.
// Non-test callers in the child modules import them directly.
#[cfg(test)]
pub(crate) use super::git_ops::catchup_and_ff_to_main;
#[cfg(test)]
pub(crate) use super::{ApplyResult, ApplyStatus};

/// Information about a live agent session, sufficient for in-place merge.
pub(crate) struct LiveSessionInfo {
    pub worktree_path: PathBuf,
    pub idle_notify: Arc<tokio::sync::Notify>,
    pub msg_tx: tokio::sync::mpsc::UnboundedSender<crate::engine::AgentUserInput>,
    /// Last-activity clock for the session (epoch millis), used by the async
    /// in-place conflict-merge task as its liveness source — the same signal
    /// `apply_now` polls to abort a wedged (alive-but-silent) CC merge.
    pub last_event_at: Arc<std::sync::atomic::AtomicI64>,
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
///
/// Separate from the user-facing [`crate::runtime::CodingAgent`] enum
/// (which identifies *which* backend a session uses); this trait is the
/// engine-internal abstraction over the operations `change_ops` needs to
/// invoke on a coding-agent runtime (apply, hardening, merge, resume).
pub(crate) trait CodingAgentChangeOps: Send + Sync {
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

/// Resolved coding-agent-kind context for a change's owning thread. Read
/// once at the start of `apply_change` so every downstream branch (hardening
/// gate, restart gating, AppUiRefresh emit) uses the same values.
#[derive(Clone, Debug, Default)]
pub(crate) struct ApplyKindContext {
    pub kind: CodingAgentKind,
    pub app_id: Option<String>,
}

impl ApplyKindContext {
    pub fn is_app(&self) -> bool {
        matches!(self.kind, CodingAgentKind::App)
    }

    /// True for Lucidos-source changes (not app, not external). The
    /// implementation-plan floor applies only here: app threads don't plan,
    /// and external repos have no `docs/plans/` convention or `lucidos planned`
    /// marker. Legacy NULL `coding_agent_kind` parses to `Lucidos`, so old
    /// rows are gated — correct, since they ARE Lucidos-source.
    pub fn is_lucidos_source(&self) -> bool {
        matches!(self.kind, CodingAgentKind::Lucidos)
    }
}

impl super::LucidosEngine {
    /// Emit `AppUiRefreshRequested` when an app change's merged files
    /// include anything the iframe bundles. Call right after each
    /// `emit_change_applied` success in `apply_change` so open app iframes
    /// reload to pick up CC's edits.
    pub(crate) async fn maybe_emit_app_ui_refresh(
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

/// Read `coding_agent_kind` + `app_id` for a thread. App threads encode the
/// app id as the last segment of `coding_agent_folder` (`<ws>/data/apps/<id>/`)
/// — kept out of `thread_summaries` as its own column per §4.6 of the design
/// plan, so we recover it from the folder string here.
pub(crate) async fn load_apply_kind_context(
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

/// Current time as epoch milliseconds (i64). Used for liveness tracking.
pub(crate) fn now_epoch_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// The message a refused apply carries back to its caller (the HTTP handler,
/// the Apply-All driver, the `apply_change` LLM tool). Deliberately says the
/// change still lands on its own, because it does: the resolution session's
/// completion is what applies it.
pub(crate) const MERGE_OWNED_BY_RESOLVER_MESSAGE: &str =
    "A merge resolution is already in flight for this change. The coding-agent \
     session owns the merge until it finishes; the change applies automatically \
     when it does.";

/// Who is allowed to move `main` for a change right now.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum MergeOwnership {
    /// A conflict-resolution session holds the merge. Every other apply path
    /// must refuse: no fast-forward, no worktree reset, no `ChangeApplied`.
    ResolverOwnsIt,
    /// Nobody is resolving. The caller may run the normal apply tiers.
    CallerMayMerge,
}

/// Decide whether an in-flight merge-conflict resolution owns a change's merge.
///
/// The durable half is the `MergeConflictDetected` pairing: every tier opens it
/// through `start_merge_and_get_prompt`, and it closes on `ChangeApplied` /
/// `ChangeApplyFailed` / `MergeResolutionCleared`. That is what was missing on
/// 2026-08-11, when a second `apply_change` for the same change found the
/// resolver's own live session, fast-forwarded `main` at step 2 of the
/// resolver's 5-step prompt, and then reset the resolver's worktree under it.
/// The only re-entrancy guard at the time was the in-memory
/// `apply_now_in_progress` flag, which the detached Tier-2 and Tier-3 merge
/// spawns never set.
///
/// `resolver_present` is the second half, and it is deliberately about THE
/// resolver, not about any live session. A live session that happens to share
/// the thread is not evidence that anyone is resolving: a pairing stranded by a
/// crash plus a later, unrelated turn on the same thread would otherwise refuse
/// every Apply for the length of that turn, over and over. So the term is
/// `AgentSession::conflict_change_id == Some(change_id)`, set where each tier
/// binds its resolution to a session.
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
    /// Change-scoped [`decide_merge_ownership`]: is THIS change's resolution
    /// still in flight? Used by `apply_change_inner` before it touches git.
    pub(crate) async fn merge_ownership_for_change(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
    ) -> MergeOwnership {
        let resolver_present = self.live_resolver_change(thread_id).await == Some(change_id);
        if !resolver_present {
            // Skip the query when nobody is resolving: the answer cannot change
            // the verdict, and Apply is on the user's click path.
            return MergeOwnership::CallerMayMerge;
        }
        decide_merge_ownership(
            self.conflict_pairing_open_or_unknown(thread_id, change_id)
                .await,
            true,
        )
    }

    /// Thread-scoped twin of [`Self::merge_ownership_for_change`], for callers
    /// that act on a thread rather than a known change (`apply_now`). Asks the
    /// live session which change it is resolving, then checks that pairing, so
    /// an unrelated open pairing on the same thread cannot mislabel it.
    pub(crate) async fn merge_ownership_for_thread(&self, thread_id: Uuid) -> MergeOwnership {
        let Some(change_id) = self.live_resolver_change(thread_id).await else {
            return MergeOwnership::CallerMayMerge;
        };
        decide_merge_ownership(
            self.conflict_pairing_open_or_unknown(thread_id, change_id)
                .await,
            true,
        )
    }

    /// The durable half: `Some(open)`, or `None` when the query could not run.
    async fn conflict_pairing_open_or_unknown(
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
                log!(
                    "[Changes] conflict_pairing_open({}, {}) failed: {}. Treating the resolution as in flight",
                    thread_id,
                    change_id,
                    e
                );
                None
            }
        }
    }

    /// The change a live session on this thread is currently resolving, if any.
    /// `is_live` matters as much as the binding: a phantom or exited session is
    /// nobody working (see `AgentSession::is_live`).
    async fn live_resolver_change(&self, thread_id: Uuid) -> Option<Uuid> {
        let guard = self.agent_sessions.lock().await;
        guard
            .get(&thread_id)
            .filter(|s| s.is_live())
            .and_then(|s| s.conflict_change_id)
    }

    /// Record that this thread's live session is carrying `change_id`'s
    /// conflict resolution. Used by the Tier-1 in-place merge, whose session
    /// predates the resolution; Tier 2 and Tier 3 set the same field at session
    /// registration instead. A missing session is a no-op: the merge prompt is
    /// about to fail to send anyway, and the guard falls back to "nobody is
    /// resolving", which is true.
    pub(crate) async fn bind_session_to_conflict_resolution(
        &self,
        thread_id: Uuid,
        change_id: Uuid,
    ) {
        let mut guard = self.agent_sessions.lock().await;
        if let Some(s) = guard.get_mut(&thread_id) {
            s.conflict_change_id = Some(change_id);
        }
    }
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
        HardenMarkerState::Missing => match changes.get_pending_by_branch(branch_name).await {
            Ok(Some(c)) => c.hardened,
            Ok(None) => false,
            Err(e) => {
                log!(
                    "[Changes] branch_is_hardened: get_pending_by_branch({}): {} — \
                     treating as not hardened (safer than letting a stale check pass)",
                    branch_name,
                    e
                );
                false
            }
        },
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

/// Revert commits identified by pre/post merge SHAs.
/// Handles both merge commits (using correct -m parent) and fast-forwarded ranges.
pub(crate) async fn revert_with_shas(
    repo_root: &Path,
    pre_sha: &str,
    post_sha: &str,
    branch_name: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // A parent list nobody could read is NOT "this is a single-parent commit".
    // Swallowing the failure sends a merge commit down the range-revert arm
    // below. That then fails with a confusing error, about a question git was
    // never asked. `git_cmd` returns `Err` on a spawn failure and on its 30s
    // timeout, and a non-zero exit means the sha did not resolve.
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
                // The reverted content is staged at this point, so a failed
                // commit leaves the repo root holding it. The user acts on
                // this message, so it carries git's own reason rather than a
                // bare "failed".
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

#[cfg(test)]
#[path = "../change_ops_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../change_ops_engine_origin_stamping_tests.rs"]
mod engine_origin_stamping;
