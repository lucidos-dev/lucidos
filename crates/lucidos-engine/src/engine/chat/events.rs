use std::path::Path;
use uuid::Uuid;

use crate::core::blobs::write_blob_from_base64;
use crate::engine::thread_events::{ActorMode, MessageOrigin, ThreadDirection};

/// Bad-base64 / unsupported-mime entries are dropped + logged; the event
/// still emits with the surviving hashes, matching the migration's
/// partial-failure policy.
fn images_to_hashes(workspace: &Path, images: Option<&[crate::api::ChatImage]>) -> Vec<String> {
    let Some(images) = images else {
        return Vec::new();
    };
    images
        .iter()
        .filter_map(|img| match write_blob_from_base64(workspace, &img.base64) {
            Ok(blob) => Some(blob.hash),
            Err(e) => {
                crate::log!("[Image] make_message_received: blob write failed: {}", e);
                None
            }
        })
        .collect()
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
    workspace: &Path,
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
        workspace,
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
    workspace: &Path,
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
        user_image_hashes: images_to_hashes(workspace, user_images),
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

/// The clock-free half of the pre-emit rule (see
/// [`crate::engine::LucidosEngine::pre_emit_chat_message_received`] for what
/// each exclusion costs and why). Split out so the rule is readable and
/// testable on its own; the caller pairs it with the open-question lookup,
/// which needs the database.
pub(super) fn chat_message_is_pre_emittable(
    mode: ActorMode,
    use_coding_agent: Option<bool>,
    thread_exists: bool,
) -> bool {
    mode == ActorMode::Human && use_coding_agent != Some(true) && thread_exists
}

impl crate::engine::LucidosEngine {
    /// Persist a chat follow-up's `MessageReceived` at the API boundary, before
    /// the handler acks, and report it back so the spawned turn does not emit a
    /// second one.
    ///
    /// This is what makes a `200` from `POST /chat/stream` mean "your message is
    /// in the thread, with its sequence assigned". The handler otherwise spawns
    /// the work and returns immediately, and the spawned task waits for a Thread
    /// Queue slot before it emits, so the gap between the ack and the write is
    /// unbounded at pool-max. A client that serializes its sends still could not
    /// rely on the ack for ordering across that gap: the second request's task
    /// could win the race and take the lower sequence, which is unrecoverable
    /// because `created` is stamped at emit time and the request carries no
    /// client-side ordering data. See
    /// `docs/plans/2026-07-30-serialize-chat-sends-per-thread.md`.
    ///
    /// Returns `None` when this request must NOT be pre-emitted, and the caller
    /// then behaves exactly as before:
    ///
    /// - **Not a person typing** (`mode != Human`). Agent and engine callers
    ///   have their own boundary events and their own ordering.
    /// - **A coding-agent dispatch.** That path deliberately emits its
    ///   `MessageReceived` *after* a Codex redirect interrupt reaches a
    ///   boundary, so the interrupted turn's terminal sequences first; emitting
    ///   here would reorder the exchange.
    /// - **A thread that does not exist yet.** Its first message has nothing to
    ///   be out of order with, and the starter event stays behind whatever the
    ///   slow path emits for a new thread.
    /// - **A thread with an open question.** A typed reply there is rerouted to
    ///   `UserQuestionAnswered` and never becomes a `MessageReceived`, so
    ///   emitting one would be a spurious extra event.
    /// - **A failed emit.** Logged, then treated as no pre-emission, so a
    ///   transient hiccup costs the ordering guarantee for one message rather
    ///   than the message itself.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn pre_emit_chat_message_received(
        &self,
        thread_id: Option<Uuid>,
        thread_exists: bool,
        mode: ActorMode,
        use_coding_agent: Option<bool>,
        user_message: &str,
        user_images: Option<&[crate::api::ChatImage]>,
        device_id: Option<&str>,
        model: Option<&str>,
        reasoning_effort: Option<&str>,
        event_id: Option<&str>,
        origin: Option<MessageOrigin>,
    ) -> Option<crate::engine::PreEmittedOrigin> {
        let thread_id = thread_id?;
        if !chat_message_is_pre_emittable(mode, use_coding_agent, thread_exists) {
            return None;
        }
        // An open question turns the next typed message into its answer.
        // Same lookup the turn itself uses, so the two agree.
        if crate::engine::agent_question::lookup_active_question_tool_use_id(self.pool(), thread_id)
            .await
            .is_some()
        {
            return None;
        }

        // Stamp the same resolved model / effort the turn would have stamped,
        // so per-thread model memory reads back an unchanged value. The turn
        // re-resolves with this event excluded, landing on the same answer.
        let (model, reasoning_effort) =
            crate::core::PreferenceStore::resolve_chat_overrides_for_thread(
                self.pool(),
                Some(thread_id),
                None,
                model.map(str::to_string),
                reasoning_effort.map(str::to_string),
            )
            .await;
        let device_name = match device_id {
            Some(did) => crate::core::DeviceStore::tooltip_info(self.pool(), did).await,
            None => None,
        };

        use crate::engine::thread_events::{EventChannel, EventMeta};
        let emitted = self
            .event_bus
            .emit(crate::engine::event_bus::BusEvent::Thread {
                thread_id,
                event: make_message_received(
                    self.workspace_path(),
                    user_message,
                    user_images,
                    device_id,
                    device_name,
                    // A human message never carries spawn linkage
                    // (`validate_mode_and_spawn` rejects it).
                    None,
                    None,
                    mode,
                    model.as_deref(),
                    reasoning_effort.as_deref(),
                    origin,
                ),
                meta: EventMeta {
                    event_id: event_id.and_then(|s| Uuid::parse_str(s).ok()),
                    channel: Some(EventChannel::Chat),
                    ..EventMeta::NONE
                },
            })
            .await;
        match emitted {
            Ok(Some(result)) => Some(crate::engine::PreEmittedOrigin::Message(result.event_id)),
            Ok(None) => {
                crate::log!(
                    "[Chat] Pre-ack MessageReceived for thread {} returned no result; the turn will emit it",
                    thread_id
                );
                None
            }
            Err(e) => {
                crate::log!(
                    "[Chat] Pre-ack MessageReceived for thread {} failed ({}); the turn will emit it",
                    thread_id,
                    e
                );
                None
            }
        }
    }
}

