use super::*;

const SECTION_TRANSITION_EVENTS: &[(&str, &str)] = &[
    ("ThreadArchived", "archived"),
];

// 25. start_events_set_status_running
#[test]
fn start_events_with_transitions_set_running() {
    // Start classification is about exchange grouping (UI), not status.
    // Some Start events (MissingHardeningDetected, MergeConflictDetected) don't
    // set status=running in event_bus.rs. This test only checks that Start events
    // that DO have a status transition always set Running (never idle/waiting).
    let transitions: std::collections::HashMap<&str, StatusTransition> =
        status_transitions().into_iter().collect();
    for event_type in all_persisted_event_types() {
        if classify_event(event_type) == Some(EventClass::Start) {
            if let Some(t) = transitions.get(event_type) {
                if let StatusRule::Set(s) = &t.status {
                    assert_eq!(
                        *s,
                        ThreadStatus::Running,
                        "Start event '{}' with Set status should set Running, not {:?}",
                        event_type,
                        s
                    );
                }
            }
        }
    }
}

// 26. terminal_events_never_set_running
#[test]
fn terminal_events_never_set_running() {
    let transitions: std::collections::HashMap<&str, StatusTransition> =
        status_transitions().into_iter().collect();
    for event_type in all_persisted_event_types() {
        if classify_event(event_type) == Some(EventClass::Terminal) {
            if let Some(t) = transitions.get(event_type) {
                match &t.status {
                    StatusRule::Set(s) => assert_ne!(
                        *s,
                        ThreadStatus::Running,
                        "Terminal event '{}' should not set Running",
                        event_type
                    ),
                    StatusRule::ConditionalCc(w, wo) => {
                        assert_ne!(
                            *w,
                            ThreadStatus::Running,
                            "Terminal '{}' withChanges should not be Running",
                            event_type
                        );
                        assert_ne!(
                            *wo,
                            ThreadStatus::Running,
                            "Terminal '{}' withoutChanges should not be Running",
                            event_type
                        );
                    }
                    StatusRule::NoChange => {}
                }
            }
        }
    }
}

// 27. section_transition_events_are_valid_persisted_events
#[test]
fn section_transition_events_are_valid_persisted_events() {
    let all = all_persisted_event_types();
    for (event, _) in SECTION_TRANSITION_EVENTS {
        assert!(
            all.contains(event),
            "SECTION_TRANSITION_EVENTS contains '{}' which is not a persisted event type",
            event
        );
    }
}

// ── Phase 0: Pre-refactor safety net ──

/// Cross-validate: events with status_transitions Setting → Running must be
/// classified as Start (or at least not Terminal). Events Setting → Idle/Waiting
/// must NOT be classified as Start.
#[test]
fn status_transition_classification_consistency() {
    let transitions: std::collections::HashMap<&str, StatusTransition> =
        status_transitions().into_iter().collect();
    for (event, transition) in &transitions {
        let class = classify_event(event);
        match &transition.status {
            StatusRule::Set(ThreadStatus::Running) => {
                // Running-setters should be Start or Activity, never Terminal
                if let Some(c) = class {
                    assert_ne!(
                        c,
                        EventClass::Terminal,
                        "Event '{}' sets Running but is classified as Terminal",
                        event
                    );
                }
            }
            StatusRule::Set(ThreadStatus::Idle)
            | StatusRule::Set(ThreadStatus::Waiting)
            | StatusRule::Set(ThreadStatus::Failed) => {
                // Idle/Waiting/Failed-setters should NOT be Start
                if let Some(c) = class {
                    assert_ne!(
                        c,
                        EventClass::Start,
                        "Event '{}' sets non-running terminal status but is classified as Start",
                        event
                    );
                }
            }
            _ => {} // ConditionalCc and NoChange are fine in any class
        }
    }
}

/// Cross-validate: CcFlagRule != None should only appear on CC-relevant events
/// (Change*, ClaudeCode*, MergeConflict*, ThreadArchived). This catches accidental
/// coding_agent_proposed/coding_agent_applying mutations on chat-only events.
#[test]
fn cc_flag_rules_only_on_cc_relevant_events() {
    let cc_relevant_prefixes = ["Change", "CodingAgent", "MergeConflict", "ThreadArchived"];
    for (event, transition) in status_transitions() {
        if transition.cc_flags != CcFlagRule::None {
            let is_cc_relevant = cc_relevant_prefixes
                .iter()
                .any(|prefix| event.starts_with(prefix));
            assert!(
                is_cc_relevant,
                "Event '{}' has CcFlagRule {:?} but doesn't match any CC-relevant prefix {:?}",
                event, transition.cc_flags, cc_relevant_prefixes
            );
        }
    }
}

