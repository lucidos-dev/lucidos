#![cfg(any(test, feature = "test-support"))]

use super::event_bus::{BusEvent, EventBus};
use super::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent, WakeMode};
use uuid::Uuid;

pub async fn spawn_parent_child(bus: &EventBus, child_channel: EventChannel) -> (Uuid, Uuid) {
    spawn_parent_child_with_mode(bus, child_channel, None).await
}

pub async fn spawn_parent_child_with_mode(
    bus: &EventBus,
    child_channel: EventChannel,
    wake_mode: Option<WakeMode>,
) -> (Uuid, Uuid) {
    let parent_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();

    bus.emit(BusEvent::Thread {
        thread_id: parent_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "do something".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    spawn_child_of(bus, parent_id, child_id, child_channel, wake_mode).await;
    (parent_id, child_id)
}

pub async fn spawn_child_of(
    bus: &EventBus,
    parent_id: Uuid,
    child_id: Uuid,
    child_channel: EventChannel,
    wake_mode: Option<WakeMode>,
) {
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode,
            text: "child task".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(parent_id),
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(child_channel),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

pub async fn emit_cc_session_started(bus: &EventBus, child_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id: child_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            session_id: "test-session".into(),
            branch: "claude-code/test".into(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

pub async fn emit_thread_message(
    bus: &EventBus,
    thread_id: Uuid,
    parent: Option<Uuid>,
    text: &str,
) {
    bus.emit(thread_message(thread_id, parent, text))
        .await
        .unwrap();
}

pub fn thread_message(thread_id: Uuid, parent: Option<Uuid>, text: &str) -> BusEvent {
    BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: text.into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: parent,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            ..EventMeta::NONE
        },
    }
}

pub async fn emit_cc_idle(
    bus: &EventBus,
    thread_id: Uuid,
    has_changes: bool,
    cc_session_id: Option<&str>,
) {
    emit_cc_idled(bus, thread_id, has_changes, cc_session_id).await;
    if has_changes {
        emit_change_proposed(
            bus,
            thread_id,
            &format!("claude-code/test-{thread_id}"),
            false,
        )
        .await;
    }
}

pub async fn emit_cc_idled(
    bus: &EventBus,
    thread_id: Uuid,
    has_changes: bool,
    cc_session_id: Option<&str>,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::CodingAgentIdled {
            has_changes,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: cc_session_id.map(Into::into),
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
            turn_end: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

pub fn test_change_id(thread_id: Uuid) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, thread_id.as_bytes())
}

pub async fn emit_change_proposed(
    bus: &EventBus,
    thread_id: Uuid,
    branch: &str,
    requires_restart: bool,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            content: Default::default(),
            change_id: test_change_id(thread_id).to_string(),
            description: Some("Test change".into()),
            files: vec!["test.rs".into()],
            requires_restart,
            origin: None,
            commit_sha: None,
            branch_name: branch.into(),
            repo_root: "/tmp".into(),
            hardened: false,
            incomplete: false,
            set_aside: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}

pub async fn emit_cc_message_received(
    bus: &EventBus,
    thread_id: Uuid,
    parent: Option<Uuid>,
    text: &str,
) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: text.into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: parent,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}
