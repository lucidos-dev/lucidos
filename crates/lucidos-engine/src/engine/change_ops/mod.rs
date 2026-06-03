use super::agent_session::CodingAgentKind;
use super::event_bus::SystemEvent;
use super::git_ops::{
    any_iframe_bundled_file_changed, git_cmd, harden_marker_state, HardenMarkerState,
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
#[path = "../change_ops_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../change_ops_engine_origin_stamping_tests.rs"]
mod engine_origin_stamping;
