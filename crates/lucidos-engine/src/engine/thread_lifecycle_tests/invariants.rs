use super::*;

const SECTION_TRANSITION_EVENTS: &[(&str, &str)] = &[("ThreadArchived", "archived")];

// 25. start_events_set_status_running
#[test]
fn start_events_with_transitions_set_running() {
    // Start classification is about exchange grouping (UI), not status.
    // Some Start events (MissingHardeningDetected, MergeConflictDetected) don't
    // set status=running in event_bus_projection_thread.rs. This test only
    // checks that Start events with a status transition always set Running
    // (never idle/waiting).
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
            _ => {} // NoChange is fine in any class
        }
    }
}

/// Cross-validate: CcFlagRule != None should only appear on CC-relevant events
/// (Change*, ClaudeCode*, MergeConflict*, ThreadArchived). This catches accidental
/// change-state mutations on chat-only events.
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
    assert!(is_blocking(
        cc,
        WaitingForUserAnswer,
        Archived,
        false,
        false
    ));

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

/// `own_blocker` names the clause that holds, and the status clauses win over a
/// pending change: a waiting thread with a change asks for the answer first.
#[test]
fn own_blocker_names_the_clause() {
    use ArchiveState::*;
    use ThreadStatus::*;
    use ThreadType::*;
    let cc = CodingAgent;

    let (none, proposed, held) = (ChangeWork::None, ChangeWork::Proposed, ChangeWork::Held);
    assert_eq!(
        own_blocker(Chat, Running, Archived, none, false),
        Some(OwnBlocker::Running)
    );
    assert_eq!(
        own_blocker(cc, WaitingForUserAnswer, Inbox, proposed, false),
        Some(OwnBlocker::Question)
    );
    assert_eq!(
        own_blocker(cc, Idle, Inbox, proposed, false),
        Some(OwnBlocker::PendingChange)
    );
    assert_eq!(own_blocker(cc, Idle, Archived, proposed, false), None);
    assert_eq!(own_blocker(cc, Idle, Inbox, proposed, true), None);
    assert_eq!(own_blocker(Chat, Idle, Inbox, proposed, false), None);
    // A parked agent with an unproposed diff keeps Archive and Delete back
    // until Stop waiting proposes it (ADR 0395).
    assert_eq!(
        own_blocker(cc, Idle, Inbox, held, false),
        Some(OwnBlocker::HeldProposal)
    );
    assert_eq!(own_blocker(cc, Idle, Inbox, held, true), None);
    assert_eq!(own_blocker(cc, Idle, Inbox, none, false), None);
}

/// Only a live wait turns unproposed work into held work. A proposal wins
/// over a wait, and unproposed work with no wait is not held (ADR 0395).
#[test]
fn change_work_is_held_only_while_a_wait_is_live() {
    use ChangeStateKind::{None as NoWork, Proposed, Unproposed};
    assert_eq!(ChangeWork::of(Proposed, true), ChangeWork::Proposed);
    assert_eq!(ChangeWork::of(Proposed, false), ChangeWork::Proposed);
    assert_eq!(ChangeWork::of(Unproposed, true), ChangeWork::Held);
    assert_eq!(ChangeWork::of(Unproposed, false), ChangeWork::None);
    assert_eq!(ChangeWork::of(NoWork, true), ChangeWork::None);
    assert_eq!(ChangeWork::of(NoWork, false), ChangeWork::None);
}

/// The quoted values inside the migration's `CHECK (<column> ... IN (...))`.
fn check_values(migration: &str, column: &str) -> Vec<String> {
    let from = migration
        .find(&format!("{column} IN ("))
        .unwrap_or_else(|| panic!("no CHECK list for {column}"));
    let list = &migration[from..];
    let list = &list[list.find('(').unwrap() + 1..list.find(')').unwrap()];
    list.split(',')
        .map(|v| v.trim().trim_matches('\'').to_string())
        .collect()
}

const CHANGE_STATE_MIGRATION: &str =
    include_str!("../../../migrations/20261009125328_coding_agent_change_state.sql");

