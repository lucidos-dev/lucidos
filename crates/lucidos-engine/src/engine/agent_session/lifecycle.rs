/// What the idle handler should do after CC reports a turn-ending Result.
/// The natural turn terminator and `CodingAgentIdled` have already landed —
/// nothing here emits an additional terminal event. `EndSession` MUST
/// translate to a plain loop break at the call site, not `stop.notify_one()`.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum IdleAction {
    /// Conflict-resolution session: break the run loop so post-loop cleanup
    /// tears down CC and removes the merge worktree.
    EndSession,
    /// Normal user session: kill CC; the next message resumes a fresh process.
    ExitSubprocess,
    /// Engine shutdown: leave CC for `recover_orphaned_worktrees` to resume
    /// after restart.
    Nothing,
}

/// Conflict resolution wins regardless of shutdown — the merge worktree is
/// throwaway and recovery never resumes a conflict session.
pub(super) fn idle_action(is_conflict: bool, is_shutdown: bool) -> IdleAction {
    if is_conflict {
        IdleAction::EndSession
    } else if is_shutdown {
        IdleAction::Nothing
    } else {
        IdleAction::ExitSubprocess
    }
}

/// Per-Result termination decision once `idle_action` returned
/// `ExitSubprocess`. Each `KeepAlive` variant names its own reason so the
/// log line at the call site is derived from the variant — no risk of the
/// log message drifting from the actual cause.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum TerminateDecision {
    /// Kill CC via `agent_cancel.cancel()`. The next message resumes a
    /// fresh CC process via `--resume`.
    Terminate,
    /// One or more follow-up user messages are inflight (queued in
    /// `msg_rx` or already merged into this Result). Keep CC alive so the
    /// next turn consumes them without a respawn round-trip. `inflight` is
    /// `prev_pending_followups - 1` to match the pre-existing log line.
    KeepAliveForFollowup { inflight: u32 },
    /// The chat-agent's `run_bash_background` LLM tool still has a task
    /// running for this thread. `spawn_bash_completion_watcher` will push a
    /// resume prompt into `msg_tx` when the bash completes; killing CC
    /// here would force the wake path through stale-session recovery —
    /// exactly the regression that gate exists to prevent.
    KeepAliveForBgBash,
}

/// Decide whether to terminate the CC subprocess at idle.
///
/// `prev_pending_followups` is the value of `pending_followups` **after**
/// the per-Result `swap(0)` — values > 1 mean a follow-up message arrived
/// between the prior turn and this one and CC must stay alive to consume
/// it without a respawn round-trip.
///
/// Precedence: followup > chat-agent bg bash > terminate. Followup wins
/// because it's user-initiated and the strongest signal; the chat-agent
/// bg-bash signal only matters when there's nothing queued — it keeps CC
/// alive so `spawn_bash_completion_watcher` can push a resume prompt when
/// the bash finishes (killing CC would force that wake through stale-session
/// recovery).
pub(super) fn terminate_decision(
    prev_pending_followups: u32,
    bg_bash_running: bool,
) -> TerminateDecision {
    if prev_pending_followups > 1 {
        TerminateDecision::KeepAliveForFollowup {
            inflight: prev_pending_followups - 1,
        }
    } else if bg_bash_running {
        TerminateDecision::KeepAliveForBgBash
    } else {
        TerminateDecision::Terminate
    }
}

