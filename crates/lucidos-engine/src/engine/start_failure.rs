//! Failure types reported before the first coding-agent prompt.

use crate::engine::thread_events::CodingAgentExit;

/// A coding agent that exited before the engine could send its first prompt.
///
/// It travels as the `Err` of a spawn, and the turn's failure terminator reads
/// it to attach the exit to `ResponseFailed`. A caller that only formats the
/// error sees `message`, the CLI's own words.
#[derive(Debug)]
pub struct AgentStartFailure {
    pub message: String,
    pub exit: CodingAgentExit,
}

impl std::fmt::Display for AgentStartFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AgentStartFailure {}

/// The exit a spawn error carries, when it is an [`AgentStartFailure`].
pub fn agent_exit_of(
    error: &(dyn std::error::Error + Send + Sync + 'static),
) -> Option<CodingAgentExit> {
    error.downcast_ref::<AgentStartFailure>().map(|f| f.exit)
}

/// A remote start the gate parked on the owner's card. It is waiting, not
/// failed, so no caller writes a terminator: the card holds the turn.
#[derive(Debug)]
pub struct StartAwaitsOwner;

impl std::fmt::Display for StartAwaitsOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "An agent on a paired gateway started this thread, so it waits for the owner's \
             answer on its card."
        )
    }
}

impl std::error::Error for StartAwaitsOwner {}

/// Whether `error` is a run parked on the owner's card rather than a failure.
pub fn awaits_owner(error: &(dyn std::error::Error + Send + Sync + 'static)) -> bool {
    error.is::<StartAwaitsOwner>()
}
