use crate::engine::message_events::IMAGE_DESCRIPTION_PROMPT;
#[cfg(test)]
use crate::engine::message_events::{
    emit_routing_failure, make_message_received, make_message_received_with_origin,
};
#[cfg(test)]
use crate::engine::thread_events::{ActorMode, ThreadDirection};
#[cfg(test)]
use uuid::Uuid;

/// Generate a brief description of user-attached images using Flash.
/// Standalone function so it can be spawned into a background task.
///
/// `capture` makes the call and records it for token accounting.
pub(super) async fn describe_images(
    provider: &dyn crate::llm::provider::LlmProvider,
    images: &[crate::engine::types::ChatImage],
    // The `reasoning_image_description` preference. It used to be `None`, which
    // `gemini_generation_config` reads as `high`, so a caption was paying for
    // the model's deepest thinking.
    reasoning_effort: Option<&str>,
    capture: &crate::engine::AuxCapture,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use crate::llm::provider::{ContentBlock, Message, MessageContent};

    let mut blocks: Vec<ContentBlock> = vec![ContentBlock::Text {
        text: IMAGE_DESCRIPTION_PROMPT.to_string(),
    }];
    for img in images {
        // Fit each image to the LLM size target (compress only if over) so the
        // description pass can't trip the provider's per-image limit either.
        blocks.push(super::images::image_content_block(
            img.clone().fit_for_llm(),
        ));
    }

    let messages = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Blocks(blocks),
    }];

    let response = capture
        .chat(
            provider,
            messages,
            vec![],
            crate::llm::ModelSelection::default().with_effort(reasoning_effort),
            None,
            None,
        )
        .await?;
    response
        .content
        .ok_or_else(|| "No description returned".into())
}

/// Format a duration as a human-readable relative age (e.g., "2h ago", "3d ago").
pub(super) fn format_relative_age(duration: chrono::Duration) -> String {
    let secs = duration.num_seconds();
    if secs < 60 {
        return "just now".to_string();
    }
    let mins = secs / 60;
    if mins < 60 {
        return format!("{}m ago", mins);
    }
    let hours = mins / 60;
    if hours < 24 {
        return format!("{}h ago", hours);
    }
    let days = hours / 24;
    format!("{}d ago", days)
}

#[cfg(test)]
mod origin_invariants {
    use super::*;
    use crate::engine::thread_events::{MessageOrigin, ThreadEvent};
    use uuid::Uuid;

    #[test]
    fn device_origin_with_human_mode_ok() {
        let origin = MessageOrigin::Device {
            device_id: "dev-1".into(),
        };
        let res = make_message_received_with_origin(
            std::path::Path::new(""),
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            None,
            Some(origin),
            None,
        );
        assert!(res.is_ok());
    }

    #[test]
    fn device_origin_with_agent_mode_rejected() {
        let origin = MessageOrigin::Device {
            device_id: "dev-1".into(),
        };
        let res = make_message_received_with_origin(
            std::path::Path::new(""),
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Agent,
            None,
            None,
            None,
            Some(origin),
            None,
        );
        assert!(res.is_err(), "Device origin must reject non-Human mode");
    }

    #[test]
    fn api_origin_with_human_mode_ok() {
        let origin = MessageOrigin::Api {
            user_agent: Some("curl/8".into()),
            mode: ActorMode::Human,
            source_thread_id: None,
            emitting_trigger_id: None,
        };
        let res = make_message_received_with_origin(
            std::path::Path::new(""),
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            None,
            Some(origin),
            None,
        );
        assert!(res.is_ok());
    }

    #[test]
    fn api_origin_with_agent_mode_ok() {
        // Third-party SDK identifying as an agent (LLM acting on user's behalf).
        let origin = MessageOrigin::Api {
            user_agent: Some("MyApp/1.0".into()),
            mode: ActorMode::Agent,
            source_thread_id: None,
            emitting_trigger_id: None,
        };
        let res = make_message_received_with_origin(
            std::path::Path::new(""),
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Agent,
            None,
            None,
            None,
            Some(origin),
            None,
        );
        assert!(
            res.is_ok(),
            "Api origin must accept Agent mode (third-party SDKs)"
        );
    }

