use std::collections::HashSet;
use std::path::PathBuf;

use crate::engine::agent_session::resume::deterministic_worktree_path;
use crate::engine::git_ops::{is_external_repo_path, worktrees_dir};

#[test]
fn lost_session_path_uses_deterministic_path_when_thread_mapped() {
    let workspace = PathBuf::from("/tmp/ws");
    let thread_id = uuid::Uuid::new_v4();
    let path = super::lost_session_worktree_path(&workspace, Some(thread_id));
    assert_eq!(path, deterministic_worktree_path(&workspace, thread_id));
}

#[test]
fn lost_session_path_falls_back_to_cc_random_when_unmapped() {
    let workspace = PathBuf::from("/tmp/ws");
    let path = super::lost_session_worktree_path(&workspace, None);
    let parent = path
        .parent()
        .expect("path has a parent (the worktrees dir)");
    assert_eq!(parent, worktrees_dir(&workspace));
    let name = path
        .file_name()
        .expect("path has a file name")
        .to_string_lossy();
    assert!(
        name.starts_with("cc-"),
        "unmapped branches keep the legacy random name; got {}",
        name
    );
    assert!(
        !name.starts_with("thread-"),
        "must NOT use the deterministic thread- prefix without a real thread id; got {}",
        name
    );
}

/// Regression: recovery must classify the workspace's own Lucidos worktree
/// as internal, even when the marker file contains a `repo_id` for it.
/// Pre-fix, the recovery sweep keyed on `marker_repo_id.is_some()` and
/// skipped `propose_branch_changes` for crashed Claude Code sessions, leaving real
/// commits with `coding_agent_proposed=false`.
#[test]
fn lucidos_repo_worktree_is_not_external() {
    let lucidos = PathBuf::from("/Users/me/IdeaProjects/lucidos");
    assert!(!is_external_repo_path(&lucidos, &lucidos));
}

#[test]
fn external_repo_worktree_is_external() {
    let lucidos = PathBuf::from("/Users/me/IdeaProjects/lucidos");
    let external = PathBuf::from("/Users/me/IdeaProjects/some-other-repo");
    assert!(is_external_repo_path(&external, &lucidos));
}

/// Groups branch sets used to filter recovery candidates.
#[derive(Default)]
pub(crate) struct RecoveryFilter {
    pub pending: HashSet<String>,
    pub already_recovered: HashSet<String>,
    pub idle: HashSet<String>,
    pub merged: HashSet<String>,
    pub actively_running: HashSet<String>,
    pub completed_change: HashSet<String>,
    pub known: HashSet<String>,
    pub discovered: HashSet<String>,
}

/// Simulates the recovery filtering logic from `recover_orphaned_worktrees`.
/// Returns the branches that would be recovered (not skipped).
pub(crate) fn filter_recovery_candidates(
    candidates: &[(PathBuf, String)],
    f: &RecoveryFilter,
) -> Vec<String> {
    candidates
        .iter()
        .filter(|(_, branch)| {
            (!f.pending.contains(branch) || f.actively_running.contains(branch))
                && !f.already_recovered.contains(branch)
                && !f.idle.contains(branch)
                && !f.completed_change.contains(branch)
                && (!f.merged.contains(branch) || f.actively_running.contains(branch))
                && (f.known.contains(branch) || f.actively_running.contains(branch))
        })
        .map(|(_, branch)| branch.clone())
        .collect()
}

/// Identifies actively running branches whose worktrees were lost (not found
/// by the worktree scan). These need fresh worktrees created for recovery.
/// Returns branches that should get new worktrees.
pub(crate) fn find_worktreeless_active_branches(f: &RecoveryFilter) -> Vec<String> {
    f.actively_running
        .iter()
        .filter(|branch| {
            !f.discovered.contains(*branch)
                && !f.already_recovered.contains(*branch)
                && !f.completed_change.contains(*branch)
                && !f.idle.contains(*branch)
        })
        .cloned()
        .collect()
}

#[test]
fn lost_worktree_active_session_discovered_from_db() {
    // Bug: A Claude Code session was actively running when the engine restarted.
    // The worktree directory was cleaned up (macOS temp cleanup, git prune)
    // before the next engine started. The worktree scan finds nothing, but
    // the DB shows the session was actively running (last lifecycle event
    // is SessionStarted with no subsequent idle/end).
    // Without DB-based discovery, the session is silently lost.
    let result = find_worktreeless_active_branches(&RecoveryFilter {
        actively_running: HashSet::from(["claude-code/lost-worktree-active".to_string()]),
        ..Default::default()
    });
    assert_eq!(
        result,
        vec!["claude-code/lost-worktree-active".to_string()],
        "Actively running session with lost worktree must be discovered from DB"
    );
}

#[test]
fn lost_worktree_already_discovered_not_duplicated() {
    // Branch already found by worktree scan — don't create a second worktree.
    let result = find_worktreeless_active_branches(&RecoveryFilter {
        discovered: HashSet::from(["claude-code/has-worktree".to_string()]),
        actively_running: HashSet::from(["claude-code/has-worktree".to_string()]),
        ..Default::default()
    });
    assert!(
        result.is_empty(),
        "Branch already discovered via worktree scan must not get a duplicate worktree"
    );
}

