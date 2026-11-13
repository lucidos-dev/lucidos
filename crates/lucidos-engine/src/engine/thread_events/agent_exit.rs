use serde::{Deserialize, Serialize};

/// Where in its life a coding agent's process ended on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingAgentExitStage {
    /// The process exited before its session began: before Claude Code's first
    /// startup line, or before the Codex app-server handshake finished.
    Startup,
    /// The process exited after its session began, without ending the turn.
    MidTurn,
}

/// How a coding agent's process ended, carried on `ResponseFailed` so the card
/// can say what happened. The CLI's own words travel in `error`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodingAgentExit {
    pub stage: CodingAgentExitStage,
    /// The process exit code. `None` when a signal ended it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_names_are_snake_case() {
        let exit = CodingAgentExit {
            stage: CodingAgentExitStage::MidTurn,
            code: Some(1),
        };
        assert_eq!(
            serde_json::to_value(exit).unwrap(),
            serde_json::json!({ "stage": "mid_turn", "code": 1 })
        );
        let startup = CodingAgentExit {
            stage: CodingAgentExitStage::Startup,
            code: None,
        };
        assert_eq!(
            serde_json::to_value(startup).unwrap(),
            serde_json::json!({ "stage": "startup" })
        );
    }
}