    #[test]
    fn api_origin_with_engine_mode_ok() {
        // External engine code making deterministic API calls.
        let origin = MessageOrigin::Api {
            user_agent: Some("ScriptRunner/2.0".into()),
            mode: ActorMode::Engine,
            source_thread_id: None,
            emitting_trigger_id: None,
        };
        let res = make_message_received_with_origin(
            std::path::Path::new(""),
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Engine,
            None,
            None,
            None,
            Some(origin),
            None,
        );
        assert!(res.is_ok(), "Api origin must accept Engine mode");
    }

    #[test]
    fn api_origin_with_mismatched_mode_rejected() {
        // The `mode` field on Api must match the request's claimed mode.
        let origin = MessageOrigin::Api {
            user_agent: None,
            mode: ActorMode::Agent,
            source_thread_id: None,
            emitting_trigger_id: None,
        };
        let res = make_message_received_with_origin(
            std::path::Path::new(""),
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            None,
            Some(origin),
            None,
        );
        assert!(res.is_err(), "Api origin mode must match request mode");
    }

    #[test]
    fn workspace_origin_with_matching_mode_ok() {
        let origin = MessageOrigin::Workspace {
            workspace: "dev".into(),
            caller_gateway: None,
            routing_gateway: None,
            thread_id: None,
            event_id: None,
            user_agent: None,
            mode: ActorMode::Human,
            remote_start: None,
            verdict: None,
        };
        let res = make_message_received_with_origin(
            std::path::Path::new(""),
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            None,
            Some(origin),
            None,
        );
        assert!(res.is_ok());
    }

    #[test]
    fn workspace_origin_with_mismatched_mode_rejected() {
        let origin = MessageOrigin::Workspace {
            workspace: "dev".into(),
            caller_gateway: None,
            routing_gateway: None,
            thread_id: None,
            event_id: None,
            user_agent: None,
            mode: ActorMode::Human,
            remote_start: None,
            verdict: None,
        };
        let res = make_message_received_with_origin(
            std::path::Path::new(""),
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Agent,
            None,
            None,
            None,
            Some(origin),
            None,
        );
        assert!(
            res.is_err(),
            "Workspace origin must reject mode that doesn't match the carried field"
        );
    }

    #[test]
    fn thread_link_origin_with_matching_agent_mode_ok() {
        let origin = MessageOrigin::ThreadLink {
            thread_id: Uuid::new_v4(),
            title: None,
            spawning_event_id: None,
            mode: ActorMode::Agent,
            direction: ThreadDirection::Parent,
        };
        let res = make_message_received_with_origin(
            std::path::Path::new(""),
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Agent,
            None,
            None,
            None,
            Some(origin),
            None,
        );
        assert!(res.is_ok());
    }

    #[test]
    fn thread_link_origin_with_mismatched_mode_rejected() {
        let origin = MessageOrigin::ThreadLink {
            thread_id: Uuid::new_v4(),
            title: None,
            spawning_event_id: None,
            mode: ActorMode::Agent,
            direction: ThreadDirection::Parent,
        };
        let res = make_message_received_with_origin(
            std::path::Path::new(""),
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            None,
            Some(origin),
            None,
        );
        assert!(
            res.is_err(),
            "ThreadLink origin must reject mode that doesn't match the carried field"
        );
    }

    #[test]
    fn engine_origin_with_engine_mode_ok() {
        let origin = MessageOrigin::Engine {
            reason: crate::engine::thread_events::EngineReason::ContinuationStarted,
        };
        let res = make_message_received_with_origin(
            std::path::Path::new(""),
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Engine,
            None,
            None,
            None,
            Some(origin),
            None,
        );
        assert!(res.is_ok());
    }

    #[test]
    fn engine_origin_with_human_mode_rejected() {
        let origin = MessageOrigin::Engine {
            reason: crate::engine::thread_events::EngineReason::HardenRetrigger,
        };
        let res = make_message_received_with_origin(
            std::path::Path::new(""),
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            None,
            Some(origin),
            None,
        );
        assert!(res.is_err(), "Engine origin must reject non-Engine mode");
    }