#[test]
fn lost_worktree_already_recovered_skipped() {
    let result = find_worktreeless_active_branches(&RecoveryFilter {
        actively_running: HashSet::from(["claude-code/recovered-before".to_string()]),
        already_recovered: HashSet::from(["claude-code/recovered-before".to_string()]),
        ..Default::default()
    });
    assert!(
        result.is_empty(),
        "Branch already recovered must not be re-discovered"
    );
}

#[test]
fn lost_worktree_completed_change_skipped() {
    let result = find_worktreeless_active_branches(&RecoveryFilter {
        actively_running: HashSet::from(["claude-code/done".to_string()]),
        completed_change: HashSet::from(["claude-code/done".to_string()]),
        ..Default::default()
    });
    assert!(
        result.is_empty(),
        "Branch with completed change must not be re-discovered"
    );
}

#[test]
fn lost_worktree_mixed_scenario() {
    // Multiple branches with lost worktrees — mix of states.
    let result = find_worktreeless_active_branches(&RecoveryFilter {
        discovered: HashSet::from(["claude-code/has-worktree".to_string()]),
        actively_running: HashSet::from([
            "claude-code/has-worktree".to_string(),
            "claude-code/needs-recovery".to_string(),
            "claude-code/already-done".to_string(),
        ]),
        completed_change: HashSet::from(["claude-code/already-done".to_string()]),
        ..Default::default()
    });
    assert_eq!(
        result,
        vec!["claude-code/needs-recovery".to_string()],
        "Only the lost-worktree branch needing recovery should be returned"
    );
}

#[test]
fn idle_sessions_are_skipped_during_recovery() {
    let candidates = vec![
        (
            PathBuf::from("/tmp/wt-1"),
            "claude-code/20260317-100000".to_string(),
        ),
        (
            PathBuf::from("/tmp/wt-2"),
            "claude-code/20260317-110000".to_string(),
        ),
        (
            PathBuf::from("/tmp/wt-3"),
            "claude-code/20260317-120000".to_string(),
        ),
    ];

    // wt-2 was idle before restart — should be skipped
    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            idle: HashSet::from(["claude-code/20260317-110000".to_string()]),
            known: HashSet::from([
                "claude-code/20260317-100000".to_string(),
                "claude-code/20260317-110000".to_string(),
                "claude-code/20260317-120000".to_string(),
            ]),
            ..Default::default()
        },
    );
    assert_eq!(result.len(), 2);
    assert!(result.contains(&"claude-code/20260317-100000".to_string()));
    assert!(result.contains(&"claude-code/20260317-120000".to_string()));
    assert!(!result.contains(&"claude-code/20260317-110000".to_string()));
}

#[test]
fn all_skip_conditions_work_together() {
    let candidates = vec![
        (PathBuf::from("/tmp/wt-a"), "claude-code/a".to_string()), // has pending change
        (PathBuf::from("/tmp/wt-b"), "claude-code/b".to_string()), // already recovered
        (PathBuf::from("/tmp/wt-c"), "claude-code/c".to_string()), // was idle
        (PathBuf::from("/tmp/wt-d"), "claude-code/d".to_string()), // needs recovery
        (PathBuf::from("/tmp/wt-e"), "claude-code/e".to_string()), // already merged
    ];

    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            pending: HashSet::from(["claude-code/a".to_string()]),
            already_recovered: HashSet::from(["claude-code/b".to_string()]),
            idle: HashSet::from(["claude-code/c".to_string()]),
            merged: HashSet::from(["claude-code/e".to_string()]),
            known: HashSet::from([
                "claude-code/a".to_string(),
                "claude-code/b".to_string(),
                "claude-code/c".to_string(),
                "claude-code/d".to_string(),
                "claude-code/e".to_string(),
            ]),
            ..Default::default()
        },
    );
    assert_eq!(result, vec!["claude-code/d".to_string()]);
}

#[test]
fn no_candidates_means_no_recovery() {
    let result = filter_recovery_candidates(&[], &RecoveryFilter::default());
    assert!(result.is_empty());
}

#[test]
fn all_idle_means_no_recovery() {
    let candidates = vec![
        (PathBuf::from("/tmp/wt-1"), "claude-code/x".to_string()),
        (PathBuf::from("/tmp/wt-2"), "claude-code/y".to_string()),
    ];
    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            idle: HashSet::from(["claude-code/x".to_string(), "claude-code/y".to_string()]),
            known: HashSet::from(["claude-code/x".to_string(), "claude-code/y".to_string()]),
            ..Default::default()
        },
    );
    assert!(result.is_empty());
}

