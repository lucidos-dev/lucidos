//! Resolve the actor (`MessageOrigin`) for an inbound HTTP request.
//!
//! Every mutating endpoint that emits an event (apply/discard/revert a change,
//! create/edit a thread, run a trigger, etc.) MUST stamp the produced event with
//! an actor so the timeline can show *who* initiated each system action.
//!
//! Cross-workspace caller info travels in the request **body** (the `caller_*`
//! fields on `ChatRequest`), not in headers. Handlers parse and validate those
//! fields up front, then bundle them into a `CallerOrigin` and pass it here.
//!
//! Resolution (mode = `Human`):
//! 1. `caller` set → `Workspace` (other Lucidos workspace; carries optional
//!    thread/event id from the request body)
//! 2. `device_id` present     → `Device` (label looked up by caller from the
//!    `devices` table)
//! 3. else                    → `Api` (carries `User-Agent`)
//!
//! Mode = `Agent` or `Engine`:
//! - With `caller` set → `Workspace` (cross-workspace agent/engine call)
//! - Otherwise → `ParentThread` when `parent_thread_id` is set, else `None`
//!   (callers must construct `MessageOrigin::Engine { reason }` directly).
//!
//! Caller fields are user-controllable — treat as a display hint only,
//! never for authorization.

use crate::engine::thread_events::{ActorMode, MessageOrigin, ThreadDirection};
use sqlx::PgPool;
use uuid::Uuid;

/// Header carrying the originating browser/device id on mutating endpoints
/// that don't accept a request body (apply/discard/revert, pin/unpin, etc.).
/// Frontend's `json()` helper sets this from `getDeviceId()`.
pub const HEADER_DEVICE_ID: &str = "x-lucidos-device-id";

/// Cross-workspace origin info, extracted from request body `caller_*` fields.
/// `Some(_)` means this is a cross-workspace POST; `None` means same-workspace.
/// Mutual exclusion vs `parent_thread_id` is enforced upstream by
/// `validate_mode_and_spawn`, so this doesn't need to defend against both.
#[derive(Debug, Clone)]
pub struct CallerOrigin {
    pub workspace: String,
    pub thread_id: Option<Uuid>,
    pub event_id: Option<Uuid>,
    /// Upstream actor mode (Human/Agent/Engine of the calling workspace).
    pub mode: ActorMode,
}

pub fn build_message_origin(
    headers: &axum::http::HeaderMap,
    mode: ActorMode,
    device_id: Option<&str>,
    device_label: Option<String>,
    parent_thread_id: Option<Uuid>,
    parent_thread_title: Option<String>,
    spawning_event_id: Option<Uuid>,
    caller: Option<CallerOrigin>,
) -> Option<MessageOrigin> {
    let user_agent = header_str(headers, "user-agent");
    if let Some(c) = caller {
        return Some(MessageOrigin::Workspace {
            workspace: c.workspace,
            thread_id: c.thread_id,
            event_id: c.event_id,
            user_agent,
            mode: c.mode,
        });
    }
    match mode {
        ActorMode::Human => {
            if let Some(id) = device_id {
                Some(MessageOrigin::Device {
                    device_id: id.to_string(),
                    label: device_label
                        .unwrap_or_else(|| crate::core::devices::resolve_device_name(None, id)),
                })
            } else {
                Some(MessageOrigin::Api {
                    user_agent,
                    mode: ActorMode::Human,
                })
            }
        }
        ActorMode::Agent | ActorMode::Engine => {
            // No caller → must be same-workspace parent-thread spawn.
            parent_thread_id.map(|id| MessageOrigin::ThreadLink {
                thread_id: id,
                title: parent_thread_title,
                spawning_event_id,
                mode,
                direction: ThreadDirection::Parent,
            })
        }
    }
}

/// Convenience: build an actor for a `User`-initiated mutating endpoint that
/// has no parent-thread context (apply/discard/revert, settings writes, etc.).
///
/// `device_id` and `device_label` are optional explicit overrides — when both
/// are `None`, the header `x-lucidos-device-id` (if present) supplies the id
/// and the resulting `Device` actor uses the `device-<short>` fallback label.
/// Callers that have access to the `devices` table should prefer
/// `user_actor_resolved` so the popover shows the stored device name.
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
        None,
    )
}

/// Like `user_actor` but enriches the device origin with the stored device
/// label from the `devices` table, so the popover renders "Chrome on Mac" (or
/// the `device-<short>` fallback) instead of an opaque id. Use this — not
/// `user_actor` directly — at every mutating HTTP handler.
///
/// `device_id_override` lets handlers that receive the device id in the
/// request body (e.g. per-device preferences) supply it explicitly; otherwise
/// the `x-lucidos-device-id` header is used.
pub async fn user_actor_resolved(
    headers: &axum::http::HeaderMap,
    pool: &PgPool,
    device_id_override: Option<&str>,
) -> Option<MessageOrigin> {
    let header_did = header_str(headers, HEADER_DEVICE_ID);
    let effective_did = device_id_override.or(header_did.as_deref());
    let device_label = match effective_did {
        Some(d) => crate::core::DeviceStore::display_name(pool, d).await,
        None => None,
    };
    user_actor(headers, device_id_override, device_label)
}