/// I2: the database admits exactly the Rust state kinds and reasons. A variant
/// added on one side only fails here, before a row can carry it.
#[test]
fn the_migration_check_lists_equal_the_rust_enums() {
    let kinds: Vec<String> = ChangeStateKind::ALL
        .iter()
        .map(|k| k.as_str().to_string())
        .collect();
    assert_eq!(
        check_values(CHANGE_STATE_MIGRATION, "coding_agent_change_state"),
        kinds
    );
    let reasons: Vec<String> = UnproposedReason::ALL
        .iter()
        .map(|r| r.as_str().to_string())
        .collect();
    assert_eq!(
        check_values(CHANGE_STATE_MIGRATION, "coding_agent_unproposed_reason"),
        reasons
    );
}

/// I2: each wire name is the column text, so the serde form and `as_str`
/// never drift, and `parse` reads back what `as_str` wrote.
#[test]
fn the_state_and_reason_wire_names_equal_their_column_text() {
    for kind in ChangeStateKind::ALL {
        assert_eq!(serde_json::to_value(kind).unwrap(), kind.as_str());
        assert_eq!(ChangeStateKind::parse(kind.as_str()), Some(kind));
    }
    for reason in UnproposedReason::ALL {
        assert_eq!(serde_json::to_value(reason).unwrap(), reason.as_str());
        assert_eq!(UnproposedReason::parse(reason.as_str()), Some(reason));
    }
}

/// I2: the plan reasons derive from `PlanHold`, never restated. Every hold
/// maps to its own reason, and only `TurnIncomplete` comes from elsewhere.
#[test]
fn every_plan_hold_maps_to_its_own_reason() {
    use crate::engine::change_ops::PlanHold;
    let from_holds: Vec<UnproposedReason> = [
        PlanHold::Missing,
        PlanHold::AwaitingApproval,
        PlanHold::OutsideBound,
    ]
    .into_iter()
    .map(UnproposedReason::from)
    .collect();
    let expected: Vec<UnproposedReason> = UnproposedReason::ALL
        .into_iter()
        .filter(|r| *r != UnproposedReason::TurnIncomplete)
        .collect();
    assert_eq!(from_holds, expected);
}

/// I1 in the type: each detail rides only on its own kind, and the column
/// reader maps every legal row back to the state that wrote it.
#[test]
fn the_column_reader_round_trips_every_legal_state() {
    let mut states = vec![
        CodingAgentChangeState::None,
        CodingAgentChangeState::Unproposed { reason: None },
        CodingAgentChangeState::Proposed {
            requires_restart: false,
        },
        CodingAgentChangeState::Proposed {
            requires_restart: true,
        },
    ];
    states.extend(
        UnproposedReason::ALL.map(|r| CodingAgentChangeState::Unproposed { reason: Some(r) }),
    );
    for state in states {
        let (reason, restart) = match state {
            CodingAgentChangeState::Unproposed { reason } => (reason.map(|r| r.as_str()), false),
            CodingAgentChangeState::Proposed { requires_restart } => (None, requires_restart),
            CodingAgentChangeState::None => (None, false),
        };
        assert_eq!(
            CodingAgentChangeState::from_columns(state.kind().as_str(), reason, restart),
            state
        );
    }
}

/// The home thread outranks everything, the thread's own blocker outranks a
/// descendant's, and the enum's order picks the strongest descendant.
#[test]
fn action_blocker_picks_one_reason_in_priority_order() {
    use OwnBlocker::*;
    assert_eq!(action_blocker(Some(Running), true, None), Blocker::Home);
    assert_eq!(
        action_blocker(Some(PendingChange), false, Some(Running)),
        Blocker::PendingChange
    );
    assert_eq!(
        action_blocker(
            None,
            false,
            [Some(PendingChange), Some(Question)]
                .into_iter()
                .flatten()
                .min()
        ),
        Blocker::DescendantQuestion
    );
    assert_eq!(
        action_blocker(None, false, Some(Running)),
        Blocker::DescendantRunning
    );
    assert_eq!(action_blocker(None, false, None), Blocker::None);
}

/// A descendant blocker's slug is its own blocker's slug behind `descendant_`.
/// The generated TS builds the descendant slug that way, so this pins it.
#[test]
fn descendant_blocker_slugs_prefix_the_own_slugs() {
    for own in OwnBlocker::ALL {
        assert_eq!(
            action_blocker(Some(own), false, None).as_str(),
            own.as_str()
        );
        assert_eq!(
            action_blocker(None, false, Some(own)).as_str(),
            format!("descendant_{}", own.as_str())
        );
    }
}
