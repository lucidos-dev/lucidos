use crate::engine::thread_events::CodingAgentExit;

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

pub fn agent_exit_of(
    error: &(dyn std::error::Error + Send + Sync + 'static),
) -> Option<CodingAgentExit> {
    error.downcast_ref::<AgentStartFailure>().map(|f| f.exit)
}

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

pub fn awaits_owner(error: &(dyn std::error::Error + Send + Sync + 'static)) -> bool {
    error.is::<StartAwaitsOwner>()
}