/// Decide whether to snapshot the worktree as a pending change when CC
/// goes idle.
///
/// Auto-propose surfaces the worktree as a pending change with an Apply
/// button. We only do that when the turn ended cleanly (Generated) — any
/// other terminal (Failed, Canceled, Aborted, silent resume) is "half-
/// assed work" the user shouldn't be invited to apply blind. The work
/// stays in the worktree on the branch either way; the user just doesn't
/// get a spurious Apply card built from a crash, an upstream API drop, a
/// mid-turn cancel, or an engine shutdown.
///
/// External repos never produce Lucidos changes (the dev pushes/PRs from
/// the session itself). Shutdown is mid-work, not genuinely idle.
///
/// Conflict-resolution sessions are the subtle case: they run in a
/// `merge-tmp/<change-id>` worktree where the merge IS the original
/// change being applied. Proposing here creates a phantom second change
/// row keyed on the temp branch, which the post-commit hook never tags
/// (different worktree path), so it shows up in the UI without an
/// inline timeline card. The original change's `ChangeApplied` lands
/// shortly after — leaving the phantom orphaned at "pending". Refuse
/// here so the merge work flows back into the change being applied.
///
/// Background bash (the chat-agent `run_bash_background` tool or CC's own
/// `Bash{run_in_background:true}`) deliberately does NOT gate this. A CC
/// that idles `Generated` while a background task runs proposes the change
/// immediately so Apply surfaces without delay. Correctness is covered by
/// harden-at-apply: an un-hardened change re-runs `/harden` (tests included)
/// before it can merge, so a background test that hadn't finished can't land
/// broken work. App/external changes skip `/harden` and accept that risk.
/// This replaced an earlier gate whose only automatic recovery was a 5-minute
/// nudge — the wait that gate imposed was worse than the rare wasted re-harden
/// it prevented.
pub(super) fn should_propose_change_at_idle(
    wt_has_changes: bool,
    is_external_repo: bool,
    is_shutdown: bool,
    is_conflict_session: bool,
    terminal_kind: &Option<TerminalKind>,
) -> bool {
    wt_has_changes
        && !is_external_repo
        && !is_shutdown
        && !is_conflict_session
        && matches!(terminal_kind, Some(TerminalKind::Generated))
}

/// Which terminal event closes the current CC turn. Cancel/Abort variants
/// carry the typed cause so the emit site cannot mis-classify why the turn
/// ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TerminalKind {
    Generated,
    Canceled(crate::engine::thread_events::CancelCause),
    Aborted(crate::engine::thread_events::AbortCause),
    /// CC reported the turn ended in failure (mid-stream API error,
    /// `error_max_turns`, etc.). Carries the user-facing reason so
    /// `make_terminal_event` can populate `ResponseFailed.error` —
    /// without this the partial response renders as a complete answer.
    Failed { error: String },
}

/// True when CC was woken up with no user-initiated content — engine-internal
/// warm-up resumes only. The follow-up `Result` event from such a turn is a
/// no-op and must not produce a terminal/idle event (the previous turn's
/// `CodingAgentIdled` already records the active `cc_session_id`). Image-only
/// turns count as content, so the call site must include images in the check —
/// otherwise CC delivers an answer but the thread row stays stuck at `running`.
pub(super) fn is_silent_resume(user_text_empty: bool, has_images: bool) -> bool {
    user_text_empty && !has_images
}

/// User-facing error message for the empty-response branch of `classify_result`.
/// Surfaces an OOM-killed bash (exit 137), a SIGTERM'd subprocess (exit 143), or
/// any other path where CC produces a Result with no real assistant text. Without
/// this branch the empty Result classifies as `Generated` and the turn looks
/// successful in the UI even though the user got nothing back.
pub(super) const EMPTY_RESPONSE_ERROR: &str =
    "Claude Code produced no visible response — the session ended without a real \
     assistant message. This usually means a tool call was killed (OOM with exit 137, \
     SIGTERM with exit 143, or similar). The work is incomplete; re-prompt with a \
     narrower scope.";