    /// Regression: child threads spawned by the LLM `run_thread` tool call this
    /// helper with `mode = Agent`, `parent_thread_id = Some(_)`, and
    /// `explicit_origin = None`. The synthesized origin must be
    /// `ThreadLink { direction: Parent, mode: Agent }` — without it, downstream
    /// consumers can't attribute the spawn to the parent agent run.
    #[test]
    fn synthesizes_thread_link_parent_origin_for_agent_spawn() {
        let parent_id = Uuid::new_v4();
        let spawn_id = Some(Uuid::new_v4());

        let event = make_message_received(
            std::path::Path::new(""),
            "do the thing",
            None,
            None,
            Some(parent_id),
            spawn_id,
            None,
            ActorMode::Agent,
            None,
            None,
            None,
            None,
            None,
        );

        match event {
            crate::engine::thread_events::ThreadEvent::MessageReceived { origin, .. } => {
                match origin {
                    Some(MessageOrigin::ThreadLink {
                        thread_id,
                        spawning_event_id,
                        mode,
                        direction,
                        ..
                    }) => {
                        assert_eq!(thread_id, parent_id);
                        assert_eq!(spawning_event_id, spawn_id);
                        assert_eq!(mode, ActorMode::Agent);
                        assert_eq!(direction, ThreadDirection::Parent);
                    }
                    other => panic!("expected ThreadLink origin, got {:?}", other),
                }
            }
            other => panic!("expected MessageReceived, got {:?}", other),
        }
    }

    /// Regression: parent threads receiving a child's "[Child thread completed]"
    /// callback now stamp `ThreadLink { direction: Child, mode: Agent }` at the
    /// emit site. This test covers the validation path — the explicit
    /// child-direction origin must validate OK with `mode = Agent`.
    #[test]
    fn thread_link_child_direction_with_agent_mode_ok() {
        let origin = MessageOrigin::ThreadLink {
            thread_id: Uuid::new_v4(),
            title: Some("child task".into()),
            spawning_event_id: None,
            mode: ActorMode::Agent,
            direction: ThreadDirection::Child,
        };
        let res = make_message_received_with_origin(
            std::path::Path::new(""),
            "[Child thread completed] ...",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Agent,
            None,
            None,
            None,
            Some(origin),
            None,
        );
        assert!(res.is_ok());
    }