#[test]
fn merged_branch_skipped_during_recovery() {
    // A session whose branch was already merged to main (e.g., changes applied,
    // then an Apply-time hardening session emitted SessionStarted after
    // CodingAgentIdled, then engine restarted). The SQL idle_branches check
    // misses this because SessionStarted after CodingAgentIdled makes it
    // look non-idle. But git shows no diff vs main.
    let candidates = vec![
        (
            PathBuf::from("/tmp/wt-1"),
            "claude-code/already-merged".to_string(),
        ),
        (
            PathBuf::from("/tmp/wt-2"),
            "claude-code/has-changes".to_string(),
        ),
    ];

    let merged = HashSet::from(["claude-code/already-merged".to_string()]);
    let known = HashSet::from([
        "claude-code/already-merged".to_string(),
        "claude-code/has-changes".to_string(),
    ]);

    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            merged,
            known,
            ..Default::default()
        },
    );
    assert_eq!(result, vec!["claude-code/has-changes".to_string()]);
}

#[test]
fn completed_change_branches_skipped_during_recovery() {
    // Branches with applied or discarded changes should not be re-recovered.
    // The original session completed and proposed changes that were resolved.
    let candidates = vec![
        (
            PathBuf::from("/tmp/wt-1"),
            "claude-code/applied".to_string(),
        ),
        (
            PathBuf::from("/tmp/wt-2"),
            "claude-code/discarded".to_string(),
        ),
        (
            PathBuf::from("/tmp/wt-3"),
            "claude-code/needs-recovery".to_string(),
        ),
    ];
    let completed = HashSet::from([
        "claude-code/applied".to_string(),
        "claude-code/discarded".to_string(),
    ]);
    let known = HashSet::from([
        "claude-code/applied".to_string(),
        "claude-code/discarded".to_string(),
        "claude-code/needs-recovery".to_string(),
    ]);

    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            completed_change: completed,
            known,
            ..Default::default()
        },
    );
    assert_eq!(result, vec!["claude-code/needs-recovery".to_string()]);
}

#[test]
fn unknown_branches_skipped_unless_actively_running() {
    // Branches with no original thread in the DB (from a reset DB context)
    // should be skipped. Only actively running sessions are recovered.
    let candidates = vec![
        (
            PathBuf::from("/tmp/wt-1"),
            "claude-code/unknown-idle".to_string(),
        ),
        (
            PathBuf::from("/tmp/wt-2"),
            "claude-code/unknown-running".to_string(),
        ),
        (PathBuf::from("/tmp/wt-3"), "claude-code/known".to_string()),
    ];
    let actively_running = HashSet::from(["claude-code/unknown-running".to_string()]);
    let known = HashSet::from(["claude-code/known".to_string()]);

    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            actively_running,
            known,
            ..Default::default()
        },
    );
    assert_eq!(result.len(), 2);
    assert!(result.contains(&"claude-code/unknown-running".to_string()));
    assert!(result.contains(&"claude-code/known".to_string()));
    // unknown-idle is skipped — no DB record, not running
    assert!(!result.contains(&"claude-code/unknown-idle".to_string()));
}

#[test]
fn resumed_after_idle_is_not_treated_as_idle() {
    // A session that idled, then resumed working (user sent follow-up),
    // then died mid-work should NOT be in idle_branches.
    // The SQL query excludes sessions with activity after last idle —
    // including MessageReceived (user follow-up from chat.rs),
    // CodingAgentPromptSent (automated or user audit trail from CC event loop),
    // CodingAgentUserMessageSent, CodingAgentToolCalled, CodingAgentTextStreamed.
    // This test validates the filtering logic: if the SQL correctly
    // excludes such sessions from idle_branches, they get recovered.
    let candidates = vec![
        (
            PathBuf::from("/tmp/wt-1"),
            "claude-code/truly-idle".to_string(),
        ),
        (
            PathBuf::from("/tmp/wt-2"),
            "claude-code/resumed-then-died".to_string(),
        ),
    ];

    // Only the truly idle session should be in idle_branches.
    // The resumed-then-died session has CC activity after its last idle
    // event, so the SQL query excludes it from idle_branches.
    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            idle: HashSet::from(["claude-code/truly-idle".to_string()]),
            known: HashSet::from([
                "claude-code/truly-idle".to_string(),
                "claude-code/resumed-then-died".to_string(),
            ]),
            ..Default::default()
        },
    );
    assert_eq!(result, vec!["claude-code/resumed-then-died".to_string()]);
}

#[test]
fn restarted_after_idle_is_not_treated_as_idle() {
    // A session that idled, then had a new SessionStarted (e.g., an
    // Apply-time hardening session), then the process died should NOT be
    // in idle_branches. The SQL NOT EXISTS clause must include SessionStarted
    // so that a new session start after CodingAgentIdled marks it as non-idle.
    // Bug: without SessionStarted in the NOT EXISTS check, the idle query
    // incorrectly classifies this as idle and skips recovery.
    let candidates = vec![
        (
            PathBuf::from("/tmp/wt-1"),
            "claude-code/truly-idle".to_string(),
        ),
        (
            PathBuf::from("/tmp/wt-2"),
            "claude-code/rehardened-then-died".to_string(),
        ),
    ];
    // The rehardened-then-died session has a SessionStarted after its last
    // CodingAgentIdled, so the SQL query must exclude it from idle_branches.
    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            idle: HashSet::from(["claude-code/truly-idle".to_string()]),
            known: HashSet::from([
                "claude-code/truly-idle".to_string(),
                "claude-code/rehardened-then-died".to_string(),
            ]),
            ..Default::default()
        },
    );
    assert_eq!(result, vec!["claude-code/rehardened-then-died".to_string()]);
}

