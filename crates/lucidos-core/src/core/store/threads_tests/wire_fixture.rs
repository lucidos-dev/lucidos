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
        awaited_children_count: 2,
        awaited_by_parent: true,
        total_children_count: 4,
        blocking_descendant_count: 5,
        attention_descendant_count: 6,
        archivable_sub_thread_count: 7,
        is_stopped_child: true,
        hold: None,
        read_requested: true,
        reply_seen_devices: vec!["d1".into()],
        apply_in_flight: true,
        live_event_wait_count: 1,
        live_event_waits: sqlx::types::Json(vec![EventWaitSummary {
            background_timing: None,
            wait_id: uuid::Uuid::from_u128(7),
            on: vec![],
            reason: "the build".into(),
            expires_at: at("2026-01-02T04:00:00Z"),
        }]),
        cache_keep_alive_status: None,
        status: "waiting_for_user_answer".into(),
        summary_version: 42,
        coding_agent_change_state: "proposed".into(),
        coding_agent_unproposed_reason: None,
        coding_agent_requires_restart: true,
        coding_agent_is_external_repo: false,
        last_revived_at: Some(at("2026-01-02T03:04:09Z")),
        is_saved: true,
        pin_folder: None,
        is_home: false,
        has_response: true,
        parent_thread_id: Some("00000000-0000-0000-0000-000000000002".into()),
        parent_thread_title: Some("Parent".into()),
        successor_thread_id: Some("00000000-0000-0000-0000-000000000004".into()),
        successor_thread_title: Some("Successor".into()),
        successor_of: Some("00000000-0000-0000-0000-000000000005".into()),
        successor_of_title: None,
        trigger_id: None,
        trigger_name: None,
        cc_repo_id: Some("00000000-0000-0000-0000-000000000003".into()),
        cc_repo_name: Some("example-repo".into()),
        coding_agent_kind: Some("lucidos".into()),
        coding_agent_folder: Some("/home/user/example-repo".into()),
        coding_agent: Some("claude-code".into()),
        coding_agents: vec!["claude-code".into(), "codex".into()],
        state: "composing".into(),
        compose_text: "draft".into(),
        compose_images: serde_json::json!([]),
        compose_mode: Some("claude_code".into()),
        compose_selection: None,
        compose_epoch: 9,
        compose_restorations: serde_json::json!([]),
    }
}

