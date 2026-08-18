use super::*;

// 1. every_persisted_event_is_classified
#[test]
fn every_persisted_event_is_classified() {
    for event_type in all_persisted_event_types() {
        assert!(
            classify_event(event_type).is_some(),
            "Persisted event '{}' is not classified",
            event_type
        );
    }
}

// 1b. Every persisted event type must resolve through `resolve_transition`
// without falling into the "_ => violation('Unknown event type')" arm.
// Without this, adding a new ThreadEvent variant to `all_persisted_event_types`
// + `classify_event` is silently insufficient: emit_or_log swallows the error
// and the event never lands in the events table.
#[test]
fn every_persisted_event_resolves_in_lifecycle() {
    for event_type in all_persisted_event_types() {
        for thread_type in [ThreadType::Chat, ThreadType::CodingAgent] {
            let result = resolve_transition(event_type, thread_type, ArchiveState::Archived, false);
            if let Err(err) = &result {
                assert!(
                    !err.reason.contains("Unknown event type"),
                    "'{}' on {:?} hits the catch-all 'Unknown event type' arm — \
                     add it to the no_change list (or a more specific arm) in \
                     thread_lifecycle.rs::resolve_transition",
                    event_type,
                    thread_type
                );
            }
        }
    }
}

// 2. metadata_events_are_correct
#[test]
fn metadata_events_are_correct() {
    let metadata_events = [
        "ThreadTitleGenerated",
        "ThreadTitleRenamed",
        "ThreadSaved",
        "ThreadUnsaved",
    ];
    for event_type in &metadata_events {
        assert_eq!(
            classify_event(event_type),
            Some(EventClass::Metadata),
            "'{}' should be Metadata",
            event_type
        );
    }
}

// 3. cc_events_never_classified_as_start
#[test]
fn cc_events_never_classified_as_start() {
    let cc_specific = [
        "CodingAgentTextStreamed",
        "CodingAgentToolCalled",
        "CodingAgentToolResult",
        "CodingAgentPromptSent",
        "CodingAgentIdled",
    ];
    for event_type in &cc_specific {
        let class = classify_event(event_type).unwrap();
        assert_ne!(
            class,
            EventClass::Start,
            "CC-specific event '{}' should not be Start",
            event_type
        );
    }
}

// 4. both_thread_types_share_same_legal_sections
#[test]
fn both_thread_types_share_same_legal_sections() {
    assert!(is_section_legal(ThreadType::Chat, ArchiveState::Archived));
    assert!(is_section_legal(ThreadType::Chat, ArchiveState::Inbox));
    assert!(is_section_legal(
        ThreadType::CodingAgent,
        ArchiveState::Archived
    ));
    assert!(is_section_legal(
        ThreadType::CodingAgent,
        ArchiveState::Inbox
    ));
}

// 5. response_generated_surfaces_chat_to_inbox
#[test]
fn response_generated_surfaces_chat_to_inbox() {
    let result = resolve_transition(
        "ResponseGenerated",
        ThreadType::Chat,
        ArchiveState::Archived,
        false,
    )
    .unwrap();
    assert_eq!(result.new_section, Some(ArchiveState::Inbox));
}

// 6. response_generated_does_not_surface_cc
#[test]
fn response_generated_does_not_surface_cc() {
    let result = resolve_transition(
        "ResponseGenerated",
        ThreadType::CodingAgent,
        ArchiveState::Archived,
        false,
    )
    .unwrap();
    assert_eq!(result.new_section, None);
}

// 7. claude_code_idled_surfaces_cc_to_inbox
#[test]
fn claude_code_idled_surfaces_cc_to_inbox() {
    let result = resolve_transition(
        "CodingAgentIdled",
        ThreadType::CodingAgent,
        ArchiveState::Archived,
        false,
    )
    .unwrap();
    assert_eq!(result.new_section, Some(ArchiveState::Inbox));
}

// 8. claude_code_idled_rejected_for_chat
#[test]
fn claude_code_idled_rejected_for_chat() {
    let result = resolve_transition(
        "CodingAgentIdled",
        ThreadType::Chat,
        ArchiveState::Archived,
        false,
    );
    assert!(result.is_err());
}

// 9. change_applied_keeps_inbox — thread stays in REVIEW so Archive button appears
#[test]
fn change_applied_keeps_inbox() {
    let result = resolve_transition(
        "ChangeApplied",
        ThreadType::CodingAgent,
        ArchiveState::Inbox,
        false,
    )
    .unwrap();
    assert_eq!(
        result.new_section, None,
        "ChangeApplied must NOT change section — Archive button needs to appear"
    );
}

// 10. change_applied_no_op_if_not_in_inbox
#[test]
fn change_applied_no_op_if_not_in_inbox() {
    let result = resolve_transition(
        "ChangeApplied",
        ThreadType::CodingAgent,
        ArchiveState::Archived,
        false,
    )
    .unwrap();
    assert_eq!(result.new_section, None);
}

// 12. thread_archived_clears_inbox_both_types
#[test]
fn thread_archived_clears_inbox_both_types() {
    let chat = resolve_transition(
        "ThreadArchived",
        ThreadType::Chat,
        ArchiveState::Inbox,
        false,
    )
    .unwrap();
    assert_eq!(chat.new_section, Some(ArchiveState::Archived));

    let cc = resolve_transition(
        "ThreadArchived",
        ThreadType::CodingAgent,
        ArchiveState::Inbox,
        false,
    )
    .unwrap();
    assert_eq!(cc.new_section, Some(ArchiveState::Archived));
}

