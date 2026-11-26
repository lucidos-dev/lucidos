use super::*;

#[test]
fn running_status_maps_to_current() {
    assert_eq!(
        display_section(
            ArchiveState::Archived,
            ThreadStatus::Running,
            false,
            false,
            false,
            false,
            false
        ),
        DisplaySection::Current
    );
}

#[test]
fn inbox_maps_to_current() {
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::Idle,
            false,
            false,
            false,
            false,
            false
        ),
        DisplaySection::Current
    );
}

#[test]
fn saved_default_idle_maps_to_saved() {
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::Idle,
            true,
            false,
            false,
            false,
            false
        ),
        DisplaySection::Saved
    );
}

#[test]
fn saved_overrides_running_to_saved() {
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::Running,
            true,
            false,
            false,
            false,
            false
        ),
        DisplaySection::Saved
    );
}

#[test]
fn archived_idle_not_saved_maps_to_archive() {
    assert_eq!(
        display_section(
            ArchiveState::Archived,
            ThreadStatus::Idle,
            false,
            false,
            false,
            false,
            false
        ),
        DisplaySection::Archive
    );
}

#[test]
fn active_children_idle_maps_to_current() {
    assert_eq!(
        display_section(
            ArchiveState::Archived,
            ThreadStatus::Idle,
            false,
            true,
            false,
            false,
            false
        ),
        DisplaySection::Current
    );
}

#[test]
fn active_children_saved_still_saved() {
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::Idle,
            true,
            true,
            false,
            false,
            false
        ),
        DisplaySection::Saved
    );
}

#[test]
fn running_with_active_children_stays_current() {
    assert_eq!(
        display_section(
            ArchiveState::Archived,
            ThreadStatus::Running,
            false,
            true,
            false,
            false,
            false
        ),
        DisplaySection::Current
    );
}

#[test]
fn inbox_with_active_children_maps_to_current() {
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::Idle,
            false,
            true,
            false,
            false,
            false
        ),
        DisplaySection::Current
    );
}

#[test]
fn inbox_idle_no_children_maps_to_current() {
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::Idle,
            false,
            false,
            false,
            false,
            false
        ),
        DisplaySection::Current
    );
}

#[test]
fn archived_with_pending_changes_routes_to_current() {
    assert_eq!(
        display_section(
            ArchiveState::Archived,
            ThreadStatus::Idle,
            false,
            false,
            true,
            false,
            false
        ),
        DisplaySection::Current
    );
}

#[test]
fn saved_overrides_pending() {
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::Idle,
            true,
            false,
            true,
            false,
            false
        ),
        DisplaySection::Saved
    );
}

#[test]
fn running_overrides_pending() {
    assert_eq!(
        display_section(
            ArchiveState::Archived,
            ThreadStatus::Running,
            false,
            false,
            true,
            false,
            false
        ),
        DisplaySection::Current
    );
}

#[test]
fn attention_descendant_overrides_active() {
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::Idle,
            false,
            true,
            false,
            true,
            false
        ),
        DisplaySection::Current
    );
}

#[test]
fn attention_descendant_overrides_running() {
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::Running,
            false,
            false,
            false,
            true,
            false,
        ),
        DisplaySection::Current
    );
}

#[test]
fn attention_descendant_overrides_archive() {
    assert_eq!(
        display_section(
            ArchiveState::Archived,
            ThreadStatus::Idle,
            false,
            false,
            false,
            true,
            false,
        ),
        DisplaySection::Current
    );
}

#[test]
fn attention_descendant_loses_to_saved() {
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::Idle,
            true,
            false,
            false,
            true,
            false,
        ),
        DisplaySection::Saved
    );
}

#[test]
fn is_attention_needing_wfua_is_attention_regardless_of_archive() {
    for archive in [ArchiveState::Inbox, ArchiveState::Archived] {
        for ttype in [ThreadType::Chat, ThreadType::CodingAgent] {
            assert!(
                is_attention_needing(
                    ttype,
                    ThreadStatus::WaitingForUserAnswer,
                    archive,
                    false,
                    false,
                    false,
                    false,
                ),
                "WFUA must be attention-needing (type={:?}, archive={:?})",
                ttype,
                archive,
            );
        }
    }
}

