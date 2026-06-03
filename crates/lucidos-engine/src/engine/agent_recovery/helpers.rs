//! Recovery reason constants and free helper functions used across the
//! agent-recovery phases. Re-exported from mod.rs so existing
//! `agent_recovery::X` paths keep resolving.

use super::super::event_bus::{BusEvent, EventBus};
use super::super::git_ops::git_cmd;
use super::super::thread_events::{EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent};
use std::path::Path;
use uuid::Uuid;


/// Re-emit `ChangeProposed` flipping the row to `incomplete: true`. Called
/// by orphan recovery for branches that were mid-CC-turn at engine restart
/// — the existing pending row was populated by per-commit emits during the
/// dying turn, so its description / files reflect work the user never
/// confirmed. Apply must require explicit confirmation; the `incomplete`
/// flag is the existing signal for that (see commit `1e8736839`).
///
/// Caller passes the already-loaded `Change` so this avoids re-querying
/// what recover_orphaned_worktrees just fetched via `list_pending`. No-op
/// when the row is already flagged.
pub(crate) async fn mark_pending_change_incomplete(
    event_bus: &EventBus,
    thread_id: Uuid,
    change: &crate::core::changes::Change,
) {
    if change.incomplete {
        return;
    }
    event_bus
        .emit_or_log(
            BusEvent::Thread {
                thread_id,
                event: ThreadEvent::ChangeProposed {
                    change_id: change.id.to_string(),
                    description: Some(change.description.clone()),
                    files: change.files.clone(),
                    requires_restart: change.requires_restart,
                    origin: Some(MessageOrigin::engine(EngineReason::OrphanRecovery)),
                    commit_sha: None,
                    branch_name: change.branch_name.clone(),
                    repo_root: change.repo_root.clone(),
                    hardened: change.hardened,
                    incomplete: true,
                    path: String::new(),
                    diff: String::new(),
                },
                meta: EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    ..EventMeta::NONE
                },
            },
            "[Recovery] ChangeProposed re-emit marking incomplete",
        )
        .await;
}

/// Tag stamped on `CodingAgentIdled.reason` when the engine surfaces a
/// mid-turn-crashed Claude Code session as "interrupted, click to continue" instead of
/// auto-resuming it. The frontend reads this constant to render the continue
/// affordance; the spawn dispatcher ignores `CodingAgentIdled` entirely so
/// the user's click is what produces the next spawn (via `ContinuationRequested`).
pub const ENGINE_RESTART_INTERRUPT_REASON: &str = "engine_restart_interrupt";

/// Tag stamped on `ContinuationRequested.reason` when the user clicks the
/// "click to continue" affordance after a mid-turn interrupt. The continue
/// endpoint emits with this reason; the spawn dispatcher classifies it as a
/// `SpawnTrigger::ContinuationRequested` and starts the next CC turn.
pub const USER_CLICKED_CONTINUE_REASON: &str = "user_clicked_continue";

/// Tag stamped on `ContinuationRequested.reason` when the user answers an
/// `AskUserQuestion` after the Claude Code subprocess has been torn down at idle.
/// `notify()` is a no-op in that window, so this signal makes the spawn
/// dispatcher boot a fresh `--resume` subprocess; the resumed CC re-runs
/// the hook, which reads the persisted answer from the DB.
pub const ANSWERED_AFTER_IDLE_REASON: &str = "answered_after_idle";

/// Tag stamped on `ContinuationRequested.reason` when the hung-subprocess watchdog
/// killed CC after silence past its inactivity limit. Same downstream
/// pipeline as `USER_CLICKED_CONTINUE_REASON` — only the boundary-exchange
/// gate (`continue_should_open_resume_exchange`) reads the reason — so the
/// user gets an automatic `--resume` without having to click Continue.
pub const AUTO_RECOVERY_AFTER_HANG_REASON: &str = "auto_recovery_after_hang";

