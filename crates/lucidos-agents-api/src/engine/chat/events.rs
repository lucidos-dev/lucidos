use crate::engine::message_events::IMAGE_DESCRIPTION_PROMPT;
#[cfg(test)]
use crate::engine::message_events::{
    emit_routing_failure, make_message_received, make_message_received_with_origin,
};
#[cfg(test)]
use crate::engine::thread_events::{ActorMode, ThreadDirection};
#[cfg(test)]
use uuid::Uuid;

pub(super) async fn describe_images(
    provider: &dyn crate::llm::provider::LlmProvider,
    images: &[crate::engine::types::ChatImage],
    reasoning_effort: Option<&str>,
    capture: &crate::engine::AuxCapture,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use crate::llm::provider::{ContentBlock, Message, MessageContent};

    let mut blocks: Vec<ContentBlock> = vec![ContentBlock::Text {
        text: IMAGE_DESCRIPTION_PROMPT.to_string(),
    }];
    for img in images {
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

    struct CallRecorder {
        efforts: std::sync::Mutex<Vec<Option<String>>>,
        image_lens: std::sync::Mutex<Vec<usize>>,
    }

    impl CallRecorder {
        fn new() -> Self {
            Self {
                efforts: std::sync::Mutex::new(Vec::new()),
                image_lens: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl crate::llm::provider::LlmProvider for CallRecorder {
        async fn chat(
            &self,
            messages: Vec<crate::llm::provider::Message>,
            _tools: Vec<crate::llm::provider::ToolDefinition>,
            selection: crate::llm::ModelSelection<'_>,
            _system_prompt: Option<&str>,
            _on_token: Option<crate::llm::provider::TokenCallback>,
            _call: crate::llm::metered::CallToken,
        ) -> Result<crate::llm::provider::LlmResponse, Box<dyn std::error::Error + Send + Sync>>
        {
            self.efforts
                .lock()
                .unwrap()
                .push(selection.reasoning_effort.map(str::to_string));
            for message in &messages {
                if let crate::llm::provider::MessageContent::Blocks(blocks) = &message.content {
                    for block in blocks {
                        if let crate::llm::provider::ContentBlock::Image { data, .. } = block {
                            self.image_lens.lock().unwrap().push(data.len());
                        }
                    }
                }
            }
            Ok(crate::llm::provider::LlmResponse {
                content: Some("a caption".into()),
                ..Default::default()
            })
        }
    }

    fn tiny_image() -> crate::engine::types::ChatImage {
        crate::engine::types::ChatImage {
            base64: "AAAA".to_string(),
            mime_type: "image/png".to_string(),
        }
    }

    #[tokio::test]
    async fn describe_images_sends_the_reasoning_effort_it_is_given() {
        let provider = CallRecorder::new();
        let capture =
            crate::engine::AuxCapture::discarding(crate::engine::ContextPurpose::ImageDescribe);

        let caption = describe_images(&provider, &[tiny_image()], Some("low"), &capture)
            .await
            .unwrap();

        assert_eq!(caption, "a caption");
        assert_eq!(
            *provider.efforts.lock().unwrap(),
            vec![Some("low".to_string())],
            "a None here reads as the highest effort and the caption pays for deep thinking"
        );
    }

    #[tokio::test]
    async fn describe_images_fits_an_oversized_image_before_sending() {
        use base64::Engine as _;
        let noise: image::ImageBuffer<image::Rgba<u8>, Vec<u8>> =
            image::ImageBuffer::from_fn(1400, 1400, |x, y| {
                let mut h = x.wrapping_mul(2_654_435_761) ^ y.wrapping_mul(2_246_822_519);
                h ^= h >> 15;
                h = h.wrapping_mul(0x85EB_CA6B);
                h ^= h >> 13;
                let b = h.to_le_bytes();
                image::Rgba([b[0], b[1], b[2], 255])
            });
        let mut png = std::io::Cursor::new(Vec::new());
        noise.write_to(&mut png, image::ImageFormat::Png).unwrap();
        let big = crate::engine::types::ChatImage {
            base64: base64::engine::general_purpose::STANDARD.encode(png.into_inner()),
            mime_type: "image/png".to_string(),
        };
        assert!(big.base64.len() > crate::engine::types::MAX_IMAGE_BASE64_BYTES);

        let provider = CallRecorder::new();
        let capture =
            crate::engine::AuxCapture::discarding(crate::engine::ContextPurpose::ImageDescribe);
        describe_images(&provider, &[big], None, &capture)
            .await
            .unwrap();

        let lens = provider.image_lens.lock().unwrap().clone();
        assert_eq!(lens.len(), 1);
        assert!(
            lens[0] <= crate::engine::types::MAX_IMAGE_BASE64_BYTES,
            "the description pass sent {} base64 bytes unfitted",
            lens[0]
        );
    }
}
