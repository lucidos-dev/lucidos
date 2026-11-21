//! Construct the message and routing events shared by chat and the thread queue.

use std::path::Path;
use uuid::Uuid;

use crate::core::blobs::write_blob_from_base64;
use crate::engine::thread_events::{ActorMode, MessageOrigin, ThreadDirection, WakeMode};

/// Bad-base64 / unsupported-mime entries are dropped + logged; the event
/// still emits with the surviving hashes, matching the migration's
/// partial-failure policy.
pub fn images_to_hashes(
    workspace: &Path,
    images: Option<&[crate::engine::types::ChatImage]>,
) -> Vec<String> {
    let Some(images) = images else {
        return Vec::new();
    };
    images
        .iter()
        .filter_map(|img| match write_blob_from_base64(workspace, &img.base64) {
            Ok(blob) => Some(blob.hash),
            Err(e) => {
                crate::log_legacy!("[Image] blob write failed, image dropped: {}", e);
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
/// The API resolves `explicit_origin` from headers and DB state. Older callers
/// can omit it and get an origin derived from `device_id` or
/// `parent_thread_id`.
#[allow(clippy::too_many_arguments)]
pub fn make_message_received(
    workspace: &Path,
    user_message: &str,
    user_images: Option<&[crate::engine::types::ChatImage]>,
    device_id: Option<&str>,
    parent_thread_id: Option<Uuid>,
    spawning_event_id: Option<Uuid>,
    wake_mode: Option<WakeMode>,
    mode: ActorMode,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
    provider: Option<&str>,
    explicit_origin: Option<MessageOrigin>,
    voice_session_id: Option<Uuid>,
) -> crate::engine::thread_events::ThreadEvent {
    let origin = explicit_origin
        .or_else(|| synthesize_legacy_origin(mode, device_id, parent_thread_id, spawning_event_id));
    make_message_received_with_origin(
        workspace,
        user_message,
        user_images,
        device_id,
        parent_thread_id,
        spawning_event_id,
        wake_mode,
        mode,
        model,
        reasoning_effort,
        provider,
        origin,
        voice_session_id,
    )
    .expect("API boundary must build origin matching mode; legacy synthesis is always valid")
}

/// Build a MessageReceived event with an explicit origin, validating that the
/// origin variant matches the mode. Returns Err on mismatch so the API
/// boundary can reject malformed requests instead of persisting impossible state.
#[allow(clippy::too_many_arguments)]
pub fn make_message_received_with_origin(
    workspace: &Path,
    user_message: &str,
    user_images: Option<&[crate::engine::types::ChatImage]>,
    device_id: Option<&str>,
    parent_thread_id: Option<Uuid>,
    spawning_event_id: Option<Uuid>,
    wake_mode: Option<WakeMode>,
    mode: ActorMode,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
    provider: Option<&str>,
    origin: Option<MessageOrigin>,
    // Set only by the voice path. Every other caller types.
    voice_session_id: Option<Uuid>,
) -> Result<crate::engine::thread_events::ThreadEvent, Box<dyn std::error::Error + Send + Sync>> {
    if let Some(o) = &origin {
        validate_origin_mode(o, mode)?;
    }
    Ok(crate::engine::thread_events::ThreadEvent::MessageReceived {
        text: user_message.to_string(),
        user_image_hashes: images_to_hashes(workspace, user_images),
        device_id: device_id.map(|s| s.to_string()),
        image_description: None,
        parent_thread_id,
        spawning_event_id,
        wake_mode,
        mode,
        model: model.map(|s| s.to_string()),
        reasoning_effort: reasoning_effort.map(|s| s.to_string()),
        provider: provider.map(|s| s.to_string()),
        origin,
        voice_session_id,
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
    parent_thread_id: Option<Uuid>,
    spawning_event_id: Option<Uuid>,
) -> Option<MessageOrigin> {
    match mode {
        ActorMode::Human => device_id.map(|id| MessageOrigin::Device {
            device_id: id.to_string(),
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

/// The whole text half of an image-description request. Named so
/// `core::aux_context_backfill` can size a historical call from the same
/// string the live path sends, instead of a constant that drifts from it.
pub use crate::llm::auxiliary_prompts::IMAGE_DESCRIPTION_PROMPT;

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
pub async fn emit_routing_failure(
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
                spend_cap: None,
                agent_exit: None,
                provider_error: None,
            },
            meta: EventMeta::NONE,
        })
        .await
    {
        Ok(_) => Ok(()),
        Err(e) => {
            log_legacy!(
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
