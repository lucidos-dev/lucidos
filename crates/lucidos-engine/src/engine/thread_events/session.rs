use serde::{Deserialize, Serialize};

/// Why a session ended. The frontend uses it for status and display logic.
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

/// Why a voice session ended.
///
/// A cause rather than a free string, so a reader can branch on it and a
/// trigger can filter on it. Distinct from [`SessionEndReason`], which is the
/// coding-agent session: the two share a word and nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceSessionEndReason {
    /// The caller hung up. The ordinary ending.
    Hangup,
    /// The caller said the conversation was over, and the talker rang off for
    /// them. Told apart from [`Self::Hangup`] for the reason
    /// [`Self::Disconnected`] already is: the two read alike to the user and
    /// differently in the log.
    ///
    /// It ends the call and never the work. A turn in flight keeps running.
    AgentHangup,
    /// The socket died without a goodbye: the network dropped, or the page
    /// closed. Indistinguishable from a hangup to the user, and worth telling
    /// apart in the log.
    Disconnected,
    /// The talker failed and the call could not go on. Carries no message here:
    /// the engine logs the detail, and a reason a trigger can match on is worth
    /// more than prose nobody reads.
    ProviderFailed,
    /// The engine went away under the call. Emitted by the shutdown path, or by
    /// the boot sweep for a start whose process never got that far.
    EngineShutdown,
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