#[test]
fn actively_running_session_with_pending_change_still_recovered() {
    // Contract: actively-running Claude Code sessions are recovered AND their pending
    // change is preserved. The resumed session's next idle dedupes via
    // propose_change updating the existing row in place.
    let candidates = vec![
        (
            PathBuf::from("/tmp/wt-1"),
            "claude-code/active-with-pending".to_string(),
        ),
        (
            PathBuf::from("/tmp/wt-2"),
            "claude-code/idle-with-pending".to_string(),
        ),
        (
            PathBuf::from("/tmp/wt-3"),
            "claude-code/active-no-pending".to_string(),
        ),
    ];

    let pending = HashSet::from([
        "claude-code/active-with-pending".to_string(),
        "claude-code/idle-with-pending".to_string(),
    ]);

    let actively_running = HashSet::from([
        "claude-code/active-with-pending".to_string(),
        "claude-code/active-no-pending".to_string(),
    ]);

    let known = HashSet::from([
        "claude-code/active-with-pending".to_string(),
        "claude-code/idle-with-pending".to_string(),
        "claude-code/active-no-pending".to_string(),
    ]);

    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            pending,
            actively_running,
            known,
            ..Default::default()
        },
    );

    // wt-1: actively running + pending → recover (pending preserved by recovery loop)
    // wt-2: idle + pending → skip (user reviews the change at their leisure)
    // wt-3: actively running + no pending → recover normally
    assert_eq!(result.len(), 2);
    assert!(result.contains(&"claude-code/active-with-pending".to_string()));
    assert!(result.contains(&"claude-code/active-no-pending".to_string()));
    assert!(!result.contains(&"claude-code/idle-with-pending".to_string()));
}

#[test]
fn followup_before_cc_output_not_treated_as_idle() {
    // Bug fix: user sends a follow-up to an idle Claude Code session. chat.rs emits
    // MessageReceived and routes the message to CC via msg_tx. The CC event
    // loop picks it up and emits CodingAgentPromptSent. But the engine restarts
    // BEFORE CC produces any tool calls or text output.
    //
    // Old behavior: the SQL idle_branches query only checked for
    // CodingAgentUserMessageSent (never emitted!), CodingAgentToolCalled, and
    // CodingAgentTextStreamed. With none of those present, the session was
    // incorrectly classified as idle and the worktree was cleaned up — silently
    // losing the user's follow-up.
    //
    // Fix: the SQL now also checks for MessageReceived and CodingAgentPromptSent.
    // Both are emitted during the follow-up flow, so the session is correctly
    // classified as active even before CC produces output.
    //
    // This test validates the filter logic side: if the SQL correctly excludes
    // the session from idle_branches (because MessageReceived exists after idle),
    // the filter recovers it.
    let candidates = vec![
        (
            PathBuf::from("/tmp/wt-1"),
            "claude-code/truly-idle".to_string(),
        ),
        (
            PathBuf::from("/tmp/wt-2"),
            "claude-code/followup-no-output".to_string(),
        ),
    ];

    // The SQL correctly excluded followup-no-output from idle_branches
    // because MessageReceived exists after CodingAgentIdled.
    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            idle: HashSet::from(["claude-code/truly-idle".to_string()]),
            actively_running: HashSet::from(["claude-code/followup-no-output".to_string()]),
            known: HashSet::from([
                "claude-code/truly-idle".to_string(),
                "claude-code/followup-no-output".to_string(),
            ]),
            ..Default::default()
        },
    );
    assert_eq!(
        result,
        vec!["claude-code/followup-no-output".to_string()],
        "Session with user follow-up (MessageReceived after idle) but no CC output yet \
             must be recovered — the follow-up would otherwise be silently lost"
    );
}

#[test]
fn idled_then_resumed_session_with_pending_change_still_recovered() {
    // Real-world scenario: "Execute and Fix E2E Tests" thread.
    // 1. Session ran, idled (CodingAgentIdled)
    // 2. User sent follow-up ("yeah nice run it now")
    // 3. CC resumed (tool calls for running e2e tests)
    // 4. Engine killed mid-work → ChangeProposed auto-emitted
    //
    // The session is NOT in idle_branches (CC activity after idle).
    // The session IS in actively_running_branches (post-idle CC activity
    // detected by the UNION clause in the SQL query).
    // The session HAS a pending change.
    // → Must bypass the pending check and be recovered.
    //
    // Note: this test validates the filter logic. The SQL query that
    // populates actively_running_branches handles the idle→resumed
    // detection via a UNION clause checking for activity events
    // (MessageReceived, CodingAgentPromptSent, CodingAgentToolCalled, etc.)
    // after the last CodingAgentIdled.
    let candidates = vec![
        (
            PathBuf::from("/tmp/wt-1"),
            "claude-code/idled-resumed-pending".to_string(),
        ),
        (
            PathBuf::from("/tmp/wt-2"),
            "claude-code/truly-idle-pending".to_string(),
        ),
    ];

    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            pending: HashSet::from([
                "claude-code/idled-resumed-pending".to_string(),
                "claude-code/truly-idle-pending".to_string(),
            ]),
            actively_running: HashSet::from(["claude-code/idled-resumed-pending".to_string()]),
            idle: HashSet::from(["claude-code/truly-idle-pending".to_string()]),
            known: HashSet::from([
                "claude-code/idled-resumed-pending".to_string(),
                "claude-code/truly-idle-pending".to_string(),
            ]),
            ..Default::default()
        },
    );

    assert_eq!(
        result,
        vec!["claude-code/idled-resumed-pending".to_string()],
        "Session that idled, resumed via follow-up, then was killed must be recovered \
             even with a pending change — the pending change is stale"
    );
}