/// Decide what to emit when a CC `Result` event arrives — both the terminal
/// event kind and whether `CodingAgentIdled` should follow it. Returning both
/// decisions together prevents the TOCTOU race that would otherwise occur if
/// `is_shutdown` were read twice and observed different values across the two
/// branches: `ResponseGenerated` would fire (CC really finished) but the idle
/// event would be skipped, leaving the thread row stuck at `running`.
///
/// Every non-silent, non-shutdown Result is a turn boundary and emits
/// `CodingAgentIdled`. Inflight-followup race protection (deciding whether to
/// keep the subprocess alive) is the run-loop's responsibility — see
/// `AgentSession.pending_followups`.
///
/// Precedence: silent_resume > shutdown > user_hit_stop > cc_error >
/// text_is_empty > generated. Shutdown wins because the engine is going down
/// regardless of how CC ended the turn — the next process emits `Aborted` and
/// the recovery path re-resumes from there.
///
/// `user_hit_stop` sits ABOVE `cc_error`: the `Stop` button (Cancel = Esc) now
/// routes through CC's native interrupt, and an *interrupted* turn comes back as
/// a `Result` with `is_error: true` (CC reports the aborted turn — e.g.
/// `stop_reason=tool_use`, an `[ede_diagnostic]` line). That error is *caused by*
/// the user's cancel, so it must classify as `Canceled`, not `Failed` — otherwise
/// the user sees a red "Failed" dot for a turn they deliberately stopped, and the
/// branch-preservation gate (which keys on `Canceled`) never fires, so the
/// session loses its branch and can't resume. A real upstream failure on a turn
/// the user *didn't* stop still classifies as `Failed` (user_hit_stop is false
/// there). `text_is_empty` stays below user_hit_stop for the same reason — a
/// cancel landing on an empty Result is still a cancel.
pub(super) fn classify_result(
    is_silent_resume: bool,
    user_hit_stop: bool,
    is_shutdown: bool,
    cc_error: Option<String>,
    text_is_empty: bool,
) -> (Option<TerminalKind>, bool) {
    if is_silent_resume {
        return (None, false);
    }
    use crate::engine::thread_events::{AbortCause, CancelCause};
    let terminal = if is_shutdown {
        TerminalKind::Aborted(AbortCause::EngineShutdown)
    } else if user_hit_stop {
        TerminalKind::Canceled(CancelCause::UserStop)
    } else if let Some(error) = cc_error {
        TerminalKind::Failed { error }
    } else if text_is_empty {
        TerminalKind::Failed {
            error: EMPTY_RESPONSE_ERROR.to_string(),
        }
    } else {
        TerminalKind::Generated
    };
    let emit_idle = !is_shutdown;
    (Some(terminal), emit_idle)
}

/// Auto-commit the worktree on session cleanup ONLY when the last turn
/// finished cleanly (Generated) AND the user didn't ask to discard.
///
/// Anything else — safety-net abort (CC died mid-stream), Failed (upstream
/// API drop / OOM / empty Result), Canceled (user stop), Aborted (engine
/// shutdown), silent-resume warmup — leaves the worktree's contents
/// uncommitted on the branch. The post-commit hook fires per commit and
/// emits a per-commit `ChangeProposed`; without this gate, every cleanup
/// auto-commit on a half-finished session published a spurious Apply card
/// for partial work even though the aggregate `should_propose_change_at_idle`
/// gate had already refused.
///
/// Recovery is unaffected: `recover_orphaned_worktrees` re-spawns CC inside
/// the same worktree, and CC sees uncommitted dirt the same as committed
/// state — the user's prior edits are preserved either way.
pub(super) fn should_auto_commit_on_cleanup(
    should_discard: bool,
    last_terminal: &Option<TerminalKind>,
) -> bool {
    !should_discard && matches!(last_terminal, Some(TerminalKind::Generated))
}

/// True when an arriving `Result` is the "stale --resume" signal — CC echoed
/// our forwarded user message back as an empty answer because the persisted
/// session id no longer exists. The run-loop responds by killing the worktree
/// and retrying with a fresh spawn.
///
/// `cc_error.is_none()` is load-bearing: an empty Result with `is_error: true`
/// is a real upstream failure (network drop, 5xx), not an expired session.
/// Without that gate a transient API error would delete the worktree and
/// branch, destroying user work.
pub(super) fn is_stale_resume_signal(
    has_resume_session: bool,
    result_text_empty: bool,
    buffered_text_empty: bool,
    no_prior_results_this_turn: bool,
    user_message_present: bool,
    cc_error: bool,
) -> bool {
    has_resume_session
        && result_text_empty
        && buffered_text_empty
        && no_prior_results_this_turn
        && user_message_present
        && !cc_error
}

