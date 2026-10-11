//! The *turn-end gate*: what a turn must meet before its response is recorded
//! (ADR 0417).
//!
//! Every *turn-end requirement* is checked here. Both turn ends call the gate:
//! the chat loop before `ResponseGenerated`, and the session runner around
//! `CodingAgentIdled`. Each requirement says how it is enforced: a forced
//! decision the engine makes, or a session re-entry that sends a coding agent
//! back to do the work.
//!
//! The re-entry rules are the *proposal hold*'s, which owns when Lucidos-source
//! work lacks a plan marker or a harden, and what the agent is told. This
//! module owns only that a hold sends the agent back, and how often.

mod read_decision;

use crate::engine::change_ops::ProposalHold;

/// One rule a turn must meet before it ends, in the order the gate checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnEndRequirement {
    /// The turn says whether its reply is worth reading. A forced decision.
    ReadDecision,
    /// The proposal hold kept the turn's work: a session re-entry, since only
    /// the agent can set a plan marker or run `/harden`.
    Reentry(ProposalHold),
}

/// What a coding-agent session knows at its turn end, for the re-entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SessionFacts {
    /// The branch HEAD, the key each re-entry is bounded by. An unreadable
    /// HEAD keys as the empty string, so it still sends back once.
    pub(crate) head: Option<String>,
    /// Why the proposal hold kept this idle's work, if it did.
    pub(crate) held: Option<ProposalHold>,
}

/// The state one turn ends in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct TurnEndState {
    /// Whether this turn recorded a read decision.
    pub(crate) read_decided: bool,
    /// Present only for a session whose turn completed. Absent after a Stop,
    /// a failure or an abort, and for a chat turn.
    pub(crate) session: Option<SessionFacts>,
}

impl TurnEndState {
    /// A coding-agent turn end. Its re-entry facts count only when the turn
    /// completed: never after a user Stop, which means stop.
    pub(crate) fn at_session_idle(
        read_decided: bool,
        completed: bool,
        facts: SessionFacts,
    ) -> Self {
        Self {
            read_decided,
            session: completed.then_some(facts),
        }
    }

    /// The requirements this turn has not met, in the gate's order.
    pub(crate) fn unmet(&self) -> Vec<TurnEndRequirement> {
        let mut unmet = Vec::new();
        if !self.read_decided {
            unmet.push(TurnEndRequirement::ReadDecision);
        }
        if let Some(hold) = self.session.as_ref().and_then(|s| s.held) {
            unmet.push(TurnEndRequirement::Reentry(hold));
        }
        unmet
    }
}

/// The branch HEAD each kind of hold last sent the agent back for.
///
/// Held for the session's life, so each kind sends the agent back at most once
/// per HEAD. An agent that ignores one ends its turn rather than looping, and
/// new commits earn a new one. The plan and the harden keep separate keys, so
/// settling the plan still lets the harden ask through at the same HEAD.
#[derive(Debug, Default)]
pub(crate) struct ReentryLedger {
    plan: Option<String>,
    hardening: Option<String>,
}

impl ReentryLedger {
    fn slot(&mut self, hold: ProposalHold) -> &mut Option<String> {
        match hold {
            ProposalHold::Plan(_) => &mut self.plan,
            ProposalHold::HardeningMissing => &mut self.hardening,
        }
    }

    /// The hold `state` makes due, recorded as sent. An idle sends at most
    /// one re-entry, since a branch has at most one hold.
    pub(crate) fn take_due(&mut self, state: &TurnEndState) -> Option<ProposalHold> {
        let head = state
            .session
            .as_ref()
            .and_then(|s| s.head.clone())
            .unwrap_or_default();
        state.unmet().into_iter().find_map(|requirement| {
            let TurnEndRequirement::Reentry(hold) = requirement else {
                return None;
            };
            let slot = self.slot(hold);
            (slot.as_deref() != Some(head.as_str())).then(|| {
                *slot = Some(head.clone());
                hold
            })
        })
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