#[test]
fn recovery_reuses_original_thread_when_available() {
    use std::collections::HashMap;
    use uuid::Uuid;

    let original_thread = Uuid::new_v4();
    let branch_to_thread: HashMap<String, Uuid> =
        HashMap::from([("claude-code/20260317-100000".to_string(), original_thread)]);

    // Branch with known thread → reuse original
    let branch = "claude-code/20260317-100000";
    let thread_id = branch_to_thread
        .get(branch)
        .copied()
        .unwrap_or_else(Uuid::new_v4);
    assert_eq!(thread_id, original_thread);

    // Branch without known thread → creates new UUID
    let unknown_branch = "claude-code/20260317-999999";
    let thread_id2 = branch_to_thread
        .get(unknown_branch)
        .copied()
        .unwrap_or_else(Uuid::new_v4);
    assert_ne!(thread_id2, original_thread);
}

#[test]
fn interrupted_session_without_idle_event_gets_recovered() {
    // During shutdown, CodingAgentIdled is suppressed (shutting_down flag).
    // A session that was actively working when the engine restarted will
    // NOT have CodingAgentIdled as its last event → NOT in idle_branches
    // → recover_orphaned_worktrees picks it up for recovery.
    let candidates = vec![
        (
            PathBuf::from("/tmp/wt-idle"),
            "claude-code/genuinely-idle".to_string(),
        ),
        (
            PathBuf::from("/tmp/wt-interrupted"),
            "claude-code/interrupted-mid-work".to_string(),
        ),
    ];

    // Only the genuinely idle session has CodingAgentIdled.
    // The interrupted session's CodingAgentIdled was suppressed during shutdown,
    // so it's NOT in idle_branches.
    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            idle: HashSet::from(["claude-code/genuinely-idle".to_string()]),
            known: HashSet::from([
                "claude-code/genuinely-idle".to_string(),
                "claude-code/interrupted-mid-work".to_string(),
            ]),
            ..Default::default()
        },
    );
    assert_eq!(
        result,
        vec!["claude-code/interrupted-mid-work".to_string()],
        "Session interrupted during shutdown (no CodingAgentIdled) must be recovered"
    );
}

#[test]
fn completed_session_with_response_generated_skips_recovery() {
    // Some Claude Code sessions (recovery, Apply-time hardening) complete with
    // ResponseGenerated but never emit CodingAgentIdled. Without
    // ResponseGenerated in the idle query, these sessions were incorrectly
    // recovered on every restart.
    let candidates = vec![
        (
            PathBuf::from("/tmp/wt-1"),
            "claude-code/completed-no-idle".to_string(),
        ),
        (
            PathBuf::from("/tmp/wt-2"),
            "claude-code/actually-interrupted".to_string(),
        ),
    ];

    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            idle: HashSet::from(["claude-code/completed-no-idle".to_string()]),
            known: HashSet::from([
                "claude-code/completed-no-idle".to_string(),
                "claude-code/actually-interrupted".to_string(),
            ]),
            ..Default::default()
        },
    );
    assert_eq!(
        result,
        vec!["claude-code/actually-interrupted".to_string()],
        "Session completed with ResponseGenerated (no CodingAgentIdled) must NOT be recovered"
    );
}

#[test]
fn running_session_with_no_git_changes_still_recovered() {
    // Bug: A Claude Code session killed mid-work before producing any git changes
    // was incorrectly skipped by the git-level "already merged" check.
    // The session had SessionStarted as its last lifecycle event (not idle),
    // and the idle_branches SQL correctly identified it as non-idle,
    // but the git check saw no commits and no uncommitted changes and
    // skipped it. These sessions MUST be resumed — they just hadn't
    // produced any output yet before the engine restarted.
    let candidates = vec![
        (
            PathBuf::from("/tmp/wt-1"),
            "claude-code/no-changes-but-running".to_string(),
        ),
        (
            PathBuf::from("/tmp/wt-2"),
            "claude-code/truly-merged".to_string(),
        ),
    ];

    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            merged: HashSet::from([
                "claude-code/no-changes-but-running".to_string(),
                "claude-code/truly-merged".to_string(),
            ]),
            actively_running: HashSet::from(["claude-code/no-changes-but-running".to_string()]),
            known: HashSet::from([
                "claude-code/no-changes-but-running".to_string(),
                "claude-code/truly-merged".to_string(),
            ]),
            ..Default::default()
        },
    );

    // The actively running session must be recovered despite no git changes.
    // The truly merged session (not actively running) is correctly skipped.
    assert_eq!(
        result,
        vec!["claude-code/no-changes-but-running".to_string()],
        "Running session with no git changes must be recovered, truly merged must be skipped"
    );
}

