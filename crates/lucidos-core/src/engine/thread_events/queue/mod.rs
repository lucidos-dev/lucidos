mod policy;
mod request;

pub use policy::{
    AdmissionCounts, AdmissionDecision, CapacityPolicy, OverflowPolicy, ThreadQueueKind,
    DEFAULT_MAX_CONCURRENT_CHILDREN_PER_THREAD, DEFAULT_MAX_EVENT_TRIGGER_DEPTH,
};
pub use request::{truncate_summary, ThreadQueueRequest};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppContext {
    pub app_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlContext {
    pub url: String,
    #[serde(default)]
    pub title: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UserAddressing {
    pub mentions: Vec<String>,
    pub agent_mentioned: bool,
}
