//! The *turn-end gate*: what a turn must meet before its response is recorded
//! (ADR 0417).
//!
//! Every *turn-end requirement* is checked here. Both turn ends call the gate:
//! the chat loop before `ResponseGenerated`, and the session runner around
//! `CodingAgentIdled`. Each requirement says how it is enforced: a forced
//! decision the engine makes, or a session re-entry that sends a coding agent
//! back to do the work. The forced decisions are the read decision and, for a
//! thread that reports to a parent, the wake decision. Beside them, a turn that
//! leaves its thread waiting with the keep-alive on gets an expected wake.
//!
//! Three re-entries exist. The *ceiling re-entry* asks a session past its
//! *context ceiling* to hand off (ADR 0629). The *approved-plan re-entry* tells
//! a session to build the plan the user approved this turn, when the turn
//! changed no source file. The *proposal hold*'s owns when Lucidos-source work
//! lacks a plan marker or a harden, and what the agent is told. This module
//! owns only that each sends the agent back, and how often.

pub(crate) mod expected_wake_decision;
pub(crate) mod read_decision;
pub(crate) mod wake_decision;

pub use read_decision::{
    decide_read_for_eval, read_decision_chat_judge, AnsweredQuestion, EvalReadDecision, InFlight,
    OpeningSource, PushedNotification, TurnRead,
};
pub(crate) use wake_decision::TurnFinish;
pub use wake_decision::{
    decide_wake_for_eval, wake_decision_chat_judge, EvalWakeDecision, ParentView,
};

pub(crate) use crate::engine::turn_requirements::*;

#[cfg(test)]
use crate::engine::change_ops::ProposalHold;

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