#[test]
fn double_restart_incomplete_recovery_not_in_already_recovered() {
    // Bug: Double engine restart. Restart #1 emits ContinuationStarted, starts CC
    // recovery. Restart #2 kills CC before it finishes. The already_recovered
    // SQL now checks that the latest ContinuationStarted was followed by a
    // completion event (CodingAgentIdled, ResponseGenerated, SessionEnded).
    // With no completion → branch NOT in already_recovered → re-recovered.
    //
    // Event sequence:
    //   SessionStarted → ... → CodingAgentIdled → ContinuationStarted → [killed]
    //
    // Expected sets after SQL queries on restart #2:
    //   already_recovered: empty (ContinuationStarted not followed by completion)
    //   idle_branches: empty (ContinuationStarted counts as activity after idle)
    //   actively_running_branches: contains branch (ContinuationStarted after idle)
    let candidates = vec![(
        PathBuf::from("/tmp/wt-1"),
        "claude-code/double-restart".to_string(),
    )];

    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            actively_running: HashSet::from(["claude-code/double-restart".to_string()]),
            known: HashSet::from(["claude-code/double-restart".to_string()]),
            ..Default::default()
        },
    );
    assert_eq!(
        result,
        vec!["claude-code/double-restart".to_string()],
        "Incomplete recovery (killed before completion) must be re-recovered on next restart"
    );
}

#[test]
fn double_restart_completed_recovery_stays_in_already_recovered() {
    // Counterpart to the above: if recovery DID complete (ContinuationStarted
    // followed by CodingAgentIdled), the branch IS in already_recovered
    // and should NOT be re-recovered.
    //
    // Event sequence:
    //   SessionStarted → ... → CodingAgentIdled → ContinuationStarted →
    //   CodingAgentPromptSent → SessionStarted → ... → CodingAgentIdled
    //
    // Expected sets:
    //   already_recovered: contains branch (recovery completed)
    //   idle_branches: contains branch (CodingAgentIdled is last, no activity after)
    let candidates = vec![(
        PathBuf::from("/tmp/wt-1"),
        "claude-code/completed-recovery".to_string(),
    )];

    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            already_recovered: HashSet::from(["claude-code/completed-recovery".to_string()]),
            idle: HashSet::from(["claude-code/completed-recovery".to_string()]),
            known: HashSet::from(["claude-code/completed-recovery".to_string()]),
            ..Default::default()
        },
    );
    assert!(
        result.is_empty(),
        "Completed recovery (followed by CodingAgentIdled) must NOT be re-recovered"
    );
}

#[test]
fn double_restart_lost_worktree_incomplete_recovery_rediscovered() {
    // Double-restart variant where the worktree was also lost.
    // ContinuationStarted emitted but not followed by completion →
    // NOT in already_recovered → branch rediscovered for fresh worktree.
    let result = find_worktreeless_active_branches(&RecoveryFilter {
        actively_running: HashSet::from(["claude-code/double-restart-lost".to_string()]),
        ..Default::default()
    });
    assert_eq!(
        result,
        vec!["claude-code/double-restart-lost".to_string()],
        "Lost worktree with incomplete recovery must get a fresh worktree"
    );
}

#[test]
fn duplicate_repo_scan_does_not_duplicate_recovery() {
    use std::collections::HashMap;
    use uuid::Uuid;

    let thread_id = Uuid::new_v4();
    let branch_a = "claude-code/20260416-162934-stale".to_string();
    let branch_b = "claude-code/20260416-162942-real".to_string();

    let branch_to_thread: HashMap<String, Uuid> =
        HashMap::from([(branch_a.clone(), thread_id), (branch_b.clone(), thread_id)]);

    let candidates = vec![
        (PathBuf::from("/tmp/wt-1"), branch_a),
        (PathBuf::from("/tmp/wt-2"), branch_b),
    ];

    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            known: HashSet::from([
                "claude-code/20260416-162934-stale".to_string(),
                "claude-code/20260416-162942-real".to_string(),
            ]),
            ..Default::default()
        },
    );
    assert_eq!(result.len(), 2, "Both branches pass branch-level filtering");

    let mut seen_threads: HashSet<Uuid> = HashSet::new();
    let mut recovered: Vec<String> = Vec::new();
    for (_, branch) in &candidates {
        let tid = branch_to_thread.get(branch).copied().unwrap();
        if seen_threads.insert(tid) {
            recovered.push(branch.clone());
        }
    }
    assert_eq!(
        recovered.len(),
        1,
        "Only one recovery per thread_id — duplicate must be skipped"
    );
}

/// Phase 5.3 contract: the constant the recovery path stamps on synthetic
/// `CodingAgentIdled.reason` must be a stable string the frontend can
/// match on. Any rename here is a frontend break — keep this test as the
/// canary.
#[test]
fn engine_restart_interrupt_reason_constant_is_stable() {
    assert_eq!(
        super::ENGINE_RESTART_INTERRUPT_REASON,
        "engine_restart_interrupt"
    );
}

