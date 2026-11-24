use crate::workspace_operation::{
    OperationKind, ReadKind, ReadRequest, WorkspaceOperation, WorkspaceOperationClaim,
};
use crate::{Claim, StampKey};
use std::collections::BTreeMap;

#[test]
fn envelopes_bind_the_nonce_routing_identity_and_complete_operation() {
    use crate::workspace_operation::{OperationEnvelope, RoutingRequest};
    let envelope = OperationEnvelope {
        request: RoutingRequest {
            via: None,
            to: Some("target".into()),
            operation: WorkspaceOperation::Read {
                request: read(ReadKind::ThreadMessages, Some("thread-1")),
            },
        },
        caller_gateway: "source-gateway".into(),
        caller_workspace: "source".into(),
        caller_thread_id: "caller-thread".into(),
        verdict: String::new(),
        nonce: "request-1".into(),
    };
    let key = StampKey::generate().unwrap();
    let digest = envelope.digest().unwrap();
    let stamp = key.sign(Claim::WorkspaceOperation(envelope.claim(&digest)), 100);
    assert!(key.verify(
        &stamp,
        Claim::WorkspaceOperation(envelope.claim(&digest)),
        100
    ));
    let original = serde_json::to_value(&envelope).unwrap();
    for (pointer, replacement) in [
        ("/nonce", "request-2"),
        ("/caller_gateway", "forged"),
        ("/caller_workspace", "forged"),
        ("/caller_thread_id", "forged"),
        ("/verdict", "allowed"),
        ("/request/to", "other-target"),
        ("/request/via", "other-gateway"),
        ("/request/via", "local"),
        ("/request/operation/request/thread_id", "other-thread"),
    ] {
        let mut changed = original.clone();
        *changed.pointer_mut(pointer).unwrap() = replacement.into();
        let changed: OperationEnvelope = serde_json::from_value(changed).unwrap();
        let digest = changed.digest().unwrap();
        assert!(
            !key.verify(
                &stamp,
                Claim::WorkspaceOperation(changed.claim(&digest)),
                100
            ),
            "{pointer}"
        );
    }
}

fn read(kind: ReadKind, thread: Option<&str>) -> ReadRequest {
    ReadRequest {
        kind,
        thread_id: thread.map(str::to_owned),
        query: BTreeMap::new(),
    }
}

#[test]
fn read_routes_are_closed_and_cannot_escape_the_thread_path() {
    assert_eq!(
        read(ReadKind::ThreadMessages, Some("thread-1"))
            .path()
            .unwrap(),
        "/api/v1/threads/thread-1/messages"
    );
    for thread in [
        None,
        Some("../secrets"),
        Some("a%2fb"),
        Some("a?b"),
        Some(""),
    ] {
        assert!(read(ReadKind::ThreadMessages, thread).path().is_err());
    }
    assert!(read(ReadKind::Threads, Some("thread-1")).path().is_err());
    let mut request = read(ReadKind::ThreadMessages, Some("thread-1"));
    request.query.insert("path".into(), "/secrets".into());
    assert!(request.path().is_err());
}

#[test]
fn wire_operations_reject_unknown_fields_kinds_and_non_object_spawn_bodies() {
    for json in [
        r#"{"kind":"delete-thread","body":{}}"#,
        r#"{"kind":"spawn","body":[],"method":"GET"}"#,
        r#"{"kind":"spawn","body":[]}"#,
        r#"{"kind":"read","request":{"kind":"thread-messages","thread_id":"thread-1","path":"/secrets"}}"#,
    ] {
        assert!(serde_json::from_str::<WorkspaceOperation>(json).is_err());
    }
}

