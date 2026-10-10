use crate::engine::change_ops::{PlanHold, ProposalHold};
use crate::engine::thread_lifecycle::ThreadStatus;
use serde::{Deserialize, Serialize};

/// Why a `ResponseCanceled` was emitted. Cancellation is always a user-driven
/// action that interrupts a *real* in-flight response. The actor on
/// `EventMeta` identifies the user, and this enum identifies what they did. New
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

/// Why a `ResponseAborted` was emitted. Aborts are system-driven cleanup:
/// the engine or the OS terminated the process, or the engine settled a
/// projection whose live process was already gone. The actor on `EventMeta`
/// records *who triggered* the cleanup (a user button can fire stale-settle),
/// not who decided to terminate the work. That is always the system. New emit
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
    /// True when the abort came from an engine restart (shutdown or recovery
    /// sweep) rather than from the turn itself failing. `SafetyNet`,
    /// `ProcessKilled`, `StaleSettle`, `SessionDropped` and the legacy
    /// `Unknown` are not.
    ///
    /// **It does not say whether the child comes back.** It has no actor axis,
    /// so it cannot tell the user's own *Switch to new version* (which
    /// auto-resumes) from a crash (which does not). Whatever depends on a
    /// resume keys on [`promises_auto_resume`](Self::promises_auto_resume):
    /// the turn's verdict, the parent fan-in and its in-tx counter mirror. The
    /// fan-in uses this one only to tell a restart cut, which it reports as
    /// `interrupted`, from a turn that failed on its own.
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
    /// Deliberately NOT [`is_transient`](Self::is_transient), which only says a
    /// restart caused the abort and has no actor axis at all.
    pub fn promises_auto_resume(&self, actor: Option<&super::MessageOrigin>) -> bool {
        matches!(self, Self::EngineShutdown)
            && matches!(actor, Some(super::MessageOrigin::Device { .. }))
    }

    /// SQL fragment for the `status` column on the `thread_summaries` row when
    /// this abort lands, given the actor that stamped it. Three outcomes:
    ///
    /// * **`StaleSettle`** is engine cleanup of a stuck row whose process was
    ///   already gone, fired by a user button (Stop / Apply / Discard / Archive /
    ///   Interrupt). No real abort happened, so it uses the cancel-style
    ///   `STATUS_FROM_PROPOSED_CHANGE` mapping rather than any verdict.
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
    /// The paused arm keyed on [`is_transient`](Self::is_transient) until it got
    /// crashes wrong for the reason that method's own doc gives. The verdict is
    /// about whether anyone is coming back for this turn, and only the actor
    /// can say.
    ///
    /// **A pending change does not change the answer.** Both verdict arms used
    /// to open with a pending-change check writing `'waiting'`, on the
    /// reasoning that a change to review outranks the interruption. That
    /// ordering is right and it survives, but it belongs to the *reader*: the
    /// frontend's `resolveVisualStatus` already returns `failed` and `paused`
    /// ahead of `changes`, and the change state is what carries the
    /// change. Saying it a second time here cost the verdict outright, because
    /// `'waiting'` is not a `PRESERVED_STATUS_VERDICTS` value: the dying
    /// subprocess's drain landed milliseconds later and wrote `'running'` over
    /// it. It also hid such a thread from the boot floor, which is scoped
    /// `status = 'paused'`. See
    /// `docs/plans/2026-08-22-a-restart-verdict-survives-a-pending-change.md`.
    ///
    /// Both verdicts must also survive the dying turn's trailing events. See
    /// `event_bus::preserving_verdict`, whose list this function feeds.
    pub fn status_sql(&self, actor: Option<&super::MessageOrigin>) -> &'static str {
        match self {
            Self::StaleSettle => crate::engine::event_bus::STATUS_FROM_PROPOSED_CHANGE,
            _ if self.promises_auto_resume(actor) => ThreadStatus::Paused.sql_literal(),
            _ => ThreadStatus::Failed.sql_literal(),
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
/// `EventWaitExpired`, which re-opens the thread rather than stopping it.
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
    /// waiting indicator.
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

impl EventWaitCancelCause {
    /// The cancel ended one subscription and left the thread open. Nothing
    /// re-opens it, so whatever the wait held back falls due now: a child's
    /// card for its parent, and a held idle proposal (ADR 0395). An archive or
    /// a discard ended the thread itself, and its own paths settle both.
    pub fn leaves_thread_open(self) -> bool {
        matches!(self, Self::UserStop | Self::AgentStandDown)
    }
}

/// Why a turn end left work on its branch unproposed (ADR 0400). The hold
/// variants are [`ProposalHold`]'s, mapped by the exhaustive `From` below, so a
/// new hold cannot compile without a wire name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnproposedReason {
    /// The branch has no implementation-plan marker.
    PlanMissing,
    /// A plan is recorded but the user has not approved it.
    PlanAwaitingApproval,
    /// A bounded security fix reaches past the files it named.
    OutsideBound,
    /// The work never ran `/harden`, so Apply would harden it first.
    HardeningMissing,
    /// The turn did not finish: a Stop, a failure, an abort or a cut-off.
    TurnIncomplete,
}

impl UnproposedReason {
    pub const ALL: [Self; 5] = [
        Self::PlanMissing,
        Self::PlanAwaitingApproval,
        Self::OutsideBound,
        Self::HardeningMissing,
        Self::TurnIncomplete,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlanMissing => "plan_missing",
            Self::PlanAwaitingApproval => "plan_awaiting_approval",
            Self::OutsideBound => "outside_bound",
            Self::HardeningMissing => "hardening_missing",
            Self::TurnIncomplete => "turn_incomplete",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == value)
    }
}

impl From<ProposalHold> for UnproposedReason {
    fn from(hold: ProposalHold) -> Self {
        match hold {
            ProposalHold::Plan(PlanHold::Missing) => Self::PlanMissing,
            ProposalHold::Plan(PlanHold::AwaitingApproval) => Self::PlanAwaitingApproval,
            ProposalHold::Plan(PlanHold::OutsideBound) => Self::OutsideBound,
            ProposalHold::HardeningMissing => Self::HardeningMissing,
        }
    }
}
