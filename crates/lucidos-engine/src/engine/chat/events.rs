use uuid::Uuid;

use crate::engine::thread_events::{ActorMode, MessageOrigin, ThreadDirection};

/// Serialize user images to JSON values for event payloads.
fn serialize_images(images: Option<&[crate::api::ChatImage]>) -> Vec<serde_json::Value> {
    images
        .map(|imgs| {
            imgs.iter()
                .map(|img| {
                    serde_json::json!({
                        "base64": &img.base64,
                        "mime_type": &img.mime_type,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Build a MessageReceived thread event with standard fields.
/// Used by both fast-paths (CC and non-CC injection) and the main exchange path.
///
/// `model` and `reasoning_effort` are stamped on the event so the frontend's
/// route tooltip can display them while the response is still streaming
/// (before ResponseGenerated fires). For CC dispatches, leave both None — CC
/// emits CodingAgentSettingsChanged at session start instead.
///
/// `explicit_origin` is the structured origin captured at the API boundary
/// (headers + DB lookups). When None, a legacy origin is synthesized from
/// `device_id` / `parent_thread_id` so older call sites that don't yet supply
/// an explicit origin still produce a coherent `origin` field.
#[allow(clippy::too_many_arguments)]
pub(crate) fn make_message_received(
    user_message: &str,
    user_images: Option<&[crate::api::ChatImage]>,
    device_id: Option<&str>,
    device_name: Option<String>,
    parent_thread_id: Option<Uuid>,
    spawning_event_id: Option<Uuid>,
    mode: ActorMode,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
    explicit_origin: Option<MessageOrigin>,
) -> crate::engine::thread_events::ThreadEvent {
    let origin = explicit_origin.or_else(|| {
        synthesize_legacy_origin(
            mode,
            device_id,
            device_name.as_deref(),
            parent_thread_id,
            spawning_event_id,
        )
    });
    make_message_received_with_origin(
        user_message,
        user_images,
        device_id,
        device_name,
        parent_thread_id,
        spawning_event_id,
        mode,
        model,
        reasoning_effort,
        origin,
    )
    .expect("API boundary must build origin matching mode; legacy synthesis is always valid")
}

/// Build a MessageReceived event with an explicit origin, validating that the
/// origin variant matches the mode. Returns Err on mismatch so the API
/// boundary can reject malformed requests instead of persisting impossible state.
#[allow(clippy::too_many_arguments)]
pub(super) fn make_message_received_with_origin(
    user_message: &str,
    user_images: Option<&[crate::api::ChatImage]>,
    device_id: Option<&str>,
    device_name: Option<String>,
    parent_thread_id: Option<Uuid>,
    spawning_event_id: Option<Uuid>,
    mode: ActorMode,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
    origin: Option<MessageOrigin>,
) -> Result<crate::engine::thread_events::ThreadEvent, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(o) = &origin {
        validate_origin_mode(o, mode)?;
    }
    Ok(crate::engine::thread_events::ThreadEvent::MessageReceived {
        text: user_message.to_string(),
        images: serialize_images(user_images),
        device_id: device_id.map(|s| s.to_string()),
        device: device_name,
        image_description: None,
        parent_thread_id,
        spawning_event_id,
        mode,
        model: model.map(|s| s.to_string()),
        reasoning_effort: reasoning_effort.map(|s| s.to_string()),
        origin,
    })
}

/// Enforce the `MessageOrigin ↔ ActorMode` contract by deferring to
/// `MessageOrigin::mode()` as the single source of truth. Any drift between
/// the carried mode and the variant's intrinsic/derived mode is a bug.
fn validate_origin_mode(
    origin: &MessageOrigin,
    mode: ActorMode,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let derived = origin.mode();
    if mode != derived {
        return Err(format!(
            "MessageOrigin/mode mismatch: origin {:?} implies {:?}, got {:?}",
            origin, derived, mode
        )
        .into());
    }
    Ok(())
}

fn synthesize_legacy_origin(
    mode: ActorMode,
    device_id: Option<&str>,
    device_name: Option<&str>,
    parent_thread_id: Option<Uuid>,
    spawning_event_id: Option<Uuid>,
) -> Option<MessageOrigin> {
    match mode {
        ActorMode::Human => device_id.map(|id| MessageOrigin::Device {
            device_id: id.to_string(),
            label: crate::core::devices::resolve_device_name(device_name, id),
        }),
        ActorMode::Agent => parent_thread_id.map(|id| MessageOrigin::ThreadLink {
            thread_id: id,
            title: None,
            spawning_event_id,
            mode: ActorMode::Agent,
            direction: ThreadDirection::Parent,
        }),
        ActorMode::Engine => parent_thread_id.map(|id| MessageOrigin::ThreadLink {
            thread_id: id,
            title: None,
            spawning_event_id,
            mode: ActorMode::Engine,
            direction: ThreadDirection::Parent,
        }),
    }
}

/// Generate a brief description of user-attached images using Flash.
/// Standalone function so it can be spawned into a background task.
pub(super) async fn describe_images(
    provider: &crate::llm::vertex::VertexProvider,
    images: &[crate::api::ChatImage],
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use crate::llm::provider::{ContentBlock, LlmProvider, Message, MessageContent};

    let mut blocks: Vec<ContentBlock> = vec![ContentBlock::Text {
        text: "Describe the image and transcribe ALL visible text exactly as it appears. Include every detail: names, dates, times, numbers, labels, headings. If multiple images, number them.".to_string(),
    }];
    for img in images {
        blocks.push(ContentBlock::Image {
            source_type: "base64".to_string(),
            media_type: img.mime_type.clone(),
            data: img.base64.clone(),
        });
    }

    let messages = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Blocks(blocks),
    }];

    let response = provider
        .chat(messages, vec![], None, None, None, None)
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

/// Emit ResponseFailed to close a dangling exchange and return an error.
/// Used when a session/thread disappears between the existence check and the
/// channel send (TOCTOU window in the follow-up fast-paths).
pub(super) async fn emit_routing_failure(
    bus: &dyn crate::engine::event_bus::EventBusEmitter,
    thread_id: Uuid,
    error: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crate::engine::thread_events::EventMeta;
    if let Err(e) = bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                error: error.to_string(),
            },
            meta: EventMeta::NONE,
        })
        .await
    {
        log!(
            "[Chat] Failed to emit ResponseFailed for thread {}: {}",
            thread_id,
            e
        );
    }
    Err(error.into())
}

#[cfg(test)]
mod origin_invariants {
    use super::*;
    use crate::engine::thread_events::MessageOrigin;
    use uuid::Uuid;

    #[test]
    fn device_origin_with_human_mode_ok() {
        let origin = MessageOrigin::Device {
            device_id: "dev-1".into(),
            label: "Chrome on Mac".into(),
        };
        let res = make_message_received_with_origin(
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            Some(origin),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn device_origin_with_agent_mode_rejected() {
        let origin = MessageOrigin::Device {
            device_id: "dev-1".into(),
            label: "Chrome on Mac".into(),
        };
        let res = make_message_received_with_origin(
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Agent,
            None,
            None,
            Some(origin),
        );
        assert!(res.is_err(), "Device origin must reject non-Human mode");
    }

    #[test]
    fn api_origin_with_human_mode_ok() {
        let origin = MessageOrigin::Api {
            user_agent: Some("curl/8".into()),
            mode: ActorMode::Human,
        };
        let res = make_message_received_with_origin(
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            Some(origin),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn api_origin_with_agent_mode_ok() {
        // Third-party SDK identifying as an agent (LLM acting on user's behalf).
        let origin = MessageOrigin::Api {
            user_agent: Some("MyApp/1.0".into()),
            mode: ActorMode::Agent,
        };
        let res = make_message_received_with_origin(
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Agent,
            None,
            None,
            Some(origin),
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
        };
        let res = make_message_received_with_origin(
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Engine,
            None,
            None,
            Some(origin),
        );
        assert!(res.is_ok(), "Api origin must accept Engine mode");
    }

    #[test]
    fn api_origin_with_mismatched_mode_rejected() {
        // The `mode` field on Api must match the request's claimed mode.
        let origin = MessageOrigin::Api {
            user_agent: None,
            mode: ActorMode::Agent,
        };
        let res = make_message_received_with_origin(
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            Some(origin),
        );
        assert!(res.is_err(), "Api origin mode must match request mode");
    }

    #[test]
    fn workspace_origin_with_matching_mode_ok() {
        let origin = MessageOrigin::Workspace {
            workspace: "personal".into(),
            thread_id: None,
            event_id: None,
            user_agent: None,
            mode: ActorMode::Human,
        };
        let res = make_message_received_with_origin(
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            Some(origin),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn workspace_origin_with_mismatched_mode_rejected() {
        let origin = MessageOrigin::Workspace {
            workspace: "personal".into(),
            thread_id: None,
            event_id: None,
            user_agent: None,
            mode: ActorMode::Human,
        };
        let res = make_message_received_with_origin(
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Agent,
            None,
            None,
            Some(origin),
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
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Agent,
            None,
            None,
            Some(origin),
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
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            Some(origin),
        );
        assert!(
            res.is_err(),
            "ParentThread origin must reject mode that doesn't match the carried field"
        );
    }

    #[test]
    fn engine_origin_with_engine_mode_ok() {
        let origin = MessageOrigin::Engine {
            reason: crate::engine::thread_events::EngineReason::SessionRecovered,
        };
        let res = make_message_received_with_origin(
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Engine,
            None,
            None,
            Some(origin),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn engine_origin_with_human_mode_rejected() {
        let origin = MessageOrigin::Engine {
            reason: crate::engine::thread_events::EngineReason::HardenRetrigger,
        };
        let res = make_message_received_with_origin(
            "hi",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Human,
            None,
            None,
            Some(origin),
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
            "do the thing",
            None,
            None,
            None,
            Some(parent_id),
            spawn_id,
            ActorMode::Agent,
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
            "[Child thread completed] ...",
            None,
            None,
            None,
            None,
            None,
            ActorMode::Agent,
            None,
            None,
            Some(origin),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn no_origin_is_always_ok() {
        let res = make_message_received_with_origin(
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
        );
        assert!(res.is_ok());
        let res2 = make_message_received_with_origin(
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
        );
        assert!(res2.is_ok());
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
        assert!(result.is_err());

        let events = mock.emitted_events();
        assert_eq!(events.len(), 1, "must emit exactly one event");
        match &events[0] {
            BusEvent::Thread {
                thread_id, event, ..
            } => {
                assert_eq!(*thread_id, tid);
                match event {
                    ThreadEvent::ResponseFailed { error } => {
                        assert_eq!(error, "session gone");
                    }
                    other => panic!("expected ResponseFailed, got {:?}", other),
                }
            }
            other => panic!("expected Thread event, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn routing_failure_returns_error_with_message() {
        let mock = MockEventBus::new();
        let tid = Uuid::new_v4();

        let err = emit_routing_failure(
            &mock,
            tid,
            "Thread ended while routing message. Please try again.",
        )
        .await
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Thread ended while routing message. Please try again."
        );
    }

    #[tokio::test]
    async fn routing_failure_emits_even_when_bus_fails() {
        let mock = MockEventBus::new();
        *mock.fail_with.lock().unwrap() = Some("db down".into());
        let tid = Uuid::new_v4();

        // Should still return Err (the routing error), not the bus error
        let result = emit_routing_failure(&mock, tid, "session gone").await;
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().to_string(), "session gone");
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
