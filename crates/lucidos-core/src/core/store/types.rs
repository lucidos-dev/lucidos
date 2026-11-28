use crate::engine::thread_events::AgentParticipant;
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize)]
pub struct Step {
    pub description: String,
    pub tool_name: Option<String>,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_tokens: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_messages: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trimmed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_called_event_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type")]
pub enum ResponseEvent {
    #[serde(rename = "text")]
    Text { md: String },
    #[serde(rename = "step")]
    Step {
        description: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_name: Option<String>,
        success: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        context_tokens: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        context_messages: Option<usize>,
        #[serde(skip_serializing_if = "Option::is_none")]
        trimmed: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_called_event_id: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<Step>,
    #[serde(skip)]
    pub image_handles: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub user_image_hashes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed: Option<bool>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub canceled: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub aborted: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub text_chunks: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<ResponseEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentParticipant>,
    #[serde(skip)]
    pub sender_device_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConversationMessage {
    pub role: String,
    pub content: String,
    pub event_id: Uuid,
    pub timestamp: DateTime<Utc>,
    pub steps: Vec<Step>,
    pub images: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConversationSnapshot {
    pub at_event_id: Uuid,
    pub point_in_time: DateTime<Utc>,
    pub messages: Vec<ConversationMessage>,
    pub artifacts: Vec<String>,
    pub git_commit: Option<String>,
}