#[test]
fn event_filters_are_preserved_but_thread_cannot_be_overridden_in_query() {
    let mut request = read(ReadKind::Events, Some("thread-1"));
    request
        .query
        .insert("before_event_id".into(), "evt-before".into());
    request.query.insert("limit".into(), "10".into());
    let pairs = request.query_pairs().unwrap();
    assert!(pairs.contains(&("thread_id".into(), "thread-1".into())));
    assert!(pairs.contains(&("before_event_id".into(), "evt-before".into())));
    request.query.insert("thread_id".into(), "thread-2".into());
    assert!(request.query_pairs().is_err());
    let mut ladder = read(ReadKind::IntelligenceLadder, Some("thread-1"));
    ladder.query.insert("surface".into(), "codex".into());
    assert!(ladder.path().is_err());
}

#[test]
fn argument_digest_is_canonical_and_covers_nested_values_array_order_and_kind() {
    let operation = |json: &str| serde_json::from_str::<WorkspaceOperation>(json).unwrap();
    let a = operation(
        r#"{"kind":"spawn","body":{"message":"hello","nested":{"a":1,"b":2},"items":[1,2]}}"#,
    );
    let b = operation(
        r#"{"body":{"items":[1,2],"nested":{"b":2,"a":1},"message":"hello"},"kind":"spawn"}"#,
    );
    assert_eq!(a.digest().unwrap(), b.digest().unwrap());
    for other in [
        r#"{"kind":"spawn","body":{"message":"hello","nested":{"a":1,"b":3},"items":[1,2]}}"#,
        r#"{"kind":"spawn","body":{"message":"hello","nested":{"a":1,"b":2},"items":[2,1]}}"#,
        r#"{"kind":"read","request":{"kind":"threads"}}"#,
    ] {
        assert_ne!(a.digest().unwrap(), operation(other).digest().unwrap());
    }
}

#[test]
fn operation_stamps_bind_every_routing_provenance_argument_and_verdict_field() {
    let key = StampKey::generate().unwrap();
    let operation = WorkspaceOperationClaim {
        kind: OperationKind::Read,
        routing_gateway: "gateway-a",
        target_workspace: "notes@cloud",
        caller_gateway: "gateway-b",
        caller_workspace: "source",
        caller_thread_id: "thread-1",
        arguments_digest: "digest-1",
        verdict: "",
    };
    let stamp = key.sign(Claim::WorkspaceOperation(operation), 1_000);
    assert!(key.verify(&stamp, Claim::WorkspaceOperation(operation), 1_000));
    for other in [
        WorkspaceOperationClaim {
            kind: OperationKind::Spawn,
            ..operation
        },
        WorkspaceOperationClaim {
            routing_gateway: "gateway-c",
            ..operation
        },
        WorkspaceOperationClaim {
            target_workspace: "other",
            ..operation
        },
        WorkspaceOperationClaim {
            caller_gateway: "gateway-c",
            ..operation
        },
        WorkspaceOperationClaim {
            caller_workspace: "other",
            ..operation
        },
        WorkspaceOperationClaim {
            caller_thread_id: "thread-2",
            ..operation
        },
        WorkspaceOperationClaim {
            arguments_digest: "digest-2",
            ..operation
        },
        WorkspaceOperationClaim {
            verdict: "allowed",
            ..operation
        },
    ] {
        assert!(!key.verify(&stamp, Claim::WorkspaceOperation(other), 1_000));
    }
    let device = Claim::Device {
        workspace_id: "notes@cloud",
        device_id: "source",
    };
    let spawn = Claim::WorkspaceCall {
        target_workspace: "notes@cloud",
        caller_workspace: "source",
        caller_thread_id: "thread-1",
        verdict: "allowed",
    };
    for other in [device, spawn] {
        assert!(!key.verify(&stamp, other, 1_000));
        assert!(!key.verify(
            &key.sign(other, 1_000),
            Claim::WorkspaceOperation(operation),
            1_000
        ));
    }
    assert!(!key.verify(&stamp, Claim::WorkspaceOperation(operation), 1_061));
    assert!(!StampKey::generate().unwrap().verify(
        &stamp,
        Claim::WorkspaceOperation(operation),
        1_000
    ));
}
