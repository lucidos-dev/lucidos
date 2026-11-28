use super::*;
pub(super) use crate::engine::event_bus::{BusEvent, EventBus};
pub(super) use crate::engine::thread_events::{EventChannel, EventMeta, ThreadEvent};
pub(super) use crate::test_support::{setup_test_db, teardown_test_db};

pub(super) fn aggregate_proposed(change_id: Uuid, branch: &str, repo_root: &str) -> ThreadEvent {
    ThreadEvent::ChangeProposed {
        content: Default::default(),
        change_id: change_id.to_string(),
        description: Some("aggregate description".to_string()),
        files: vec!["a.rs".to_string(), "b.rs".to_string()],
        requires_restart: false,
        origin: None,
        commit_sha: None,
        branch_name: branch.to_string(),
        repo_root: repo_root.to_string(),
        hardened: true,
        incomplete: false,
        set_aside: false,
        path: String::new(),
        diff: String::new(),
    }
}

pub(super) fn applied_event(
    change_id: Uuid,
    commits: &[&str],
    requires_restart: bool,
) -> ThreadEvent {
    ThreadEvent::ChangeApplied {
        content: Default::default(),
        change_id: change_id.to_string(),
        requires_restart,
        client_update: false,
        commits: commits.iter().map(|s| s.to_string()).collect(),
        thread_title: None,
        actor: None,
        pre_merge_sha: None,
        post_merge_sha: None,
        landing_merge_sha: None,
        path: String::new(),
    }
}

pub(super) async fn start_cc_thread(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::SessionStarted {
            sandbox: None,
            coding_agent: crate::engine::thread_events::CodingAgent::ClaudeCode,
            session_id: format!("cc-{thread_id}"),
            branch: String::new(),
            repo_id: None,
            coding_agent_kind: Default::default(),
            coding_agent_folder: String::new(),
            app_id: None,
            integrated: false,
            auto_compact_window: None,
            auto_compact_arm: None,
            continuity: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::ClaudeCode),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

pub(super) async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
}
