use std::collections::HashSet;
use std::path::PathBuf;

use crate::engine::agent_recovery::branch_awaits_recovery;
use crate::engine::git_ops::{is_external_repo_path, worktrees_dir};
use crate::engine::session_queries::deterministic_worktree_path;

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

#[derive(Default)]
pub(crate) struct RecoveryFilter {
    pub pending: HashSet<String>,
    pub already_recovered: HashSet<String>,
    pub idle: HashSet<String>,
    pub actively_running: HashSet<String>,
    pub known: HashSet<String>,
    pub discovered: HashSet<String>,
}

pub(crate) fn filter_recovery_candidates(
    candidates: &[(PathBuf, String)],
    f: &RecoveryFilter,
) -> Vec<String> {
    candidates
        .iter()
        .filter(|(_, branch)| {
            (!f.pending.contains(branch) || f.actively_running.contains(branch))
                && (f.actively_running.contains(branch) || !f.already_recovered.contains(branch))
                && branch_awaits_recovery(branch, &f.idle, f.pending.contains(branch))
                && (f.known.contains(branch) || f.actively_running.contains(branch))
        })
        .map(|(_, branch)| branch.clone())
        .collect()
}

pub(crate) fn find_worktreeless_active_branches(f: &RecoveryFilter) -> Vec<String> {
    f.actively_running
        .iter()
        .filter(|branch| !f.discovered.contains(*branch) && !f.idle.contains(*branch))
        .cloned()
        .collect()
}

