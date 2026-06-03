use serde::{Deserialize, Serialize};

/// Why a `ResponseCanceled` was emitted. Cancellation is always a user-driven
/// action that interrupts a *real* in-flight response — the actor on
/// `EventMeta` identifies the user, this enum identifies what they did. New
/// emit sites must specify the cause; `Unknown` only appears on legacy DB
/// rows persisted before the field existed.
///
/// If you want to settle a thread whose process is already gone, that's an
/// abort (`AbortCause::StaleSettle`), not a cancel — nothing was running to
/// cancel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelCause {
    /// User clicked the Stop button on a running response.
    UserStop,
    /// User clicked Apply / Discard / Archive on a Claude Code session that was still
    /// running — the action implies "stop the current turn first."
    UserAction,
    /// Pre-typed-cause legacy event, or a now-removed cause string (e.g. the
    /// retired `stale_settle` cancel cause that's now an abort cause). Catches
    /// anything unrecognized so old DB rows replay cleanly. Never emit fresh.
    #[serde(other)]
    Unknown,
}

/// Why a `ResponseAborted` was emitted. Aborts are system-driven cleanup —
/// the engine or the OS terminated the process, or the engine settled a
/// projection whose live process was already gone. The actor on `EventMeta`
/// records *who triggered* the cleanup (a user button can fire stale-settle),
/// not who decided to terminate the work — that's always the system. New emit
/// sites must specify the cause; `Unknown` only appears on legacy DB rows
/// persisted before the field existed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbortCause {
    /// Engine is shutting down — every running Claude Code session gets a clean abort
    /// so the next process can resume from a known state.
    EngineShutdown,
    /// Safety net fired in `run_session`: CC's event loop ended without a
    /// `Result` event (process crash, stream EOF before Result, parser
    /// glitch). The thread surfaces in error state; any commits CC made
    /// before dying stay on the branch but are NOT proposed as a change
    /// (see `SessionEndAction::CrashedKeepBranch`).
    SafetyNet,
    /// Engine started up and found a session marked `running` with no live
    /// process — recovery emitted an abort to settle the projection.
    RecoveryAfterRestart,
    /// Claude Code subprocess died unexpectedly (OS signal, panic, external `kill`).
    ProcessKilled,
    /// Engine settled a thread the projection still showed as `running` but
    /// for which no live process existed. Surfaces a stuck UI; not a real
    /// process kill (the process was already gone). The user's action that
    /// exposed the stuck row (Stop / Apply / Discard / Archive / Interrupt)
    /// flows through as the actor, but no real response was canceled — the
    /// thread is just being cleaned up.
    StaleSettle,
    /// Pre-typed-cause legacy event or unrecognized cause string. Never emit
    /// fresh.
    #[serde(other)]
    Unknown,
}

impl AbortCause {
    /// True when the abort is expected to be followed by a fresh `SessionStarted`
    /// (engine shutdown, recovery sweep) — the child is mid-retry, not done.
    /// Callers must NOT decrement the parent's `active_children_count` or fire
    /// the completion callback in this case, or the resumed child's eventual
    /// `CodingAgentIdled` would be orphaned.
    ///
    /// `SafetyNet`, `ProcessKilled`, and `StaleSettle` are NOT transient: the
    /// thread sits in error state (or, for stale-settle, was already done and
    /// is just being projection-cleaned) and no fresh `SessionStarted` will
    /// follow — the parent's counter must drop so it doesn't display as Active
    /// forever. `Unknown` is legacy; treat as terminal so the prior
    /// decrement-on-abort behavior holds for old DB rows.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::EngineShutdown | Self::RecoveryAfterRestart)
    }

    /// SQL fragment for the `status` column on the `thread_summaries` row when
    /// this abort lands. Most aborts surface a red `failed` indicator (with
    /// pending changes overriding to `waiting`); `StaleSettle` is engine
    /// cleanup of a stuck row whose process was already gone — fired by a user
    /// button (Stop / Apply / Discard / Archive / Interrupt). No real abort
    /// happened, so it uses the cancel-style mapping (idle, or waiting if
    /// pending changes) rather than the red failed indicator.
    pub fn status_sql(&self) -> &'static str {
        match self {
            Self::StaleSettle => crate::engine::event_bus::STATUS_FROM_PROPOSED_CHANGE,
            _ => "CASE WHEN coding_agent_proposed THEN 'waiting' ELSE 'failed' END",
        }
    }
}
