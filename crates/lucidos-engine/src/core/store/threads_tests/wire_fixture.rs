//! Pins the wire bytes of `ThreadSummary` and `ThreadAggregate`.
//!
//! Each fixture runs a fixed `ThreadRow` through the production mapping and
//! compares the JSON to a literal. A change to a field's Rust type must leave
//! these bytes alone: the frontend reads them without a translation layer.

use super::*;
use crate::engine::thread_state::ThreadState;

fn at(s: &str) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::parse_from_rfc3339(s).unwrap().to_utc()
}

fn fixture_row() -> ThreadRow {
    ThreadRow {
        thread_id: "00000000-0000-0000-0000-000000000001".into(),
        title: Some("Fixture thread".into()),
        first_message: None,
        source: "claude_code".into(),
        initiator: "user".into(),
        created_at: at("2026-01-02T03:04:05Z"),
        last_activity: at("2026-01-02T03:04:06Z"),
        last_user_action: at("2026-01-02T03:04:07Z"),
        last_agent_action: at("2026-01-02T03:04:08Z"),
        message_count: 3,
        section: "inbox".into(),
        active_children_count: 1,
        waiting_children_count: 2,
        total_children_count: 4,
        blocking_descendant_count: 5,
        attention_descendant_count: 6,
        is_stopped_child: true,
        live_event_wait_count: 1,
        live_event_waits: sqlx::types::Json(vec![EventWaitSummary {
            wait_id: uuid::Uuid::from_u128(7),
            on: vec![],
            reason: "the build".into(),
            expires_at: at("2026-01-02T04:00:00Z"),
        }]),
        status: "waiting_for_user_answer".into(),
        summary_version: 42,
        coding_agent_has_diff: true,
        coding_agent_proposed: true,
        coding_agent_requires_restart: true,
        coding_agent_incomplete: true,
        coding_agent_is_external_repo: false,
        last_revived_at: Some(at("2026-01-02T03:04:09Z")),
        is_saved: true,
        has_response: true,
        parent_thread_id: Some("00000000-0000-0000-0000-000000000002".into()),
        parent_thread_title: Some("Parent".into()),
        trigger_id: None,
        trigger_name: None,
        cc_repo_id: Some("00000000-0000-0000-0000-000000000003".into()),
        cc_repo_name: Some("example-repo".into()),
        coding_agent_kind: Some("lucidos".into()),
        coding_agent_folder: Some("/home/user/example-repo".into()),
        coding_agent: Some("claude-code".into()),
        state: "composing".into(),
        compose_text: "draft".into(),
        compose_images: serde_json::json!([]),
        compose_mode: Some("claude_code".into()),
        compose_selection: None,
        compose_epoch: 9,
    }
}

const WAIT_JSON: &str = r#"[{"wait_id":"00000000-0000-0000-0000-000000000007","on":[],"reason":"the build","expires_at":"2026-01-02T04:00:00Z"}]"#;

#[test]
fn thread_summary_serializes_to_the_pinned_bytes() {
    let summary = EventStore::rows_to_thread_summaries(vec![fixture_row()])
        .unwrap()
        .remove(0);
    let expected = format!(
        concat!(
            r#"{{"thread_id":"00000000-0000-0000-0000-000000000001","title":"Fixture thread","#,
            r#""channel":"claude_code","initiator":"user","created_at":"2026-01-02T03:04:05Z","#,
            r#""last_activity":"2026-01-02T03:04:06Z","last_user_action":"2026-01-02T03:04:07Z","#,
            r#""last_agent_action":"2026-01-02T03:04:08Z","message_count":3,"saved":true,"#,
            r#""section":"inbox","active_children_count":1,"waiting_children_count":2,"#,
            r#""total_children_count":4,"live_event_wait_count":1,"live_event_waits":{waits},"#,
            r#""blocking_descendant_count":5,"attention_descendant_count":6,"is_stopped_child":true,"#,
            r#""status":"waiting_for_user_answer","summary_version":42,"coding_agent_has_diff":true,"#,
            r#""coding_agent_proposed":true,"coding_agent_requires_restart":true,"coding_agent_incomplete":true,"#,
            r#""coding_agent_is_external_repo":false,"#,
            r#""last_revived_at":"2026-01-02T03:04:09Z","#,
            r#""parent_thread_id":"00000000-0000-0000-0000-000000000002","parent_thread_title":"Parent","#,
            r#""cc_repo_id":"00000000-0000-0000-0000-000000000003","cc_repo_name":"example-repo","#,
            r#""coding_agent_kind":"lucidos","coding_agent_folder":"/home/user/example-repo","#,
            r#""coding_agent":"claude-code","state":"composing","compose_text":"draft","#,
            r#""compose_images":[],"compose_mode":"claude_code","compose_epoch":9}}"#,
        ),
        waits = WAIT_JSON,
    );
    assert_eq!(serde_json::to_string(&summary).unwrap(), expected);
}

#[test]
fn thread_aggregate_serializes_to_the_pinned_bytes() {
    let aggregate = row_to_thread_aggregate(fixture_row()).unwrap();
    let expected = format!(
        concat!(
            r#"{{"threadId":"00000000-0000-0000-0000-000000000001","title":"Fixture thread","#,
            r#""channel":"claude_code","initiator":"user","createdAt":"2026-01-02T03:04:05Z","#,
            r#""lastActivity":"2026-01-02T03:04:06Z","lastUserAction":"2026-01-02T03:04:07Z","#,
            r#""lastAgentAction":"2026-01-02T03:04:08Z","messageCount":3,"section":"inbox","#,
            r#""status":"waiting_for_user_answer","summaryVersion":42,"activeChildrenCount":1,"#,
            r#""waitingChildrenCount":2,"#,
            r#""totalChildrenCount":4,"blockingDescendantCount":5,"attentionDescendantCount":6,"#,
            r#""isStoppedChild":true,"liveEventWaitCount":1,"liveEventWaits":{waits},"#,
            r#""codingAgentHasDiff":true,"codingAgentProposed":true,"codingAgentRequiresRestart":true,"#,
            r#""codingAgentIncomplete":true,"#,
            r#""codingAgentIsExternalRepo":false,"isSaved":true,"#,
            r#""hasResponse":true,"lastRevivedAt":"2026-01-02T03:04:09Z","#,
            r#""parentThreadId":"00000000-0000-0000-0000-000000000002","parentThreadTitle":"Parent","#,
            r#""ccRepoId":"00000000-0000-0000-0000-000000000003","ccRepoName":"example-repo","#,
            r#""codingAgentKind":"lucidos","codingAgentFolder":"/home/user/example-repo","#,
            r#""codingAgent":"claude-code","state":"composing"}}"#,
        ),
        waits = WAIT_JSON,
    );
    assert_eq!(serde_json::to_string(&aggregate).unwrap(), expected);
}

/// Every value the two columns hold serializes back to the column's own text.
#[test]
fn every_status_and_state_value_keeps_its_wire_text() {
    for status in ThreadStatus::ALL {
        let mut row = fixture_row();
        row.status = status.as_str().into();
        let json = serde_json::to_value(row_to_thread_aggregate(row).unwrap()).unwrap();
        assert_eq!(json["status"], status.as_str());
    }
    for state in [
        ThreadState::Composing,
        ThreadState::Active,
        ThreadState::Discarded,
    ] {
        let mut row = fixture_row();
        row.state = state.as_str().into();
        let json = serde_json::to_value(row_to_thread_aggregate(row).unwrap()).unwrap();
        assert_eq!(json["state"], state.as_str());
    }
}
