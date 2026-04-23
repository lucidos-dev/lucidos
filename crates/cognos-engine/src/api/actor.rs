//! Resolve the actor (`MessageOrigin`) for an inbound HTTP request.
//!
//! Every mutating endpoint that emits an event (apply/discard/revert a change,
//! create/edit a thread, run a trigger, etc.) MUST stamp the produced event with
//! an actor so the timeline can show *who* initiated each system action.
//!
//! Header precedence (mode = `Human`):
//! 1. `X-Cognos-Workspace` set → `Workspace` (other CognOS workspace; carries
//!    optional thread/event id headers)
//! 2. `device_id` present     → `Device` (label looked up by caller from the
//!    `devices` table)
//! 3. else                    → `Api` (carries `User-Agent`)
//!
//! Mode = `Agent` or `Engine`:
//! - With `X-Cognos-Workspace` set → `Workspace` (cross-workspace agent/engine call)
//! - Otherwise → `ParentThread` when `parent_thread_id` is set, else `None`
//!   (callers must construct `MessageOrigin::Engine { reason }` directly).
//!
//! Workspace headers are user-controllable — treat as a display hint only,
//! never for authorization.

use crate::engine::http::workspace_client::{
    HEADER_EVENT_ID, HEADER_MODE, HEADER_THREAD_ID, HEADER_WORKSPACE,
};
use crate::engine::thread_events::{ActorMode, MessageOrigin};
use uuid::Uuid;

/// Header carrying the originating browser/device id on mutating endpoints
/// that don't accept a request body (apply/discard/revert, pin/unpin, etc.).
/// Frontend's `json()` helper sets this from `getDeviceId()`.
pub const HEADER_DEVICE_ID: &str = "x-cognos-device-id";

pub fn build_message_origin(
    headers: &axum::http::HeaderMap,
    mode: ActorMode,
    device_id: Option<&str>,
    device_label: Option<String>,
    parent_thread_id: Option<Uuid>,
    parent_thread_title: Option<String>,
    spawning_event_id: Option<Uuid>,
) -> Option<MessageOrigin> {
    let user_agent = header_str(headers, "user-agent");
    match mode {
        ActorMode::Human => {
            if let Some(workspace) = header_str(headers, HEADER_WORKSPACE) {
                Some(MessageOrigin::Workspace {
                    workspace,
                    thread_id: header_uuid(headers, HEADER_THREAD_ID),
                    event_id: header_uuid(headers, HEADER_EVENT_ID),
                    user_agent,
                    mode: workspace_mode_from_header(headers),
                })
            } else if let Some(id) = device_id {
                Some(MessageOrigin::Device {
                    device_id: id.to_string(),
                    label: device_label.unwrap_or_else(|| "Unknown device".to_string()),
                })
            } else {
                Some(MessageOrigin::Api { user_agent })
            }
        }
        ActorMode::Agent | ActorMode::Engine => {
            // Cross-workspace agent/engine call carries workspace header.
            if let Some(workspace) = header_str(headers, HEADER_WORKSPACE) {
                return Some(MessageOrigin::Workspace {
                    workspace,
                    thread_id: header_uuid(headers, HEADER_THREAD_ID),
                    event_id: header_uuid(headers, HEADER_EVENT_ID),
                    user_agent,
                    mode: workspace_mode_from_header(headers),
                });
            }
            // Otherwise must have a parent thread context.
            parent_thread_id.map(|id| MessageOrigin::ParentThread {
                thread_id: id,
                title: parent_thread_title,
                spawning_event_id,
                mode,
            })
        }
    }
}

/// Resolve the `ActorMode` for a `MessageOrigin::Workspace` from the
/// `X-Cognos-Mode` header. Defaults to `Human` for backward compatibility
/// with older workspace clients that don't yet stamp the header.
fn workspace_mode_from_header(headers: &axum::http::HeaderMap) -> ActorMode {
    match header_str(headers, HEADER_MODE).as_deref() {
        Some("agent") => ActorMode::Agent,
        Some("engine") => ActorMode::Engine,
        _ => ActorMode::Human,
    }
}

/// Convenience: build an actor for a `User`-initiated mutating endpoint that
/// has no parent-thread context (apply/discard/revert, settings writes, etc.).
///
/// `device_id` and `device_label` are optional explicit overrides — when both
/// are `None`, the header `x-cognos-device-id` (if present) supplies the id
/// and the resulting `Device` actor uses "Unknown device" as the label.
/// Callers that have access to the `devices` table should look up the label
/// themselves and pass both args explicitly.
pub fn user_actor(
    headers: &axum::http::HeaderMap,
    device_id: Option<&str>,
    device_label: Option<String>,
) -> Option<MessageOrigin> {
    let header_did = if device_id.is_none() {
        header_str(headers, HEADER_DEVICE_ID)
    } else {
        None
    };
    let effective_did = device_id.or(header_did.as_deref());
    build_message_origin(
        headers,
        ActorMode::Human,
        effective_did,
        device_label,
        None,
        None,
        None,
    )
}

