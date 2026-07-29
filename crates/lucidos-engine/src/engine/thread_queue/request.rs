//! [`ThreadQueueRequest`] — the serializable description of one background
//! spawn. Persisted verbatim in the `ThreadQueued` event payload and the
//! `thread_queue.request` column so a restart can re-fire entries that never
//! ran to completion. Images travel as content-addressed blob hashes, never
//! inline base64 (same rule as `MessageReceived.user_image_hashes`).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::ThreadQueueKind;
use crate::engine::thread_events::{ActorMode, MessageOrigin};
use crate::runtime::CodingAgent;

fn default_coding_agent() -> CodingAgent {
    CodingAgent::ClaudeCode
}

/// Truncation width for panel summaries.
const SUMMARY_MAX_CHARS: usize = 120;

/// Truncate a string to [`SUMMARY_MAX_CHARS`] characters for a Thread Queue
/// panel summary, appending `…` when it was cut. Char-based (never byte-slices,
/// so multi-byte text can't panic). Shared by [`ThreadQueueRequest::summary`]
/// and the user-slot acquire path so the width lives in one place.
pub(crate) fn truncate_summary(text: &str) -> String {
    let truncated: String = text.chars().take(SUMMARY_MAX_CHARS).collect();
    if text.chars().count() > SUMMARY_MAX_CHARS {
        format!("{truncated}…")
    } else {
        truncated
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ThreadQueueRequest {
    /// A trigger matched a domain/thread event (`scheduler::task_runner::
    /// handle_domain_event`). The trigger config is re-read at execution
    /// time; only the firing context is captured here.
    EventTrigger {
        trigger_id: String,
        event_type: String,
        event_payload: serde_json::Value,
        depth: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin_thread_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_event_id: Option<Uuid>,
    },
    /// A trigger's cron schedule fired (`run_task_loop` / missed-grace
    /// catch-up). Config re-read at execution time.
    Cron { trigger_id: String },
    /// `run_thread` LLM tool — chat sub-thread spawn.
    SubThread {
        prompt: String,
        child_thread_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_thread_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawning_event_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        /// Set by the executor's prepare step once the eager
        /// `MessageReceived` is emitted (admission time), so execution does
        /// not re-emit it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pre_emitted_origin: Option<Uuid>,
    },
    /// `run_coding_agent` LLM tool — coding-agent thread spawn.
    CodingAgent {
        prompt: String,
        cc_thread_id: Uuid,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        image_hashes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_thread_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawning_event_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repo_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_id: Option<String>,
        #[serde(default = "default_coding_agent")]
        coding_agent: CodingAgent,
    },
    /// Agent/Engine-mode `POST /api/v1/chat/submit` that starts a NEW thread —
    /// cross-workspace task POSTs and `lucidos spawn-thread` CLI calls.
    /// Executed through `process_message_with_steps` with the captured
    /// `origin`; counts as `sub-thread` or `coding-agent` depending on
    /// `use_coding_agent`.
    AgentChat {
        message: String,
        thread_id: Uuid,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        image_hashes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none", alias = "use_claude_code")]
        use_coding_agent: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repo_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cc_model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        coding_agent: Option<CodingAgent>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        mode: ActorMode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_thread_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawning_event_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_id: Option<String>,
    },
}

impl ThreadQueueRequest {
    /// Capacity bucket this request counts against.
    pub fn kind(&self) -> ThreadQueueKind {
        match self {
            Self::EventTrigger { .. } => ThreadQueueKind::EventTrigger,
            Self::Cron { .. } => ThreadQueueKind::Cron,
            Self::SubThread { .. } => ThreadQueueKind::SubThread,
            Self::CodingAgent { .. } => ThreadQueueKind::CodingAgent,
            Self::AgentChat {
                use_coding_agent, ..
            } => {
                if *use_coding_agent == Some(true) {
                    ThreadQueueKind::CodingAgent
                } else {
                    ThreadQueueKind::SubThread
                }
            }
        }
    }

    /// The owning trigger, for per-trigger caps and FIFO.
    pub fn trigger_id(&self) -> Option<&str> {
        match self {
            Self::EventTrigger { trigger_id, .. } | Self::Cron { trigger_id } => Some(trigger_id),
            _ => None,
        }
    }

    /// Pre-allocated thread id for spawn kinds; trigger kinds bind none
    /// (each fire creates its own thread at execution).
    pub fn thread_id(&self) -> Option<Uuid> {
        match self {
            Self::SubThread {
                child_thread_id, ..
            } => Some(*child_thread_id),
            Self::CodingAgent { cc_thread_id, .. } => Some(*cc_thread_id),
            Self::AgentChat { thread_id, .. } => Some(*thread_id),
            Self::EventTrigger { .. } | Self::Cron { .. } => None,
        }
    }

    /// Human preview shown in the Thread Queue panel. `trigger_name` is the
    /// submit-time config name (configs can be renamed while queued; the
    /// panel shows the name as of enqueue).
    pub fn summary(&self, trigger_name: Option<&str>) -> String {
        let truncate = truncate_summary;
        match self {
            Self::EventTrigger {
                trigger_id,
                event_type,
                ..
            } => format!(
                "{} ← {}",
                trigger_name.unwrap_or(trigger_id.as_str()),
                event_type
            ),
            Self::Cron { trigger_id } => format!(
                "{} (scheduled)",
                trigger_name.unwrap_or(trigger_id.as_str())
            ),
            Self::SubThread { prompt, .. } | Self::CodingAgent { prompt, .. } => truncate(prompt),
            Self::AgentChat { message, .. } => truncate(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_chat_kind_follows_use_coding_agent() {
        let mk = |cc: bool| ThreadQueueRequest::AgentChat {
            message: "do it".into(),
            thread_id: Uuid::new_v4(),
            event_id: None,
            image_hashes: vec![],
            device_id: None,
            model: None,
            reasoning_effort: None,
            use_coding_agent: Some(cc),
            repo_id: None,
            cc_model: None,
            coding_agent: None,
            title: None,
            mode: ActorMode::Agent,
            origin: None,
            parent_thread_id: None,
            spawning_event_id: None,
            app_id: None,
        };
        assert_eq!(mk(false).kind(), ThreadQueueKind::SubThread);
        assert_eq!(mk(true).kind(), ThreadQueueKind::CodingAgent);
    }

    #[test]
    fn request_serde_roundtrips_through_jsonb_shape() {
        // The request column / event payload must survive a serialize →
        // deserialize cycle byte-exactly enough to re-fire after restart.
        let req = ThreadQueueRequest::EventTrigger {
            trigger_id: "trig-1".into(),
            event_type: "DataImported".into(),
            event_payload: serde_json::json!({"rows": 3}),
            depth: 1,
            origin_thread_id: Some(Uuid::new_v4()),
            source_event_id: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["type"], "event-trigger");
        let back: ThreadQueueRequest = serde_json::from_value(json).unwrap();
        match back {
            ThreadQueueRequest::EventTrigger {
                trigger_id,
                event_type,
                depth,
                ..
            } => {
                assert_eq!(trigger_id, "trig-1");
                assert_eq!(event_type, "DataImported");
                assert_eq!(depth, 1);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn summary_prefers_trigger_name_and_truncates_prompts() {
        let req = ThreadQueueRequest::Cron {
            trigger_id: "abc".into(),
        };
        assert_eq!(
            req.summary(Some("Morning news")),
            "Morning news (scheduled)"
        );
        assert_eq!(req.summary(None), "abc (scheduled)");

        let long = "x".repeat(500);
        let req = ThreadQueueRequest::SubThread {
            prompt: long,
            child_thread_id: Uuid::new_v4(),
            parent_thread_id: None,
            spawning_event_id: None,
            title: None,
            model: None,
            reasoning_effort: None,
            pre_emitted_origin: None,
        };
        let s = req.summary(None);
        assert!(
            s.chars().count() <= SUMMARY_MAX_CHARS + 1,
            "got {}",
            s.len()
        );
        assert!(s.ends_with('…'));
    }
}