/// Cross-validate: MESSAGE_COUNT_EVENTS should all be Start events —
/// only user-initiated "start of exchange" events increment the count.
#[test]
fn message_count_events_are_start_events() {
    for event in MESSAGE_COUNT_EVENTS {
        let class = classify_event(event);
        assert_eq!(
                class,
                Some(EventClass::Start),
                "MESSAGE_COUNT_EVENT '{}' should be classified as Start (it starts a new exchange), got {:?}",
                event, class
            );
    }
}

/// Cross-validate: every event that has a StatusTransition must be a persisted
/// event type. Transient events should never appear in status_transitions().
#[test]
fn status_transitions_only_contain_persisted_events() {
    let all = all_persisted_event_types();
    for (event, _) in status_transitions() {
        assert!(
            all.contains(&event),
            "status_transitions() contains '{}' which is not in all_persisted_event_types(). \
                 Transient events must not have status transitions.",
            event
        );
    }
}

/// Cross-validate: Start events that set status=Running should also appear in
/// LAST_ACTIVITY_EVENTS (they start a new exchange, so they're activity).
/// Activity-classified events (like CodingAgentPromptSent) may set Running
/// without updating last_activity — they only update last_revived_at.
#[test]
fn start_running_setters_are_in_last_activity_events() {
    for (event, transition) in status_transitions() {
        if let StatusRule::Set(ThreadStatus::Running) = transition.status {
            if classify_event(event) == Some(EventClass::Start) {
                assert!(
                    LAST_ACTIVITY_EVENTS.contains(&event),
                    "Start event '{}' sets status=Running but is NOT in LAST_ACTIVITY_EVENTS. \
                         Start events begin new exchanges and should update last_activity.",
                    event
                );
            }
        }
    }
}

#[test]
fn child_thread_completed_is_start_class() {
    assert_eq!(
        classify_event("ChildThreadCompleted"),
        Some(EventClass::Start),
        "ChildThreadCompleted is an exchange-starter — its render is the rich card \
         (see docs/plans/2026-05-12-child-completion-card-design.md)"
    );
}

#[test]
fn is_blocking_definition() {
    use ArchiveState::*;
    use ThreadStatus::*;
    use ThreadType::*;
    let cc = CodingAgent;
    let chat = Chat;

    // Running / WaitingForUserAnswer always block, regardless of archive_state
    // — active work cannot be "already terminal", so the Archived short-circuit
    // must not mask it.
    assert!(is_blocking(chat, Running, Archived, false, false));
    assert!(is_blocking(cc, Running, Archived, true, false));
    assert!(is_blocking(cc, Running, Archived, true, true));
    assert!(is_blocking(cc, WaitingForUserAnswer, Archived, false, false));

    // Archived + Idle does NOT block — the user dismissed the thread and
    // it isn't stranding active work. Holds even with pending changes (the
    // cascade clears dangling change rows before archiving).
    assert!(!is_blocking(chat, Idle, Archived, false, false));
    assert!(!is_blocking(cc, Idle, Archived, false, false));
    assert!(!is_blocking(cc, Idle, Archived, true, false));

    // Inbox + Running blocks (both thread types) regardless of repo.
    assert!(is_blocking(chat, Running, Inbox, false, false));
    assert!(is_blocking(cc, Running, Inbox, false, false));
    assert!(is_blocking(cc, Running, Inbox, false, true));

    // Inbox + WaitingForUserAnswer blocks.
    assert!(is_blocking(cc, WaitingForUserAnswer, Inbox, false, false));
    assert!(is_blocking(chat, WaitingForUserAnswer, Inbox, false, false));
    assert!(is_blocking(cc, WaitingForUserAnswer, Inbox, false, true));

    // Inbox + has_pending_changes blocks for in-workspace CC only.
    assert!(is_blocking(cc, Idle, Inbox, true, false));
    assert!(!is_blocking(chat, Idle, Inbox, true, false));
    // External-repo CC with pending changes is the carve-out: the frontend
    // surfaces Archive (not Apply) for these, and the cascade handler clears
    // the change with ChangeApplied before archiving — so it must NOT block.
    assert!(!is_blocking(cc, Idle, Inbox, true, true));

    // Idle no-pending in Inbox does not block.
    assert!(!is_blocking(chat, Idle, Inbox, false, false));
    assert!(!is_blocking(cc, Idle, Inbox, false, false));
    assert!(!is_blocking(cc, Idle, Inbox, false, true));

    // Waiting (CC pending-review) status alone doesn't block; the
    // has_pending_changes signal does.
    assert!(!is_blocking(cc, Waiting, Inbox, false, false));

    // Failed status alone does not block — it's a terminal-ish state, not active work.
    assert!(!is_blocking(cc, Failed, Inbox, false, false));
    assert!(!is_blocking(chat, Failed, Inbox, false, false));
}
