use super::*;

// 16. running_status_maps_to_current
#[test]
fn running_status_maps_to_current() {
    assert_eq!(
        display_section(
            ArchiveState::Archived,
            ThreadStatus::Running,
            false,
            false,
            false,
            false
        ),
        DisplaySection::Current
    );
}

// 17. inbox_maps_to_current
#[test]
fn inbox_maps_to_current() {
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::Idle,
            false,
            false,
            false,
            false
        ),
        DisplaySection::Current
    );
}

// 18. saved_default_idle_maps_to_saved
#[test]
fn saved_default_idle_maps_to_saved() {
    assert_eq!(
        display_section(
            ArchiveState::Archived,
            ThreadStatus::Idle,
            true,
            false,
            false,
            false
        ),
        DisplaySection::Saved
    );
}

// 19. saved_overrides_running — under the new routing saved wins everything
#[test]
fn saved_overrides_running_to_saved() {
    assert_eq!(
        display_section(
            ArchiveState::Archived,
            ThreadStatus::Running,
            true,
            false,
            false,
            false
        ),
        DisplaySection::Saved
    );
}

// 20. archived_idle_not_saved_maps_to_archive
#[test]
fn archived_idle_not_saved_maps_to_archive() {
    assert_eq!(
        display_section(
            ArchiveState::Archived,
            ThreadStatus::Idle,
            false,
            false,
            false,
            false
        ),
        DisplaySection::Archive
    );
}

// 21. active_children_idle_maps_to_current — running/waiting/delegated child work all surface in Current
#[test]
fn active_children_idle_maps_to_current() {
    assert_eq!(
        display_section(
            ArchiveState::Archived,
            ThreadStatus::Idle,
            false,
            true,
            false,
            false
        ),
        DisplaySection::Current
    );
}

// 22. active_children_saved_still_saved — save overrides delegated work
#[test]
fn active_children_saved_still_saved() {
    assert_eq!(
        display_section(
            ArchiveState::Archived,
            ThreadStatus::Idle,
            true,
            true,
            false,
            false
        ),
        DisplaySection::Saved
    );
}

// 22a. running_with_active_children_stays_current
#[test]
fn running_with_active_children_stays_current() {
    assert_eq!(
        display_section(
            ArchiveState::Archived,
            ThreadStatus::Running,
            false,
            true,
            false,
            false
        ),
        DisplaySection::Current
    );
}

// 22b. inbox_with_active_children_maps_to_current
#[test]
fn inbox_with_active_children_maps_to_current() {
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::Idle,
            false,
            true,
            false,
            false
        ),
        DisplaySection::Current
    );
}

// 22c. inbox_idle_no_children_maps_to_current
#[test]
fn inbox_idle_no_children_maps_to_current() {
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::Idle,
            false,
            false,
            false,
            false
        ),
        DisplaySection::Current
    );
}

// 22d. archived_with_pending_changes_routes_to_current — pending changes
//      outrank archive so users can never lose unresolved work behind the
//      archive curtain. Once the user resolves all pending changes, the
//      thread settles into Archive (covered by 20).
#[test]
fn archived_with_pending_changes_routes_to_current() {
    assert_eq!(
        display_section(
            ArchiveState::Archived,
            ThreadStatus::Idle,
            false,
            false,
            true,
            false
        ),
        DisplaySection::Current
    );
}

// 22e. saved_overrides_pending — saving is still the strongest claim;
//      a pending change on a saved thread surfaces via the Saved section's
//      CTA badge, not by overriding routing.
#[test]
fn saved_overrides_pending() {
    assert_eq!(
        display_section(
            ArchiveState::Archived,
            ThreadStatus::Idle,
            true,
            false,
            true,
            false
        ),
        DisplaySection::Saved
    );
}

// 22f. running_overrides_pending — running keeps it in Current so live work isn't masked
//      by a pending change row carried in from a previous turn.
#[test]
fn running_overrides_pending() {
    assert_eq!(
        display_section(
            ArchiveState::Archived,
            ThreadStatus::Running,
            false,
            false,
            true,
            false
        ),
        DisplaySection::Current
    );
}

// 22g. attention_descendant_overrides_active — attention-bubble rule: if any
//      descendant needs user attention (WFUA, or CC with pending changes),
//      the parent surfaces in Current even when sibling work is still running
//      locally OR via has_active_children. Otherwise the permission card
//      would only be reachable via the child's own row.
#[test]
fn attention_descendant_overrides_active() {
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::Idle,
            false,
            true, // has_active_children
            false,
            true, // has_attention_descendants
        ),
        DisplaySection::Current
    );
}

// 22h. attention_descendant_overrides_running — even when the thread is
//      mid-turn (Running), an attention-needing descendant still bubbles
//      to Current. Symmetric with 22g via the local-Running route.
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
        ),
        DisplaySection::Current
    );
}

// 22i. attention_descendant_overrides_archive — same archive-curtain rule
//      as has_pending_changes: a thread the user archived must still
//      surface in Current while a descendant needs attention, so the
//      attention card isn't lost behind the curtain.
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
        ),
        DisplaySection::Current
    );
}