#[test]
fn a_held_waiting_thread_needs_no_attention_yet_still_blocks() {
    for ttype in [ThreadType::Chat, ThreadType::CodingAgent] {
        assert!(!is_attention_needing(
            ttype,
            ThreadStatus::WaitingForUserAnswer,
            ArchiveState::Inbox,
            false,
            false,
            false,
            true,
        ));
        assert!(is_blocking(
            ttype,
            ThreadStatus::WaitingForUserAnswer,
            ArchiveState::Inbox,
            false,
            false,
        ));
    }
    assert!(
        is_attention_needing(
            ThreadType::CodingAgent,
            ThreadStatus::WaitingForUserAnswer,
            ArchiveState::Inbox,
            true,
            false,
            false,
            true,
        ),
        "a held thread's pending change still needs the owner"
    );
}

#[test]
fn is_attention_needing_archived_short_circuits_pending_changes() {
    assert!(
        !is_attention_needing(
            ThreadType::CodingAgent,
            ThreadStatus::Idle,
            ArchiveState::Archived,
            true,
            false,
            false,
            false,
        ),
        "Archived + pending_changes must return false — matches is_blocking"
    );
}

#[test]
fn is_attention_needing_in_workspace_cc_pending_changes_is_attention() {
    assert!(is_attention_needing(
        ThreadType::CodingAgent,
        ThreadStatus::Idle,
        ArchiveState::Inbox,
        true,
        false,
        false,
        false,
    ));
}

#[test]
fn is_attention_needing_external_repo_carve_out() {
    assert!(!is_attention_needing(
        ThreadType::CodingAgent,
        ThreadStatus::Idle,
        ArchiveState::Inbox,
        true,
        true,
        false,
        false,
    ));
}

#[test]
fn is_attention_needing_chat_pending_changes_does_not_apply() {
    assert!(!is_attention_needing(
        ThreadType::Chat,
        ThreadStatus::Idle,
        ArchiveState::Inbox,
        true,
        false,
        false,
        false,
    ));
}

#[test]
fn is_attention_needing_running_is_not_attention() {
    assert!(!is_attention_needing(
        ThreadType::CodingAgent,
        ThreadStatus::Running,
        ArchiveState::Inbox,
        false,
        false,
        false,
        false,
    ));
}