/// The whole text half of an image-description request. Named so
/// `core::aux_context_backfill` can size a historical call from the same
/// string the live path sends, instead of a constant that drifts from it.
pub(crate) const IMAGE_DESCRIPTION_PROMPT: &str = "Describe the image and transcribe ALL visible text exactly as it appears. Include every detail: names, dates, times, numbers, labels, headings. If multiple images, number them.";

/// Generate a brief description of user-attached images using Flash.
/// Standalone function so it can be spawned into a background task.
///
/// `capture` records the call for token accounting. Its request size counts
/// the encoded image bytes, which is what the provider actually charged for.
pub(super) async fn describe_images(
    provider: &dyn crate::llm::provider::LlmProvider,
    images: &[crate::api::ChatImage],
    // The `reasoning_image_description` preference. It used to be `None`, which
    // `gemini_generation_config` reads as `high`, so a caption was paying for
    // the model's deepest thinking.
    reasoning_effort: Option<&str>,
    capture: Option<&crate::engine::AuxCapture>,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    use crate::llm::provider::{ContentBlock, Message, MessageContent};

    let mut blocks: Vec<ContentBlock> = vec![ContentBlock::Text {
        text: IMAGE_DESCRIPTION_PROMPT.to_string(),
    }];
    let mut request_chars = IMAGE_DESCRIPTION_PROMPT.chars().count();
    for img in images {
        // Fit each image to the LLM size target (compress only if over) so the
        // description pass can't trip the provider's per-image limit either.
        let fitted = img.clone().fit_for_llm();
        request_chars += fitted.base64.len();
        blocks.push(super::images::image_content_block(fitted));
    }

    let messages = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Blocks(blocks),
    }];

    let response = provider
        .chat(messages, vec![], None, None, None, reasoning_effort)
        .await?;
    if let Some(capture) = capture {
        capture
            .record(provider.default_model(), request_chars, &response)
            .await;
    }
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

