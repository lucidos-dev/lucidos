use super::*;

// ── ChangeProposed must surface CC threads in REVIEW ──

#[test]
fn change_proposed_surfaces_cc_to_inbox() {
    // Bug: ChangeProposed was in the no_change bucket, so CC threads with
    // proposed changes went to ARCHIVE instead of REVIEW when there was no
    // prior CodingAgentIdled (or user had already read the thread).
    let result = resolve_transition(
        "ChangeProposed",
        ThreadType::CodingAgent,
        ArchiveState::Archived,
        false,
    )
    .unwrap();
    assert_eq!(
        result.new_section,
        Some(ArchiveState::Inbox),
        "ChangeProposed must surface CC thread to inbox so the Apply panel appears"
    );
}

#[test]
fn change_proposed_keeps_cc_in_inbox_if_already_in_inbox() {
    let result = resolve_transition(
        "ChangeProposed",
        ThreadType::CodingAgent,
        ArchiveState::Inbox,
        false,
    )
    .unwrap();
    assert_eq!(result.new_section, Some(ArchiveState::Inbox));
}

#[test]
fn cc_running_shows_no_close_actions() {
    // Live thread: no close actions; only the Save retention toggle appends.
    let actions = available_thread_actions(
        ThreadType::CodingAgent,
        ThreadStatus::Running,
        ArchiveState::Inbox,
        true,
        false,
        false,
        false,
        false,
        false,
    );
    assert_eq!(actions, vec![Action::Save]);
}

// ── UserQuestionAsked / UserQuestionAnswered tests ──

#[test]
fn user_question_asked_surfaces_cc_to_inbox() {
    let result = resolve_transition(
        "UserQuestionAsked",
        ThreadType::CodingAgent,
        ArchiveState::Archived,
        false,
    )
    .unwrap();
    assert_eq!(result.new_section, Some(ArchiveState::Inbox));
}

#[test]
fn user_question_asked_surfaces_chat_to_inbox() {
    // The chat agent's `ask_user_question` tool raises the same
    // `UserQuestionAsked` event CC's tool raises. The lifecycle must treat
    // both thread types identically — surface to inbox so the question
    // card renders and the thread enters REVIEW.
    let result = resolve_transition(
        "UserQuestionAsked",
        ThreadType::Chat,
        ArchiveState::Archived,
        false,
    )
    .unwrap();
    assert_eq!(result.new_section, Some(ArchiveState::Inbox));
}

#[test]
fn user_question_answered_no_section_change() {
    let result = resolve_transition(
        "UserQuestionAnswered",
        ThreadType::CodingAgent,
        ArchiveState::Inbox,
        false,
    )
    .unwrap();
    assert_eq!(
        result.new_section, None,
        "answer event keeps thread in REVIEW until next terminal"
    );
}

#[test]
fn user_question_answered_no_section_change_on_chat() {
    let result = resolve_transition(
        "UserQuestionAnswered",
        ThreadType::Chat,
        ArchiveState::Inbox,
        false,
    )
    .unwrap();
    assert_eq!(
        result.new_section, None,
        "chat answer event keeps thread in REVIEW (same as CC)"
    );
}

#[test]
fn user_question_asked_classified_action_required() {
    assert_eq!(
        classify_event("UserQuestionAsked"),
        Some(EventClass::ActionRequired)
    );
}

#[test]
fn user_question_asked_status_transition() {
    let transitions: std::collections::HashMap<&str, StatusTransition> =
        status_transitions().into_iter().collect();
    let t = transitions
        .get("UserQuestionAsked")
        .expect("status transition");
    assert_eq!(
        t.status,
        StatusRule::Set(ThreadStatus::WaitingForUserAnswer)
    );
    assert_eq!(t.cc_flags, CcFlagRule::None);
}

#[test]
fn user_question_answered_status_transition_resumes_running() {
    let transitions: std::collections::HashMap<&str, StatusTransition> =
        status_transitions().into_iter().collect();
    let t = transitions
        .get("UserQuestionAnswered")
        .expect("status transition");
    assert_eq!(t.status, StatusRule::Set(ThreadStatus::Running));
}

/// Regression: when a chat follow-up MR arrives during the prior turn's stream
/// and the new turn's first event is `MemoryRecalled`, that event must bump
/// status back to Running. Without this, the frontend's stale-exchange detector
/// (`exchange-status.ts::exchangeStatus`, the `threadIdle && hasSteps &&
/// !isComplete` branch) briefly classifies the active turn as `'aborted'` in
/// the window between MemoryRecalled landing over SSE and the next activity
/// event (typically ThoughtStreamed) arriving milliseconds later. Reproduced on
/// a real chat thread.
#[test]
fn memory_recalled_bumps_status_to_running() {
    let transitions: std::collections::HashMap<&str, StatusTransition> =
        status_transitions().into_iter().collect();
    let t = transitions
        .get("MemoryRecalled")
        .expect("MemoryRecalled must have a status transition");
    assert_eq!(t.status, StatusRule::Set(ThreadStatus::Running));
    assert_eq!(t.cc_flags, CcFlagRule::None);
    assert!(
        LAST_ACTIVITY_EVENTS.contains(&"MemoryRecalled"),
        "MemoryRecalled must appear in LAST_ACTIVITY_EVENTS so the thread \
         timestamp stays current when memory recall is the only step of \
         a turn before the LLM starts streaming"
    );
}