fn header_str(headers: &axum::http::HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
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

    fn caller(
        name: &str,
        thread_id: Option<Uuid>,
        event_id: Option<Uuid>,
        mode: ActorMode,
    ) -> Option<CallerOrigin> {
        Some(CallerOrigin {
            workspace: name.into(),
            thread_id,
            event_id,
            mode,
        })
    }

    #[test]
    fn build_origin_user_with_caller_yields_workspace() {
        let src_thread = Uuid::new_v4();
        let src_event = Uuid::new_v4();
        let h = headers_with(&[("user-agent", "lucidos-engine/0.1")]);
        let origin = build_message_origin(
            &h,
            ActorMode::Human,
            Some("dev-1"),
            Some("dev label".into()),
            None,
            None,
            None,
            caller(
                "personal",
                Some(src_thread),
                Some(src_event),
                ActorMode::Human,
            ),
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
                assert_eq!(user_agent.as_deref(), Some("lucidos-engine/0.1"));
            }
            other => panic!("expected Workspace, got {:?}", other),
        }
    }

    #[test]
    fn build_origin_user_caller_takes_precedence_over_device() {
        let h = headers_with(&[]);
        let origin = build_message_origin(
            &h,
            ActorMode::Human,
            Some("dev-1"),
            Some("Chrome".into()),
            None,
            None,
            None,
            caller("work", None, None, ActorMode::Human),
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
    fn build_origin_user_with_device_id_no_label_falls_back_to_short_id() {
        let h = headers_with(&[]);
        let origin = build_message_origin(
            &h,
            ActorMode::Human,
            Some("abcdef0123456789"),
            None,
            None,
            None,
            None,
            None,
        );
        match origin {
            Some(MessageOrigin::Device { label, .. }) => assert_eq!(label, "device-abcdef01"),
            other => panic!("expected Device, got {:?}", other),
        }
    }

    #[test]
    fn build_origin_user_short_label_handles_short_id() {
        // Defensive: device_id shorter than 8 chars must not panic.
        let h = headers_with(&[]);
        let origin = build_message_origin(
            &h,
            ActorMode::Human,
            Some("ab"),
            None,
            None,
            None,
            None,
            None,
        );
        match origin {
            Some(MessageOrigin::Device { label, .. }) => assert_eq!(label, "device-ab"),
            other => panic!("expected Device, got {:?}", other),
        }
    }

    #[test]
    fn build_origin_user_no_device_no_caller_yields_api() {
        let h = headers_with(&[("user-agent", "curl/8")]);
        let origin = build_message_origin(
            &h,
            ActorMode::Human,
            None,
            None,
            None,
            None,
            None,
            None,
        );
        match origin {
            Some(MessageOrigin::Api { user_agent, mode }) => {
                assert_eq!(user_agent.as_deref(), Some("curl/8"));
                assert_eq!(mode, ActorMode::Human);
            }
            other => panic!("expected Api, got {:?}", other),
        }
    }

    #[test]
    fn build_origin_human_mode_with_caller_yields_workspace_human_mode() {
        let h = headers_with(&[]);
        let origin = build_message_origin(
            &h,
            ActorMode::Human,
            None,
            None,
            None,
            None,
            None,
            caller("personal", None, None, ActorMode::Human),
        );
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
    fn build_origin_agent_mode_with_caller_yields_workspace_agent_mode() {
        // CallerOrigin carries the upstream mode — the calling workspace's
        // actor (Agent here) flows straight into the Workspace branch.
        let h = headers_with(&[]);
        let origin = build_message_origin(
            &h,
            ActorMode::Agent,
            None,
            None,
            None,
            None,
            None,
            caller("personal", None, None, ActorMode::Agent),
        );
        match origin {
            Some(MessageOrigin::Workspace { mode, .. }) => assert_eq!(mode, ActorMode::Agent),
            other => panic!("expected Workspace agent mode, got {:?}", other),
        }
    }

    #[test]
    fn build_origin_engine_mode_with_caller_yields_workspace_engine_mode() {
        let h = headers_with(&[]);
        let origin = build_message_origin(
            &h,
            ActorMode::Engine,
            None,
            None,
            None,
            None,
            None,
            caller("personal", None, None, ActorMode::Engine),
        );
        match origin {
            Some(MessageOrigin::Workspace { mode, .. }) => assert_eq!(mode, ActorMode::Engine),
            other => panic!("expected Workspace engine mode, got {:?}", other),
        }
    }

    #[test]
    fn build_origin_agent_mode_with_parent_thread_yields_thread_link_parent_agent_mode() {
        let parent_id = Uuid::new_v4();
        let origin = build_message_origin(
            &headers_with(&[]),
            ActorMode::Agent,
            None,
            None,
            Some(parent_id),
            Some("parent".into()),
            None,
            None,
        );
        match origin {
            Some(MessageOrigin::ThreadLink {
                thread_id,
                mode,
                direction,
                ..
            }) => {
                assert_eq!(thread_id, parent_id);
                assert_eq!(mode, ActorMode::Agent);
                assert_eq!(direction, ThreadDirection::Parent);
            }
            other => panic!("expected ThreadLink, got {:?}", other),
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
            None,
        );
        assert!(origin.is_none());
    }

    #[test]
    fn build_origin_engine_mode_with_parent_thread_yields_thread_link_engine_mode() {
        let parent_id = Uuid::new_v4();
        let origin = build_message_origin(
            &headers_with(&[]),
            ActorMode::Engine,
            None,
            None,
            Some(parent_id),
            None,
            None,
            None,
        );
        match origin {
            Some(MessageOrigin::ThreadLink {
                mode, direction, ..
            }) => {
                assert_eq!(mode, ActorMode::Engine);
                assert_eq!(direction, ThreadDirection::Parent);
            }
            other => panic!("expected ThreadLink, got {:?}", other),
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
        let h = headers_with(&[("x-lucidos-device-id", "abcdef0123456789")]);
        let actor = user_actor(&h, None, None);
        match actor {
            Some(MessageOrigin::Device { device_id, label }) => {
                assert_eq!(device_id, "abcdef0123456789");
                assert_eq!(label, "device-abcdef01");
            }
            other => panic!("expected Device from header, got {:?}", other),
        }
    }

    #[test]
    fn user_actor_explicit_id_takes_precedence_over_header() {
        let h = headers_with(&[("x-lucidos-device-id", "dev-from-header")]);
        let actor = user_actor(&h, Some("dev-from-arg"), Some("Chrome".into()));
        match actor {
            Some(MessageOrigin::Device { device_id, label }) => {
                assert_eq!(device_id, "dev-from-arg");
                assert_eq!(label, "Chrome");
            }
            other => panic!("expected explicit Device, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn user_actor_resolved_enriches_device_label_from_db() {
        // Regression: every mutating handler used to call `user_actor(.., None, None)`
        // and never looked up the label, so events stamped Device { label: <fallback> }
        // — visible as the bare `device-<short>` placeholder in the actor popover.
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        crate::core::DeviceStore::register(&pool, "test-device-1", Some("Mozilla/5.0"))
            .await
            .unwrap();
        crate::core::DeviceStore::rename(&pool, "test-device-1", Some("My MacBook"))
            .await
            .unwrap();

        let h = headers_with(&[("x-lucidos-device-id", "test-device-1")]);
        let actor = user_actor_resolved(&h, &pool, None).await;
        match actor {
            Some(MessageOrigin::Device { device_id, label }) => {
                assert_eq!(device_id, "test-device-1");
                assert_eq!(
                    label, "My MacBook",
                    "label must come from devices table, not the device-<short> fallback"
                );
            }
            other => panic!("expected Device with db-looked-up label, got {:?}", other),
        }

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn user_actor_resolved_uses_explicit_device_id_override() {
        // The settings endpoint takes device_id from the request body, not the header.
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        crate::core::DeviceStore::register(&pool, "from-body", None)
            .await
            .unwrap();
        crate::core::DeviceStore::rename(&pool, "from-body", Some("Body Device"))
            .await
            .unwrap();

        let h = headers_with(&[("x-lucidos-device-id", "from-header")]);
        let actor = user_actor_resolved(&h, &pool, Some("from-body")).await;
        match actor {
            Some(MessageOrigin::Device { device_id, label }) => {
                assert_eq!(device_id, "from-body", "explicit override beats header");
                assert_eq!(label, "Body Device");
            }
            other => panic!("expected Device using override, got {:?}", other),
        }

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn user_actor_resolved_no_caller_no_device_yields_api() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let h = headers_with(&[("user-agent", "curl/8")]);
        let actor = user_actor_resolved(&h, &pool, None).await;
        match actor {
            Some(MessageOrigin::Api { user_agent, mode }) => {
                assert_eq!(user_agent.as_deref(), Some("curl/8"));
                assert_eq!(mode, ActorMode::Human);
            }
            other => panic!("expected Api, got {:?}", other),
        }
        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn user_actor_resolved_unknown_device_id_falls_back_to_short_label() {
        // Device id in header but row missing in DB — falls back via
        // build_message_origin's `device-<short>` derivation, never panics.
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let h = headers_with(&[("x-lucidos-device-id", "no-such-device")]);
        let actor = user_actor_resolved(&h, &pool, None).await;
        match actor {
            Some(MessageOrigin::Device { device_id, label }) => {
                assert_eq!(device_id, "no-such-device");
                assert_eq!(label, "device-no-such-");
            }
            other => panic!("expected Device with fallback label, got {:?}", other),
        }
        crate::test_support::teardown_test_db(&db_name).await;
    }
}