/// Should the SpawnConsumer's `Continue` handler emit `ContinuationStarted`
/// for a `ContinuationRequested` carrying this `reason`?
///
/// `ContinuationStarted` opens a new "Resumed after engine restart" exchange
/// in the timeline. That label is only honest when the continuation is in
/// response to an actual mid-turn engine restart (user clicked Continue) or
/// an engine-driven auto-resume after a hung-subprocess watchdog fire — both
/// cases the user should see as a fresh boundary in the timeline. For
/// `answered_after_idle` the user answered an `AskUserQuestion` after CC's
/// subprocess was torn down at idle — the follow-up CC events should attach
/// to the existing `UserQuestionAsked` exchange instead of being mislabeled
/// as a recovery.
///
/// Default-deny on unknown / missing reasons: a future `ContinuationRequested`
/// reason must opt-in explicitly rather than inheriting a "Resumed after
/// engine restart" boundary by accident.
pub fn continue_should_open_resume_exchange(reason: Option<&str>) -> bool {
    matches!(
        reason,
        Some(USER_CLICKED_CONTINUE_REASON) | Some(AUTO_RECOVERY_AFTER_HANG_REASON)
    )
}

/// User message the spawn consumer hands to `run_direct_agent` when actuating
/// a `SpawnRequest::Continue`. **Must be non-empty.**
///
/// `claude --print --resume` reads stdin in stream-json mode and waits
/// indefinitely for at least one input line before emitting its `system/init`
/// event. The engine keeps the input channel open across the session lifetime,
/// so EOF never arrives on its own — without an explicit input, CC parks
/// forever, `events_rx.recv()` never resolves, and the thread sits "Running"
/// until the next engine restart tears the subprocess down.
///
/// The string mirrors the placeholder CC itself injects on `--resume` of an
/// unfinished tool_use (see `agent_session/run_session.rs` and
/// `agent_session/reconstruct.rs`), so CC ingests it as a plain user turn and
/// proceeds against the resumed conversation state. The richer recovery
/// payload (system-prompt override, pending-merge context, etc.) will replace
/// this call site later; until then this constant guarantees the non-empty
/// stdin precondition.
pub const CONTINUE_RESUME_USER_MESSAGE: &str = "Continue from where you left off.";

/// Remove a stale worktree directory and delete its branch.
/// Best-effort — failures are silently ignored since the worktree
/// will just be skipped again on next restart.
pub(crate) async fn cleanup_stale_worktree(wt_path: &Path, branch_name: &str, repo_root: &Path) {
    let Some(wt_str) = wt_path.to_str() else {
        log!(
            "[Recovery] cleanup_stale_worktree skipped (non-UTF8 path): {}",
            wt_path.display()
        );
        return;
    };
    let _ = git_cmd(&["worktree", "remove", "--force", wt_str], repo_root).await;
    let _ = git_cmd(&["branch", "-D", branch_name], repo_root).await;
}

/// Resolve which thread an orphaned `claude-code/*` worktree (found by the
/// recovery scan) should be surfaced under as a resumable Claude Code session.
/// Returns the thread that a recorded `SessionStarted` / pending change maps
/// `branch_name` to, or `None` when no thread owns the branch.
///
/// `None` means **skip**. A worktree whose originating thread is gone (its
/// events were pruned, leaving only the directory on disk) has no conversation
/// to continue. Recovery used to fabricate a fresh `Uuid` here and emit
/// `CodingAgentIdled` to it — but a thread with no projection row defaults to
/// `Chat` / `Archived` in the lifecycle gate, which rejects `CodingAgentIdled`
/// as CC-only. The emit failed on *every* engine restart (spamming the audit
/// log with `Thread lifecycle violation: 'CodingAgentIdled' is not valid for
/// Chat threads`) and never actually surfaced anything resumable. The orphaned
/// worktree is left for the worktree-cleanup worker to reclaim on its own
/// schedule instead.
///
/// Pure decision — exposed at module scope so the skip behaviour is unit-tested
/// without a live engine + git worktree (mirrors `classify_archive_decision`).
pub(crate) fn orphan_recovery_target(
    branch_to_thread: &std::collections::HashMap<String, Uuid>,
    branch_name: &str,
) -> Option<Uuid> {
    branch_to_thread.get(branch_name).copied()
}

