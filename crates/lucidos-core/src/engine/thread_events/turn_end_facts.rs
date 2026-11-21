use serde::{Deserialize, Serialize};

/// How the coding agent ended its turn, as its driver saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnEndStatus {
    /// The agent reported the turn complete.
    Completed,
    /// The agent reported the turn interrupted.
    Interrupted,
    /// The agent reported the turn failed, or refused to start it.
    Failed,
    /// The agent sent no terminal. The driver wrote the turn end itself, for
    /// example when the process died.
    Synthesized,
}

/// What a coding-agent turn end looked like, stored on `CodingAgentIdled`.
/// Read it to tell which path ended a turn that ended early. Codex fills every
/// field. The exec driver fills `status` only, and Claude Code sends none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnEndFacts {
    pub status: TurnEndStatus,
    /// The turn completed a message that Codex marked as its final answer.
    #[serde(default)]
    pub final_answer: bool,
    /// The count of agent messages the turn completed.
    #[serde(default)]
    pub agent_messages: u32,
    /// The count of follow-up inputs Codex accepted into this turn as steers.
    #[serde(default)]
    pub steered_inputs: u32,
    /// The type of the last item the turn completed, in Codex's own words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_item: Option<String>,
}

impl TurnEndFacts {
    /// Facts that hold the status only, for a driver that sees no more.
    pub fn of_status(status: TurnEndStatus) -> Self {
        Self {
            status,
            final_answer: false,
            agent_messages: 0,
            steered_inputs: 0,
            last_item: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wire_names_are_snake_case() {
        let facts = TurnEndFacts {
            status: TurnEndStatus::Interrupted,
            final_answer: true,
            agent_messages: 2,
            steered_inputs: 1,
            last_item: Some("commandExecution".to_string()),
        };
        assert_eq!(
            serde_json::to_value(&facts).unwrap(),
            serde_json::json!({
                "status": "interrupted",
                "final_answer": true,
                "agent_messages": 2,
                "steered_inputs": 1,
                "last_item": "commandExecution",
            })
        );
        assert_eq!(
            serde_json::to_value(TurnEndFacts::of_status(TurnEndStatus::Synthesized)).unwrap(),
            serde_json::json!({
                "status": "synthesized",
                "final_answer": false,
                "agent_messages": 0,
                "steered_inputs": 0,
            })
        );
    }
}