// 22j. attention_descendant_loses_to_saved — Saved still wins. The
//      attention card surfaces via the saved section's CTA, not by
//      overriding routing.
#[test]
fn attention_descendant_loses_to_saved() {
    assert_eq!(
        display_section(
            ArchiveState::Inbox,
            ThreadStatus::Idle,
            true, // is_saved
            false,
            false,
            true,
        ),
        DisplaySection::Saved
    );
}

// ── is_attention_needing predicate ── pinned per branch so any future
// rewrite (or any drift between this predicate and its SQL mirrors in
// migrations / `event_bus_projection.rs`) fails here at the unit layer.

#[test]
fn is_attention_needing_wfua_is_attention_regardless_of_archive() {
    // Mirror of `is_blocking`: WFUA always wins, even on an archived row.
    for archive in [ArchiveState::Inbox, ArchiveState::Archived] {
        for ttype in [ThreadType::Chat, ThreadType::CodingAgent] {
            assert!(
                is_attention_needing(
                    ttype,
                    ThreadStatus::WaitingForUserAnswer,
                    archive,
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
fn is_attention_needing_archived_short_circuits_pending_changes() {
    // Critical guard: an archived CC thread with pending changes is NOT
    // attention-needing — `is_blocking` says the same (clause 2 returns
    // false). The migration backfill SQL and the runtime SQL must agree
    // — the drift caught by `/harden` was exactly this case missing in
    // the migration's CTE filter.
    assert!(
        !is_attention_needing(
            ThreadType::CodingAgent,
            ThreadStatus::Idle,
            ArchiveState::Archived,
            true,  // has_pending_changes
            false, // is_external_repo
        ),
        "Archived + pending_changes must return false — matches is_blocking"
    );
}

#[test]
fn is_attention_needing_in_workspace_cc_pending_changes_is_attention() {
    // Non-archived in-workspace CC with a pending change is the canonical
    // REVIEW-bubble case: user must Apply or Discard.
    assert!(is_attention_needing(
        ThreadType::CodingAgent,
        ThreadStatus::Idle,
        ArchiveState::Inbox,
        true,
        false,
    ));
}

#[test]
fn is_attention_needing_external_repo_carve_out() {
    // External-repo CC with pending changes is NOT attention-needing —
    // the WaitingBanner shows Archive instead of Apply/Discard, so
    // there's nothing for the user to resolve at the parent level.
    // Mirrors `is_blocking` clause 3.
    assert!(!is_attention_needing(
        ThreadType::CodingAgent,
        ThreadStatus::Idle,
        ArchiveState::Inbox,
        true, // has_pending_changes
        true, // is_external_repo
    ));
}

#[test]
fn is_attention_needing_chat_pending_changes_does_not_apply() {
    // `has_pending_changes` is CC-only — a chat thread with the flag
    // accidentally set (shouldn't happen, but the predicate is strict)
    // does not bubble attention.
    assert!(!is_attention_needing(
        ThreadType::Chat,
        ThreadStatus::Idle,
        ArchiveState::Inbox,
        true,
        false,
    ));
}

#[test]
fn is_attention_needing_running_is_not_attention() {
    // Running is delegated work (Active), not user attention (Review).
    // This is the key distinction from `is_blocking`.
    assert!(!is_attention_needing(
        ThreadType::CodingAgent,
        ThreadStatus::Running,
        ArchiveState::Inbox,
        false,
        false,
    ));
}

#[test]
fn is_attention_needing_subset_of_is_blocking() {
    // Invariant: `is_attention_needing` is a STRICT SUBSET of `is_blocking`.
    // For every combination of inputs: attention => blocking. Pinning this
    // prevents drift across the two predicates as either evolves.
    for status in [
        ThreadStatus::Idle,
        ThreadStatus::Running,
        ThreadStatus::WaitingForUserAnswer,
    ] {
        for archive in [ArchiveState::Inbox, ArchiveState::Archived] {
            for ttype in [ThreadType::Chat, ThreadType::CodingAgent] {
                for pending in [false, true] {
                    for external in [false, true] {
                        let attention =
                            is_attention_needing(ttype, status, archive, pending, external);
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

// ── available_thread_actions tests ──
//
// Returned in cascade priority order so the front-most close LAYER is
// positional: [DiscardDraft?, Discard?, Apply?, Archive?, Unsave|Save].
// The Save/Unsave retention toggle always appends exactly one entry for a
// focused thread (matches the always-present prompt section toggle), so the
// close set is the prefix before it.
//
// Args: (thread_type, status, stored_section, has_pending_changes,
//        descendants_block_archive, has_unsent_draft, is_saved).

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
    );
    assert_eq!(actions, vec![Action::Archive, Action::Save]);
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
    );
    assert_eq!(actions, vec![Action::Save]);
}

#[test]
fn chat_running_shows_no_close_actions() {
    // Live thread: no close actions (draft excluded here), Save still offered.
    let actions = available_thread_actions(
        ThreadType::Chat,
        ThreadStatus::Running,
        ArchiveState::Inbox,
        false,
        false,
        false,
        false,
    );
    assert_eq!(actions, vec![Action::Save]);
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
    );
    assert_eq!(actions, vec![Action::Discard, Action::Apply, Action::Save]);
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
    );
    assert_eq!(actions, vec![Action::Archive, Action::Save]);
}

#[test]
fn external_repo_cc_no_pending_change_shows_archive() {
    // External repo threads: coding_agent_proposed=true in thread_summaries but no
    // pending row in the changes table (runtime skips propose_change).
    // has_pending_changes=false → Archive, not Apply/Discard.
    let actions = available_thread_actions(
        ThreadType::CodingAgent,
        ThreadStatus::Waiting,
        ArchiveState::Inbox,
        false,
        false,
        false,
        false,
    );
    assert_eq!(
        actions,
        vec![Action::Archive, Action::Save],
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
    );
    assert_eq!(actions, vec![Action::Save]);
}

#[test]
fn cc_archived_with_pending_changes_shows_apply_discard() {
    // display_section surfaces archived+pending threads in Current specifically
    // so the user can never lose unresolved work behind the archive curtain.
    // available_thread_actions must keep the action set in sync — otherwise the
    // user sees the thread in Current with dots but has no buttons to resolve it.
    let actions = available_thread_actions(
        ThreadType::CodingAgent,
        ThreadStatus::Waiting,
        ArchiveState::Archived,
        true,
        false,
        false,
        false,
    );
    assert_eq!(actions, vec![Action::Discard, Action::Apply, Action::Save]);
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
    );
    assert_eq!(
        actions,
        vec![Action::Save],
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
    );
    assert_eq!(
        actions,
        vec![Action::Save],
        "Idle CC with blocking descendants must not surface Archive"
    );
}

#[test]
fn cc_inbox_pending_changes_still_show_apply_discard_when_descendants_block() {
    // Pending changes outrank the descendants-block-archive gate: the user
    // must resolve the change before the cascading-archive logic kicks in.
    let actions = available_thread_actions(
        ThreadType::CodingAgent,
        ThreadStatus::Waiting,
        ArchiveState::Inbox,
        true,
        true,
        false,
        false,
    );
    assert_eq!(actions, vec![Action::Discard, Action::Apply, Action::Save]);
}

// ── new axes: draft + save/unsave ──

#[test]
fn draft_is_front_most_close_layer() {
    // DiscardDraft leads the close set; Archive still follows when archivable.
    let actions = available_thread_actions(
        ThreadType::Chat,
        ThreadStatus::Idle,
        ArchiveState::Inbox,
        false,
        false,
        true, // has_unsent_draft
        false,
    );
    assert_eq!(
        actions,
        vec![Action::DiscardDraft, Action::Archive, Action::Save]
    );
}

#[test]
fn draft_discard_available_while_running() {
    // A draft is orthogonal to run state — discardable even while the thread
    // is live, when the close set is otherwise suppressed.
    let actions = available_thread_actions(
        ThreadType::Chat,
        ThreadStatus::Running,
        ArchiveState::Inbox,
        false,
        false,
        true, // has_unsent_draft
        false,
    );
    assert_eq!(actions, vec![Action::DiscardDraft, Action::Save]);
}

#[test]
fn full_cascade_draft_change_then_save() {
    // Draft + pending change: DiscardDraft then Discard/Apply (the change layer
    // suppresses Archive), then the Save toggle.
    let actions = available_thread_actions(
        ThreadType::CodingAgent,
        ThreadStatus::Waiting,
        ArchiveState::Inbox,
        true,
        false,
        true, // has_unsent_draft
        false,
    );
    assert_eq!(
        actions,
        vec![
            Action::DiscardDraft,
            Action::Discard,
            Action::Apply,
            Action::Save
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
        true, // is_saved
    );
    assert_eq!(actions, vec![Action::Archive, Action::Unsave]);
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
        true, // is_saved
    );
    assert_eq!(
        actions,
        vec![Action::Discard, Action::Apply, Action::Unsave]
    );
}

// ── a subscription is not a park ── a thread holding an *event wait* is
// plain idle: it does not block, it needs no attention, and Archive stays
// offered. That is the 2026-08-06 change (every wait is detached), and the
// blocking predicate has two SQL mirrors
// (`event_bus_projection_propagation.rs`), so pin the relation here.

/// The documented relation in `is_attention_needing`'s doc comment, asserted
/// across every status rather than trusted as prose.
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
                        let attention =
                            is_attention_needing(ttype, status, archive, pending, external);
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

/// A subscribed thread is idle, so it keeps the ordinary idle action set,
/// Archive included. Archiving one is legitimate and is NOT a way to strand a
/// subscription behind the archive curtain: the archive cancels every live wait
/// on the thread (`EventWaitCancelCause::ThreadArchived`, applied off the bus in
/// `event_wait::dispatcher::cancel_waits_ended_by`).
#[test]
fn a_subscribed_thread_is_idle_and_offers_archive() {
    let actions = available_thread_actions(
        ThreadType::Chat,
        ThreadStatus::Idle,
        ArchiveState::Inbox,
        false,
        false,
        false,
        false,
    );
    assert_eq!(actions, vec![Action::Archive, Action::Save]);
}
