use crate::engine::thread_events::CodingAgentExit;

/// A coding agent that exited before the engine could send its first prompt.
///
/// It travels as the `Err` of a spawn, and the turn's failure terminator reads
/// it to attach the exit to `ResponseFailed`. A caller that only formats the
/// error sees `message`, the CLI's own words.
#[derive(Debug)]
pub(crate) struct AgentStartFailure {
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
pub(crate) fn agent_exit_of(
    error: &(dyn std::error::Error + Send + Sync + 'static),
) -> Option<CodingAgentExit> {
    error.downcast_ref::<AgentStartFailure>().map(|f| f.exit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::thread_events::CodingAgentExitStage;

    #[test]
    fn a_boxed_start_failure_gives_back_its_exit_and_prints_its_message() {
        let exit = CodingAgentExit {
            stage: CodingAgentExitStage::Startup,
            code: Some(1),
        };
        let boxed: Box<dyn std::error::Error + Send + Sync> = Box::new(AgentStartFailure {
            message: "no login".into(),
            exit,
        });
        assert_eq!(boxed.to_string(), "no login");
        assert_eq!(agent_exit_of(boxed.as_ref()), Some(exit));
    }

    #[test]
    fn a_plain_error_carries_no_exit() {
        let plain: Box<dyn std::error::Error + Send + Sync> = "boom".into();
        assert_eq!(agent_exit_of(plain.as_ref()), None);
    }
}
