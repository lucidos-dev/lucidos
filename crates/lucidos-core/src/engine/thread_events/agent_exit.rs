use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodingAgentExitStage {
    Startup,
    MidTurn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodingAgentExit {
    pub stage: CodingAgentExitStage,
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