/// Did the most recent **Claude Code** turn for this thread end cleanly?
///
/// Returns `true` iff the latest CC-channel terminal event is
/// `ResponseGenerated`. Used by stale-session recovery to decide whether a
/// freshly-proposed change should carry `incomplete: true` (mid-turn crash /
/// abort / cancel) or `incomplete: false` (clean Generated, but the per-turn
/// `propose_change` never landed — e.g. the engine died between the idle and
/// the proposal). Without this distinction the recovery flags a clean change
/// as incomplete and the Apply UI surfaces a misleading "this came from a
/// failed turn" confirm dialog.
///
/// **Channel-scoped.** A thread can mix chat-agent, trigger, and CC
/// terminal events (verified empirically: `payload->>'channel'` takes
/// values `'claude_code'`, `'trigger'`, or NULL for chat). Without
/// `payload->>'channel' = 'claude_code'` filtering, a chat
/// `ResponseGenerated` arriving after the CC subprocess died (no
/// engine-restart-recovery synthesized `ResponseAborted` because the engine
/// itself didn't crash) would shadow the CC failure and falsely report
/// clean. Stale-session recovery is CC-only — the chat agent's exit state
/// has no bearing on whether a CC branch is mid-edit.
///
/// The four `Response*` terminal events are the source of truth on the CC
/// channel; any other state (or no CC terminal row at all) means the prior
/// CC turn never produced a clean Generated, which we treat as not-clean.
///
/// Pure DB query — exposed at module scope so tests can exercise it without
/// instantiating a full `LucidosEngine`.
pub(crate) async fn last_turn_ended_cleanly(
    pool: &sqlx::PgPool,
    thread_id: Uuid,
) -> bool {
    let row: Option<String> = sqlx::query_scalar(
        "SELECT event_type FROM events \
         WHERE thread_id = $1 \
           AND event_type IN ('ResponseGenerated', 'ResponseAborted', 'ResponseCanceled', 'ResponseFailed') \
           AND payload->>'channel' = 'claude_code' \
         ORDER BY sequence DESC LIMIT 1",
    )
    .bind(thread_id)
    .fetch_optional(pool)
    .await
    .unwrap_or_else(|e| {
        // Mirrors the recovery sweep's other best-effort queries
        // (`list_pending`, `list_completed_branches` in
        // `recover_orphaned_worktrees`): log and fall through. Returning
        // None here biases the caller toward `incomplete: true`, which is
        // the safe direction — Apply will require explicit confirmation
        // rather than auto-applying possibly-mid-turn work.
        log!(
            "[Recovery] last_turn_ended_cleanly({}): {} — defaulting to not-clean (incomplete=true)",
            thread_id,
            e
        );
        None
    });
    matches!(row.as_deref(), Some("ResponseGenerated"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn orphan_recovery_target_surfaces_known_branch_and_skips_unknown() {
        let tid = Uuid::new_v4();
        let mut branch_to_thread = HashMap::new();
        branch_to_thread.insert("claude-code/known".to_string(), tid);

        // A branch a SessionStarted maps to a thread surfaces under it.
        assert_eq!(
            orphan_recovery_target(&branch_to_thread, "claude-code/known"),
            Some(tid)
        );

        // A branch with no originating thread (events pruned, worktree
        // orphaned) MUST skip — never fabricate a phantom thread. Fabricating
        // one made recovery emit `CodingAgentIdled` to a row-less thread, which
        // the lifecycle gate rejects as CC-only (row-less threads default to
        // Chat/Archived), failing on every engine restart.
        assert_eq!(
            orphan_recovery_target(&branch_to_thread, "claude-code/orphaned"),
            None
        );
    }
}