#[test]
fn waiting_for_user_answer_routes_to_current_when_inbox() {
    // The transition contract guarantees `UserQuestionAsked` stamps
    // archive_state = Inbox, so the typical WFUA thread routes to Current.
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::WaitingForUserAnswer,
            false,
            false,
            false,
            false,
        ),
        DisplaySection::Current
    );
    // An archived legacy WFUA row routes to Archive (no special case anymore).
    // The transition contract enforces Inbox so this state shouldn't occur
    // in practice; the assertion documents the new behavior.
    assert_eq!(
        display_section(
            ArchiveState::Archived,
            ThreadStatus::WaitingForUserAnswer,
            false,
            false,
            false,
            false,
        ),
        DisplaySection::Archive
    );
    // Saved beats WFUA — the saved-section badge surfaces the question.
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::WaitingForUserAnswer,
            true,
            false,
            false,
            false,
        ),
        DisplaySection::Saved
    );
    // Active children keep it in Current (still live work).
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::WaitingForUserAnswer,
            false,
            true,
            false,
            false,
        ),
        DisplaySection::Current
    );
}

#[test]
fn waiting_for_user_answer_returns_no_actions() {
    // Mid-turn: Apply/Discard must never appear (incomplete work) and Archive
    // is replaced by a separately-rendered Cancel.
    for section in [ArchiveState::Inbox, ArchiveState::Archived] {
        for has_changes in [true, false] {
            let actions = available_thread_actions(
                ThreadType::CodingAgent,
                ThreadStatus::WaitingForUserAnswer,
                section,
                has_changes,
                false,
                false,
                false,
                false,
                false,
            );
            assert_eq!(
                actions,
                vec![Action::Save],
                "WaitingForUserAnswer must yield no close actions (section={:?}, has_changes={}); Cancel is rendered separately, Save toggle still appends",
                section,
                has_changes,
            );
        }
    }
}

#[test]
fn user_question_events_are_persisted() {
    assert!(all_persisted_event_types().contains(&"UserQuestionAsked"));
    assert!(all_persisted_event_types().contains(&"UserQuestionAnswered"));
}

// CodingAgentSettingsChanged must be accepted for CC threads (persisted metadata, no section change)
#[test]
fn cc_settings_changed_accepted_for_cc_threads() {
    let result = resolve_transition(
        "CodingAgentSettingsChanged",
        ThreadType::CodingAgent,
        ArchiveState::Archived,
        false,
    );
    assert!(
        result.is_ok(),
        "CodingAgentSettingsChanged must be accepted for CC threads"
    );
    let result = result.unwrap();
    assert_eq!(
        result.new_section, None,
        "CodingAgentSettingsChanged must not change section"
    );
}

#[test]
fn cc_settings_changed_rejected_for_chat_threads() {
    let result = resolve_transition(
        "CodingAgentSettingsChanged",
        ThreadType::Chat,
        ArchiveState::Archived,
        false,
    );
    assert!(
        result.is_err(),
        "CodingAgentSettingsChanged must be rejected for Chat threads"
    );
}

#[test]
fn cc_settings_changed_is_classified() {
    assert!(
        classify_event("CodingAgentSettingsChanged").is_some(),
        "CodingAgentSettingsChanged must be classified"
    );
}

#[test]
fn cc_settings_changed_is_persisted_event() {
    assert!(
        all_persisted_event_types().contains(&"CodingAgentSettingsChanged"),
        "CodingAgentSettingsChanged must be in all_persisted_event_types()"
    );
}

// 22b. last_activity_events_are_valid_persisted_events
#[test]
fn last_activity_events_are_valid_persisted_events() {
    let all = all_persisted_event_types();
    for event in LAST_ACTIVITY_EVENTS {
        assert!(
            all.contains(event),
            "LAST_ACTIVITY_EVENTS contains '{}' which is not a persisted event type",
            event
        );
    }
}

// Events that update last_activity in the backend projection
// (event_bus_projection_thread.rs) must be in LAST_ACTIVITY_EVENTS so the
// frontend drawer timestamp stays in sync.
#[test]
fn required_events_are_in_last_activity() {
    for event in &[
        "ToolCalled",
        "ToolResult",
        "TextStreamed",
        "ThoughtStreamed",
        "CodingAgentTextStreamed",
        "CodingAgentToolCalled",
        "CodingAgentToolResult",
        "TriggerCompleted",
    ] {
        assert!(
            LAST_ACTIVITY_EVENTS.contains(event),
            "'{}' must be in LAST_ACTIVITY_EVENTS: it updates last_activity in \
                 event_bus_projection_thread.rs but the frontend won't sync without it",
            event
        );
    }
}

#[test]
fn status_transition_events_are_valid_persisted_events() {
    let all = all_persisted_event_types();
    for (event, _) in status_transitions() {
        assert!(
            all.contains(&event),
            "status_transitions contains '{}' which is not a persisted event type",
            event
        );
    }
}

#[test]
fn message_count_events_are_valid_persisted_events() {
    let all = all_persisted_event_types();
    for event in MESSAGE_COUNT_EVENTS {
        assert!(
            all.contains(event),
            "MESSAGE_COUNT_EVENTS contains '{}' which is not a persisted event type",
            event
        );
    }
}
