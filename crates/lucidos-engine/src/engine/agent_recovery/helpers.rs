//! Recovery reason constants and free helper functions used across the
//! agent-recovery phases. Re-exported from mod.rs so existing
//! `agent_recovery::X` paths keep resolving.

use super::super::event_bus::{BusEvent, EventBus};
use super::super::git_ops::git_cmd;
use super::super::thread_events::{
    EngineReason, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use std::path::{Path, PathBuf};
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

/// Tag stamped on `ContinuationRequested.reason` when the backend ended a turn
/// on a TRANSIENT upstream API failure (its own `API Error: …` message, e.g. a
/// connection closed mid-response) and the engine resumes the session instead of
/// leaving the thread dead behind a red dot.
///
/// This closes an asymmetry, not a new policy: when the same network failure
/// manifests as SILENCE, the hung-subprocess watchdog already kills the
/// subprocess and auto-resumes via `AUTO_RECOVERY_AFTER_HANG_REASON`. Only the
/// case where the backend notices the drop first, and reports it, had no
/// recovery, so two unattended nightly runs sat dead for four and eight hours on
/// 2026-08-04 waiting for a human to type anything at all.
///
/// Unlike the watchdog reasons this one is BOUNDED
/// (`MAX_API_ERROR_AUTO_RESUMES`), because the trigger is a failure the backend
/// might reproduce immediately: a persistently broken upstream must surface,
/// not loop. See `auto_resume_after_api_error`.
pub const AUTO_RESUME_AFTER_API_ERROR_REASON: &str = "auto_resume_after_api_error";

/// Tag stamped on `ContinuationRequested.reason` when recovery auto-resumes an
/// in-flight coding-agent thread after a **user-initiated** *Switch to new
/// version* (detected by a device-attributed teardown `ResponseAborted`). A crash
/// leaves no such boundary → the thread gets the manual "Continue" affordance
/// instead, so work that may have crashed the engine can't loop. Opens a
/// "Resumed" boundary exchange like the other automatic-resume reasons.
pub const AUTO_RESUME_AFTER_SWITCH_REASON: &str = "auto_resume_after_switch";

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
/// `auto_resume_after_api_error` opts in for the same reason the watchdog does:
/// the turn genuinely ended (its `ResponseFailed` is already in the timeline),
/// and what follows is a new attempt at the same work, which the user should see
/// as its own boundary rather than as text appended to the failed answer.
///
/// Default-deny on unknown / missing reasons: a future `ContinuationRequested`
/// reason must opt-in explicitly rather than inheriting a "Resumed after
/// engine restart" boundary by accident.
pub fn continue_should_open_resume_exchange(reason: Option<&str>) -> bool {
    matches!(
        reason,
        Some(USER_CLICKED_CONTINUE_REASON)
            | Some(AUTO_RECOVERY_AFTER_HANG_REASON)
            | Some(AUTO_RESUME_AFTER_SWITCH_REASON)
            | Some(AUTO_RESUME_AFTER_API_ERROR_REASON)
    )
}

/// What the spawn consumer must do after a `SpawnRequest::Continue`'s
/// `run_direct_agent` returns. See [`continue_recovery`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContinueRecovery {
    /// The turn ran; the run loop emitted its own terminal. Nothing to do.
    Nothing,
    /// Stale resume on the FIRST attempt — re-run once with a fresh session
    /// (no resume sid) and the conversation reconstructed into the prompt.
    RetryFresh,
    /// Errored with no retry left. Settle the projection so the thread cannot
    /// sit at `running` with no live subprocess.
    Settle,
}

