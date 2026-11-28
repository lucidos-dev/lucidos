//! INVARIANT: [`ThreadQueueRequest`] — the serializable description of one background
//! spawn. Persisted verbatim in the `ThreadQueued` event payload and the
//! `thread_queue.request` column so a restart can re-fire entries that never
//! ran to completion. Images travel as content-addressed blob hashes, never
//! inline base64 (same rule as `MessageReceived.user_image_hashes`).

use crate::core::event_hooks::HookArgs;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{AppContext, ThreadQueueKind, UrlContext, UserAddressing};
use crate::engine::thread_events::CodingAgent;
use crate::engine::thread_events::{ActorMode, MessageOrigin, RetryOf, WakeMode};

fn default_coding_agent() -> CodingAgent {
    CodingAgent::ClaudeCode
}

const SUMMARY_MAX_CHARS: usize = 120;

pub fn truncate_summary(text: &str) -> String {
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
    EventTrigger {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_of: Option<RetryOf>,
        trigger_id: String,
        event_type: String,
        event_payload: serde_json::Value,
        depth: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin_thread_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        source_event_id: Option<Uuid>,
    },
    Cron {
        trigger_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_of: Option<RetryOf>,
    },
    SubThread {
        prompt: String,
        #[serde(default, skip_serializing_if = "HookArgs::is_empty")]
        hook_args: HookArgs,
        child_thread_id: Uuid,
        #[serde(default)]
        depth: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_thread_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawning_event_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wake_mode: Option<WakeMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pre_emitted_origin: Option<Uuid>,
        /// INVARIANT: Who launched this thread, for the message route popover. Carried
        /// separately from `parent_thread_id` because a `relation: "top"`
        /// spawn has an origin but deliberately no callback linkage (see
        /// `agentic_loop_special_tool::spawn_origin`). Persisted with the
        /// request so a spawn that queues across a restart keeps it; absent on
        /// rows written before the field existed, which fall back to the
        /// linkage-derived origin in `synthesize_legacy_origin`.
        ///
        /// Both emit sites derive the turn's mode from this origin
        /// (`MessageOrigin::mode_or`), so an engine-seeded spawn carries an
        /// `Engine` origin. [`ThreadQueueRequest::is_attributed`] flags a
        /// request that names neither an origin nor a parent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
    },
    CodingAgent {
        prompt: String,
        #[serde(default, skip_serializing_if = "HookArgs::is_empty")]
        hook_args: HookArgs,
        cc_thread_id: Uuid,
        #[serde(default)]
        depth: u32,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        image_hashes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_thread_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawning_event_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        wake_mode: Option<WakeMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        repo_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_id: Option<String>,
        #[serde(default = "default_coding_agent")]
        coding_agent: CodingAgent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        /// INVARIANT: Who launched this thread. Same split as `SubThread::origin`:
        /// attribution for the popover, independent of the callback linkage
        /// above. MUST be Agent-mode: `run_agent_thread_spawn` stamps
        /// `ActorMode::Agent`, and its only caller is the `run_coding_agent` tool.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        origin: Option<MessageOrigin>,
    },
    AgentChat {
        message: String,
        #[serde(default, skip_serializing_if = "HookArgs::is_empty")]
        hook_args: HookArgs,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        user_addressing: Option<Box<UserAddressing>>,
        thread_id: Uuid,
        #[serde(default)]
        depth: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        event_id: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        image_hashes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        device_id: Option<String>,
        /// INVARIANT: Per-turn context, resolved by the submitting handler and retained
        /// verbatim across admission waits and restart recovery.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_context: Option<AppContext>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_context: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        url_context: Option<Box<UrlContext>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        conflict_change_id: Option<Uuid>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_effort: Option<String>,
        /// INVARIANT: The backend this turn was pinned to, carried so a queued turn
        /// re-executes on the one it was submitted for. Dropping it here would
        /// substitute the model's own preferred provider on a requeue, which is
        /// the silent move the honoured-or-refused rule exists to prevent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
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
        wake_mode: Option<WakeMode>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        app_id: Option<String>,
        /// INVARIANT: An *integrated sub-thread* spawn. Persisted with the request, so a
        /// spawn that queues across a restart still folds into its parent.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        integrated: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spawning_trigger_id: Option<String>,
    },
}

impl ThreadQueueRequest {
    pub fn coding_agent_command(&self) -> Option<crate::engine::thread_events::CodingAgentCommand> {
        match self {
            Self::AgentChat {
                message,
                use_coding_agent: Some(true),
                mode: ActorMode::Human,
                ..
            } => crate::engine::thread_events::CodingAgentCommand::from_prompt(message),
            _ => None,
        }
    }

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

