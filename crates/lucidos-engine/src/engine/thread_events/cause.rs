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
    /// A user follow-up arrived while a turn was in flight, so the engine
    /// interrupted the live turn to run the follow-up as the next turn (the
    /// mid-turn redirect: see `arm_followup_redirect`). Mechanically a cancel (no
    /// `ResponseGenerated`, no change proposal for the redirected-away partial
    /// work), but NOT a user Stop: the user steered, they didn't abandon. The
    /// frontend renders this neutrally — like the chat/CC follow-up — instead of
    /// the "Canceled ✕" + "Response canceled" panel that `UserStop` gets.
    SupersededByFollowup,
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
    /// The `run_session` future was dropped instead of completed — its caller
    /// was cancelled, so the whole session went with it mid-turn. The classic
    /// source is an HTTP handler that awaited a session inline and lost its
    /// client (the 2026-07-28 Apply-over-mobile incident: the merge session
    /// died 72 s in when iOS Safari dropped the connection). Distinct from
    /// `ProcessKilled` (the subprocess died under a live loop) and from
    /// `SafetyNet` (the loop ran to EOF without a `Result`): here the loop
    /// itself never got to run its cleanup, so the abort is emitted by the
    /// session entry's drop-guard.
    SessionDropped,
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
    ///
    /// This is a question about the **parent's counter**, not about how the turn
    /// reads to the user. It has no actor axis, so it cannot tell the user's own
    /// *Switch to new version* (which auto-resumes) from a crash recovery sweep
    /// (which does not). [`status_sql`](Self::status_sql) keyed on it until
    /// 2026-08-06 and got crashes wrong for exactly that reason; the verdict now
    /// keys on [`promises_auto_resume`](Self::promises_auto_resume) instead.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::EngineShutdown | Self::RecoveryAfterRestart)
    }

    /// True when this abort is the teardown boundary of a **user-initiated**
    /// *Switch to new version*, i.e. when the engine has PROMISED to resume the
    /// turn by itself. The fingerprint is both halves together: cause
    /// `EngineShutdown` **and** a `Device` actor.
    ///
    /// This is the Rust form of `agent_recovery::SWITCH_TEARDOWN_ABORT_SQL`, the
    /// single definition both resume gates key on (`switch_was_user_initiated`
    /// for coding agents, `chat::recovery::switch_resume_candidates` for chat and
    /// trigger threads), and of the frontend's `abortPromisesAutoResume`
    /// (`store/thread-events/exchange-render.ts`), which withholds the Continue
    /// button on exactly this shape. Three surfaces, one rule: a turn reads
    /// `paused` iff a resume was promised iff no Continue button is offered.
    ///
    /// **A device actor alone is not the fingerprint.** `StaleSettle`
    /// deliberately carries the actor of whichever user button exposed a stuck
    /// row (Stop / Apply / Discard / Archive / Interrupt), so an actor-only test
    /// would read a user Stop as a switch. Nor is `EngineShutdown` alone: a
    /// teardown nobody requested (`stop.sh`, an external SIGUSR1, ctrl-c) emits
    /// the same cause with a system actor, and no resume gate picks that up.
    ///
    /// Which threads get the device half is a question about the TEARDOWN, never
    /// about when a thread became in-flight. Every `EngineShutdown` emit in one
    /// teardown reads the same `LucidosEngine::teardown_actor`, so the pre-emit,
    /// the `shutdown_active_threads` fallback and the `emit_stop_terminal` abort
    /// arm cannot disagree. They did until 2026-08-07, when only the pre-emit had
    /// the actor: see
    /// `docs/plans/2026-08-07-teardown-actor-is-one-value-for-the-whole-teardown.md`.
    ///
    /// Deliberately NOT [`is_transient`](Self::is_transient), which answers a
    /// different question (may the parent's `active_children_count` decrement?)
    /// and has no actor axis at all.
    pub fn promises_auto_resume(&self, actor: Option<&super::MessageOrigin>) -> bool {
        matches!(self, Self::EngineShutdown)
            && matches!(actor, Some(super::MessageOrigin::Device { .. }))
    }

    /// SQL fragment for the `status` column on the `thread_summaries` row when
    /// this abort lands, given the actor that stamped it. Three outcomes:
    ///
    /// * **`StaleSettle`** is engine cleanup of a stuck row whose process was
    ///   already gone, fired by a user button (Stop / Apply / Discard / Archive /
    ///   Interrupt). No real abort happened, so it uses the cancel-style mapping
    ///   (idle, or waiting if pending changes) rather than any verdict.
    /// * **A promised auto-resume** ([`promises_auto_resume`](Self::promises_auto_resume),
    ///   the user's own *Switch to new version*) surfaces `paused`: nothing
    ///   failed, and the engine brings the turn back by itself, usually within
    ///   seconds. Reporting that as `failed` was the original bug: a switch
    ///   painted every in-flight thread with the red error dot for work already
    ///   on its way back.
    /// * **Everything else** is a real interruption nobody promised to undo, and
    ///   keeps the red `failed` indicator: `SafetyNet`, `ProcessKilled`,
    ///   `SessionDropped`, `Unknown`, every `RecoveryAfterRestart` (the crash
    ///   boundary, and the boot floor's withdrawal of a resume promise it could
    ///   not keep), and a system-actor `EngineShutdown`.
    ///
    /// The paused arm keyed on [`is_transient`](Self::is_transient) until
    /// 2026-08-06, which was too wide in exactly the direction that matters:
    /// `RecoveryAfterRestart` is transient, so a crash, and the boot floor
    /// handing the Continue button *back*, both sat behind a reassuring pause
    /// glyph and stayed out of the needs-attention count. Transience is about the
    /// parent's child counter; the verdict is about whether anyone is coming
    /// back for this turn, and only the actor can say.
    ///
    /// Pending changes override both verdicts to `waiting`: a change ready to
    /// review is more actionable than either the interruption or the failure.
    ///
    /// Both verdicts must also survive the dying turn's trailing events. See
    /// `event_bus::preserving_verdict`, whose list this function feeds.
    pub fn status_sql(&self, actor: Option<&super::MessageOrigin>) -> &'static str {
        match self {
            Self::StaleSettle => crate::engine::event_bus::STATUS_FROM_PROPOSED_CHANGE,
            _ if self.promises_auto_resume(actor) => {
                "CASE WHEN coding_agent_proposed THEN 'waiting' ELSE 'paused' END"
            }
            _ => "CASE WHEN coding_agent_proposed THEN 'waiting' ELSE 'failed' END",
        }
    }
}

