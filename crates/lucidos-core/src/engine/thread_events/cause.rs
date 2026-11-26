use super::change_hold::{PlanHold, ProposalHold};
use crate::engine::thread_lifecycle::ThreadStatus;
use serde::{Deserialize, Serialize};

/// INVARIANT: Why a `ResponseCanceled` was emitted. Cancellation is always a user-driven
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
    UserStop,
    UserAction,
    /// INVARIANT: A user follow-up arrived while a turn was in flight, so the engine
    /// interrupted the live turn to run the follow-up as the next turn (the
    /// mid-turn redirect: see `arm_followup_redirect`). Mechanically a cancel (no
    /// `ResponseGenerated`, no change proposal for the redirected-away partial
    /// work), but NOT a user Stop: the user steered, they didn't abandon. The
    /// frontend renders this neutrally — like the chat/CC follow-up — instead of
    /// the "Canceled ✕" + "Response canceled" panel that `UserStop` gets.
    SupersededByFollowup,
    EffortRestart,
    Refused,
    Hold,
    #[serde(other)]
    Unknown,
}

impl CancelCause {
    pub const ALL: [Self; 7] = [
        Self::UserStop,
        Self::UserAction,
        Self::SupersededByFollowup,
        Self::EffortRestart,
        Self::Refused,
        Self::Hold,
        Self::Unknown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::UserStop => "user_stop",
            Self::UserAction => "user_action",
            Self::SupersededByFollowup => "superseded_by_followup",
            Self::EffortRestart => "effort_restart",
            Self::Refused => "refused",
            Self::Hold => "hold",
            Self::Unknown => "unknown",
        }
    }

    pub fn is_redirect(self) -> bool {
        matches!(self, Self::SupersededByFollowup | Self::EffortRestart)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AbortCause {
    EngineShutdown,
    SafetyNet,
    RecoveryAfterRestart,
    ProcessKilled,
    StaleSettle,
    SessionDropped,
    SnapshotRestored,
    #[serde(other)]
    Unknown,
}

impl AbortCause {
    /// INVARIANT: True when the abort came from an engine restart (shutdown or recovery
    /// sweep) rather than from the turn itself failing. `SafetyNet`,
    /// `ProcessKilled`, `StaleSettle`, `SessionDropped`, `SnapshotRestored` and the legacy
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

    /// INVARIANT: True when this abort is the teardown boundary of a **user-initiated**
    /// *Switch to new version*, i.e. when the engine has PROMISED to resume the
    /// turn by itself. The fingerprint is both halves together: cause
    /// `EngineShutdown` **and** a `Device` actor.
    ///
    /// This is the Rust form of `crate::engine::switch_recovery_sql::SWITCH_TEARDOWN_ABORT_SQL`, the
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

    /// INVARIANT: SQL fragment for the `status` column on the `thread_summaries` row when
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
    ///   `SessionDropped`, `SnapshotRestored`, `Unknown`, every `RecoveryAfterRestart` (the crash
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventWaitCancelCause {
    UserStop,
    AgentStandDown,
    /// INVARIANT: The thread was archived. Archive is a legitimate way to stop a
    /// subscription, and leaving one live behind the archive curtain would wake
    /// a thread the user considers closed. The confirm in `handleArchiveThread`
    /// names every subscription the cascade would stop before it happens.
    ThreadArchived,
    ThreadDiscarded,
    ThreadCanceled,
    SnapshotRestored,
    #[serde(other)]
    Unknown,
}

impl EventWaitCancelCause {
    pub fn leaves_thread_open(self) -> bool {
        matches!(self, Self::UserStop | Self::AgentStandDown)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnproposedReason {
    PlanMissing,
    PlanChanged,
    PlanUnreadable,
    PlanAwaitingApproval,
    PlanOnlyCode,
    OutsideBound,
    HardeningMissing,
    TurnIncomplete,
    AwaitingLanding,
    IntegratedIntoParent,
    CarriedBySuccessor,
    HookRefused,
}

impl UnproposedReason {
    pub const ALL: [Self; 12] = [
        Self::PlanMissing,
        Self::PlanChanged,
        Self::PlanUnreadable,
        Self::PlanAwaitingApproval,
        Self::PlanOnlyCode,
        Self::OutsideBound,
        Self::HardeningMissing,
        Self::TurnIncomplete,
        Self::AwaitingLanding,
        Self::IntegratedIntoParent,
        Self::CarriedBySuccessor,
        Self::HookRefused,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlanChanged => "plan_changed",
            Self::PlanMissing => "plan_missing",
            Self::PlanUnreadable => "plan_unreadable",
            Self::PlanAwaitingApproval => "plan_awaiting_approval",
            Self::PlanOnlyCode => "plan_only_code",
            Self::OutsideBound => "outside_bound",
            Self::HardeningMissing => "hardening_missing",
            Self::TurnIncomplete => "turn_incomplete",
            Self::AwaitingLanding => "awaiting_landing",
            Self::IntegratedIntoParent => "integrated_into_parent",
            Self::CarriedBySuccessor => "carried_by_successor",
            Self::HookRefused => "hook_refused",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == value)
    }
}

impl From<ProposalHold> for UnproposedReason {
    fn from(hold: ProposalHold) -> Self {
        match hold {
            ProposalHold::Plan(PlanHold::Changed) => Self::PlanChanged,
            ProposalHold::Plan(PlanHold::Missing) => Self::PlanMissing,
            ProposalHold::Plan(PlanHold::Unreadable) => Self::PlanUnreadable,
            ProposalHold::Plan(PlanHold::AwaitingApproval | PlanHold::PlanOnlyAwaitingApproval) => {
                Self::PlanAwaitingApproval
            }
            ProposalHold::Plan(PlanHold::PlanOnlyCode) => Self::PlanOnlyCode,
            ProposalHold::Plan(PlanHold::OutsideBound) => Self::OutsideBound,
            ProposalHold::HardeningMissing => Self::HardeningMissing,
            ProposalHold::IntegratedIntoParent => Self::IntegratedIntoParent,
            ProposalHold::CarriedBySuccessor => Self::CarriedBySuccessor,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPauseCause {
    Mention,
    Chip,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationUnpauseCause {
    Unpause,
    Archived,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldLiftCause {
    Resume,
    Archived,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnqueueReason {
    Discarded,
    SetAside,
    Removed,
    BatchFailed,
    LandingFailed,
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;

    #[test]
    fn restored_snapshot_never_promises_resume_or_transient_retry() {
        let cause: AbortCause = serde_json::from_str("\"snapshot_restored\"").unwrap();
        assert_eq!(cause, AbortCause::SnapshotRestored);
        assert!(!cause.is_transient());
        assert!(!cause.promises_auto_resume(None));
        assert!(
            !cause.promises_auto_resume(Some(&super::super::MessageOrigin::Device {
                device_id: "owner-device".into(),
            }))
        );
        assert_eq!(
            serde_json::to_string(&cause).unwrap(),
            "\"snapshot_restored\""
        );
    }
}