/// The *Switch to new version* fingerprint has consumers beyond the two resume
/// gates now, and one of them is in another language: `abortPromisesAutoResume`
/// in `crates/lucidos-app/src/store/thread-events/exchange-render.ts` reads the
/// same `engine_shutdown` + device-actor pair off the event to withhold the
/// Continue button while the engine auto-resumes the turn.
///
/// TypeScript cannot import this constant, so this is the canary: change either
/// half of the pair and the frontend keeps reading the old shape, which is wrong
/// in whichever direction the change went (a button offered on a turn already
/// resuming, or withheld on one nothing will resume). Both halves are asserted
/// separately because both are load-bearing: a device actor alone is not the
/// fingerprint, since `StaleSettle` deliberately carries the actor of whichever
/// button exposed the stuck row.
///
/// The in-Rust consumer, `AbortCause::promises_auto_resume`, is checked against
/// the same constant here rather than trusted to stay parallel by inspection: it
/// decides the `paused` verdict, so a drift between it and the SQL would show a
/// user a pause glyph on a turn no gate will resume.
#[test]
fn switch_teardown_fingerprint_is_stable_for_the_frontend_mirror() {
    use crate::engine::thread_events::{AbortCause, MessageOrigin};

    let sql = crate::engine::agent_recovery::SWITCH_TEARDOWN_ABORT_SQL;
    assert!(
        sql.contains("event_type = 'ResponseAborted'"),
        "the fingerprint must still be about ResponseAborted: {sql}"
    );
    assert!(
        sql.contains("'engine_shutdown'"),
        "the cause half of the fingerprint changed; update abortPromisesAutoResume: {sql}"
    );
    assert!(
        sql.contains("'device'"),
        "the actor half of the fingerprint changed; update abortPromisesAutoResume: {sql}"
    );

    let device = MessageOrigin::Device {
        device_id: "dev-1".to_string(),
        label: "My MacBook".to_string(),
    };
    assert!(
        AbortCause::EngineShutdown.promises_auto_resume(Some(&device)),
        "the Rust predicate must match the same pair the SQL selects"
    );
    assert!(
        !AbortCause::EngineShutdown.promises_auto_resume(Some(&MessageOrigin::system())),
        "a system actor is not the switch fingerprint, so it promises no resume"
    );
    assert!(
        !AbortCause::RecoveryAfterRestart.promises_auto_resume(Some(&device)),
        "the boot floor's withdrawal carries no promise, whoever it is attributed to"
    );
}

/// Phase 5.3 contract: a synthetic `CodingAgentIdled` carrying
/// `reason = engine_restart_interrupt` must round-trip through serde so
/// the projection + SSE consumers see the reason field intact. Without
/// this, the UI's "continue?" affordance never renders.
#[test]
fn coding_agent_idled_engine_restart_interrupt_roundtrips() {
    use crate::engine::thread_events::ThreadEvent;
    use crate::runtime::CodingAgent;
    let event = ThreadEvent::CodingAgentIdled {
        has_changes: true,
        is_external_repo: false,
        requires_restart: true,
        cc_session_id: Some("sid-abc".to_string()),
        coding_agent: CodingAgent::ClaudeCode,
        reason: Some(super::ENGINE_RESTART_INTERRUPT_REASON.to_string()),
        worktree_path: None,
        worktree_head_sha: None,
        bg_bash_pending: false,
    };
    let json = serde_json::to_value(&event).expect("serializes");
    assert_eq!(json["type"], "CodingAgentIdled");
    assert_eq!(json["reason"], "engine_restart_interrupt");
    let back: ThreadEvent = serde_json::from_value(json).expect("deserializes");
    match back {
        ThreadEvent::CodingAgentIdled {
            reason,
            cc_session_id,
            has_changes,
            requires_restart,
            ..
        } => {
            assert_eq!(reason.as_deref(), Some("engine_restart_interrupt"));
            assert_eq!(cc_session_id.as_deref(), Some("sid-abc"));
            assert!(has_changes);
            assert!(requires_restart);
        }
        other => panic!("expected CodingAgentIdled, got {:?}", other),
    }
}

/// `continue_should_open_resume_exchange` gates the SpawnConsumer's
/// "Resumed after engine restart" emit. The user-clicked-continue path
/// and the engine's auto-recovery-after-hang path both qualify —
/// `answered_after_idle` resumes inside an existing `UserQuestionAsked`
/// exchange and would mislabel as a recovery.
#[test]
fn continue_resume_exchange_gate_opens_for_user_continue_and_auto_recovery() {
    use super::{
        continue_should_open_resume_exchange, ANSWERED_AFTER_IDLE_REASON,
        AUTO_RECOVERY_AFTER_HANG_REASON, USER_CLICKED_CONTINUE_REASON,
    };
    assert!(continue_should_open_resume_exchange(Some(
        USER_CLICKED_CONTINUE_REASON
    )));
    assert!(
        continue_should_open_resume_exchange(Some(AUTO_RECOVERY_AFTER_HANG_REASON)),
        "watchdog-triggered auto-resume must open a fresh boundary so the timeline shows the user that work was interrupted and resumed"
    );
    assert!(!continue_should_open_resume_exchange(Some(
        ANSWERED_AFTER_IDLE_REASON
    )));
    assert!(
        !continue_should_open_resume_exchange(None),
        "missing reason must default-deny — only known recovery reasons open the exchange"
    );
    assert!(
            !continue_should_open_resume_exchange(Some("future_reason_not_yet_handled")),
            "unknown reasons must default-deny so a future ContinuationRequested source can't accidentally inherit the recovery boundary"
        );
}

