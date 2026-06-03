use serde::{Deserialize, Serialize};

/// Why a session ended — used by frontend for status/display logic.
///
/// Mostly terminal-only (Phase 4 of the CC resume architecture): every CC turn
/// ends with `CodingAgentIdled` as a turn boundary; `SessionEnded` fires when
/// the thread is truly done. Removed non-terminal reasons (`Completed`,
/// `ChangesProposed`, `ChangesApplied`, `AutoEnded`, `UserEnded`, `Discarded`)
/// survive in the DB on legacy rows — they deserialize as `LegacyNonTerminal`
/// via `#[serde(other)]` so old data doesn't crash.
///
/// `StaleResume` is the one transient exception: emitted when CC returns an
/// empty Result against a stale `--resume` token. The chat handler retries
/// internally with a fresh session; `event_bus` skips the status flip and the
/// frontend skips the AbortPanel so the user doesn't see a phantom "Aborted"
/// during the retry window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionEndReason {
    /// Engine graceful stop. Frontend renders as "Aborted".
    Shutdown,
    /// Claude Code subprocess crashed irrecoverably.
    Panic,
    /// User explicitly closed the thread (Phase 10 will wire the UI; no emit
    /// site uses this yet, but the variant is reserved so the frontend can
    /// branch on it.)
    Closed,
    /// CC's `--resume <sid>` returned an empty Result — the prior session is
    /// gone. The engine emits this so restart-recovery's auto-detect resolver
    /// (`resolve_resume_context`) sees SessionEnded as the latest lifecycle
    /// event and does NOT try to resume the stale sid; the chat handler then
    /// retries the user's message against a fresh session. Frontend treats
    /// this as a transient lifecycle marker — no "Aborted" display, status
    /// stays `running` until the retry's SessionStarted lands.
    StaleResume,
    /// Catch-all for legacy DB rows persisted before this enum was reduced to
    /// terminal-only reasons. Treat as a harmless terminal end on read; never
    /// emit going forward.
    #[serde(other)]
    LegacyNonTerminal,
}

impl SessionEndReason {
    /// True when the session-end is mid-flight — the engine is still working on
    /// the current turn and a fresh `SessionStarted` is expected to follow.
    /// Callers should NOT treat the SessionEnded as a turn boundary in this
    /// case (don't flip status to terminal, don't decrement parent child
    /// counters, don't fire completion callbacks). Defaults to `false` so any
    /// future variant is treated as terminal unless explicitly opted in.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::StaleResume)
    }
}

/// Outcome of a child thread that just completed, surfaced to the parent via
/// `ThreadEvent::ChildThreadCompleted`. Replaces the prose discriminator
/// strings (`"completed with proposed changes"`, `"completed (no changes)"`,
/// `"completed"`, `"failed"`) the previous `[Child thread completed]`
/// callback embedded in its plaintext body.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildCompletionStatus {
    /// Child finished its turn normally (with or without proposed changes).
    Success,
    /// Child terminated with `ResponseFailed` — the parent should treat the
    /// summary as an error string and decide whether to retry / give up.
    Failure,
    /// CC child idled with no pending changes proposed. Distinguished from
    /// `Success` so the parent can branch on "did the child actually produce
    /// reviewable work?" without re-querying the changes table.
    NoChanges,
    /// Child was canceled by the user (`ResponseCanceled`). The summary is
    /// the partial response text (or empty); the parent can decide whether to
    /// retry, prompt again, or give up. `ResponseAborted` (system-driven, e.g.
    /// engine restart) deliberately does NOT surface here — that case is
    /// transient and the engine resumes the child on next visit.
    Canceled,
}
