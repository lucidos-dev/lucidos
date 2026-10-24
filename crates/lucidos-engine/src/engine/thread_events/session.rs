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

/// Whether a coding-agent session carried the thread's earlier context. Carried
/// by `SessionStarted`, so the thread can say when a session started over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum SessionContinuity {
    /// The thread's first session. There was nothing to carry.
    First,
    /// The session resumed the previous one, so its context carried over.
    Resumed,
    /// The thread had a session before, and this one starts without it.
    Fresh { reason: FreshSessionReason },
    /// A *successor session*: the thread handed off, and this session starts
    /// from the handoff's opening.
    Handoff {
        /// The *handoff checkpoint* the handoff ran at, when one did.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        checkpoint: Option<String>,
        /// The checkpoint's progress, as `<done>/<total>`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<String>,
        opening: HandoffOpening,
        /// The opening itself, which the transcript folds under the session's
        /// card. The first input may also carry the message that started it.
        message: String,
    },
}

/// What a *successor session* starts from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HandoffOpening {
    /// The *handoff narrative* the previous session wrote.
    Narrative,
    /// A *handoff brief*: the engine's note on how to rebuild the context.
    Brief,
}

/// Why a session on a thread that had one before started fresh.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FreshSessionReason {
    /// The thread's change was applied, which ends its session.
    ChangeApplied,
    /// The thread's change was discarded, which ends its session.
    ChangeDiscarded,
    /// The previous session ended rather than went idle.
    SessionEnded,
    /// The recorded branch is gone, and no rename of it was proven.
    BranchGone,
    /// The coding agent refused to resume the previous session.
    ResumeFailed,
    /// No resumable session was recorded for the thread's account.
    NoResumableSession,
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
    /// The child is not continuing. The user archived, deleted or discarded
    /// it, or an agent canceled its own child. A person's Stop is never this:
    /// it sends `ChildThreadStopped` (ADR 0252). The summary says what ended
    /// it; the parent can decide whether to retry, prompt again, or give up.
    Canceled,
    /// An engine restart cut the child's turn, and nothing promised to resume
    /// it: a crash, or a teardown the user did not ask for. The child is not
    /// done and not dead. A follow-up continues it. The user's own switch is
    /// never this: it resumes the child, and the resumed turn reports.
    Interrupted,
}

/// The retired *handoff consent* that let a child's successor thread start
/// under its parent (ADR 0507). Kept so legacy `SuccessorThreadSpawned` events
/// still deserialize. A handoff needs no authority now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HandoffConsent {
    /// The owner's device asked, or a caller carrying their standing
    /// instruction did.
    Owner,
    /// The handoff spent an owner approval (ADR 0387 and 0493).
    OwnerApproval,
    /// The handoff passed on the consent its predecessor carried.
    Carried,
}

/// What a child's clean finish does to its parent, declared by the spawn: the
/// child's *wake mode* (ADR 0451). A failure wakes the parent under every mode
/// but `NoWake`, which files every report (ADR 0545).
///
/// A child spawned with no mode (before modes existed) has none, and its turns
/// fall back to the *wake decision*.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WakeMode {
    /// The user reads the result, and the parent may still want it. A clean
    /// report is filed, and the parent reads it on its next turn.
    NoWake,
    /// The parent needs the result to continue, so every report wakes it.
    Wake,
    /// The parent combines several results. A clean report wakes it only when
    /// no other child of its batch is still working, and is filed before that.
    WakeWhenAllDone,
}

impl WakeMode {
    /// Every mode, in the order the docs list them.
    pub const ALL: [Self; 3] = [Self::NoWake, Self::Wake, Self::WakeWhenAllDone];

    /// The wire name, which is also the stored column value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoWake => "no-wake",
            Self::Wake => "wake",
            Self::WakeWhenAllDone => "wake-when-all-done",
        }
    }

    /// Read a wire name. `None` for anything else.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == value)
    }

    /// The wire names, comma-separated, for an error that lists them.
    pub fn names() -> String {
        Self::ALL
            .iter()
            .map(|mode| mode.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// The mode a child spawn runs under: the one it declared, else the
    /// default for its parent. The home thread's work is for the user, so its
    /// children default to `NoWake`. Every other parent defaults to `Wake`.
    pub fn for_spawn(declared: Option<Self>, parent_is_home: bool) -> Self {
        declared.unwrap_or(if parent_is_home {
            Self::NoWake
        } else {
            Self::Wake
        })
    }

    /// Read a spawn's optional `wake_mode` argument. A value that names no
    /// mode is an error listing the valid ones, never a silent default.
    pub fn from_arg(value: Option<&str>) -> Result<Option<Self>, String> {
        match value {
            None => Ok(None),
            Some(name) => Self::parse(name).map(Some).ok_or_else(|| {
                format!("`wake_mode` must be one of {}, not `{name}`", Self::names())
            }),
        }
    }
}

/// One pending change held by a sub-thread of the thread a
/// `ChildThreadCompleted` reports on: any descendant, not the thread itself.
///
/// A snapshot taken when the card was sent. The parent reads it to know what
/// work waits below, so it never reads "no pending changes" over a subtree
/// full of them.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SubThreadPendingChange {
    pub change_id: uuid::Uuid,
    /// The sub-thread that proposed the change.
    pub thread_id: uuid::Uuid,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_title: Option<String>,
    /// Same meaning as `PendingThreadState::unsettled`: that sub-thread was still
    /// working on the change when the card was sent.
    pub thread_unsettled: bool,
}

/// The branch an integrated sub-thread hands up to its parent, named on the
/// child's report. Set only when the child's work is hardened and waits for
/// the parent's agent to merge it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntegratedBranch {
    pub branch_name: String,
    /// The head the child's `/harden` certified.
    pub head_sha: String,
}

#[cfg(test)]
mod tests {
    use super::WakeMode;

    #[test]
    fn each_wake_mode_name_is_its_serde_name() {
        for mode in WakeMode::ALL {
            assert_eq!(serde_json::to_value(mode).unwrap(), mode.as_str());
            assert_eq!(WakeMode::parse(mode.as_str()), Some(mode));
        }
    }

    #[test]
    fn a_spawn_defaults_by_parent_and_a_declared_mode_wins() {
        assert_eq!(WakeMode::for_spawn(None, true), WakeMode::NoWake);
        assert_eq!(WakeMode::for_spawn(None, false), WakeMode::Wake);
        for mode in WakeMode::ALL {
            assert_eq!(WakeMode::for_spawn(Some(mode), true), mode);
            assert_eq!(WakeMode::for_spawn(Some(mode), false), mode);
        }
    }

    #[test]
    fn an_unknown_wake_mode_is_refused_with_the_valid_names() {
        assert_eq!(WakeMode::from_arg(None), Ok(None));
        assert_eq!(WakeMode::from_arg(Some("wake")), Ok(Some(WakeMode::Wake)));
        let err = WakeMode::from_arg(Some("file")).unwrap_err();
        assert!(err.contains("no-wake, wake, wake-when-all-done"), "{err}");
    }
}