/// Decide what terminal event the stop arm of the run loop should emit.
///
/// Precedence: shutdown > user-action suppress > idle race > real Cancel.
/// Shutdown wins because the engine is going down regardless of which UI
/// button triggered the stop. Apply / Discard / Archive set
/// `suppress_user_terminal=true` because their own lifecycle event
/// (`ChangeApplied` / `ChangeDiscarded` / `ThreadArchived`) is the
/// terminator. An idle CC means the previous turn's `ResponseGenerated`
/// already terminated it, so even a real Cancel click that races in
/// emits nothing — labeling a finished turn "Canceled" would lie.
pub(super) fn stop_terminal_kind(
    is_shutdown: bool,
    is_waiting: bool,
    suppress_user_terminal: bool,
) -> Option<TerminalKind> {
    use crate::engine::thread_events::{AbortCause, CancelCause};
    if is_shutdown {
        if is_waiting {
            None
        } else {
            Some(TerminalKind::Aborted(AbortCause::EngineShutdown))
        }
    } else if suppress_user_terminal || is_waiting {
        None
    } else {
        Some(TerminalKind::Canceled(CancelCause::UserStop))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum SessionEndAction {
    Propose,
    KeepExternalBranch,
    /// Safety net fired (CC EOF without Result event) and commits exist on the
    /// branch. Keep the commits on disk so the user can resume and continue,
    /// but do NOT emit `ChangeProposed` — surfacing a half-finished crash as
    /// a ready-to-apply pending change misleads the user. The thread's
    /// terminal event is `ResponseAborted`, so the UI shows the crash state.
    CrashedKeepBranch,
    /// The user cancelled the turn (Stop button = Esc). Cancel is a *resumable
    /// turn boundary*, not a terminator — the next message `--resume`s the same
    /// `cc_session_id` on this branch. Keep the branch (even with zero commits)
    /// so `resolve_branch_for_resume` finds it; do NOT propose, because a
    /// cancelled turn is half-finished work the user shouldn't be invited to
    /// apply (mirrors `should_propose_change_at_idle`). Without this the
    /// no-commits cancel path falls into `CleanupBranches` → `git branch -D`,
    /// which is exactly what made a cancel spawn a fresh, amnesiac CC session.
    KeepCanceledBranch,
    CleanupBranches,
}

/// External repos with commits keep their branch even when the diff is empty —
/// the user owns push/PR for that ref and we won't `branch -D` something they
/// might want to keep.
///
/// `safety_net_fired` is set when CC's event loop ended without a Result event
/// (process crash, stream EOF, parser glitch). In that case any commits on the
/// branch reflect partial work — they stay on disk via `CrashedKeepBranch` but
/// are never proposed as a change. External repos keep `KeepExternalBranch`
/// regardless because the user owns push/PR for their own refs.
///
/// `user_canceled` is set when the last terminal was a user-driven
/// `Canceled(UserStop)` (the Stop button, which now routes through CC's native
/// interrupt/Esc). A cancel keeps the branch so the session stays resumable,
/// and never proposes — it ranks above `Propose`/`CleanupBranches` but below the
/// external/crash keep-branch arms (those already keep the branch and carry
/// more specific semantics). It can't co-occur with `safety_net_fired`: a
/// cancel emits a terminal event, so `safety_net_fired` (= no terminal) is
/// false.
pub(super) fn classify_session_end_action(
    has_commits: bool,
    proposal_files_empty: bool,
    is_external_repo: bool,
    safety_net_fired: bool,
    user_canceled: bool,
) -> SessionEndAction {
    match (has_commits, is_external_repo, safety_net_fired) {
        (true, true, _) => SessionEndAction::KeepExternalBranch,
        (true, false, true) => SessionEndAction::CrashedKeepBranch,
        _ if user_canceled => SessionEndAction::KeepCanceledBranch,
        (true, false, false) if !proposal_files_empty => SessionEndAction::Propose,
        _ => SessionEndAction::CleanupBranches,
    }
}

/// Clear all per-turn flags at a turn boundary so prior-turn state can't
/// leak into the next. `emitted_terminal_event` in particular gates the
/// post-loop safety net; without resetting it, a follow-up that ends with
/// CC exiting before producing a Result silently skips the safety net.
/// `last_terminal_kind` is per-turn for the same reason: the cleanup gate
/// (`should_auto_commit_on_cleanup`) must reflect THIS turn's outcome,
/// not whatever the previous turn happened to leave behind.
pub(super) fn reset_per_turn_flags(
    is_waiting: &mut bool,
    last_emitted_idle: &mut bool,
    emitted_terminal_event: &mut bool,
    user_hit_stop: &mut bool,
    last_terminal_kind: &mut Option<TerminalKind>,
) {
    *is_waiting = false;
    *last_emitted_idle = false;
    *emitted_terminal_event = false;
    *user_hit_stop = false;
    *last_terminal_kind = None;
}

/// 10 minutes of CC silence in the narrow "awaiting Anthropic response"
/// window = subprocess hung. The window the watchdog protects is the
/// originally-observed bug: the network died during an in-flight Anthropic
/// API call, the kernel didn't notice (no RST, no FIN), and CC sat forever
/// on a dead TCP socket. Without this watchdog the engine waits forever —
/// the user sees a permanently "Working" thread even after restoring
/// network.
///
/// The watchdog is opt-in: it arms ONLY when CC is mid-turn AND no tool is
/// currently executing. Tool execution (Bash, Read, Grep, AskUserQuestion,
/// TaskOutput, agent sub-tasks, anything) is legitimate silence that we
/// trust CC to time out itself. Earlier wall-clock-only versions of this
/// watchdog killed CC during a 5-min `TaskOutput` poll on a background task
/// and during multi-minute `AskUserQuestion` waits — both legitimate, both
/// destroyed user work. The opt-in design fires for the dead-socket case
/// it was designed for and stays out of every other legitimate-silence
/// case.
pub(super) const WATCHDOG_INACTIVITY_LIMIT_MS: i64 = 10 * 60 * 1000;

/// How often the agentic loop's `select!` re-evaluates the watchdog. Coarse
/// granularity (30s) — the limit is 10 min, so detection latency on a hung
/// subprocess is at most `WATCHDOG_INACTIVITY_LIMIT_MS + WATCHDOG_TICK`.
/// Smaller values would burn the `agent_sessions` mutex on every tick for
/// no benefit.
pub(super) const WATCHDOG_TICK_INTERVAL_SECS: u64 = 30;

/// Half the fire limit so the per-tick diagnostic surfaces before the fire
/// (rules out "elapsed wasn't far enough" when post-morteming a stuck thread).
pub(super) const WATCHDOG_DIAG_LOG_THRESHOLD_MS: i64 = WATCHDOG_INACTIVITY_LIMIT_MS / 2;

/// Outcome of one watchdog tick. The non-`Fire` variants name the gate that
/// held so the diagnostic log can pin the cause — the May-2026 stuck-thread
/// incident lacked this and post-hoc analysis could only guess.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum WatchdogGate {
    Fire,
    NotStale,
    SkipIsWaiting,
    SkipBadHeartbeat,
    /// Inner count surfaces in the diagnostic log so a stuck AskUserQuestion
    /// is identifiable.
    SkipToolsInFlight(i32),
}

impl WatchdogGate {
    /// Stable snake_case tag for the diagnostic log so scrapers can grep
    /// without parsing the Debug repr.
    pub(super) fn diag_tag(&self) -> &'static str {
        match self {
            Self::Fire => "fire",
            Self::NotStale => "not_stale",
            Self::SkipIsWaiting => "skip_is_waiting",
            Self::SkipBadHeartbeat => "skip_bad_heartbeat",
            Self::SkipToolsInFlight(_) => "skip_tools_in_flight",
        }
    }
}

