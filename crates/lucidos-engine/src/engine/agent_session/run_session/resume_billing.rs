//! Restored usage keeps historical attribution without pinning current billing.

use crate::core::EventStore;
use crate::runtime::agent_runtime::CodexUsageSnapshot;
use serde_json::Value;
use uuid::Uuid;

pub(super) async fn restored_provider(
    store: &EventStore,
    thread_id: Uuid,
    launch_id: Uuid,
    snapshot: &CodexUsageSnapshot,
) -> Option<String> {
    let history = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        store.coding_agent_capture_before_launch(thread_id, launch_id),
    )
    .await;
    match history {
        Ok(Ok(Some((capture, session_id, agent)))) => {
            matching_provider(snapshot, &capture, session_id.as_deref(), agent.as_deref())
                .map(String::from)
        }
        Ok(Ok(None)) => None,
        Ok(Err(error)) => {
            log!(
                "[AgentSession] restored usage history unavailable for {}: {}",
                thread_id,
                error
            );
            None
        }
        Err(_) => {
            log!(
                "[AgentSession] restored usage history timed out for {}",
                thread_id
            );
            None
        }
    }
}

fn matching_provider<'a>(
    snapshot: &CodexUsageSnapshot,
    capture: &'a Value,
    session_id: Option<&str>,
    agent: Option<&str>,
) -> Option<&'a str> {
    if session_id != Some(snapshot.session_id.as_str())
        || agent != Some("codex")
        || capture["producer"] != "codex"
        || !capture["parent_tool_use_id"].is_null()
    {
        return None;
    }
    let usage = &capture["usage"];
    let total = snapshot
        .input_tokens
        .saturating_add(snapshot.cache_read_tokens)
        .saturating_add(snapshot.cache_creation_tokens);
    for (field, expected) in [
        ("input_tokens", total),
        ("output_tokens", snapshot.output_tokens),
        ("cache_read_tokens", snapshot.cache_read_tokens),
        ("cache_creation_tokens", snapshot.cache_creation_tokens),
    ] {
        if usage[field].as_u64() != Some(u64::from(expected)) {
            return None;
        }
    }
    // Only providers the Codex evidence parser can establish are eligible.
    match capture["provider"].as_str()? {
        provider @ ("openai" | "chatgpt-plan") => Some(provider),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn snapshot() -> CodexUsageSnapshot {
        CodexUsageSnapshot {
            session_id: "codex-session".into(),
            turn_id: Some("old-turn".into()),
            input_tokens: 30,
            output_tokens: 5,
            cache_read_tokens: 60,
            cache_creation_tokens: 10,
        }
    }

    fn capture() -> Value {
        json!({"type": "ContextCaptured", "producer": "codex", "model": "test-model",
            "context_window": 200_000, "sections": [], "estimated_total_tokens": 105,
            "provider": "openai", "usage": {"input_tokens": 100, "output_tokens": 5,
                "cache_read_tokens": 60, "cache_creation_tokens": 10}})
    }

    #[test]
    fn codex_resume_requires_the_exact_capture_and_backend_session() {
        let snapshot = snapshot();
        let capture = capture();
        assert_eq!(
            matching_provider(&snapshot, &capture, Some("codex-session"), Some("codex")),
            Some("openai")
        );
        for (session, agent) in [
            (None, Some("codex")),
            (Some("other"), Some("codex")),
            (Some("codex-session"), Some("claude-code")),
        ] {
            assert_eq!(matching_provider(&snapshot, &capture, session, agent), None);
        }
        for field in [
            "input_tokens",
            "output_tokens",
            "cache_read_tokens",
            "cache_creation_tokens",
        ] {
            let mut changed = capture.clone();
            changed["usage"][field] = json!(999);
            assert_eq!(
                matching_provider(&snapshot, &changed, Some("codex-session"), Some("codex")),
                None
            );
        }
        for patch in [
            json!({"provider": "unknown"}),
            json!({"provider": null}),
            json!({"provider": "anthropic"}),
            json!({"parent_tool_use_id": "child"}),
            json!({"producer": "claude_code"}),
        ] {
            let mut changed = capture.clone();
            changed
                .as_object_mut()
                .unwrap()
                .extend(patch.as_object().unwrap().clone());
            assert_eq!(
                matching_provider(&snapshot, &changed, Some("codex-session"), Some("codex")),
                None
            );
        }
    }

    #[tokio::test]
    async fn codex_resume_history_is_bound_to_the_prior_launch_not_current_auth() {
        use crate::engine::event_bus::{BusEvent, EventBus};
        use crate::engine::thread_events::{EventMeta, ThreadEvent};
        use crate::test_support::{setup_test_db, start_cc_session, teardown_test_db};
        let (pool, db_name) = setup_test_db().await;
        let store = EventStore::new(pool.clone());
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        let launch = Uuid::new_v4();
        let emit = |payload: Value| {
            let event: ThreadEvent = serde_json::from_value(payload).unwrap();
            let bus = bus.clone();
            async move {
                bus.emit(BusEvent::Thread {
                    thread_id,
                    event,
                    meta: EventMeta::NONE,
                })
                .await
                .unwrap();
            }
        };
        start_cc_session(&bus, thread_id, "test/codex-resume", None).await;
        emit(
            json!({"type": "CodingAgentSettingsChanged", "coding_agent": "codex",
            "cc_session_id": "codex-session"}),
        )
        .await;
        emit(capture()).await;
        start_cc_session(&bus, thread_id, "test/codex-resume", None).await;
        emit(
            json!({"type": "CodingAgentSettingsChanged", "coding_agent": "codex",
            "cc_session_id": "codex-session"}),
        )
        .await;
        emit(json!({"type": "CodingAgentBillingObserved", "coding_agent": "codex",
            "launch_id": launch, "attribution": {"status": "known", "provider": "chatgpt-plan", "source": "codex-account"}})).await;
        assert_eq!(
            restored_provider(&store, thread_id, launch, &snapshot())
                .await
                .as_deref(),
            Some("openai")
        );
        // A capture in this launch must not replace the historical candidate.
        let mut current = capture();
        current["provider"] = json!("chatgpt-plan");
        emit(current).await;
        assert_eq!(
            restored_provider(&store, thread_id, launch, &snapshot())
                .await
                .as_deref(),
            Some("openai")
        );
        assert!(
            restored_provider(&store, Uuid::new_v4(), launch, &snapshot())
                .await
                .is_none()
        );
        assert!(
            restored_provider(&store, thread_id, Uuid::new_v4(), &snapshot())
                .await
                .is_none()
        );
        // An unbound process must not borrow an older process's session id.
        start_cc_session(&bus, thread_id, "test/codex-resume", None).await;
        emit(capture()).await;
        let unbound = Uuid::new_v4();
        emit(json!({"type": "CodingAgentBillingObserved", "coding_agent": "codex",
            "launch_id": unbound, "attribution": {"status": "known", "provider": "openai", "source": "codex-account"}})).await;
        assert!(restored_provider(&store, thread_id, unbound, &snapshot())
            .await
            .is_none());
        pool.close().await;
        teardown_test_db(&db_name).await;
    }
}