#[test]
fn is_attention_needing_subset_of_is_blocking() {
    for status in [
        ThreadStatus::Idle,
        ThreadStatus::Running,
        ThreadStatus::WaitingForUserAnswer,
    ] {
        for archive in [ArchiveState::Inbox, ArchiveState::Archived] {
            for ttype in [ThreadType::Chat, ThreadType::CodingAgent] {
                for pending in [false, true] {
                    for external in [false, true] {
                        let attention = is_attention_needing(
                            ttype, status, archive, pending, external, false, false,
                        );
                        let blocking = is_blocking(ttype, status, archive, pending, external);
                        if attention {
                            assert!(
                                blocking,
                                "subset invariant broken: attention=true but blocking=false \
                                 for (type={:?}, status={:?}, archive={:?}, pending={}, external={})",
                                ttype, status, archive, pending, external,
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn a_stopped_child_needs_attention_and_never_blocks() {
    for ttype in [ThreadType::Chat, ThreadType::CodingAgent] {
        assert!(is_attention_needing(
            ttype,
            ThreadStatus::Idle,
            ArchiveState::Inbox,
            false,
            false,
            true,
            false,
        ));
        assert!(!is_attention_needing(
            ttype,
            ThreadStatus::Idle,
            ArchiveState::Archived,
            false,
            false,
            true,
            false,
        ));
        assert!(!is_blocking(
            ttype,
            ThreadStatus::Idle,
            ArchiveState::Inbox,
            false,
            false,
        ));
    }
}

#[test]
fn chat_inbox_idle_shows_archive_then_save() {
    let actions = available_thread_actions(
        ThreadType::Chat,
        ThreadStatus::Idle,
        ArchiveState::Inbox,
        false,
        false,
        false,
        false,
        false,
    );
    assert_eq!(actions, vec![ThreadAction::Archive, ThreadAction::Save]);
}

#[test]
fn chat_archived_idle_shows_only_save() {
    let actions = available_thread_actions(
        ThreadType::Chat,
        ThreadStatus::Idle,
        ArchiveState::Archived,
        false,
        false,
        false,
        false,
        false,
    );
    assert_eq!(actions, vec![ThreadAction::Save]);
}

#[test]
fn chat_running_shows_no_close_actions() {
    let actions = available_thread_actions(
        ThreadType::Chat,
        ThreadStatus::Running,
        ArchiveState::Inbox,
        false,
        false,
        false,
        false,
        false,
    );
    assert_eq!(actions, vec![ThreadAction::Save]);
}

#[test]
fn cc_inbox_with_changes_shows_apply_discard() {
    let actions = available_thread_actions(
        ThreadType::CodingAgent,
        ThreadStatus::Waiting,
        ArchiveState::Inbox,
        true,
        false,
        false,
        false,
        false,
    );
    assert_eq!(
        actions,
        vec![
            ThreadAction::Discard,
            ThreadAction::Apply,
            ThreadAction::Hold,
            ThreadAction::Save
        ]
    );
}

#[test]
fn cc_inbox_no_changes_shows_archive() {
    let actions = available_thread_actions(
        ThreadType::CodingAgent,
        ThreadStatus::Waiting,
        ArchiveState::Inbox,
        false,
        false,
        false,
        false,
        false,
    );
    assert_eq!(actions, vec![ThreadAction::Archive, ThreadAction::Save]);
}

#[test]
fn external_repo_cc_no_pending_change_shows_archive() {
    let actions = available_thread_actions(
        ThreadType::CodingAgent,
        ThreadStatus::Waiting,
        ArchiveState::Inbox,
        false,
        false,
        false,
        false,
        false,
    );
    assert_eq!(
        actions,
        vec![ThreadAction::Archive, ThreadAction::Save],
        "External repo threads without pending changes must show Archive, not Apply/Discard"
    );
}

#[test]
fn cc_archived_no_changes_shows_only_save() {
    let actions = available_thread_actions(
        ThreadType::CodingAgent,
        ThreadStatus::Idle,
        ArchiveState::Archived,
        false,
        false,
        false,
        false,
        false,
    );
    assert_eq!(actions, vec![ThreadAction::Save]);
}

#[test]
fn cc_archived_with_pending_changes_shows_apply_discard() {
    let actions = available_thread_actions(
        ThreadType::CodingAgent,
        ThreadStatus::Waiting,
        ArchiveState::Archived,
        true,
        false,
        false,
        false,
        false,
    );
    assert_eq!(
        actions,
        vec![
            ThreadAction::Discard,
            ThreadAction::Apply,
            ThreadAction::Hold,
            ThreadAction::Save
        ]
    );
}

#[test]
fn chat_inbox_descendants_block_archive_hides_archive() {
    let actions = available_thread_actions(
        ThreadType::Chat,
        ThreadStatus::Idle,
        ArchiveState::Inbox,
        false,
        true,
        false,
        false,
        false,
    );
    assert_eq!(
        actions,
        vec![ThreadAction::Save],
        "Idle chat with blocking descendants must not surface Archive"
    );
}

#[test]
fn cc_inbox_descendants_block_archive_hides_archive() {
    let actions = available_thread_actions(
        ThreadType::CodingAgent,
        ThreadStatus::Idle,
        ArchiveState::Inbox,
        false,
        true,
        false,
        false,
        false,
    );
    assert_eq!(
        actions,
        vec![ThreadAction::Save],
        "Idle CC with blocking descendants must not surface Archive"
    );
}

#[test]
fn cc_inbox_pending_changes_still_show_apply_discard_when_descendants_block() {
    let actions = available_thread_actions(
        ThreadType::CodingAgent,
        ThreadStatus::Waiting,
        ArchiveState::Inbox,
        true,
        true,
        false,
        false,
        false,
    );
    assert_eq!(
        actions,
        vec![
            ThreadAction::Discard,
            ThreadAction::Apply,
            ThreadAction::Hold,
            ThreadAction::Save
        ]
    );
}

#[test]
fn draft_is_front_most_close_layer() {
    let actions = available_thread_actions(
        ThreadType::Chat,
        ThreadStatus::Idle,
        ArchiveState::Inbox,
        false,
        false,
        false,
        true,
        false,
    );
    assert_eq!(
        actions,
        vec![
            ThreadAction::DiscardDraft,
            ThreadAction::Archive,
            ThreadAction::Save
        ]
    );
}

#[test]
fn draft_discard_available_while_running() {
    let actions = available_thread_actions(
        ThreadType::Chat,
        ThreadStatus::Running,
        ArchiveState::Inbox,
        false,
        false,
        false,
        true,
        false,
    );
    assert_eq!(
        actions,
        vec![ThreadAction::DiscardDraft, ThreadAction::Save]
    );
}

#[test]
fn full_cascade_draft_change_then_save() {
    let actions = available_thread_actions(
        ThreadType::CodingAgent,
        ThreadStatus::Waiting,
        ArchiveState::Inbox,
        true,
        false,
        false,
        true,
        false,
    );
    assert_eq!(
        actions,
        vec![
            ThreadAction::DiscardDraft,
            ThreadAction::Discard,
            ThreadAction::Apply,
            ThreadAction::Hold,
            ThreadAction::Save
        ]
    );
}

#[test]
fn saved_thread_shows_unsave_not_save() {
    let actions = available_thread_actions(
        ThreadType::Chat,
        ThreadStatus::Idle,
        ArchiveState::Inbox,
        false,
        false,
        false,
        false,
        true,
    );
    assert_eq!(actions, vec![ThreadAction::Archive, ThreadAction::Unsave]);
}

#[test]
fn saved_cc_pending_shows_unsave() {
    let actions = available_thread_actions(
        ThreadType::CodingAgent,
        ThreadStatus::Waiting,
        ArchiveState::Inbox,
        true,
        false,
        false,
        false,
        true,
    );
    assert_eq!(
        actions,
        vec![
            ThreadAction::Discard,
            ThreadAction::Apply,
            ThreadAction::Hold,
            ThreadAction::Unsave
        ]
    );
}

#[test]
fn blocking_equals_attention_or_running() {
    for status in [
        ThreadStatus::Idle,
        ThreadStatus::Running,
        ThreadStatus::Waiting,
        ThreadStatus::WaitingForUserAnswer,
        ThreadStatus::Paused,
        ThreadStatus::Failed,
    ] {
        for archive in [ArchiveState::Inbox, ArchiveState::Archived] {
            for ttype in [ThreadType::Chat, ThreadType::CodingAgent] {
                for pending in [false, true] {
                    for external in [false, true] {
                        let blocking = is_blocking(ttype, status, archive, pending, external);
                        let attention = is_attention_needing(
                            ttype, status, archive, pending, external, false, false,
                        );
                        let expected = attention || status == ThreadStatus::Running;
                        assert_eq!(
                            blocking, expected,
                            "is_blocking must equal is_attention_needing OR Running \
                             (status={:?}, type={:?}, archive={:?}, pending={}, external={})",
                            status, ttype, archive, pending, external,
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn a_subscribed_thread_is_idle_and_offers_archive() {
    let actions = available_thread_actions(
        ThreadType::Chat,
        ThreadStatus::Idle,
        ArchiveState::Inbox,
        false,
        false,
        true,
        false,
        false,
    );
    assert_eq!(actions, vec![ThreadAction::Archive, ThreadAction::Save]);
}

#[test]
fn a_parked_cc_thread_with_a_change_offers_what_a_running_one_does() {
    let parked = available_thread_actions(
        ThreadType::CodingAgent,
        ThreadStatus::Idle,
        ArchiveState::Inbox,
        true,
        false,
        true,
        false,
        false,
    );
    assert_eq!(
        parked,
        vec![ThreadAction::ApplyWhenSettled, ThreadAction::Save]
    );

    let running = available_thread_actions(
        ThreadType::CodingAgent,
        ThreadStatus::Running,
        ArchiveState::Inbox,
        true,
        false,
        false,
        false,
        false,
    );
    assert_eq!(
        running,
        vec![ThreadAction::ApplyWhenSettled, ThreadAction::Save],
        "a Running thread offers no close action either, plus the standing apply"
    );
}

#[test]
fn clearing_the_last_wait_restores_apply_and_discard() {
    let actions = available_thread_actions(
        ThreadType::CodingAgent,
        ThreadStatus::Idle,
        ArchiveState::Inbox,
        true,
        false,
        false,
        false,
        false,
    );
    assert_eq!(
        actions,
        vec![
            ThreadAction::Discard,
            ThreadAction::Apply,
            ThreadAction::Hold,
            ThreadAction::Save
        ]
    );
}

#[test]
fn a_parked_thread_can_still_discard_its_unsent_draft() {
    let actions = available_thread_actions(
        ThreadType::CodingAgent,
        ThreadStatus::Idle,
        ArchiveState::Inbox,
        true,
        false,
        true,
        true,
        false,
    );
    assert_eq!(
        actions,
        vec![
            ThreadAction::DiscardDraft,
            ThreadAction::ApplyWhenSettled,
            ThreadAction::Save
        ]
    );
}

#[test]
fn a_live_wait_beside_a_question_or_a_failure_offers_no_standing_apply() {
    for status in [ThreadStatus::WaitingForUserAnswer, ThreadStatus::Failed] {
        let actions = available_thread_actions(
            ThreadType::CodingAgent,
            status,
            ArchiveState::Inbox,
            true,
            false,
            true,
            false,
            false,
        );
        assert!(
            !actions.contains(&ThreadAction::ApplyWhenSettled),
            "{status:?} with a live wait must not offer a standing apply: {actions:?}"
        );
    }
}

#[test]
fn a_parked_chat_thread_keeps_archive() {
    let actions = available_thread_actions(
        ThreadType::Chat,
        ThreadStatus::Idle,
        ArchiveState::Inbox,
        true,
        false,
        true,
        false,
        false,
    );
    assert_eq!(actions, vec![ThreadAction::Archive, ThreadAction::Save]);
}

#[test]
fn a_thread_on_hold_rests_in_on_hold_whatever_it_carries() {
    for status in [ThreadStatus::Running, ThreadStatus::Idle] {
        assert_eq!(
            display_section(ArchiveState::Inbox, status, false, true, true, true, true),
            DisplaySection::OnHold
        );
    }
}

#[test]
fn a_pinned_thread_on_hold_stays_pinned() {
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::Idle,
            true,
            false,
            false,
            false,
            true
        ),
        DisplaySection::Saved
    );
}

#[test]
fn awaits_owner_answer_matches_the_blocked_sections_question_arm() {
    for thread_type in [ThreadType::Chat, ThreadType::CodingAgent] {
        for section in [ArchiveState::Archived, ArchiveState::Inbox] {
            for unattended in [false, true] {
                let moved =
                    resolve_transition("UserQuestionAsked", thread_type, section, unattended)
                        .expect("a question is legal from any section");
                assert_eq!(moved.new_section, Some(ArchiveState::Inbox));
            }
        }
    }
    for status in ThreadStatus::ALL {
        for bits in 0..16u8 {
            let [saved, children, pending, attention] = [0, 1, 2, 3].map(|i| bits & (1 << i) != 0);
            for held in [false, true] {
                let section = display_section(
                    ArchiveState::Inbox,
                    status,
                    saved,
                    children,
                    pending,
                    attention,
                    held,
                );
                let lists_in_ongoing =
                    !held && matches!(section, DisplaySection::Current | DisplaySection::Saved);
                assert_eq!(
                    awaits_owner_answer(status, held),
                    status == ThreadStatus::WaitingForUserAnswer && lists_in_ongoing,
                    "{status:?} held={held} section={section:?}"
                );
            }
        }
    }
}