/// What the post-loop safety net should emit when CC's event loop ended
/// without a natural terminator (`!emitted_terminal_event`). Always lands
/// SOMETHING terminal — either a `ContinuationRequested` so the spawn dispatcher
/// boots a fresh `--resume`, or `ResponseAborted(SafetyNet)` so the UI
/// flips to the red-dot abort state. Without this guard, a Claude Code subprocess
/// that died between events leaves the thread row stuck at `running`
/// indefinitely.
///
/// Precedence: external terminal already emitted > watchdog fired (auto-
/// recovery) > plain safety net (abort). External terminal wins because the
/// caller already landed a terminal (engine restart, race), and a second
/// terminal would relabel a finished turn. Watchdog wins over plain abort
/// because the watchdog's whole job is to bypass the user-visible abort
/// when the network died mid-call — auto-resume picks up cleanly via
/// `--resume` once the kernel notices the dead socket.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum SafetyNetAction {
    /// Loop ended with a natural terminator (Generated, Failed, Canceled,
    /// Aborted) — the in-loop emit already closed the turn.
    Nothing,
    /// Watchdog killed CC mid-call (network died, dead TCP socket). Emit
    /// `ContinuationRequested` so the spawn dispatcher boots a fresh `--resume`
    /// without surfacing a user-visible abort.
    EmitContinuationRequested,
    /// CC's event loop died without a natural terminator and the watchdog
    /// didn't fire (driver crash, EOF on stdout without a Result, parser
    /// glitch). Emit `ResponseAborted(SafetyNet)` so the UI flips to the
    /// red-dot abort state — the user needs to know the work didn't
    /// complete.
    EmitAbortedSafetyNet,
    /// An external terminal already landed (engine restart fast-path, or a
    /// race with a concurrent cancel). Don't emit again — a second
    /// terminal relabels a turn that's already closed.
    Skip,
}

