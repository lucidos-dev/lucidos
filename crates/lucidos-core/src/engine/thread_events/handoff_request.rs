use serde::{Deserialize, Serialize};

/// What a user typed at the *Successor card*, and who typed it. The successor
/// session starts on it as that user's message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HandoffRequest {
    pub message: String,
    pub by: super::MessageOrigin,
}
