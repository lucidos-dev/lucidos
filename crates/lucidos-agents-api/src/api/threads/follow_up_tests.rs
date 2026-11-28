use super::*;

#[test]
fn every_child_follow_up_error_maps_to_its_declared_status() {
    let id = Uuid::new_v4();
    let cases = [
        (ChildFollowUpError::UnknownChild(id), 404),
        (ChildFollowUpError::NotYourChild(id), 403),
        (ChildFollowUpError::ChildDiscarded(id), 409),
        (ChildFollowUpError::ChildIsDraft(id), 409),
        (ChildFollowUpError::SelfTarget(id), 400),
        (ChildFollowUpError::NoCaller, 403),
        (ChildFollowUpError::CrossWorkspaceUnsupported, 400),
        (ChildFollowUpError::Internal("boom".into()), 500),
    ];
    for (err, expected) in cases {
        let rendered = ChildFollowUpError::to_string(&err);
        let mapped = api_error(err);
        assert_eq!(
            mapped.status.as_u16(),
            expected,
            "status drifted from the ladder"
        );
        assert_eq!(
            mapped.message, rendered,
            "the body must carry the ladder's own actionable wording"
        );
    }
}

#[test]
fn follow_up_request_has_no_routing_fields() {
    let body = serde_json::json!({
        "message": "go the other way",
        "mode": "human",
        "parent_thread_id": Uuid::new_v4().to_string(),
        "use_coding_agent": true,
        "repo_id": Uuid::new_v4().to_string(),
        "caller_thread_id": Uuid::new_v4().to_string(),
        "source_thread_id": Uuid::new_v4().to_string(),
    });
    let parsed: FollowUpRequest = serde_json::from_value(body).expect("body parses");
    assert_eq!(parsed.message, "go the other way");
    assert_eq!(
        parsed.event_id, None,
        "no forbidden field may bleed into one the route does read"
    );
    assert_eq!(parsed.caller_workspace, None);

    let shown = format!("{parsed:?}");
    for field in [
        "mode",
        "parent_thread_id",
        "use_coding_agent",
        "repo_id",
        "caller_thread_id",
        "source_thread_id",
    ] {
        assert!(
            !shown.contains(field),
            "the body must not grow a `{field}` field: the caller comes from the origin token \
             and coding-agent-ness from the child's own row, got {shown}"
        );
    }

    let no_message = serde_json::json!({
        "mode": "agent",
        "parent_thread_id": Uuid::new_v4().to_string(),
        "caller_thread_id": Uuid::new_v4().to_string(),
    });
    assert!(
        serde_json::from_value::<FollowUpRequest>(no_message).is_err(),
        "message is the only required field"
    );
}

#[test]
fn a_cross_workspace_body_field_is_parsed_so_it_can_be_refused() {
    let body = serde_json::json!({ "message": "hi", "caller_workspace": "dev" });
    let parsed: FollowUpRequest = serde_json::from_value(body).expect("body parses");
    assert_eq!(parsed.caller_workspace.as_deref(), Some("dev"));
    assert_eq!(
        api_error(ChildFollowUpError::CrossWorkspaceUnsupported)
            .status
            .as_u16(),
        400
    );
}

#[test]
fn delivered_to_is_kebab_case_on_the_wire() {
    assert_eq!(delivered_to_wire(FollowUpDelivery::Running), "running");
    assert_eq!(
        delivered_to_wire(FollowUpDelivery::WaitingForUserAnswer),
        "waiting-for-user-answer"
    );
    assert_eq!(delivered_to_wire(FollowUpDelivery::Held), "held");
    assert_eq!(delivered_to_wire(FollowUpDelivery::Revived), "revived");
}

#[test]
fn the_response_carries_the_child_title_and_a_readable_delivery() {
    let child_thread_id = Uuid::new_v4();
    let ack = FollowUpAck {
        child_thread_id,
        reach: FollowUpReach::OwnChild,
        child_title: "Research the pricing page".into(),
        delivered_to: FollowUpDelivery::WaitingForUserAnswer,
    };
    let json = serde_json::to_value(FollowUpResponse::from(ack)).expect("serializes");
    assert_eq!(json["child_thread_id"], child_thread_id.to_string());
    assert_eq!(json["child_title"], "Research the pricing page");
    assert_eq!(json["delivered_to"], "waiting-for-user-answer");
    assert_eq!(json["reach"], "own-child");
    assert!(
        json["detail"]
            .as_str()
            .expect("detail is a string")
            .contains("until a human answers"),
        "the ack must say why the child has not read this yet"
    );
}