fn header_str(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

fn header_uuid(headers: &axum::http::HeaderMap, name: &str) -> Option<Uuid> {
    header_str(headers, name).and_then(|s| Uuid::parse_str(&s).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers_with(pairs: &[(&str, &str)]) -> axum::http::HeaderMap {
        let mut h = axum::http::HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                axum::http::HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn build_origin_user_with_workspace_header_yields_workspace() {
        let src_thread = Uuid::new_v4();
        let src_event = Uuid::new_v4();
        let h = headers_with(&[
            ("x-cognos-workspace", "personal"),
            ("x-cognos-thread-id", &src_thread.to_string()),
            ("x-cognos-event-id", &src_event.to_string()),
            ("user-agent", "cognos-engine/0.1"),
        ]);
        let origin = build_message_origin(
            &h,
            ActorMode::Human,
            Some("dev-1"),
            Some("dev label".into()),
            None,
            None,
            None,
        );
        match origin {
            Some(MessageOrigin::Workspace {
                workspace,
                thread_id,
                event_id,
                user_agent,
                mode: _,
            }) => {
                assert_eq!(workspace, "personal");
                assert_eq!(thread_id, Some(src_thread));
                assert_eq!(event_id, Some(src_event));
                assert_eq!(user_agent.as_deref(), Some("cognos-engine/0.1"));
            }
            other => panic!("expected Workspace, got {:?}", other),
        }
    }

    #[test]
    fn build_origin_user_workspace_takes_precedence_over_device() {
        let h = headers_with(&[("x-cognos-workspace", "work")]);
        let origin = build_message_origin(
            &h,
            ActorMode::Human,
            Some("dev-1"),
            Some("Chrome".into()),
            None,
            None,
            None,
        );
        assert!(matches!(origin, Some(MessageOrigin::Workspace { .. })));
    }

    #[test]
    fn build_origin_user_with_device_id_yields_device_with_label() {
        let h = headers_with(&[]);
        let origin = build_message_origin(
            &h,
            ActorMode::Human,
            Some("dev-1"),
            Some("Chrome on Mac".into()),
            None,
            None,
            None,
        );
        match origin {
            Some(MessageOrigin::Device { device_id, label }) => {
                assert_eq!(device_id, "dev-1");
                assert_eq!(label, "Chrome on Mac");
            }
            other => panic!("expected Device, got {:?}", other),
        }
    }

    #[test]
    fn build_origin_user_with_device_id_no_label_falls_back_to_unknown() {
        let h = headers_with(&[]);
        let origin =
            build_message_origin(&h, ActorMode::Human, Some("dev-1"), None, None, None, None);
        match origin {
            Some(MessageOrigin::Device { label, .. }) => assert_eq!(label, "Unknown device"),
            other => panic!("expected Device, got {:?}", other),
        }
    }

    #[test]
    fn build_origin_user_no_device_no_workspace_yields_api() {
        let h = headers_with(&[("user-agent", "curl/8")]);
        let origin = build_message_origin(&h, ActorMode::Human, None, None, None, None, None);
        match origin {
            Some(MessageOrigin::Api { user_agent }) => {
                assert_eq!(user_agent.as_deref(), Some("curl/8"));
            }
            other => panic!("expected Api, got {:?}", other),
        }
    }

    #[test]
    fn build_origin_human_mode_with_workspace_yields_workspace_human_mode() {
        let h = headers_with(&[("x-cognos-workspace", "personal")]);
        let origin = build_message_origin(&h, ActorMode::Human, None, None, None, None, None);
        match origin {
            Some(MessageOrigin::Workspace {
                workspace, mode, ..
            }) => {
                assert_eq!(workspace, "personal");
                assert_eq!(mode, ActorMode::Human);
            }
            other => panic!("expected Workspace, got {:?}", other),
        }
    }

    #[test]
    fn build_origin_agent_mode_with_workspace_yields_workspace_agent_mode() {
        // The `X-Cognos-Mode` header is the authoritative source for the
        // Workspace branch's mode field — see `workspace_mode_from_header`.
        let h = headers_with(&[("x-cognos-workspace", "personal"), ("x-cognos-mode", "agent")]);
        let origin = build_message_origin(&h, ActorMode::Agent, None, None, None, None, None);
        match origin {
            Some(MessageOrigin::Workspace { mode, .. }) => assert_eq!(mode, ActorMode::Agent),
            other => panic!("expected Workspace agent mode, got {:?}", other),
        }
    }

    #[test]
    fn build_origin_engine_mode_with_workspace_yields_workspace_engine_mode() {
        // The `X-Cognos-Mode` header is the authoritative source for the
        // Workspace branch's mode field — see `workspace_mode_from_header`.
        let h = headers_with(&[
            ("x-cognos-workspace", "personal"),
            ("x-cognos-mode", "engine"),
        ]);
        let origin = build_message_origin(&h, ActorMode::Engine, None, None, None, None, None);
        match origin {
            Some(MessageOrigin::Workspace { mode, .. }) => assert_eq!(mode, ActorMode::Engine),
            other => panic!("expected Workspace engine mode, got {:?}", other),
        }
    }

    #[test]
    fn build_origin_agent_mode_with_parent_thread_yields_parent_thread_agent_mode() {
        let parent_id = Uuid::new_v4();
        let origin = build_message_origin(
            &headers_with(&[]),
            ActorMode::Agent,
            None,
            None,
            Some(parent_id),
            Some("parent".into()),
            None,
        );
        match origin {
            Some(MessageOrigin::ParentThread {
                thread_id, mode, ..
            }) => {
                assert_eq!(thread_id, parent_id);
                assert_eq!(mode, ActorMode::Agent);
            }
            other => panic!("expected ParentThread, got {:?}", other),
        }
    }

    #[test]
    fn build_origin_engine_mode_with_no_parent_yields_none() {
        // Engine-initiated work without a parent thread context returns None;
        // callers must construct MessageOrigin::Engine { reason } directly.
        let origin = build_message_origin(
            &headers_with(&[]),
            ActorMode::Engine,
            None,
            None,
            None,
            None,
            None,
        );
        assert!(origin.is_none());
    }

    #[test]
    fn build_origin_engine_mode_with_parent_thread_yields_parent_thread_engine_mode() {
        let parent_id = Uuid::new_v4();
        let origin = build_message_origin(
            &headers_with(&[]),
            ActorMode::Engine,
            None,
            None,
            Some(parent_id),
            None,
            None,
        );
        match origin {
            Some(MessageOrigin::ParentThread { mode, .. }) => assert_eq!(mode, ActorMode::Engine),
            other => panic!("expected ParentThread, got {:?}", other),
        }
    }

    #[test]
    fn build_origin_user_workspace_invalid_uuid_headers_drop_silently() {
        let h = headers_with(&[
            ("x-cognos-workspace", "personal"),
            ("x-cognos-thread-id", "not-a-uuid"),
        ]);
        let origin = build_message_origin(&h, ActorMode::Human, None, None, None, None, None);
        match origin {
            Some(MessageOrigin::Workspace {
                workspace,
                thread_id,
                ..
            }) => {
                assert_eq!(workspace, "personal");
                assert!(
                    thread_id.is_none(),
                    "invalid uuid header must drop, not panic"
                );
            }
            other => panic!("expected Workspace, got {:?}", other),
        }
    }

    #[test]
    fn user_actor_convenience_passes_through_to_build_message_origin() {
        let h = headers_with(&[("user-agent", "curl/8")]);
        let actor = user_actor(&h, Some("dev-1"), Some("Chrome".into()));
        match actor {
            Some(MessageOrigin::Device { device_id, label }) => {
                assert_eq!(device_id, "dev-1");
                assert_eq!(label, "Chrome");
            }
            other => panic!("expected Device, got {:?}", other),
        }
    }

    #[test]
    fn user_actor_falls_back_to_header_device_id_when_no_explicit_id() {
        let h = headers_with(&[("x-cognos-device-id", "dev-7")]);
        let actor = user_actor(&h, None, None);
        match actor {
            Some(MessageOrigin::Device { device_id, label }) => {
                assert_eq!(device_id, "dev-7");
                assert_eq!(label, "Unknown device");
            }
            other => panic!("expected Device from header, got {:?}", other),
        }
    }

    #[test]
    fn user_actor_explicit_id_takes_precedence_over_header() {
        let h = headers_with(&[("x-cognos-device-id", "dev-from-header")]);
        let actor = user_actor(&h, Some("dev-from-arg"), Some("Chrome".into()));
        match actor {
            Some(MessageOrigin::Device { device_id, label }) => {
                assert_eq!(device_id, "dev-from-arg");
                assert_eq!(label, "Chrome");
            }
            other => panic!("expected explicit Device, got {:?}", other),
        }
    }

    #[test]
    fn build_origin_workspace_reads_mode_header() {
        let h = headers_with(&[("x-cognos-workspace", "personal"), ("x-cognos-mode", "agent")]);
        let origin = build_message_origin(&h, ActorMode::Agent, None, None, None, None, None);
        match origin {
            Some(MessageOrigin::Workspace { mode, .. }) => assert_eq!(mode, ActorMode::Agent),
            other => panic!("expected Workspace agent mode, got {:?}", other),
        }
    }
}
