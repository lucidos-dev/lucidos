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
        /// Who launched this thread, for the message route popover. Carried
        /// separately from `parent_thread_id` because a `relation: "top"`
        /// spawn has an origin but deliberately no callback linkage (see
        /// `agentic_loop_special_tool::spawn_origin`). Persisted with the
        /// request so a spawn that queues across a restart keeps it; absent on
        /// rows written before the field existed, which fall back to the
        /// linkage-derived origin in `synthesize_legacy_origin`.
        ///
        /// MUST be an `ActorMode::Agent` origin (or `None`): both emit sites
        /// pass `ActorMode::Agent`, and `make_message_received` `.expect()`s on
        /// a mode mismatch, so a `Device` origin here would panic the spawn
        /// rather than mislabel it. That is why the plugin setup thread passes
        /// `None` instead of the clicking device.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
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
        /// Backend model the session runs on, already validated against that
        /// backend's picker at the tool boundary. `None` inherits the backend
        /// default (for Claude Code, the `model` in `cc-settings.json`).
        ///
        /// Persisted with the request so a spawn that queues across a restart
        /// re-fires on the model the caller asked for. Absent on rows written
        /// before the field existed, which is exactly the old behaviour.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// Thinking budget for the session, validated with `model` above.
        /// `None` inherits the backend default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        /// Who launched this thread. Same split as `SubThread::origin`:
        /// attribution for the popover, independent of the callback linkage
        /// above, and Agent-mode for the same reason.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
    },
    /// Agent/Engine-mode `POST /api/v1/chat/stream` that starts a NEW thread:
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
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            alias = "use_claude_code"
        )]
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

    /// A queued spawn can wait behind capacity and be re-fired after a restart,
    /// so the attribution has to survive the jsonb round-trip. Both spawn kinds,
    /// in the top-relation shape: origin present, callback linkage absent.
    #[test]
    fn spawn_requests_round_trip_attribution_without_linkage() {
        use crate::engine::thread_events::ThreadDirection;

        let origin = Some(MessageOrigin::ThreadLink {
            thread_id: Uuid::new_v4(),
            title: None,
            spawning_event_id: Some(Uuid::new_v4()),
            mode: ActorMode::Agent,
            direction: ThreadDirection::Parent,
        });

        let sub = ThreadQueueRequest::SubThread {
            prompt: "run it".into(),
            child_thread_id: Uuid::new_v4(),
            parent_thread_id: None,
            spawning_event_id: None,
            title: None,
            model: None,
            reasoning_effort: None,
            pre_emitted_origin: None,
            origin: origin.clone(),
        };
        let back: ThreadQueueRequest =
            serde_json::from_value(serde_json::to_value(&sub).unwrap()).unwrap();
        match back {
            ThreadQueueRequest::SubThread {
                origin: o,
                parent_thread_id,
                ..
            } => {
                assert_eq!(o, origin);
                assert_eq!(parent_thread_id, None);
            }
            other => panic!("wrong variant: {other:?}"),
        }

        let cc = ThreadQueueRequest::CodingAgent {
            prompt: "run it".into(),
            cc_thread_id: Uuid::new_v4(),
            image_hashes: vec![],
            device_id: None,
            parent_thread_id: None,
            spawning_event_id: None,
            repo_id: None,
            title: None,
            app_id: None,
            coding_agent: CodingAgent::ClaudeCode,
            model: None,
            reasoning_effort: None,
            origin: origin.clone(),
        };
        let back: ThreadQueueRequest =
            serde_json::from_value(serde_json::to_value(&cc).unwrap()).unwrap();
        match back {
            ThreadQueueRequest::CodingAgent {
                origin: o,
                parent_thread_id,
                ..
            } => {
                assert_eq!(o, origin);
                assert_eq!(parent_thread_id, None);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    /// A queued spawn that crosses a restart must re-fire on the model the
    /// caller asked for. The request is persisted verbatim in `ThreadQueued`
    /// and in `thread_queue.request`, so a pin that does not survive this
    /// round-trip becomes a spawn that quietly runs on the backend default
    /// after a restart, which is the original bug wearing a different hat.
    #[test]
    fn the_model_and_effort_pins_survive_the_persistence_round_trip() {
        let cc = ThreadQueueRequest::CodingAgent {
            prompt: "run it".into(),
            cc_thread_id: Uuid::new_v4(),
            image_hashes: vec![],
            device_id: None,
            parent_thread_id: None,
            spawning_event_id: None,
            repo_id: None,
            title: None,
            app_id: None,
            coding_agent: CodingAgent::ClaudeCode,
            model: Some("claude-sonnet-5".into()),
            reasoning_effort: Some("low".into()),
            origin: None,
        };
        let back: ThreadQueueRequest =
            serde_json::from_value(serde_json::to_value(&cc).unwrap()).unwrap();
        match back {
            ThreadQueueRequest::CodingAgent {
                model,
                reasoning_effort,
                ..
            } => {
                assert_eq!(model.as_deref(), Some("claude-sonnet-5"));
                assert_eq!(reasoning_effort.as_deref(), Some("low"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    /// A row queued before the fields existed must still deserialize, and must
    /// come back UNPINNED rather than failing the requeue. Same back-compat
    /// rule the `origin` field follows below.
    #[test]
    fn a_coding_agent_row_without_the_pin_fields_still_deserializes() {
        let json = serde_json::json!({
            "type": "coding-agent",
            "prompt": "queued before the fields existed",
            "cc_thread_id": Uuid::new_v4(),
        });
        let back: ThreadQueueRequest = serde_json::from_value(json).unwrap();
        match back {
            ThreadQueueRequest::CodingAgent {
                model,
                reasoning_effort,
                ..
            } => {
                assert_eq!(model, None);
                assert_eq!(reasoning_effort, None);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    /// Rows written before the field existed must still deserialize, falling
    /// back to the linkage-derived origin rather than failing the requeue.
    #[test]
    fn a_request_without_the_origin_field_still_deserializes() {
        let json = serde_json::json!({
            "type": "sub-thread",
            "prompt": "legacy",
            "child_thread_id": Uuid::new_v4(),
        });
        let back: ThreadQueueRequest = serde_json::from_value(json).unwrap();
        match back {
            ThreadQueueRequest::SubThread { origin, .. } => assert_eq!(origin, None),
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
            origin: None,
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