// 13. thread_archived_is_terminal
#[test]
fn thread_archived_is_terminal() {
    assert_eq!(classify_event("ThreadArchived"), Some(EventClass::Terminal));
}

// 14. chat_sub_threads_stay_in_the_inbox
#[test]
fn chat_sub_threads_stay_in_the_inbox() {
    // A finished chat sub-thread is attended: its parent spawned it and the
    // user can see it under that parent. Routing it to Archived writes a state
    // no `ThreadArchived` event backs, so the drawer dims a row nobody
    // archived. Depth is not an input to the bottom guard.
    let result = resolve_transition(
        "ResponseGenerated",
        ThreadType::Chat,
        ArchiveState::Inbox,
        false,
    )
    .unwrap();
    assert_eq!(result.new_section, Some(ArchiveState::Inbox));
}

// 14a. unattended_trigger_runs_route_to_archived
#[test]
fn unattended_trigger_runs_route_to_archived() {
    // The one population the bottom guard still covers. A scheduled task run
    // nobody opted into reviewing hides on its terminal event. It must not
    // ask for attention on work the user never started.
    for event_type in ["ResponseGenerated", "ResponseAborted", "ResponseCanceled"] {
        let result =
            resolve_transition(event_type, ThreadType::Chat, ArchiveState::Inbox, true).unwrap();
        assert_eq!(
            result.new_section,
            Some(ArchiveState::Archived),
            "'{event_type}' on an unattended trigger run must archive it",
        );
    }
}

// 14b. response_aborted_surfaces_both_thread_types_to_inbox
#[test]
fn response_aborted_surfaces_both_thread_types_to_inbox() {
    // Chat thread: ResponseAborted → inbox
    let chat = resolve_transition(
        "ResponseAborted",
        ThreadType::Chat,
        ArchiveState::Archived,
        false,
    )
    .unwrap();
    assert_eq!(chat.new_section, Some(ArchiveState::Inbox));

    // CC thread: ResponseAborted → inbox (aborted CC needs user attention in REVIEW)
    let cc = resolve_transition(
        "ResponseAborted",
        ThreadType::CodingAgent,
        ArchiveState::Archived,
        false,
    )
    .unwrap();
    assert_eq!(cc.new_section, Some(ArchiveState::Inbox));
}

// 14c. response_aborted_chat_sub_thread_stays_in_the_inbox
#[test]
fn response_aborted_chat_sub_thread_stays_in_the_inbox() {
    // Same contract as `chat_sub_threads_stay_in_the_inbox` above. An abort
    // matters more, not less, for a sub-thread: the row is how the user finds
    // out the delegated work died.
    let result = resolve_transition(
        "ResponseAborted",
        ThreadType::Chat,
        ArchiveState::Inbox,
        false,
    )
    .unwrap();
    assert_eq!(result.new_section, Some(ArchiveState::Inbox));
}

// CC threads also receive a CodingAgentIdled after ResponseCanceled in the
// normal path, but the no-session settle fallback (claude_code.rs) emits
// ResponseCanceled alone — so the inbox transition must hold for both.
#[test]
fn response_canceled_surfaces_both_thread_types_to_inbox() {
    let chat = resolve_transition(
        "ResponseCanceled",
        ThreadType::Chat,
        ArchiveState::Archived,
        false,
    )
    .unwrap();
    assert_eq!(chat.new_section, Some(ArchiveState::Inbox));

    let cc = resolve_transition(
        "ResponseCanceled",
        ThreadType::CodingAgent,
        ArchiveState::Archived,
        false,
    )
    .unwrap();
    assert_eq!(cc.new_section, Some(ArchiveState::Inbox));
}

#[test]
fn response_canceled_chat_sub_thread_stays_in_the_inbox() {
    // Same contract as `chat_sub_threads_stay_in_the_inbox` above.
    let result = resolve_transition(
        "ResponseCanceled",
        ThreadType::Chat,
        ArchiveState::Inbox,
        false,
    )
    .unwrap();
    assert_eq!(result.new_section, Some(ArchiveState::Inbox));
}

// 14d. cc_threads_go_to_inbox_even_when_unattended
#[test]
fn cc_threads_go_to_inbox_even_when_unattended() {
    // The bottom guard's coding-agent exemption, pinned from the only side
    // that can now exercise it. Every session ends needing Apply, Discard or
    // Archive, so an unattended one must still surface.
    for event_type in ["CodingAgentIdled", "ResponseAborted", "ChangeProposed"] {
        let result = resolve_transition(
            event_type,
            ThreadType::CodingAgent,
            ArchiveState::Archived,
            true,
        )
        .unwrap();
        assert_eq!(
            result.new_section,
            Some(ArchiveState::Inbox),
            "'{event_type}' must surface a coding-agent thread even unattended",
        );
    }
}

// 15. no_transition_produces_illegal_section
#[test]
fn no_transition_produces_illegal_section() {
    let thread_types = [ThreadType::Chat, ThreadType::CodingAgent];
    let sections = [ArchiveState::Archived, ArchiveState::Inbox];

    for event_type in all_persisted_event_types() {
        for &thread_type in &thread_types {
            for &section in &sections {
                for is_unattended in [true, false] {
                    if let Ok(result) =
                        resolve_transition(event_type, thread_type, section, is_unattended)
                    {
                        if let Some(new_section) = result.new_section {
                            assert!(
                                is_section_legal(thread_type, new_section),
                                "Transition '{}' for {:?} (unattended={}) produced illegal section {:?}",
                                event_type,
                                thread_type,
                                is_unattended,
                                new_section
                            );
                        }
                    }
                }
            }
        }
    }
}