    /// INVARIANT: The event-trigger chain depth this work runs at.
    ///
    /// A spawn does NOT consume a hop: work a fire hands off runs at the fire's
    /// own depth. A hop is a trigger fire, counted once in
    /// `task_runner::fire_matching_triggers`.
    ///
    /// `Cron` roots a fresh chain, so it carries no field and answers 0. Its
    /// submit sites (the task loop, the missed-grace catch-up, the
    /// off-schedule run) are never inside a fire.
    pub fn depth(&self) -> u32 {
        match self {
            Self::EventTrigger { depth, .. }
            | Self::SubThread { depth, .. }
            | Self::CodingAgent { depth, .. }
            | Self::AgentChat { depth, .. } => *depth,
            Self::Cron { .. } => 0,
        }
    }

    /// INVARIANT: Record the submitting task's chain depth on a spawn request.
    ///
    /// Called once, by `ThreadQueue::submit` or durable saved-draft
    /// acceptance, awaited inline from the submitting task. Centralising it
    /// there is what stops a new spawn site shipping without a depth.
    ///
    /// `EventTrigger` is deliberately untouched. It already carries the
    /// dispatcher's `next_depth`. The bus subscriber that submits it has an
    /// ambient depth of 0, which would overwrite the real value with a lie.
    /// `Cron` has no field to write.
    pub fn stamp_caller_depth(&mut self, caller_depth: u32) {
        match self {
            Self::SubThread { depth, .. }
            | Self::CodingAgent { depth, .. }
            | Self::AgentChat { depth, .. } => *depth = caller_depth,
            Self::EventTrigger { .. } | Self::Cron { .. } => {}
        }
    }

    pub fn is_attributed(&self) -> bool {
        match self {
            Self::SubThread {
                origin,
                parent_thread_id,
                ..
            }
            | Self::CodingAgent {
                origin,
                parent_thread_id,
                ..
            }
            | Self::AgentChat {
                origin,
                parent_thread_id,
                ..
            } => origin.is_some() || parent_thread_id.is_some(),
            Self::EventTrigger { .. } | Self::Cron { .. } => true,
        }
    }