/// Decide what the post-loop safety net should emit. Pure function —
/// `run_session.rs` glues this to the actual emit calls.
pub(super) fn safety_net_action(
    safety_net_fired: bool,
    watchdog_fired: bool,
    external_terminal_already_emitted: bool,
) -> SafetyNetAction {
    if !safety_net_fired {
        return SafetyNetAction::Nothing;
    }
    if external_terminal_already_emitted {
        return SafetyNetAction::Skip;
    }
    if watchdog_fired {
        SafetyNetAction::EmitContinuationRequested
    } else {
        SafetyNetAction::EmitAbortedSafetyNet
    }
}

/// Pure tick outcome. Fires only when CC is mid-turn (`!is_waiting`), no
/// tool is in flight (tool execution is legitimate silence — CC owns
/// timing out the tool), and the last event is past `limit_ms`. A zero
/// `last_event_at_ms` defensively skips (heartbeat uninitialized / race).
pub(super) fn watchdog_gate(
    is_waiting: bool,
    last_event_at_ms: i64,
    now_ms: i64,
    limit_ms: i64,
    tools_in_flight: i32,
) -> WatchdogGate {
    if is_waiting {
        return WatchdogGate::SkipIsWaiting;
    }
    if last_event_at_ms <= 0 {
        return WatchdogGate::SkipBadHeartbeat;
    }
    if tools_in_flight > 0 {
        return WatchdogGate::SkipToolsInFlight(tools_in_flight);
    }
    if now_ms.saturating_sub(last_event_at_ms) > limit_ms {
        WatchdogGate::Fire
    } else {
        WatchdogGate::NotStale
    }
}

#[cfg(test)]
#[path = "lifecycle_tests/propose.rs"]
mod propose_tests;

#[cfg(test)]
#[path = "lifecycle_tests/classify.rs"]
mod classify_tests;

#[cfg(test)]
#[path = "lifecycle_tests/watchdog.rs"]
mod watchdog_tests;
