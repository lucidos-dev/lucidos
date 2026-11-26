use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandoffRequest {
    pub message: String,
    pub by: super::MessageOrigin,
}