/// Why an `EventWaitCanceled` was emitted: how a *thread subscription* was
/// stopped short of its own resolution.
///
/// Every live arm is somebody deciding to stop it, and each one is announced.
/// The **Stop waiting** button is the user ending one directly, archive and
/// discard end the thread that holds it (and the archive asks first, naming
/// what it would stop), and a stand-down is the agent retiring a watch it armed
/// after the user told it to. A timeout is not here at all: that is
/// `EventWaitExpired`, which wakes the thread rather than stopping it.
///
/// Two things are deliberately absent.
///
/// An **ordinary user message** does not stop a subscription. Typing into a
/// subscribed thread runs a normal turn and leaves every subscription exactly
/// as it was, so asking "how's it going?" cannot silently throw away a
/// forty-minute watch. Cancelling on any `MessageReceived` was the original
/// design and was rejected for exactly that.
///
/// A **thread-level Stop** does not either, as of 2026-08-07. See
/// [`Self::ThreadCanceled`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventWaitCancelCause {
    /// The **Stop waiting** button on the subscription itself, in the
    /// subscription indicator.
    UserStop,
    /// The agent stood a subscription of its own down, through
    /// `cancel_event_wait` / `lucidos event-waits cancel`. Its own arm so the
    /// event log can tell "the user told it to stand down" from a person
    /// pressing a button, from an archive, and from a timeout.
    AgentStandDown,
    /// The thread was archived. Archive is a legitimate way to stop a
    /// subscription, and leaving one live behind the archive curtain would wake
    /// a thread the user considers closed. The confirm in `handleArchiveThread`
    /// names every subscription the cascade would stop before it happens.
    ThreadArchived,
    /// The thread was discarded.
    ThreadDiscarded,
    /// **Retired, and still read.** A thread-level Stop used to stop every
    /// subscription on the thread; it no longer does, and nothing emits this.
    /// It stays in the enum, and stays deserializable, because rows written
    /// before 2026-08-07 carry it and events are append-only: dropping the arm
    /// would replay them as [`Self::Unknown`] and lose why they ended.
    ///
    /// A Stop is turn-scoped. Cancelling unrelated subscriptions from it killed
    /// a watch armed two hours earlier with nothing anywhere saying so, and a
    /// subscription has not held a turn since ADR 0049, so it was never part of
    /// what a Stop owns. Do not re-add the emit; see `api::chat::cancel_chat`.
    ThreadCanceled,
    /// Legacy or unrecognized cause, so old DB rows replay cleanly. Never emit
    /// fresh.
    #[serde(other)]
    Unknown,
}
