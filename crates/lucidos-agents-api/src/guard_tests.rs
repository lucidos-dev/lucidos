/// Stop ends the turn and leaves the thread’s subscriptions armed.
/// The event-wait E2E test also checks this behavior through the API.
#[test]
fn stopping_a_turn_never_reaches_the_thread_s_subscriptions() {
    const CANCEL_CHAT_SRC: &str = include_str!("api/chat.rs");
    let body = CANCEL_CHAT_SRC
        .split_once("pub(super) async fn cancel_chat(")
        .expect("cancel_chat still lives in api/chat.rs")
        .1;
    // Comments can name both symbols. Check for the call and construction.
    assert!(
        !body.contains("cancel_event_waits_for_thread("),
        "cancel_chat must not cancel the thread's subscriptions: Stop ends the turn"
    );
    assert!(
        !body.contains("EventWaitCancelCause::ThreadCanceled,"),
        "ThreadCanceled is retired: it is read from old rows, never emitted"
    );
}

/// The step row renders its description as well as its arguments.
/// Both emitters must redact before describing so the row cannot leak secrets.
#[test]
fn both_tool_call_emit_sites_redact_before_describing() {
    for (label, src, redact_call, describe_call) in [
        (
            "agent_session/run_session/run.rs",
            include_str!("engine/agent_session/run_session/run.rs"),
            "redact_subprocess_output_in_json(&mut input, &tool_result_secrets)",
            "describe_cc_tool(&name, &input)",
        ),
        (
            "agentic_loop/helpers.rs (emit_tool_called)",
            include_str!("engine/agentic_loop/helpers.rs"),
            "redact_postgres_secrets_in_json(&mut redacted_args)",
            "describe_tool(&tool_call.name, &redacted_args)",
        ),
    ] {
        let find = |needle: &str| {
            src.find(needle).unwrap_or_else(|| {
                panic!("{label}: `{needle}` not found. If the call was renamed or moved, update this guard rather than deleting it.")
            })
        };
        assert!(
            find(redact_call) < find(describe_call),
            "{label}: the description is built from the UNREDACTED args, so a hardcoded postgres password lands in cleartext on the persisted `description`. Redact first."
        );
    }
}