/// THE 2026-07-29 WEDGE REGRESSION GUARD (thread `cb503361`). A stale resume on
/// a continuation MUST produce a retry. `run_session` bails on a stale resume
/// without emitting a terminal — on purpose, so the projection stays `running`
/// across the retry window rather than flashing "Aborted" — which is only safe
/// if a retry follows. When the SpawnConsumer merely logged the error instead,
/// the thread sat at `running` with no live subprocess for 8 minutes: the
/// stale-resume arm also drops the `agent_sessions` entry, and that map is the
/// only thing `ExternalWatchdog` scans.
#[test]
fn stale_resume_on_a_continuation_retries_fresh() {
    use super::{continue_recovery, ContinueRecovery};
    assert_eq!(
        continue_recovery(Some(crate::engine::claude_code::STALE_RESUME_ERROR), false),
        ContinueRecovery::RetryFresh
    );
}

/// The retry is ONE-SHOT: a second stale resume settles instead of retrying
/// again, so an engine-driven continuation can never loop. (Unreachable in
/// practice — the retry passes no resume sid and `is_stale_resume_signal`
/// requires one — but crash-safety is a floor here, not an inference.)
#[test]
fn a_second_stale_resume_settles_instead_of_looping() {
    use super::{continue_recovery, ContinueRecovery};
    assert_eq!(
        continue_recovery(Some(crate::engine::claude_code::STALE_RESUME_ERROR), true),
        ContinueRecovery::Settle
    );
}

/// Every other error settles, before AND after the retry is spent. This is the
/// zombie-`running` floor: a continuation is engine-driven, so if it dies
/// without a terminal nothing else will ever clear the projection — the
/// watchdogs scan only live `agent_sessions` and the boot sweep has long since
/// run.
#[test]
fn any_other_continuation_error_settles_the_thread() {
    use super::{continue_recovery, ContinueRecovery};
    for retried in [false, true] {
        for err in [
            "Failed to start the coding agent: No such file or directory",
            "pool timed out while waiting for an open connection",
            "",
        ] {
            assert_eq!(
                continue_recovery(Some(err), retried),
                ContinueRecovery::Settle,
                "error {err:?} (retried={retried}) must settle — an unsettled `running` thread is only clearable by a user click"
            );
        }
    }
}

/// The one error that must NOT settle. The spawn guard rejects a continuation
/// that raced a user message, because a LIVE session already owns the thread —
/// so this continuation owns nothing and the `running` projection is TRUE. A
/// settle here emits a terminal against a working session, which is the same
/// lying projection as the wedge, pointed the other way. Reachable in practice:
/// the consumer dispatches off an event subscriber holding no lock on the
/// thread, so a user message and an auto-resume can both arrive at once.
#[test]
fn losing_the_race_to_a_live_session_never_settles_it() {
    use super::{continue_recovery, ContinueRecovery};
    for retried in [false, true] {
        assert_eq!(
            continue_recovery(
                Some(crate::engine::claude_code::AGENT_ALREADY_RUNNING_ERROR),
                retried
            ),
            ContinueRecovery::Nothing,
            "the winning session owns the turn and will emit its own terminal"
        );
    }
}

/// A continuation that ran normally needs nothing: the run loop already emitted
/// its own terminal. Settling here would double-terminal a healthy turn.
#[test]
fn a_successful_continuation_needs_no_recovery() {
    use super::{continue_recovery, ContinueRecovery};
    assert_eq!(continue_recovery(None, false), ContinueRecovery::Nothing);
    assert_eq!(
        continue_recovery(None, true),
        ContinueRecovery::Nothing,
        "a retry that SUCCEEDED must not be settled on top of its own terminal"
    );
}

/// Most idles carry no reason — make sure that case still round-trips
/// without leaking a `"reason": null` into the wire payload (the
/// `skip_serializing_if = "Option::is_none"` attribute owns this).
#[test]
fn coding_agent_idled_without_reason_skips_field_in_wire_format() {
    use crate::engine::thread_events::ThreadEvent;
    use crate::runtime::CodingAgent;
    let event = ThreadEvent::CodingAgentIdled {
        has_changes: false,
        is_external_repo: false,
        requires_restart: false,
        cc_session_id: None,
        coding_agent: CodingAgent::ClaudeCode,
        reason: None,
        worktree_path: None,
        worktree_head_sha: None,
        bg_bash_pending: false,
    };
    let json = serde_json::to_value(&event).expect("serializes");
    assert!(
        json.get("reason").is_none(),
        "reason: None must be skipped on the wire to keep payloads minimal: {}",
        json
    );
}