/// A wire fixture with fields populated even when ordinary summary reads omit them.
fn full_thread_summary_fixture(
    liveness: crate::engine::thread_events::liveness::ThreadLiveness,
) -> ThreadSummary {
    let mut summary = EventStore::rows_to_thread_summaries(vec![fixture_row()])
        .unwrap()
        .remove(0);
    summary.home = true;
    summary.pin_folder = Some("w48".into());
    summary.cache_keep_alive_status = Some(CacheKeepAliveStatus::Stopped {
        at: at("2026-01-02T05:00:00Z"),
        beats: 25,
        cost_usd: 0.5,
        context_tokens: 262_000,
        ping_usd: None,
        reread_usd: None,
        beat_every_secs: None,
        odds: None,
        cap_source: None,
    });
    summary.trigger_id = Some("example-trigger".into());
    summary.trigger_name = Some("Example trigger".into());
    summary.compose_selection = Some(serde_json::json!({}));
    summary.pending_sub_thread_change_count = Some(1);
    summary.has_draft = Some(true);
    summary.draft_preview = Some("draft".into());
    summary.draft_length = Some(5);
    summary.link = Some("thread:example/00000000-0000-0000-0000-000000000001".into());
    summary.liveness = Some(liveness);
    summary.hold = Some(crate::core::store::thread_hold::ThreadHoldSummary {
        reason: "lunch".into(),
        root_thread_id: "00000000-0000-0000-0000-000000000001".into(),
        placed_at: at("2026-01-02T05:00:00Z"),
        placed_by: None,
    });
    summary
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
            r#""section":"inbox","active_children_count":1,"awaited_children_count":2,"#,
            r#""awaited_by_parent":true,"#,
            r#""total_children_count":4,"live_event_wait_count":1,"live_event_waits":{waits},"#,
            r#""blocking_descendant_count":5,"attention_descendant_count":6,"archivable_sub_thread_count":7,"is_stopped_child":true,"read_requested":true,"reply_seen_devices":["d1"],"apply_in_flight":true,"#,
            r#""status":"waiting_for_user_answer","summary_version":42,"#,
            r#""coding_agent_change_state":{{"kind":"proposed","requires_restart":true}},"#,
            r#""coding_agent_is_external_repo":false,"#,
            r#""last_revived_at":"2026-01-02T03:04:09Z","#,
            r#""parent_thread_id":"00000000-0000-0000-0000-000000000002","parent_thread_title":"Parent","#,
            r#""successor":{{"thread_id":"00000000-0000-0000-0000-000000000004","title":"Successor"}},"#,
            r#""successor_of":{{"thread_id":"00000000-0000-0000-0000-000000000005","title":null}},"#,
            r#""cc_repo_id":"00000000-0000-0000-0000-000000000003","cc_repo_name":"example-repo","#,
            r#""coding_agent_kind":"lucidos","coding_agent_folder":"/home/user/example-repo","#,
            r#""coding_agent":"claude-code","coding_agents":["claude-code","codex"],"#,
            r#""state":"composing","compose_text":"draft","#,
            r#""compose_images":[],"compose_mode":"claude_code","compose_epoch":9,"compose_restorations":[]}}"#,
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
            r#""awaitedChildrenCount":2,"awaitedByParent":true,"#,
            r#""totalChildrenCount":4,"blockingDescendantCount":5,"attentionDescendantCount":6,"archivableSubThreadCount":7,"#,
            r#""isStoppedChild":true,"readRequested":true,"replySeenDevices":["d1"],"applyInFlight":true,"liveEventWaitCount":1,"liveEventWaits":{waits},"#,
            r#""cacheKeepAliveStatus":null,"#,
            r#""codingAgentChangeState":{{"kind":"proposed","requires_restart":true}},"#,
            r#""codingAgentIsExternalRepo":false,"isSaved":true,"#,
            r#""hasResponse":true,"lastRevivedAt":"2026-01-02T03:04:09Z","#,
            r#""parentThreadId":"00000000-0000-0000-0000-000000000002","parentThreadTitle":"Parent","#,
            r#""successor":{{"thread_id":"00000000-0000-0000-0000-000000000004","title":"Successor"}},"#,
            r#""successorOf":{{"thread_id":"00000000-0000-0000-0000-000000000005","title":null}},"#,
            r#""ccRepoId":"00000000-0000-0000-0000-000000000003","ccRepoName":"example-repo","#,
            r#""codingAgentKind":"lucidos","codingAgentFolder":"/home/user/example-repo","#,
            r#""codingAgent":"claude-code","codingAgents":["claude-code","codex"],"#,
            r#""state":"composing"}}"#,
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

/// Each change state is one tagged object, and each detail rides only on its
/// own kind. These bytes are the wire contract the frontend's union reads.
#[test]
fn every_change_state_serializes_to_its_tagged_object() {
    for (kind, reason, requires_restart, expected) in [
        ("none", None, false, r#"{"kind":"none"}"#),
        (
            "unproposed",
            None,
            false,
            r#"{"kind":"unproposed","reason":null}"#,
        ),
        (
            "unproposed",
            Some("plan_awaiting_approval"),
            false,
            r#"{"kind":"unproposed","reason":"plan_awaiting_approval"}"#,
        ),
        (
            "proposed",
            None,
            false,
            r#"{"kind":"proposed","requires_restart":false}"#,
        ),
    ] {
        let row = ThreadRow {
            coding_agent_change_state: kind.into(),
            coding_agent_unproposed_reason: reason.map(String::from),
            coding_agent_requires_restart: requires_restart,
            ..fixture_row()
        };
        let aggregate = serde_json::to_value(row_to_thread_aggregate(row).unwrap()).unwrap();
        assert_eq!(
            aggregate["codingAgentChangeState"].to_string(),
            expected,
            "{kind} {reason:?}"
        );
    }
}

/// The home thread's summary carries `"home":true` beside `saved`. Every other
/// summary omits the key, which is what keeps the bytes above unchanged.
#[test]
fn only_the_home_threads_summary_carries_the_home_key() {
    let summary = EventStore::rows_to_thread_summaries(vec![ThreadRow {
        is_home: true,
        ..fixture_row()
    }])
    .unwrap()
    .remove(0);
    let json = serde_json::to_string(&summary).unwrap();
    assert!(
        json.contains(r#""saved":true,"home":true,"section""#),
        "{json}"
    );
}

/// The per-event aggregate carries the same key. A client can meet the home
/// thread first through an SSE event. Without the key, the client draws that
/// thread in Current as an ordinary one.
#[test]
fn only_the_home_threads_aggregate_carries_the_home_key() {
    let aggregate = row_to_thread_aggregate(ThreadRow {
        is_home: true,
        ..fixture_row()
    })
    .unwrap();
    let json = serde_json::to_string(&aggregate).unwrap();
    assert!(
        json.contains(r#""isSaved":true,"home":true,"hasResponse""#),
        "{json}"
    );
}

/// A status rides both wires as one tagged object, the shape the frontend's
/// generated `CacheKeepAliveStatus` union reads.
#[test]
fn a_keep_alive_status_serializes_to_its_tagged_object() {
    let row = || ThreadRow {
        cache_keep_alive_status: Some(sqlx::types::Json(CacheKeepAliveStatus::Stopped {
            at: at("2026-01-02T05:00:00Z"),
            beats: 25,
            cost_usd: 0.5,
            context_tokens: 262_000,
            ping_usd: None,
            reread_usd: None,
            beat_every_secs: None,
            odds: None,
            cap_source: None,
        })),
        ..fixture_row()
    };
    let expected = r#""cache_keep_alive_status":{"state":"stopped","at":"2026-01-02T05:00:00Z","beats":25,"cost_usd":0.5,"context_tokens":262000}"#;
    let summary = EventStore::rows_to_thread_summaries(vec![row()])
        .unwrap()
        .remove(0);
    let summary = serde_json::to_string(&summary).unwrap();
    assert!(summary.contains(expected), "{summary}");
    let aggregate = serde_json::to_string(&row_to_thread_aggregate(row()).unwrap()).unwrap();
    let expected = expected.replace("cache_keep_alive_status", "cacheKeepAliveStatus");
    assert!(aggregate.contains(&expected), "{aggregate}");
}

/// The hold rides both wires with the same inner shape, which the app's
/// `ThreadHoldSummary` reads.
#[test]
fn a_hold_serializes_on_both_wires() {
    let row = || ThreadRow {
        hold: Some(sqlx::types::Json(
            crate::core::store::thread_hold::ThreadHoldSummary {
                reason: "lunch".into(),
                root_thread_id: "00000000-0000-0000-0000-000000000002".into(),
                placed_at: at("2026-01-02T05:00:00Z"),
                placed_by: Some(crate::engine::thread_events::MessageOrigin::Device {
                    device_id: "d1".into(),
                }),
            },
        )),
        ..fixture_row()
    };
    let expected = r#""hold":{"reason":"lunch","root_thread_id":"00000000-0000-0000-0000-000000000002","placed_at":"2026-01-02T05:00:00Z","placed_by":{"kind":"device","device_id":"d1"}}"#;
    let summary = EventStore::rows_to_thread_summaries(vec![row()])
        .unwrap()
        .remove(0);
    let summary = serde_json::to_string(&summary).unwrap();
    assert!(summary.contains(expected), "{summary}");
    let aggregate = serde_json::to_string(&row_to_thread_aggregate(row()).unwrap()).unwrap();
    assert!(aggregate.contains(expected), "{aggregate}");
}

/// The folder rides both wires, and an unfiled pin sends
/// no key, so every other thread's wire bytes are unchanged.
#[test]
fn a_pin_folder_serializes_on_both_wires_and_is_omitted_when_unset() {
    let filed = || ThreadRow {
        pin_folder: Some("w48".into()),
        ..fixture_row()
    };
    let expected = r#""pin_folder":"w48""#;
    let summary = EventStore::rows_to_thread_summaries(vec![filed()])
        .unwrap()
        .remove(0);
    let summary = serde_json::to_string(&summary).unwrap();
    assert!(summary.contains(expected), "{summary}");
    let aggregate = serde_json::to_string(&row_to_thread_aggregate(filed()).unwrap()).unwrap();
    assert!(aggregate.contains(r#""pinFolder":"w48""#), "{aggregate}");

    let unfiled = serde_json::to_string(&row_to_thread_aggregate(fixture_row()).unwrap()).unwrap();
    assert!(!unfiled.contains("pinFolder"), "{unfiled}");
}

mod guard_tests {
    use super::*;
    use crate::engine::thread_events::liveness::{LivenessState, ThreadLiveness};
    use std::collections::BTreeSet;

    // Keep this exhaustive even when a new field is skipped by serde.
    macro_rules! classify_summary_fields {
        ($summary:expr, { $($field:ident => $reason:literal,)* }) => {{
            let ThreadSummary { $($field: _,)* } = $summary;
            [$( (stringify!($field), $reason), )*]
        }};
    }

    #[test]
    fn every_thread_summary_field_has_a_liveness_classification() {
        let summary = full_thread_summary_fixture(ThreadLiveness {
            state: LivenessState::Idle,
            status: ThreadStatus::Idle,
            last_activity_age_secs: None,
            session: None,
            handoff: None,
            live_event_waits: vec![],
            awaited_children: vec![],
            expected_wake: None,
            hold: None,
            as_of: chrono::DateTime::UNIX_EPOCH,
        });
        let fields = classify_summary_fields!(summary, {
            thread_id => "Identity",
            title => "Display title",
            channel => "Origin",
            initiator => "Origin",
            created_at => "Creation history",
            last_activity => "liveness.last_activity_age_secs",
            last_user_action => "User action history",
            last_agent_action => "Agent action history",
            message_count => "Conversation size",
            saved => "Drawer organization",
            pin_folder => "Drawer organization",
            home => "Workspace role",
            section => "Drawer organization",
            active_children_count => "Drawer rollup, includes children this thread does not await",
            awaited_children_count => "liveness.awaited_children",
            awaited_by_parent => "liveness.awaited_children on the parent",
            total_children_count => "Family size",
            live_event_wait_count => "liveness.live_event_waits",
            live_event_waits => "liveness.live_event_waits",
            cache_keep_alive_status => "Cache maintenance, independent of thread liveness",
            blocking_descendant_count => "Archive eligibility rollup for descendants",
            attention_descendant_count => "Drawer attention rollup for descendants",
            archivable_sub_thread_count => "Overflow menu count for archiving completed sub-threads",
            is_stopped_child => "Unsettled card owed to the parent for a past Stop",
            hold => "The hold the drawer section and composer strip draw; liveness carries its own",
            read_requested => "Reply review request",
            reply_seen_devices => "Per-device read state",
            apply_in_flight => "Change application",
            status => "liveness.status",
            summary_version => "Projection revision",
            coding_agent_change_state => "Change artifact",
            coding_agent_is_external_repo => "Repository configuration",
            last_revived_at => "Turn start history",
            parent_thread_id => "Family link",
            parent_thread_title => "Family link label",
            successor => "Legacy thread link",
            successor_of => "Legacy thread link",
            trigger_id => "Origin",
            trigger_name => "Origin label",
            cc_repo_id => "Repository configuration",
            cc_repo_name => "Repository label",
            coding_agent_kind => "Coding-agent configuration",
            coding_agent_folder => "Coding-agent configuration",
            coding_agent => "Coding-agent configuration",
            coding_agents => "Session backend history",
            state => "Composer lifecycle",
            compose_text => "Draft content",
            compose_images => "Draft attachments",
            compose_mode => "Draft settings",
            compose_selection => "Draft settings",
            compose_epoch => "Draft revision",
            compose_restorations => "Draft restoration history",
            pending_sub_thread_change_count => "Descendant change artifacts",
            has_draft => "Draft metadata",
            draft_preview => "Draft content",
            draft_length => "Draft metadata",
            link => "Navigation link",
            liveness => "Liveness itself",
        });
        let json = serde_json::to_value(summary).unwrap();
        let actual: BTreeSet<_> = json
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let classified: BTreeSet<_> = fields.iter().map(|(field, _)| *field).collect();
        assert_eq!(actual, classified);
    }
}