#[test]
fn lost_worktree_active_session_discovered_from_db() {
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
fn lost_worktree_running_branch_rediscovered_despite_an_earlier_recovery() {
    let result = find_worktreeless_active_branches(&RecoveryFilter {
        actively_running: HashSet::from(["claude-code/recovered-before".to_string()]),
        already_recovered: HashSet::from(["claude-code/recovered-before".to_string()]),
        ..Default::default()
    });
    assert_eq!(
        result,
        vec!["claude-code/recovered-before".to_string()],
        "a later turn in flight needs a worktree, whatever an earlier turn's \
         recovery did"
    );
}

#[test]
fn lost_worktree_mixed_scenario() {
    let result = find_worktreeless_active_branches(&RecoveryFilter {
        discovered: HashSet::from(["claude-code/has-worktree".to_string()]),
        actively_running: HashSet::from([
            "claude-code/has-worktree".to_string(),
            "claude-code/needs-recovery".to_string(),
            "claude-code/already-done".to_string(),
        ]),
        idle: HashSet::from(["claude-code/already-done".to_string()]),
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
        (PathBuf::from("/tmp/wt-a"), "claude-code/a".to_string()),
        (PathBuf::from("/tmp/wt-b"), "claude-code/b".to_string()),
        (PathBuf::from("/tmp/wt-c"), "claude-code/c".to_string()),
        (PathBuf::from("/tmp/wt-d"), "claude-code/d".to_string()),
        (PathBuf::from("/tmp/wt-e"), "claude-code/e".to_string()),
    ];

    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            pending: HashSet::from(["claude-code/a".to_string()]),
            already_recovered: HashSet::from(["claude-code/b".to_string()]),
            idle: HashSet::from(["claude-code/c".to_string()]),
            known: HashSet::from([
                "claude-code/a".to_string(),
                "claude-code/b".to_string(),
                "claude-code/c".to_string(),
                "claude-code/d".to_string(),
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
fn only_current_turn_facts_settle_a_branch() {
    let idle = HashSet::from(["claude-code/turn-ended".to_string()]);

    assert!(
        branch_awaits_recovery("claude-code/applied-then-resumed", &idle, false),
        "an applied change on an earlier turn must not retire a live branch"
    );

    assert!(!branch_awaits_recovery(
        "claude-code/turn-ended",
        &idle,
        false
    ));

    assert!(branch_awaits_recovery(
        "claude-code/turn-ended",
        &idle,
        true
    ));

    let candidates = vec![
        (
            PathBuf::from("/tmp/wt-1"),
            "claude-code/applied-then-resumed".to_string(),
        ),
        (
            PathBuf::from("/tmp/wt-2"),
            "claude-code/turn-ended".to_string(),
        ),
    ];
    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            idle,
            known: HashSet::from([
                "claude-code/applied-then-resumed".to_string(),
                "claude-code/turn-ended".to_string(),
            ]),
            ..Default::default()
        },
    );
    assert_eq!(
        result,
        vec!["claude-code/applied-then-resumed".to_string()],
        "only the branch whose turn was still open is recovered"
    );
}

#[test]
fn unknown_branches_skipped_unless_actively_running() {
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
    assert!(!result.contains(&"claude-code/unknown-idle".to_string()));
}

#[test]
fn resumed_after_idle_is_not_treated_as_idle() {
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

    assert_eq!(result.len(), 2);
    assert!(result.contains(&"claude-code/active-with-pending".to_string()));
    assert!(result.contains(&"claude-code/active-no-pending".to_string()));
    assert!(!result.contains(&"claude-code/idle-with-pending".to_string()));
}

#[test]
fn followup_before_cc_output_not_treated_as_idle() {
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

    let branch = "claude-code/20260317-100000";
    let thread_id = branch_to_thread
        .get(branch)
        .copied()
        .unwrap_or_else(Uuid::new_v4);
    assert_eq!(thread_id, original_thread);

    let unknown_branch = "claude-code/20260317-999999";
    let thread_id2 = branch_to_thread
        .get(unknown_branch)
        .copied()
        .unwrap_or_else(Uuid::new_v4);
    assert_ne!(thread_id2, original_thread);
}

#[test]
fn interrupted_session_without_idle_event_gets_recovered() {
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
    let candidates = vec![
        (
            PathBuf::from("/tmp/wt-1"),
            "claude-code/no-changes-but-running".to_string(),
        ),
        (
            PathBuf::from("/tmp/wt-2"),
            "claude-code/no-changes-and-idle".to_string(),
        ),
    ];

    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            actively_running: HashSet::from(["claude-code/no-changes-but-running".to_string()]),
            idle: HashSet::from(["claude-code/no-changes-and-idle".to_string()]),
            known: HashSet::from([
                "claude-code/no-changes-but-running".to_string(),
                "claude-code/no-changes-and-idle".to_string(),
            ]),
            ..Default::default()
        },
    );

    assert_eq!(
        result,
        vec!["claude-code/no-changes-but-running".to_string()],
        "Running session with no git changes must be recovered; the idle one must not"
    );
}

#[test]
fn double_restart_incomplete_recovery_not_in_already_recovered() {
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
fn a_completed_recovery_does_not_settle_a_later_turn_on_the_same_branch() {
    let candidates = vec![(
        PathBuf::from("/tmp/wt-1"),
        "claude-code/resumed-then-worked-again".to_string(),
    )];

    let result = filter_recovery_candidates(
        &candidates,
        &RecoveryFilter {
            already_recovered: HashSet::from(["claude-code/resumed-then-worked-again".to_string()]),
            actively_running: HashSet::from(["claude-code/resumed-then-worked-again".to_string()]),
            known: HashSet::from(["claude-code/resumed-then-worked-again".to_string()]),
            ..Default::default()
        },
    );
    assert_eq!(
        result,
        vec!["claude-code/resumed-then-worked-again".to_string()],
        "a turn in flight NOW has no live subprocess whoever recovered the last \
         one, so the completed prior recovery must not skip it"
    );
}

#[test]
fn double_restart_lost_worktree_incomplete_recovery_rediscovered() {
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

#[test]
fn engine_restart_interrupt_reason_constant_is_stable() {
    assert_eq!(
        super::ENGINE_RESTART_INTERRUPT_REASON,
        "engine_restart_interrupt"
    );
}

#[test]
fn switch_teardown_fingerprint_is_stable_for_the_frontend_mirror() {
    use crate::engine::thread_events::{AbortCause, MessageOrigin};

    let sql = crate::engine::switch_recovery_sql::SWITCH_TEARDOWN_ABORT_SQL;
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

#[test]
fn a_device_id_header_alone_produces_the_switch_fingerprints_actor() {
    use crate::engine::thread_events::{AbortCause, MessageOrigin};

    let headers = crate::api::gateway_stamp::test_support::stamped_device_headers("picker-device");
    let actor = crate::api::actor::user_actor(&headers, None)
        .expect("a device-id header always resolves to some actor");
    assert!(
        matches!(actor, MessageOrigin::Device { .. }),
        "the picker's header must resolve to a Device actor, not an Api one"
    );
    assert!(
        AbortCause::EngineShutdown.promises_auto_resume(Some(&actor)),
        "a gateway-initiated restart must promise the same auto-resume as the \
         in-workspace switch, or the picker keeps settling turns at 'failed'"
    );

    let no_device = crate::api::actor::user_actor(&axum::http::HeaderMap::new(), None);
    assert_eq!(no_device, None, "an unidentified caller is nobody");
    assert!(
        !AbortCause::EngineShutdown.promises_auto_resume(no_device.as_ref()),
        "no actor is not the fingerprint, so it must never reach the stash"
    );
}

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
        turn_end: None,
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

#[test]
fn continue_resume_exchange_gate_opens_for_user_continue_and_auto_recovery() {
    use super::{
        continue_should_open_resume_exchange, ANSWERED_AFTER_IDLE_REASON,
        AUTO_RECOVERY_AFTER_HANG_REASON, HARDEN_REQUESTED_REASON, USER_CLICKED_CONTINUE_REASON,
    };
    assert!(continue_should_open_resume_exchange(Some(
        USER_CLICKED_CONTINUE_REASON
    )));
    assert!(
        continue_should_open_resume_exchange(Some(HARDEN_REQUESTED_REASON)),
        "Harden starts a new turn the user asked for"
    );
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

#[test]
fn stale_resume_on_a_continuation_retries_fresh() {
    use super::{continue_recovery, ContinueRecovery};
    assert_eq!(
        continue_recovery(Some(crate::engine::claude_code::STALE_RESUME_ERROR), false),
        ContinueRecovery::RetryFresh
    );
}

#[test]
fn a_second_stale_resume_settles_instead_of_looping() {
    use super::{continue_recovery, ContinueRecovery};
    assert_eq!(
        continue_recovery(Some(crate::engine::claude_code::STALE_RESUME_ERROR), true),
        ContinueRecovery::Settle
    );
}

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

#[test]
fn losing_the_claim_to_a_live_session_never_settles_it() {
    use super::{continue_recovery, ContinueRecovery};
    for retried in [false, true] {
        assert_eq!(
            continue_recovery(
                Some(crate::engine::continuation_reason::AGENT_ALREADY_RUNNING_ERROR),
                retried
            ),
            ContinueRecovery::Nothing,
            "the winning session owns the turn and will emit its own terminal"
        );
    }
}

#[test]
fn the_continue_consumer_claims_before_it_emits_or_runs() {
    const SRC: &str =
        include_str!("../../../../lucidos-engine/src/engine/engine_impl/construction.rs");
    let consumer = &SRC[SRC
        .find("SpawnRequest::Continue {")
        .expect("the consumer must still match SpawnRequest::Continue")..];
    let claim = consumer
        .find(".claim_thread_for_spawn(thread_id, None)")
        .expect("the consumer must claim the thread");
    let emit = consumer
        .find("ThreadEvent::ContinuationStarted")
        .expect("the consumer must still open the resume exchange");
    let run = consumer
        .find(".run_direct_agent(")
        .expect("the consumer must still run the agent");
    assert!(claim < emit && claim < run);
}

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
        turn_end: None,
    };
    let json = serde_json::to_value(&event).expect("serializes");
    assert!(
        json.get("reason").is_none(),
        "reason: None must be skipped on the wire to keep payloads minimal: {}",
        json
    );
}
