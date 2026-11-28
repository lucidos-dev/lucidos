use std::time::Duration;

use uuid::Uuid;

use crate::core::event_hooks::{HookArgs, FIRST_MESSAGE_EVENT_TYPE, TIMEOUT_CEILING_SECS};
use crate::engine::event_bus::HookRefusal;
use crate::engine::thread_events::{
    ActorMode, EventChannel, EventMeta, MessageOrigin, ThreadEvent,
};
use crate::engine::thread_queue::ThreadQueueRequest;
use crate::engine::LucidosEngine;

pub(crate) fn tool_result(refusal: HookRefusal) -> String {
    let reason = refusal.to_string();
    match refusal.input {
        Some(input) => serde_json::json!({
            "status": "needs-input",
            "hook_input": input,
        })
        .to_string(),
        None => format!("Error: {reason}"),
    }
}

pub(crate) async fn check(
    engine: &LucidosEngine,
    request: &ThreadQueueRequest,
) -> Result<(), HookRefusal> {
    check_with_budget(engine, request, Duration::from_secs(TIMEOUT_CEILING_SECS)).await
}

pub(crate) async fn check_with_budget(
    engine: &LucidosEngine,
    request: &ThreadQueueRequest,
    budget: Duration,
) -> Result<(), HookRefusal> {
    if matches!(
        engine
            .event_bus
            .hooks_armed_on(FIRST_MESSAGE_EVENT_TYPE)
            .await,
        Ok(false)
    ) {
        return Ok(());
    }
    let (thread_id, args, event, meta) = pending_message(request);
    let result = engine
        .event_bus
        .ask_hooks_for_input(thread_id, &event, &meta, args, budget)
        .await;
    result.map(drop)
}

fn pending_message(request: &ThreadQueueRequest) -> (Uuid, &HookArgs, ThreadEvent, EventMeta) {
    use ThreadQueueRequest::{AgentChat, CodingAgent, SubThread};
    let (text, args, parent, spawning, wake, origin, model, effort) = match request {
        AgentChat {
            message: prompt,
            hook_args,
            parent_thread_id,
            spawning_event_id,
            wake_mode,
            origin,
            model,
            reasoning_effort,
            ..
        }
        | CodingAgent {
            prompt,
            hook_args,
            parent_thread_id,
            spawning_event_id,
            wake_mode,
            origin,
            model,
            reasoning_effort,
            ..
        }
        | SubThread {
            prompt,
            hook_args,
            parent_thread_id,
            spawning_event_id,
            wake_mode,
            origin,
            model,
            reasoning_effort,
            ..
        } => (
            prompt,
            hook_args,
            parent_thread_id,
            spawning_event_id,
            wake_mode,
            origin,
            model,
            reasoning_effort,
        ),
        _ => unreachable!("only a message can ask for first-message arguments"),
    };
    let (device_id, hashes, coding, model, provider, id, mode) = match request {
        AgentChat {
            device_id,
            image_hashes,
            use_coding_agent,
            cc_model,
            provider,
            event_id,
            mode,
            ..
        } => (
            device_id.clone(),
            image_hashes.clone(),
            *use_coding_agent == Some(true),
            if *use_coding_agent == Some(true) {
                cc_model.clone()
            } else {
                model.clone()
            },
            provider.clone(),
            event_id.as_deref().and_then(|id| Uuid::parse_str(id).ok()),
            *mode,
        ),
        CodingAgent {
            device_id,
            image_hashes,
            ..
        } => (
            device_id.clone(),
            image_hashes.clone(),
            true,
            model.clone(),
            None,
            None,
            MessageOrigin::mode_or(origin.as_ref(), ActorMode::Agent),
        ),
        SubThread { provider, .. } => (
            None,
            Vec::new(),
            false,
            model.clone(),
            provider.clone(),
            None,
            MessageOrigin::mode_or(origin.as_ref(), ActorMode::Agent),
        ),
        _ => unreachable!(),
    };
    (
        request.thread_id().expect("a message has a thread id"),
        args,
        ThreadEvent::MessageReceived {
            text: text.clone(),
            user_image_hashes: hashes,
            device_id,
            image_description: None,
            parent_thread_id: *parent,
            spawning_event_id: *spawning,
            wake_mode: *wake,
            mode,
            model,
            reasoning_effort: effort.clone(),
            provider,
            origin: origin.clone(),
            voice_session_id: None,
        },
        EventMeta {
            channel: Some(if coding {
                EventChannel::ClaudeCode
            } else {
                EventChannel::Chat
            }),
            event_id: id,
            ..EventMeta::NONE
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::event_hooks::{HookAsk, HookChoice, HookVerdict};
    use crate::engine::thread_events::ThreadDirection;
    use crate::test_support::{
        arm_hook, boot_mock_engine, hook_refusals, run_on_engine_runtime, setup_test_db,
        teardown_test_db,
    };

    struct InputHook;

    #[async_trait::async_trait]
    impl crate::core::event_hooks::EventHookAsker for InputHook {
        async fn ask(&self, ask: HookAsk<'_>) -> HookVerdict {
            if ask
                .arguments
                .get("ticket")
                .is_some_and(|value| value == "TK-1")
            {
                return HookVerdict::Allowed(Vec::new());
            }
            HookVerdict::NeedsInput {
                argument: "ticket".into(),
                label: "Ticket".into(),
                create_label: None,
                choices_timeout_secs: 10,
                choices: vec![HookChoice {
                    value: "TK-1".into(),
                    label: "Existing ticket".into(),
                }],
            }
        }
    }

    #[test]
    fn an_agent_receives_choices_and_can_retry_before_queueing() {
        async fn verify() {
            let (pool, db) = setup_test_db().await;
            let (engine, _workspace) = boot_mock_engine(&db).await;
            engine
                .event_bus
                .install_event_hook_asker(std::sync::Arc::new(InputHook))
                .await
                .unwrap();
            arm_hook(&engine.event_bus, "gate", "MessageReceived").await;
            let mut request = ThreadQueueRequest::SubThread {
                prompt: "start".into(),
                hook_args: HookArgs::new(),
                child_thread_id: Uuid::new_v4(),
                depth: 0,
                parent_thread_id: None,
                spawning_event_id: None,
                wake_mode: None,
                title: None,
                model: None,
                reasoning_effort: None,
                provider: None,
                pre_emitted_origin: None,
                origin: Some(MessageOrigin::ThreadLink {
                    thread_id: Uuid::new_v4(),
                    title: None,
                    spawning_event_id: None,
                    mode: ActorMode::Agent,
                    direction: ThreadDirection::Parent,
                }),
            };
            let refusal = check(&engine, &request).await.unwrap_err();
            let result: serde_json::Value = serde_json::from_str(&tool_result(refusal)).unwrap();
            assert_eq!(result["status"], "needs-input");
            assert_eq!(result["hook_input"]["argument"], "ticket");
            assert_eq!(result["hook_input"]["choices"][0]["value"], "TK-1");
            assert_eq!(hook_refusals(&pool, "gate").await, 0);
            let ThreadQueueRequest::SubThread { hook_args, .. } = &mut request else {
                unreachable!();
            };
            hook_args.insert("ticket".into(), "TK-1".into());
            check(&engine, &request).await.unwrap();

            engine.pool().close().await;
            pool.close().await;
            teardown_test_db(&db).await;
        }
        run_on_engine_runtime(
            "engine::hook_input::tests::an_agent_receives_choices_and_can_retry_before_queueing",
            verify,
        );
    }
}
