//! Persisted Thread Queue request and capacity policy vocabulary.

mod policy;
mod request;

pub use policy::{
    AdmissionCounts, AdmissionDecision, CapacityPolicy, OverflowPolicy, ThreadQueueKind,
    DEFAULT_MAX_CONCURRENT_CHILDREN_PER_THREAD, DEFAULT_MAX_EVENT_TRIGGER_DEPTH,
};
pub use request::{truncate_summary, ThreadQueueRequest};

use serde::{Deserialize, Serialize};

/// App context sent when an app UI is open — tells the LLM which app is active.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppContext {
    pub app_id: String,
}

/// URL context sent when the user has a webpage open in the Tauri panel webview
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlContext {
    pub url: String,
    #[serde(default)]
    pub title: Option<String>,
    pub content: String,
}

/// The mentions a chat send carries, checked against the gateway's users.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UserAddressing {
    /// User ids, each once, in the order sent.
    pub mentions: Vec<String>,
    pub agent_mentioned: bool,
}