    #[test]
    fn no_origin_is_always_ok() {
        let res = make_message_received_with_origin(
            std::path::Path::new(""),
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(res.is_ok());
        let res2 = make_message_received_with_origin(
            std::path::Path::new(""),
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Agent,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(res2.is_ok());
    }

    /// A spoken message names its *voice session*, and a typed one names none.
    ///
    /// The field is the whole of how the transcript tells the two apart. Voice
    /// is a mode of a thread (ADR 0148), so a typed message sits between the
    /// same pair of session events a spoken one does.
    #[test]
    fn a_spoken_message_is_marked_and_a_typed_one_is_not() {
        let session_id = Uuid::new_v4();
        let spoken = make_message_received(
            std::path::Path::new(""),
            "what have I got running",
            None,
            Some("dev-1"),
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            None,
            None,
            Some(session_id),
        );
        let typed = make_message_received(
            std::path::Path::new(""),
            "what have I got running",
            None,
            Some("dev-1"),
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            None,
            None,
            None,
        );
        let ThreadEvent::MessageReceived {
            voice_session_id: on_spoken,
            ..
        } = spoken
        else {
            panic!("not a MessageReceived");
        };
        let ThreadEvent::MessageReceived {
            voice_session_id: on_typed,
            ..
        } = typed
        else {
            panic!("not a MessageReceived");
        };
        assert_eq!(on_spoken, Some(session_id));
        assert_eq!(on_typed, None);
    }

    /// Every row written before voice existed still decodes, and reads typed.
    #[test]
    fn a_row_from_before_voice_decodes_as_typed() {
        let legacy = serde_json::json!({
            "type": "MessageReceived",
            "text": "hi",
            "mode": "human",
        });
        let event: ThreadEvent = serde_json::from_value(legacy).expect("decode a legacy row");
        let ThreadEvent::MessageReceived {
            voice_session_id, ..
        } = event
        else {
            panic!("not a MessageReceived");
        };
        assert_eq!(voice_session_id, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- format_relative_age tests ---

    #[test]
    fn relative_age_just_now() {
        assert_eq!(
            format_relative_age(chrono::Duration::seconds(30)),
            "just now"
        );
        assert_eq!(
            format_relative_age(chrono::Duration::seconds(0)),
            "just now"
        );
    }

    #[test]
    fn relative_age_minutes() {
        assert_eq!(format_relative_age(chrono::Duration::minutes(5)), "5m ago");
        assert_eq!(
            format_relative_age(chrono::Duration::minutes(59)),
            "59m ago"
        );
    }

    #[test]
    fn relative_age_hours() {
        assert_eq!(format_relative_age(chrono::Duration::hours(2)), "2h ago");
        assert_eq!(format_relative_age(chrono::Duration::hours(23)), "23h ago");
    }

    #[test]
    fn relative_age_days() {
        assert_eq!(format_relative_age(chrono::Duration::days(1)), "1d ago");
        assert_eq!(format_relative_age(chrono::Duration::days(4)), "4d ago");
    }

    // --- emit_routing_failure tests ---

    use crate::engine::event_bus::{BusEvent, MockEventBus};
    use crate::engine::thread_events::ThreadEvent;

    #[tokio::test]
    async fn routing_failure_emits_response_failed() {
        let mock = MockEventBus::new();
        let tid = Uuid::new_v4();

        let result = emit_routing_failure(&mock, tid, "session gone").await;
        assert!(result.is_ok(), "successful emit must return Ok");

        let events = mock.emitted_events();
        assert_eq!(events.len(), 1, "must emit exactly one event");
        match &events[0] {
            BusEvent::Thread {
                thread_id, event, ..
            } => {
                assert_eq!(*thread_id, tid);
                match event {
                    ThreadEvent::ResponseFailed { error, .. } => {
                        assert_eq!(error, "session gone");
                    }
                    other => panic!("expected ResponseFailed, got {:?}", other),
                }
            }
            other => panic!("expected Thread event, got {:?}", other),
        }
    }

    /// `emit_routing_failure` must return `Ok` after a successful emit so
    /// the chat handler chain doesn't double-emit. The outer `api::chat`
    /// handler catches every `Err` from `process_message` and emits its own
    /// `ResponseFailed` — an `Err` here would land a second terminator on
    /// top of the one this function already emitted.
    #[tokio::test]
    async fn routing_failure_returns_ok_after_emit() {
        let mock = MockEventBus::new();
        let tid = Uuid::new_v4();
        let result = emit_routing_failure(&mock, tid, "session gone").await;
        assert!(
            result.is_ok(),
            "successful emit must return Ok so the outer handler doesn't double-emit \
             ResponseFailed for the same terminator"
        );
        let events = mock.emitted_events();
        assert_eq!(
            events.len(),
            1,
            "must emit exactly one ResponseFailed and then yield to the caller — \
             the caller is responsible for returning Ok(ProcessResult) so the api \
             layer's Err-branch emit doesn't run a second time"
        );
    }

    #[tokio::test]
    async fn routing_failure_returns_err_only_when_bus_fails() {
        let mock = MockEventBus::new();
        *mock.fail_with.lock().unwrap() = Some("db down".into());
        let tid = Uuid::new_v4();
        let result = emit_routing_failure(&mock, tid, "session gone").await;
        assert!(
            result.is_err(),
            "bus-emit failure is a real engine error — caller's outer handler \
             SHOULD emit its own ResponseFailed in that case (the in-flight \
             routing terminator never landed, so no double-emit risk)"
        );
        // The error message must surface the bus failure, not the original
        // routing-message — the upper layers log on Err and the bus failure
        // is the more actionable diagnostic.
        assert!(
            result.unwrap_err().to_string().contains("db down"),
            "bus-emit Err must propagate so post-mortem can find the bus cause",
        );
    }

    #[tokio::test]
    async fn routing_failure_uses_none_meta() {
        let mock = MockEventBus::new();
        let tid = Uuid::new_v4();

        let _ = emit_routing_failure(&mock, tid, "gone").await;

        let events = mock.emitted_events();
        match &events[0] {
            BusEvent::Thread { meta, .. } => {
                assert!(
                    meta.event_id.is_none(),
                    "routing failures must use EventMeta::NONE"
                );
                assert!(meta.request_event_id.is_none());
                assert!(meta.channel.is_none());
            }
            _ => panic!("expected Thread event"),
        }
    }
}
