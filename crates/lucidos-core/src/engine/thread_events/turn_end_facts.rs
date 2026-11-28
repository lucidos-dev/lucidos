use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnEndStatus {
    Completed,
    Interrupted,
    Failed,
    Synthesized,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TurnEndFacts {
    pub status: TurnEndStatus,
    #[serde(default)]
    pub final_answer: bool,
    #[serde(default)]
    pub agent_messages: u32,
    #[serde(default)]
    pub steered_inputs: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_item: Option<String>,
}

impl TurnEndFacts {
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