/// Emit `ResponseFailed` to close a dangling exchange. Returns `Ok(())` on
/// successful emit; `Err` only when the bus itself failed.
///
/// Used when a session/thread disappears between the existence check and the
/// channel send (TOCTOU window in the follow-up fast-paths).
///
/// Unanchored on purpose, and the one terminator that stays so. It fires
/// before this turn has an originating event of its own, closing the exchange
/// the vanished thread left open. There is nothing to anchor it to.
///
/// **Emit-only contract.** The caller still returns `Ok(ProcessResult{
/// response: String::new(), … })` from its fast-path, because the terminator
/// has already landed here and the turn has nothing left to do. Nothing
/// downstream would double-emit now: `process_message_with_steps` settles the
/// exchange only once one exists, and no caller adds a terminator of its own.
pub(super) async fn emit_routing_failure(
    bus: &dyn crate::engine::event_bus::EventBusEmitter,
    thread_id: Uuid,
    error: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use crate::engine::thread_events::EventMeta;
    match bus
        .emit(crate::engine::event_bus::BusEvent::Thread {
            thread_id,
            event: crate::engine::thread_events::ThreadEvent::ResponseFailed {
                error: error.to_string(),
            },
            meta: EventMeta::NONE,
        })
        .await
    {
        Ok(_) => Ok(()),
        Err(e) => {
            log!(
                "[Chat] Failed to emit ResponseFailed for thread {}: {} — outer \
                 handler will emit its own ResponseFailed since the routing \
                 terminator never landed",
                thread_id,
                e,
            );
            Err(e)
        }
    }
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
            Some(origin),
        );
        assert!(res.is_err(), "Device origin must reject non-Human mode");
    }

    #[test]
    fn api_origin_with_human_mode_ok() {
        let origin = MessageOrigin::Api {
            user_agent: Some("curl/8".into()),
            mode: ActorMode::Human,
            source_thread_id: None,
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
            source_thread_id: None,
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
            source_thread_id: None,
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
            source_thread_id: None,
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
            Some(origin),
        );
        assert!(res.is_err(), "Api origin mode must match request mode");
    }

    #[test]
    fn workspace_origin_with_matching_mode_ok() {
        let origin = MessageOrigin::Workspace {
            workspace: "dev".into(),
            thread_id: None,
            event_id: None,
            user_agent: None,
            mode: ActorMode::Human,
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
            Some(origin),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn workspace_origin_with_mismatched_mode_rejected() {
        let origin = MessageOrigin::Workspace {
            workspace: "dev".into(),
            thread_id: None,
            event_id: None,
            user_agent: None,
            mode: ActorMode::Human,
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
            Some(origin),
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
            std::path::Path::new(""),
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
            Some(origin),
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
        );
        assert!(res2.is_ok());
    }
}

#[cfg(test)]
mod pre_emit_eligibility {
    use super::*;

    /// The case the whole change exists for: a person's follow-up on a thread
    /// that already exists. Only here can two requests race, and only here does
    /// pre-emitting buy an ordering guarantee.
    #[test]
    fn human_chat_follow_up_is_pre_emittable() {
        assert!(chat_message_is_pre_emittable(ActorMode::Human, None, true));
        assert!(chat_message_is_pre_emittable(
            ActorMode::Human,
            Some(false),
            true
        ));
    }

    /// A brand-new thread's first message has nothing to be out of order with,
    /// and pre-emitting would put `MessageReceived` ahead of whatever the slow
    /// path emits for a new thread.
    #[test]
    fn first_message_on_a_new_thread_is_not_pre_emittable() {
        assert!(!chat_message_is_pre_emittable(
            ActorMode::Human,
            None,
            false
        ));
    }

    /// The coding-agent path emits its `MessageReceived` only AFTER a Codex
    /// redirect interrupt reaches a boundary, so the interrupted turn's
    /// terminal sequences first. Pre-emitting would reorder the exchange.
    #[test]
    fn coding_agent_dispatch_is_not_pre_emittable() {
        assert!(!chat_message_is_pre_emittable(
            ActorMode::Human,
            Some(true),
            true
        ));
    }

    /// Agent- and engine-mode posts carry their own boundary events and their
    /// own ordering; the API boundary must not speak for them.
    #[test]
    fn non_human_modes_are_not_pre_emittable() {
        assert!(!chat_message_is_pre_emittable(ActorMode::Agent, None, true));
        assert!(!chat_message_is_pre_emittable(
            ActorMode::Engine,
            None,
            true
        ));
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
                    ThreadEvent::ResponseFailed { error } => {
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