    pub fn trigger_id(&self) -> Option<&str> {
        match self {
            Self::EventTrigger { trigger_id, .. } | Self::Cron { trigger_id, .. } => {
                Some(trigger_id)
            }
            _ => None,
        }
    }

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
            Self::Cron { trigger_id, .. } => format!(
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
    fn trigger_requests_replay_old_and_retry_payloads() {
        use crate::engine::event_bus::SystemEvent;
        use serde_json::json;

        let retry = json!({
            "thread_id": Uuid::new_v4(), "deadline": "2030-01-01T12:00:00Z",
        });
        for mut request in [
            json!({ "type": "cron", "trigger_id": "daily-report" }),
            json!({
                "type": "event-trigger", "trigger_id": "daily-report",
                "event_type": "DataImported", "event_payload": { "rows": 3 }, "depth": 1,
                "source_event_id": Uuid::new_v4(),
            }),
        ] {
            for retry_of in [None, Some(retry.clone())] {
                if let Some(retry_of) = retry_of {
                    request["retry_of"] = retry_of;
                }
                let stored = json!({
                    "type": "ThreadQueued", "data": {
                    "entry_id": Uuid::new_v4(),
                    "kind": request["type"], "request": request,
                    "summary": "Daily report", "position": 1, }
                });
                let event: SystemEvent = serde_json::from_value(stored).unwrap();
                let SystemEvent::ThreadQueued {
                    request: stored_request,
                    ..
                } = event
                else {
                    panic!("expected ThreadQueued");
                };
                let decoded: ThreadQueueRequest = serde_json::from_value(stored_request).unwrap();
                match &decoded {
                    ThreadQueueRequest::Cron { retry_of, .. }
                    | ThreadQueueRequest::EventTrigger { retry_of, .. } => {
                        assert_eq!(retry_of.is_some(), request.get("retry_of").is_some());
                    }
                    other => panic!("wrong request: {other:?}"),
                }
                assert_eq!(serde_json::to_value(decoded).unwrap(), request);
            }
        }
    }

    #[test]
    fn agent_chat_kind_follows_use_coding_agent() {
        let mk = |cc: bool| ThreadQueueRequest::AgentChat {
            hook_args: Default::default(),
            user_addressing: None,
            depth: 0,
            message: "do it".into(),
            thread_id: Uuid::new_v4(),
            event_id: None,
            image_hashes: vec![],
            device_id: None,
            app_context: None,
            file_context: None,
            url_context: None,
            conflict_change_id: None,
            model: None,
            reasoning_effort: None,
            provider: None,
            use_coding_agent: Some(cc),
            repo_id: None,
            cc_model: None,
            coding_agent: None,
            title: None,
            mode: ActorMode::Agent,
            origin: None,
            parent_thread_id: None,
            spawning_event_id: None,
            wake_mode: None,
            app_id: None,
            integrated: false,
            spawning_trigger_id: None,
        };
        assert_eq!(mk(false).kind(), ThreadQueueKind::SubThread);
        assert_eq!(mk(true).kind(), ThreadQueueKind::CodingAgent);
    }

    #[test]
    fn an_old_agent_chat_row_reads_as_independent() {
        let row = serde_json::json!({
            "type": "agent-chat",
            "message": "do it",
            "thread_id": Uuid::new_v4(),
            "mode": "agent",
        });
        let req: ThreadQueueRequest = serde_json::from_value(row).expect("old row parses");
        assert!(matches!(
            req,
            ThreadQueueRequest::AgentChat {
                integrated: false,
                ..
            }
        ));
    }

    #[test]
    fn agent_chat_preserves_all_turn_context_across_persistence() {
        let context = serde_json::json!({
            "app_context": {"app_id": "sample-app"},
            "file_context": "[repo:sample] src/lib.rs:3-8",
            "url_context": {"url": "https://example.com", "title": "Reference", "content": "Selected page"},
            "conflict_change_id": Uuid::new_v4(),
        });
        let mut row = serde_json::json!({
            "type": "agent-chat", "message": "use this context",
            "thread_id": Uuid::new_v4(), "mode": "agent",
        });
        for (key, value) in context.as_object().unwrap() {
            row[key] = value.clone();
        }
        let request: ThreadQueueRequest = serde_json::from_value(row).unwrap();
        let restored = serde_json::to_value(request).unwrap();
        for (key, expected) in context.as_object().unwrap() {
            assert_eq!(&restored[key], expected, "queued turn lost {key}");
        }
    }

    #[test]
    fn request_serde_roundtrips_through_jsonb_shape() {
        let req = ThreadQueueRequest::EventTrigger {
            retry_of: None,
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
            hook_args: Default::default(),
            depth: 0,
            prompt: "run it".into(),
            child_thread_id: Uuid::new_v4(),
            parent_thread_id: None,
            spawning_event_id: None,
            wake_mode: None,
            title: None,
            model: None,
            reasoning_effort: None,
            provider: None,
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
            hook_args: Default::default(),
            depth: 0,
            prompt: "run it".into(),
            cc_thread_id: Uuid::new_v4(),
            image_hashes: vec![],
            device_id: None,
            parent_thread_id: None,
            spawning_event_id: None,
            wake_mode: None,
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

    #[test]
    fn the_model_and_effort_pins_survive_the_persistence_round_trip() {
        let cc = ThreadQueueRequest::CodingAgent {
            hook_args: Default::default(),
            depth: 0,
            prompt: "run it".into(),
            cc_thread_id: Uuid::new_v4(),
            image_hashes: vec![],
            device_id: None,
            parent_thread_id: None,
            spawning_event_id: None,
            wake_mode: None,
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

    #[test]
    fn a_spawn_keeps_its_wake_mode_and_an_old_row_reads_as_undeclared() {
        for kind in ["sub-thread", "coding-agent", "agent-chat"] {
            let old = serde_json::json!({
                "type": kind,
                "prompt": "queued before wake modes",
                "message": "queued before wake modes",
                "child_thread_id": Uuid::new_v4(),
                "cc_thread_id": Uuid::new_v4(),
                "thread_id": Uuid::new_v4(),
                "mode": "agent",
            });
            let back: ThreadQueueRequest = serde_json::from_value(old).unwrap();
            assert_eq!(wake_mode_of(&back), None, "{kind}");

            let mut declared = serde_json::to_value(&back).unwrap();
            declared["wake_mode"] = serde_json::json!("wake-when-all-done");
            let back: ThreadQueueRequest = serde_json::from_value(declared).unwrap();
            let again: ThreadQueueRequest =
                serde_json::from_value(serde_json::to_value(&back).unwrap()).unwrap();
            assert_eq!(
                wake_mode_of(&again),
                Some(WakeMode::WakeWhenAllDone),
                "{kind}"
            );
        }
    }

    fn wake_mode_of(request: &ThreadQueueRequest) -> Option<WakeMode> {
        match request {
            ThreadQueueRequest::SubThread { wake_mode, .. }
            | ThreadQueueRequest::CodingAgent { wake_mode, .. }
            | ThreadQueueRequest::AgentChat { wake_mode, .. } => *wake_mode,
            other => panic!("not a spawn: {other:?}"),
        }
    }

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

    fn bare_sub_thread(origin: Option<MessageOrigin>, parent: Option<Uuid>) -> ThreadQueueRequest {
        ThreadQueueRequest::SubThread {
            hook_args: Default::default(),
            depth: 0,
            prompt: "run it".into(),
            child_thread_id: Uuid::new_v4(),
            parent_thread_id: parent,
            spawning_event_id: None,
            wake_mode: None,
            title: None,
            model: None,
            reasoning_effort: None,
            provider: None,
            pre_emitted_origin: None,
            origin,
        }
    }

    #[test]
    fn a_spawn_is_attributed_only_by_an_origin_or_a_parent() {
        use crate::engine::thread_events::EngineReason;

        assert!(!bare_sub_thread(None, None).is_attributed());
        assert!(bare_sub_thread(None, Some(Uuid::new_v4())).is_attributed());
        assert!(bare_sub_thread(
            Some(MessageOrigin::engine(EngineReason::MergeConflict)),
            None
        )
        .is_attributed());

        let cc = |device_id: Option<String>| ThreadQueueRequest::CodingAgent {
            hook_args: Default::default(),
            depth: 0,
            prompt: "run it".into(),
            cc_thread_id: Uuid::new_v4(),
            image_hashes: vec![],
            device_id,
            parent_thread_id: None,
            spawning_event_id: None,
            wake_mode: None,
            repo_id: None,
            title: None,
            app_id: None,
            coding_agent: CodingAgent::ClaudeCode,
            model: None,
            reasoning_effort: None,
            origin: None,
        };
        assert!(!cc(Some("dev-1".into())).is_attributed());

        assert!(ThreadQueueRequest::Cron {
            retry_of: None,
            trigger_id: "t".into()
        }
        .is_attributed());
    }

    #[test]
    fn a_plugin_setup_origin_round_trips_through_the_request() {
        use crate::engine::thread_events::{EngineReason, PluginSetupCause, PluginSetupOccasion};

        for occasion in [
            PluginSetupOccasion::FreshInstall,
            PluginSetupOccasion::Update {
                from_version: Some("0.1.3".into()),
                cause: PluginSetupCause::InstructionsChanged,
            },
            PluginSetupOccasion::Update {
                from_version: None,
                cause: PluginSetupCause::RevisionRaised {
                    revision: 2,
                    reason: Some("The skill changed.".into()),
                },
            },
        ] {
            let origin = Some(MessageOrigin::engine(EngineReason::PluginSetup {
                plugin_id: "habit-tracker".into(),
                plugin_name: "Habit Tracker".into(),
                version: "0.1.4".into(),
                occasion,
                confirmed_on_device_id: Some("dev-1".into()),
            }));
            let req = bare_sub_thread(origin.clone(), None);
            let back: ThreadQueueRequest =
                serde_json::from_value(serde_json::to_value(&req).unwrap()).unwrap();
            match back {
                ThreadQueueRequest::SubThread { origin: o, .. } => assert_eq!(o, origin),
                other => panic!("wrong variant: {other:?}"),
            }
        }
    }

    #[test]
    fn summary_prefers_trigger_name_and_truncates_prompts() {
        let req = ThreadQueueRequest::Cron {
            retry_of: None,
            trigger_id: "abc".into(),
        };
        assert_eq!(
            req.summary(Some("Morning news")),
            "Morning news (scheduled)"
        );
        assert_eq!(req.summary(None), "abc (scheduled)");

        let long = "x".repeat(500);
        let req = ThreadQueueRequest::SubThread {
            hook_args: Default::default(),
            depth: 0,
            prompt: long,
            child_thread_id: Uuid::new_v4(),
            parent_thread_id: None,
            spawning_event_id: None,
            wake_mode: None,
            title: None,
            model: None,
            reasoning_effort: None,
            provider: None,
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

#[cfg(test)]
#[test]
fn queued_first_message_arguments_survive_recovery() {
    let id = Uuid::new_v4();
    for mut wire in [
        serde_json::json!({ "type": "sub-thread", "prompt": "Start", "child_thread_id": id }),
        serde_json::json!({ "type": "coding-agent", "prompt": "Start", "cc_thread_id": id }),
        serde_json::json!({ "type": "agent-chat", "message": "Start", "thread_id": id, "mode": "agent" }),
    ] {
        let request: ThreadQueueRequest = serde_json::from_value(wire.clone()).unwrap();
        assert!(serde_json::to_value(request)
            .unwrap()
            .get("hook_args")
            .is_none());
        wire["hook_args"] = serde_json::json!({ "ticket": "TK-42" });
        let request: ThreadQueueRequest = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(
            serde_json::to_value(request).unwrap()["hook_args"],
            wire["hook_args"]
        );
    }
}
