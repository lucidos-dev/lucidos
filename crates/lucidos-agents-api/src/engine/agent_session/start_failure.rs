use crate::engine::start_failure::{agent_exit_of, AgentStartFailure};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::thread_events::{CodingAgentExit, CodingAgentExitStage};

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