/// Decide the spawn consumer's next move for a continuation, from the error the
/// run returned (if any) and whether the fresh-session retry has already been
/// spent.
///
/// Extracted as a pure function because the consumer's async shell needs a full
/// `LucidosEngine` and cannot be exercised in a test — same reason
/// [`continue_should_open_resume_exchange`] and
/// `external_watchdog::external_watchdog_decision` are shaped this way. The
/// wiring bug this encodes was invisible precisely because the decision lived
/// inline in that untestable shell.
///
/// Two properties are load-bearing:
///
/// - **A stale resume MUST be retried, not just logged.** `run_session` bails on
///   a stale resume WITHOUT a terminal event — deliberately, so the projection
///   stays `running` across the retry window instead of flashing "Aborted". That
///   only holds if a retry actually follows. When it didn't, thread `cb503361`
///   wedged at `running` for 8 minutes (2026-07-29) with no live subprocess: the
///   stale-resume arm also drops the `agent_sessions` entry, and that map is the
///   only thing `ExternalWatchdog` scans.
/// - **The retry is one-shot.** `retried == true` settles even on another
///   `STALE_RESUME_ERROR`, so an engine-driven continuation can never loop. (It
///   is unreachable anyway — the retry passes no resume sid and
///   `is_stale_resume_signal` requires one — but crash-safety here is a floor,
///   not an inference.)
///
/// The ONE error that must NOT settle is `AGENT_ALREADY_RUNNING_ERROR`: the
/// spawn guard rejected us because a **live** session already owns the thread,
/// so this continuation owns nothing and the `running` projection is TRUE — it
/// belongs to the turn that won the race. Settling there would emit a terminal
/// against a working session and make the projection lie in the opposite
/// direction, which is strictly worse than the wedge this backstop exists to
/// prevent. Reachable whenever a continuation races a user message: the
/// consumer dispatches off an event subscriber with no lock on the thread.
///
/// Every other error settles: the failure modes that skip their own terminal are
/// exactly the ones we cannot enumerate, and settling is idempotent
/// (`settle_stuck_running_thread` re-checks `running` first).
pub fn continue_recovery(error: Option<&str>, retried: bool) -> ContinueRecovery {
    match error {
        None => ContinueRecovery::Nothing,
        Some(e) if e == crate::engine::claude_code::AGENT_ALREADY_RUNNING_ERROR => {
            ContinueRecovery::Nothing
        }
        Some(e) if !retried && e == crate::engine::claude_code::STALE_RESUME_ERROR => {
            ContinueRecovery::RetryFresh
        }
        Some(_) => ContinueRecovery::Settle,
    }
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

/// Input for the spawn consumer's **one-shot stale-resume retry** of a
/// `SpawnRequest::Continue`: the thread's conversation reconstructed from events,
/// followed by [`CONTINUE_RESUME_USER_MESSAGE`].
///
/// The retry runs with `resume_session_id: None` — the sid we just proved dead
/// cannot be reused — so the fresh subprocess starts with zero context. Without
/// the recap it would "continue from where you left off" with no idea where that
/// was. Same shape as the chat handler's stale-resume retry
/// (`chat::process_cc`), which is the path this one was missing.
pub(crate) async fn continue_retry_input(pool: &sqlx::PgPool, thread_id: Uuid) -> String {
    crate::engine::agent_session::prepend_reconstruction(
        pool,
        thread_id,
        CONTINUE_RESUME_USER_MESSAGE,
    )
    .await
}

/// Remove a stale (duplicate-recovery) worktree, deleting its branch ONLY when
/// fully merged (no unique commits). NEVER force-deletes a branch that still
/// holds work: this is the duplicate-branch recovery path (two branches map to
/// one thread from stale-resume retries), and the duplicate's unique commits
/// must survive. Delegates to the cron's safe helper — which keeps a branch with
/// unique commits (and keeps on error, conservative).
///
/// 2026-07-02: replaced an unconditional `git worktree remove --force` +
/// `branch -D` that could silently lose a duplicate branch's commits, per the
/// "never delete a possibly-useful worktree/branch" invariant.
/// Best-effort — a failure just means the worktree is skipped again next restart.
pub(crate) async fn cleanup_stale_worktree(wt_path: &Path) {
    let _ = crate::engine::worktree_cleanup::remove_worktree_and_optionally_delete_branch(
        wt_path, None,
    )
    .await;
}

/// Resolve which thread an orphaned coding-agent worktree (found by the
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
pub(crate) async fn last_turn_ended_cleanly(pool: &sqlx::PgPool, thread_id: Uuid) -> bool {
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
        // (`list_pending` in `recover_orphaned_worktrees`): log and fall
        // through. Returning
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

/// Last-resort recovery for an actively-running coding-agent branch whose ref
/// vanished from every repo (`recover_orphaned_worktrees`'s "branch not found
/// in any repo" arm) while its deterministic worktree directory survived on
/// disk. Recreates the branch ref at the worktree's recorded `HEAD` so the
/// session can be `--resume`d on the original branch instead of being torn down
/// and restarted from a fresh branch (which drops the `cc_session_id` and the
/// thread's conversation continuity — the thread-9e37697e failure mode).
///
/// Returns `Some((repo_root, worktree_path))` when the branch was recreated and
/// the session can resume; `None` (caller falls back to ending the stuck
/// session — the genuine last resort) when:
///   - the worktree directory is absent (nothing recoverable),
///   - its `HEAD` does not resolve to a real commit (a dangling symref left by
///     out-of-band ref deletion — we never *guess* a SHA, which could mis-point
///     a commit-bearing branch onto the wrong history), or
///   - the repo root can't be resolved or the `git branch` create fails.
///
/// Pure git/filesystem — no engine/DB dependency — so it is unit-testable
/// against a real worktree without instantiating a `LucidosEngine`.
pub(crate) async fn recover_branch_ref_from_worktree(
    workspace_path: &Path,
    thread_id: Uuid,
    branch_name: &str,
) -> Option<(PathBuf, PathBuf)> {
    let wt = crate::engine::agent_session::resume::deterministic_worktree_path(
        workspace_path,
        thread_id,
    );
    if !matches!(tokio::fs::try_exists(&wt).await, Ok(true)) {
        return None;
    }
    // Resolve the worktree's HEAD to a concrete commit. `--verify` makes git
    // fail (non-zero) rather than echo the literal "HEAD" when the symref
    // dangles, so a deleted-branch-but-symref-still-points-at-it worktree
    // (unrecoverable without fsck) falls through to the last-resort path
    // instead of recreating the branch at a bogus value.
    let head_sha = match git_cmd(&["rev-parse", "--verify", "HEAD"], &wt).await {
        Ok(o) if o.status.success() => {
            let sha = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if sha.is_empty() {
                return None;
            }
            sha
        }
        _ => return None,
    };
    let repo_root = crate::engine::worktree_cleanup::resolve_repo_root_from_worktree(&wt).await?;
    match git_cmd(&["branch", branch_name, &head_sha], &repo_root).await {
        Ok(o) if o.status.success() => {
            log!(
                "[Recovery] Recreated missing branch {} at {} from surviving worktree {} — session resumable",
                branch_name,
                &head_sha[..head_sha.floor_char_boundary(8)],
                wt.display()
            );
            Some((repo_root, wt))
        }
        Ok(o) => {
            log!(
                "[Recovery] Could not recreate branch {} from worktree {}: {}",
                branch_name,
                wt.display(),
                String::from_utf8_lossy(&o.stderr).trim()
            );
            None
        }
        Err(e) => {
            log!(
                "[Recovery] git branch create for {} errored: {}",
                branch_name,
                e
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::git_ops::{git_cmd, worktrees_dir};
    use std::collections::HashMap;

    /// Build a fresh git repo at `workspace` with one commit on `main` and an
    /// empty `.lucidos/worktrees/`. Returns nothing — the caller uses
    /// `workspace` as both repo root and workspace root.
    async fn init_repo(workspace: &Path) {
        git_cmd(&["init", "--initial-branch=main"], workspace)
            .await
            .unwrap();
        git_cmd(&["config", "user.email", "recover@test"], workspace)
            .await
            .unwrap();
        git_cmd(&["config", "user.name", "Recover Test"], workspace)
            .await
            .unwrap();
        tokio::fs::write(workspace.join("seed.txt"), "seed")
            .await
            .unwrap();
        git_cmd(&["add", "."], workspace).await.unwrap();
        git_cmd(&["commit", "-m", "seed"], workspace).await.unwrap();
        let _ = worktrees_dir(workspace);
    }

    async fn rev_parse(repo: &Path, rev: &str) -> Option<String> {
        let o = git_cmd(&["rev-parse", "--verify", rev], repo).await.ok()?;
        o.status
            .success()
            .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
    }

    /// Regression for the thread-9e37697e recovery half: when a resumable
    /// coding-agent branch's ref has vanished but its deterministic worktree
    /// directory survives with a resolvable HEAD, recovery recreates the branch
    /// from that HEAD (so the session can `--resume`) instead of ending the
    /// stuck session and dropping the session id.
    #[tokio::test]
    async fn recover_branch_ref_recreates_missing_branch_from_surviving_worktree() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        init_repo(&workspace).await;

        let thread_id = Uuid::new_v4();
        let branch = "claude-code/recover-test";
        let wt = crate::engine::agent_session::resume::deterministic_worktree_path(
            &workspace, thread_id,
        );
        git_cmd(
            &[
                "worktree",
                "add",
                "-b",
                branch,
                &wt.to_string_lossy(),
                "main",
            ],
            &workspace,
        )
        .await
        .unwrap();
        let head_before = rev_parse(&wt, "HEAD")
            .await
            .expect("worktree HEAD resolves");

        // Simulate the branch-ref loss while the worktree dir survives: detach
        // the worktree's HEAD (so it still resolves to a commit), then delete
        // the branch ref. This is exactly the "branch not found in any repo,
        // worktree survives" state recovery's last-resort arm guards against.
        git_cmd(&["checkout", "--detach"], &wt).await.unwrap();
        git_cmd(&["branch", "-D", branch], &workspace)
            .await
            .unwrap();
        assert!(
            rev_parse(&workspace, &format!("refs/heads/{}", branch))
                .await
                .is_none(),
            "precondition: branch ref must be gone"
        );

        let recovered = recover_branch_ref_from_worktree(&workspace, thread_id, branch).await;
        let (repo_root, wt_path) =
            recovered.expect("must recover the branch from the surviving worktree");
        assert_eq!(wt_path, wt, "returns the recovered worktree path");
        // `resolve_repo_root_from_worktree` canonicalizes (macOS resolves
        // `/var` → `/private/var`), so compare canonical forms.
        let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        assert_eq!(
            canon(&repo_root),
            canon(&workspace),
            "resolves the worktree's repo root"
        );

        let head_after = rev_parse(&workspace, &format!("refs/heads/{}", branch))
            .await
            .expect("branch ref must be recreated");
        assert_eq!(
            head_after, head_before,
            "recreated branch must point at the worktree's recorded HEAD, not a guessed commit"
        );
    }

    /// No worktree on disk → nothing to recover from → `None` so the caller
    /// falls back to ending the stuck session (the genuine last resort).
    #[tokio::test]
    async fn recover_branch_ref_returns_none_when_worktree_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        init_repo(&workspace).await;
        let _ = worktrees_dir(&workspace);

        let recovered =
            recover_branch_ref_from_worktree(&workspace, Uuid::new_v4(), "claude-code/missing")
                .await;
        assert!(
            recovered.is_none(),
            "absent worktree must not recover — caller ends the stuck session"
        );
    }

    /// A dangling HEAD symref (branch ref deleted out-of-band, worktree HEAD
    /// still says `ref: refs/heads/<branch>`) is unrecoverable without fsck —
    /// the helper must refuse rather than guess a SHA and mis-point the branch.
    #[tokio::test]
    async fn recover_branch_ref_returns_none_on_dangling_head() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().to_path_buf();
        init_repo(&workspace).await;

        let thread_id = Uuid::new_v4();
        let branch = "claude-code/dangling-test";
        let wt = crate::engine::agent_session::resume::deterministic_worktree_path(
            &workspace, thread_id,
        );
        git_cmd(
            &[
                "worktree",
                "add",
                "-b",
                branch,
                &wt.to_string_lossy(),
                "main",
            ],
            &workspace,
        )
        .await
        .unwrap();
        // Delete the branch ref directly, leaving the worktree HEAD as a
        // dangling `ref: refs/heads/<branch>` symref — `rev-parse HEAD` fails.
        let ref_path = workspace
            .join(".git/refs/heads")
            .join(branch.strip_prefix("claude-code/").unwrap());
        let _ = tokio::fs::remove_file(workspace.join(format!(".git/refs/heads/{}", branch))).await;
        let _ = tokio::fs::remove_file(&ref_path).await;
        git_cmd(&["pack-refs", "--all"], &workspace).await.ok();
        let _ = tokio::fs::remove_file(workspace.join(format!(".git/refs/heads/{}", branch))).await;

        let recovered = recover_branch_ref_from_worktree(&workspace, thread_id, branch).await;
        assert!(
            recovered.is_none(),
            "dangling HEAD symref is unrecoverable — must NOT guess a SHA"
        );
    }

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
