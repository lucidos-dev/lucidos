use super::*;

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

#[test]
fn metadata_events_are_correct() {
    let metadata_events = [
        "ThreadTitleGenerated",
        "ThreadTitleRenamed",
        "ThreadSaved",
        "ThreadUnsaved",
        "ThreadMovedToFolder",
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

#[test]
fn thread_saved_moves_an_archived_thread_to_the_inbox() {
    for thread_type in [ThreadType::Chat, ThreadType::CodingAgent] {
        let result =
            resolve_transition("ThreadSaved", thread_type, ArchiveState::Archived, false).unwrap();
        assert_eq!(result.new_section, Some(ArchiveState::Inbox));
    }
}

#[test]
fn only_the_pinned_archived_pair_is_illegal() {
    assert!(is_retention_legal(ArchiveState::Inbox, false));
    assert!(is_retention_legal(ArchiveState::Inbox, true));
    assert!(is_retention_legal(ArchiveState::Archived, false));
    assert!(!is_retention_legal(ArchiveState::Archived, true));
}

#[test]
fn thread_archived_is_terminal() {
    assert_eq!(classify_event("ThreadArchived"), Some(EventClass::Terminal));
}

#[test]
fn chat_sub_threads_stay_in_the_inbox() {
    let result = resolve_transition(
        "ResponseGenerated",
        ThreadType::Chat,
        ArchiveState::Inbox,
        false,
    )
    .unwrap();
    assert_eq!(result.new_section, Some(ArchiveState::Inbox));
}

#[test]
fn unattended_trigger_runs_route_to_archived() {
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

#[test]
fn interrupted_trigger_fire_surfaces_without_changing_status() {
    for section in [ArchiveState::Inbox, ArchiveState::Archived] {
        for unattended in [false, true] {
            let result = resolve_transition(
                "TriggerRunInterrupted",
                ThreadType::Chat,
                section,
                unattended,
            )
            .expect("interrupted fires have a lifecycle transition");
            assert_eq!(result.new_section, Some(ArchiveState::Inbox));
        }
    }
    assert!(status_transitions().iter().all(|(event, transition)| {
        *event != "TriggerRunInterrupted" || transition.status == StatusRule::NoChange
    }));
}

#[test]
fn undo_restores_an_unattended_trigger_run() {
    let result = resolve_transition(
        "ThreadUnarchived",
        ThreadType::Chat,
        ArchiveState::Archived,
        true,
    )
    .unwrap();
    assert_eq!(result.new_section, Some(ArchiveState::Inbox));
}

#[test]
fn response_aborted_surfaces_both_thread_types_to_inbox() {
    let chat = resolve_transition(
        "ResponseAborted",
        ThreadType::Chat,
        ArchiveState::Archived,
        false,
    )
    .unwrap();
    assert_eq!(chat.new_section, Some(ArchiveState::Inbox));

    let cc = resolve_transition(
        "ResponseAborted",
        ThreadType::CodingAgent,
        ArchiveState::Archived,
        false,
    )
    .unwrap();
    assert_eq!(cc.new_section, Some(ArchiveState::Inbox));
}

#[test]
fn response_aborted_chat_sub_thread_stays_in_the_inbox() {
    let result = resolve_transition(
        "ResponseAborted",
        ThreadType::Chat,
        ArchiveState::Inbox,
        false,
    )
    .unwrap();
    assert_eq!(result.new_section, Some(ArchiveState::Inbox));
}

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
    let result = resolve_transition(
        "ResponseCanceled",
        ThreadType::Chat,
        ArchiveState::Inbox,
        false,
    )
    .unwrap();
    assert_eq!(result.new_section, Some(ArchiveState::Inbox));
}

#[test]
fn cc_threads_go_to_inbox_even_when_unattended() {
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

#[test]
fn an_event_that_waits_on_the_user_never_archives() {
    for &event_type in WAITING_FOR_USER_ANSWER_EVENTS {
        let mut legal_somewhere = false;
        for thread_type in [ThreadType::Chat, ThreadType::CodingAgent] {
            for section in [ArchiveState::Archived, ArchiveState::Inbox] {
                for is_unattended in [true, false] {
                    let Ok(result) =
                        resolve_transition(event_type, thread_type, section, is_unattended)
                    else {
                        continue;
                    };
                    legal_somewhere = true;
                    assert_eq!(
                        result.new_section,
                        Some(ArchiveState::Inbox),
                        "'{event_type}' on {thread_type:?} from {section:?} \
                         (unattended={is_unattended}) must land in the inbox",
                    );
                }
            }
        }
        assert!(legal_somewhere, "'{event_type}' is legal on no thread type");
    }
}

#[test]
fn waiting_for_user_answer_events_match_the_status_table() {
    use std::collections::BTreeSet;
    let from_table: BTreeSet<&str> = status_transitions()
        .into_iter()
        .filter(|(_, t)| t.status == StatusRule::Set(ThreadStatus::WaitingForUserAnswer))
        .map(|(event_type, _)| event_type)
        .collect();
    let listed: BTreeSet<&str> = WAITING_FOR_USER_ANSWER_EVENTS.iter().copied().collect();
    assert_eq!(
        listed, from_table,
        "every event that parks a thread on the user must be in \
         WAITING_FOR_USER_ANSWER_EVENTS, or the unattended guard archives it",
    );
}

#[test]
fn only_a_waiting_thread_or_the_home_thread_refuses_archive() {
    for status in ThreadStatus::ALL {
        for thread_type in [ThreadType::Chat, ThreadType::CodingAgent] {
            for is_home in [false, true] {
                let refused =
                    check_archive_allowed(thread_type, ArchiveState::Inbox, status, is_home)
                        .is_err();
                assert_eq!(
                    refused,
                    is_home || status == ThreadStatus::WaitingForUserAnswer,
                    "archive of a {status:?} {thread_type:?} thread, home={is_home}",
                );
            }
        }
    }
}

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

#[test]
fn a_call_writes_only_metadata() {
    let voice_events = [
        "VoiceSessionStarted",
        "VoiceSessionEnded",
        "SpokenReplyGenerated",
        "SpokenMessageReceived",
    ];
    for event_type in voice_events {
        assert_eq!(
            classify_event(event_type),
            Some(EventClass::Metadata),
            "'{}' must not touch the thread's status",
            event_type
        );
        assert!(
            all_persisted_event_types().contains(&event_type),
            "'{}' is what a call leaves behind, so it has to be persisted",
            event_type
        );
        for thread_type in [ThreadType::Chat, ThreadType::CodingAgent] {
            for section in [ArchiveState::Archived, ArchiveState::Inbox] {
                let result = resolve_transition(event_type, thread_type, section, false)
                    .unwrap_or_else(|e| panic!("'{}' was rejected: {:?}", event_type, e));
                assert_eq!(
                    result.new_section, None,
                    "'{}' moved a {:?} thread out of {:?}. The caller is on the \
                     call, so surfacing it asks for attention they already give",
                    event_type, thread_type, section
                );
            }
        }
    }
}

#[test]
fn a_delegation_starts_a_turn_exactly_as_the_message_it_replaced() {
    assert_eq!(classify_event("WorkDelegated"), Some(EventClass::Start));
    assert!(all_persisted_event_types().contains(&"WorkDelegated"));
    for thread_type in [ThreadType::Chat, ThreadType::CodingAgent] {
        for section in [ArchiveState::Archived, ArchiveState::Inbox] {
            let delegated = resolve_transition("WorkDelegated", thread_type, section, false);
            let typed = resolve_transition("MessageReceived", thread_type, section, false);
            assert_eq!(
                delegated.as_ref().ok().map(|r| r.new_section),
                typed.as_ref().ok().map(|r| r.new_section),
                "a delegation and a typed message disagree for a {:?} thread in {:?}",
                thread_type,
                section
            );
        }
    }
}

#[test]
fn a_side_question_moves_nothing() {
    use crate::engine::thread_events::ThreadEvent;
    let status_rules: std::collections::BTreeMap<&str, StatusTransition> =
        status_transitions().into_iter().collect();
    for &event_type in ThreadEvent::SIDE_QUESTION_EVENT_TYPES {
        assert_eq!(
            classify_event(event_type),
            Some(EventClass::Metadata),
            "'{event_type}' must be a quiet class"
        );
        assert!(
            all_persisted_event_types().contains(&event_type),
            "'{event_type}' must be persisted so the card survives reload"
        );
        assert!(
            !status_rules.contains_key(event_type),
            "'{event_type}' must not write a status"
        );
        assert!(!LAST_ACTIVITY_EVENTS.contains(&event_type));
        assert!(!MESSAGE_COUNT_EVENTS.contains(&event_type));
        assert!(!WAITING_FOR_USER_ANSWER_EVENTS.contains(&event_type));
        for thread_type in [ThreadType::Chat, ThreadType::CodingAgent] {
            for section in [ArchiveState::Archived, ArchiveState::Inbox] {
                for unattended in [false, true] {
                    let result = resolve_transition(event_type, thread_type, section, unattended)
                        .unwrap_or_else(|e| panic!("'{event_type}' was rejected: {e:?}"));
                    assert_eq!(
                        result.new_section, None,
                        "'{event_type}' moved a {thread_type:?} thread out of {section:?}"
                    );
                }
            }
        }
    }
}
