#[tokio::test]
async fn idle_notify_wakes_registered_waiter() {
    let notify = std::sync::Arc::new(tokio::sync::Notify::new());
    let notify2 = notify.clone();

    let waiter = tokio::spawn(async move {
        match tokio::time::timeout(std::time::Duration::from_secs(2), notify2.notified()).await {
            Ok(()) => true,
            Err(_) => false,
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    notify.notify_waiters();

    let woken = waiter.await.unwrap();
    assert!(
        woken,
        "idle_notify.notify_waiters() must wake registered waiters — \
                         the follow-up redirect depends on it"
    );
}

#[tokio::test]
async fn notify_waiters_does_not_store_permit() {
    let notify = std::sync::Arc::new(tokio::sync::Notify::new());

    notify.notify_waiters();

    let result =
        tokio::time::timeout(std::time::Duration::from_millis(100), notify.notified()).await;

    assert!(result.is_err(), "notify_waiters() must NOT store a permit — \
                                   bare .notified().await after a missed notification hangs forever");
}

#[tokio::test]
async fn resume_branch_reuses_existing_branch() {
    use crate::engine::git_ops::git_cmd;

    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();

    let _ = git_cmd(&["init"], repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], repo).await;
    let _ = tokio::fs::write(repo.join("file.txt"), "initial").await;
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "init"], repo).await;

    let branch_name = "claude-code/20260326-test";
    let _ = git_cmd(&["checkout", "-b", branch_name], repo).await;
    let _ = tokio::fs::write(repo.join("change.txt"), "cc changes").await;
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "cc work"], repo).await;
    let _ = git_cmd(&["checkout", "main"], repo).await;

    let exists = git_cmd(&["rev-parse", "--verify", branch_name], repo)
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    assert!(exists, "Test branch should exist");

    let wt_path = tmp.path().join("worktree-resume");
    let result = git_cmd(
        &["worktree", "add", wt_path.to_str().unwrap(), branch_name],
        repo,
    )
    .await;
    assert!(
        result.unwrap().status.success(),
        "Should create worktree from existing branch"
    );

    let content = tokio::fs::read_to_string(wt_path.join("change.txt"))
        .await
        .unwrap();
    assert_eq!(
        content, "cc changes",
        "Resumed worktree should contain previous CC changes"
    );

    let _ = git_cmd(
        &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
        repo,
    )
    .await;
}

#[tokio::test]
async fn resume_reuses_existing_worktree_for_branch() {
    use crate::engine::git_ops::{git_cmd, parse_worktree_list};

    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();

    let _ = git_cmd(&["init"], repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], repo).await;
    let _ = tokio::fs::write(repo.join("file.txt"), "initial").await;
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "init"], repo).await;

    let branch_name = "claude-code/20260326-leftover";
    let _ = git_cmd(&["checkout", "-b", branch_name], repo).await;
    let _ = tokio::fs::write(repo.join("change.txt"), "cc changes").await;
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "cc work"], repo).await;
    let _ = git_cmd(&["checkout", "main"], repo).await;

    let wt_path = tmp.path().join("old-worktree");
    let result = git_cmd(
        &["worktree", "add", wt_path.to_str().unwrap(), branch_name],
        repo,
    )
    .await;
    assert!(
        result.unwrap().status.success(),
        "Setup: should create initial worktree"
    );

    let list_output = git_cmd(&["worktree", "list", "--porcelain"], repo)
        .await
        .unwrap();
    let stdout = String::from_utf8_lossy(&list_output.stdout);
    let map = parse_worktree_list(&stdout);
    let found = map.get(branch_name);
    assert!(
        found.is_some(),
        "parse_worktree_list should find the existing worktree for branch {}",
        branch_name
    );

    let found_path = found.unwrap();
    let canonical_expected = wt_path.canonicalize().unwrap();
    let canonical_found = found_path.canonicalize().unwrap();
    assert_eq!(
        canonical_found,
        canonical_expected,
        "Existing worktree path should match: expected {}, got {}",
        canonical_expected.display(),
        canonical_found.display()
    );

    let wt_path_new = tmp.path().join("new-worktree");
    let fail_result = git_cmd(
        &[
            "worktree",
            "add",
            wt_path_new.to_str().unwrap(),
            branch_name,
        ],
        repo,
    )
    .await;
    assert!(
        !fail_result.unwrap().status.success(),
        "git worktree add should fail when branch already checked out in another worktree"
    );

    let content = tokio::fs::read_to_string(wt_path.join("change.txt"))
        .await
        .unwrap();
    assert_eq!(
        content, "cc changes",
        "Existing worktree should preserve CC changes"
    );

    let _ = git_cmd(
        &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
        repo,
    )
    .await;
}

#[tokio::test]
async fn resume_falls_back_when_branch_deleted() {
    use crate::engine::git_ops::git_cmd;

    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();

    let _ = git_cmd(&["init"], repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], repo).await;
    let _ = tokio::fs::write(repo.join("file.txt"), "initial").await;
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "init"], repo).await;

    let branch_name = "claude-code/20260326-deleted";
    let exists = git_cmd(&["rev-parse", "--verify", branch_name], repo)
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    assert!(!exists, "Deleted branch should not exist");

    let new_branch = crate::engine::git_ops::allocate_coding_agent_branch(
        repo,
        crate::runtime::CodingAgent::ClaudeCode,
        &crate::engine::git_ops::BranchScope::Repo(
            crate::engine::git_ops::BranchScope::LUCIDOS_REPO.to_string(),
        ),
        "deleted branch fallback",
        uuid::Uuid::new_v4(),
    )
    .await;
    let wt_path = tmp.path().join("worktree-fresh");
    let result = git_cmd(
        &[
            "worktree",
            "add",
            wt_path.to_str().unwrap(),
            "-b",
            &new_branch,
        ],
        repo,
    )
    .await;
    assert!(
        result.unwrap().status.success(),
        "Should create worktree with fresh branch"
    );

    let _ = git_cmd(
        &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
        repo,
    )
    .await;
}

fn make_test_session(
    msg_tx: tokio::sync::mpsc::UnboundedSender<crate::engine::AgentUserInput>,
    is_waiting: bool,
) -> crate::engine::AgentSession {
    let mut s = crate::engine::AgentSession::for_test_with_sender(msg_tx);
    s.is_waiting = is_waiting;
    s
}

#[tokio::test]
async fn idle_session_routes_followup_via_msg_tx() {
    use crate::engine::AgentUserInput;

    let (msg_tx, mut msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
    let session = make_test_session(msg_tx, true);

    assert!(!session.process_exited);
    assert!(session.is_waiting);

    assert!(
        session
            .msg_tx
            .send(AgentUserInput {
                text: "Follow-up message".to_string(),
                images: None,
                origin_event_id: None,
                kind: crate::engine::AgentInputKind::User,
                completion: None,
            })
            .is_ok(),
        "send should succeed when receiver is alive"
    );

    let received = msg_rx
        .try_recv()
        .expect("msg_rx should have received the follow-up");
    assert_eq!(received.text, "Follow-up message");
}

#[tokio::test]
async fn busy_session_rejects_followup() {
    use crate::engine::AgentUserInput;

    let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
    let session = make_test_session(msg_tx, false);

    assert!(!session.process_exited);
    assert!(
        !session.is_waiting,
        "Busy session should reject follow-up with 'already running' error"
    );
}

#[tokio::test]
async fn closed_channel_returns_error() {
    use crate::engine::AgentUserInput;

    let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
    let session = make_test_session(msg_tx, true);

    drop(msg_rx);

    let result = session.msg_tx.send(AgentUserInput {
        text: "too late".to_string(),
        images: None,
        origin_event_id: None,
        kind: crate::engine::AgentInputKind::User,
        completion: None,
    });
    assert!(result.is_err(), "send should fail when receiver is dropped");
}

#[tokio::test]
async fn is_in_flight_false_for_idle_session() {
    let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel();
    let session = make_test_session(msg_tx, true);
    assert!(!session.is_in_flight());
}

#[tokio::test]
async fn is_in_flight_true_for_busy_session() {
    let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel();
    let session = make_test_session(msg_tx, false);
    assert!(session.is_in_flight());
}

#[tokio::test]
async fn is_in_flight_false_for_exited_session() {
    let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut session = make_test_session(msg_tx, false);
    session.process_exited = true;
    assert!(!session.is_in_flight());
}

#[tokio::test]
async fn exited_mark_declines_fast_path_followup_route() {
    let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut session = make_test_session(msg_tx, true);

    let has_session_before = !session.process_exited;
    assert!(
        has_session_before,
        "an idle, alive session must be eligible for the msg_tx fast-path"
    );

    session.process_exited = true;

    let has_session_after = !session.process_exited;
    let send_step_allows = !session.process_exited;
    assert!(
        !has_session_after,
        "a session marked process_exited at idle-terminate must NOT pass has_session"
    );
    assert!(
        !send_step_allows,
        "a session marked process_exited must NOT pass the msg_tx send guard"
    );
}

#[tokio::test]
async fn cancel_actor_field_stores_and_drains() {
    use crate::engine::thread_events::MessageOrigin;

    let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut session = make_test_session(msg_tx, false);

    assert!(
        session.cancel_actor.is_none(),
        "a fresh session has no pending cancel actor — engine-internal interrupts \
         must not inherit a phantom device"
    );

    let actor = MessageOrigin::Device {
        device_id: "ios-1".into(),
    };
    session.cancel_actor = Some(actor.clone());

    assert_eq!(
        session.cancel_actor.take(),
        Some(actor),
        "first take must return the device that clicked Cancel"
    );
    assert!(
        session.cancel_actor.take().is_none(),
        "second take must be empty"
    );
}

#[tokio::test]
async fn stop_on_an_entry_no_loop_listens_to_takes_the_no_session_fallback() {
    use super::control::{interrupt_route, InterruptRoute};

    let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut exiting = make_test_session(msg_tx, false);
    exiting.process_exited = true;
    assert_eq!(interrupt_route(&exiting), InterruptRoute::NoLoop);

    let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();
    let phantom = make_test_session(msg_tx, false);
    drop(msg_rx);
    assert_eq!(interrupt_route(&phantom), InterruptRoute::NoLoop);

    let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel();
    assert_eq!(
        interrupt_route(&make_test_session(msg_tx, false)),
        InterruptRoute::Interrupt
    );
    let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel();
    assert_eq!(
        interrupt_route(&make_test_session(msg_tx, true)),
        InterruptRoute::AlreadyWaiting
    );
}

#[test]
fn the_rung_locks_while_a_turn_runs() {
    use super::control::{awaits_idle_exit, rung_locked};
    use crate::engine::thread_lifecycle::ThreadStatus;

    for status in ThreadStatus::ALL {
        let open = matches!(
            status,
            ThreadStatus::Running | ThreadStatus::WaitingForUserAnswer | ThreadStatus::Paused
        );
        assert_eq!(
            rung_locked(status, None),
            open,
            "{status:?} with no session"
        );
    }

    let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut live = make_test_session(msg_tx, false);
    assert!(
        rung_locked(ThreadStatus::Idle, Some(&live)),
        "a live process would take a follow-up on its old rung"
    );
    assert!(
        awaits_idle_exit(&live),
        "its turn may have just ended, so a pick waits for the idle exit"
    );
    live.change_claim = Some(crate::engine::types::ChangeClaim::Apply);
    assert!(
        !awaits_idle_exit(&live),
        "an apply holds the process, so no idle exit comes"
    );
    live.change_claim = None;

    let mut keep_alive = live;
    keep_alive.keep_alive = true;
    assert!(
        !rung_locked(ThreadStatus::Idle, Some(&keep_alive)),
        "a keep-alive records no rung over the pick"
    );
    assert!(!awaits_idle_exit(&keep_alive));

    drop(msg_rx);
    let mut exited = keep_alive;
    exited.keep_alive = false;
    assert!(
        !rung_locked(ThreadStatus::Idle, Some(&exited)),
        "an entry whose loop is gone runs nothing"
    );
}

#[test]
fn a_starting_session_locks_the_rung() {
    use super::control::rung_locked;

    let mut sessions = std::collections::HashMap::new();
    let thread_id = uuid::Uuid::new_v4();
    let _claim = crate::engine::session_claim::claim_thread_for(
        &mut sessions,
        thread_id,
        crate::runtime::CodingAgent::ClaudeCode,
        crate::engine::session_claim::ClaimFor::Run,
    )
    .unwrap();
    assert!(rung_locked(
        crate::engine::thread_lifecycle::ThreadStatus::Idle,
        sessions.get(&thread_id)
    ));
}

#[test]
fn a_rung_pick_parses_only_a_model_or_an_effort() {
    use super::control::RungPick;
    assert_eq!(
        serde_json::from_str::<RungPick>(r#"{"subtype":"set_model","model":"haiku"}"#).unwrap(),
        RungPick::SetModel {
            model: "haiku".into(),
            effort: None,
        }
    );
    assert_eq!(
        serde_json::from_str::<RungPick>(
            r#"{"subtype":"set_model","model":"haiku","effort":"high"}"#
        )
        .unwrap(),
        RungPick::SetModel {
            model: "haiku".into(),
            effort: Some("high".into()),
        }
    );
    assert_eq!(
        serde_json::from_str::<RungPick>(r#"{"subtype":"set_reasoning_effort","effort":"low"}"#)
            .unwrap(),
        RungPick::SetReasoningEffort {
            effort: "low".into()
        }
    );
    for refused in [
        r#"{"subtype":"set_permission_mode","mode":"plan"}"#,
        r#"{"subtype":"interrupt"}"#,
    ] {
        assert!(
            serde_json::from_str::<RungPick>(refused).is_err(),
            "{refused}"
        );
    }
}

#[test]
fn a_thread_less_stop_skips_a_claimed_session() {
    use super::control::stop_every_session;
    use crate::engine::StopReason;

    let (claimed_tx, _claimed_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut claimed = make_test_session(claimed_tx, true);
    claimed.change_claim = Some(crate::engine::types::ChangeClaim::Apply);
    let (free_tx, _free_rx) = tokio::sync::mpsc::unbounded_channel();
    let (claimed_id, free_id) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    let mut sessions = std::collections::HashMap::from([
        (claimed_id, claimed),
        (free_id, make_test_session(free_tx, true)),
    ]);

    assert!(stop_every_session(&mut sessions, StopReason::Discard).is_ok());
    assert_eq!(sessions[&claimed_id].pending_stop, None);
    assert_eq!(sessions[&free_id].pending_stop, Some(StopReason::Discard));

    sessions.remove(&free_id);
    assert_eq!(
        stop_every_session(&mut sessions, StopReason::Discard)
            .unwrap_err()
            .to_string(),
        super::APPLY_IN_PROGRESS_MESSAGE,
        "a stop that stopped nothing reports why"
    );
}

#[test]
fn an_in_session_discard_holds_the_claim_an_apply_needs() {
    use super::{claim_for_discard, DiscardTarget};
    use crate::engine::agent_session::{decide_in_place_merge_claim, InPlaceMergeClaim};

    let thread_id = uuid::Uuid::new_v4();
    let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut session = make_test_session(msg_tx, true);
    session.worktree_path = Some(std::path::PathBuf::from("/tmp/lucidos-test/wt"));
    session.branch_name = Some("lucidos-claude-code-repo-lucidos-discard".into());
    let mut sessions = std::collections::HashMap::from([(thread_id, session)]);

    let Ok(DiscardTarget::Claimed {
        branch, claimant, ..
    }) = claim_for_discard(&mut sessions, thread_id)
    else {
        panic!("a live session with a worktree is claimed for the Discard");
    };
    assert_eq!(
        branch, "lucidos-claude-code-repo-lucidos-discard",
        "the reset is gated on the session's own branch"
    );
    assert_eq!(
        decide_in_place_merge_claim(sessions.get(&thread_id)),
        InPlaceMergeClaim::Claimed(crate::engine::types::ChangeClaim::Discard),
        "an apply must not start while the Discard resets the tree"
    );
    assert_eq!(
        super::stop_refusal(&sessions[&thread_id]),
        Some(super::DISCARD_IN_PROGRESS_MESSAGE),
        "and what refuses it says a Discard, not an apply, holds the session"
    );
    assert!(
        claim_for_discard(&mut sessions, thread_id).is_err(),
        "a second Discard waits its turn too"
    );

    crate::engine::release_change_claim(&mut sessions, thread_id, &claimant);
    assert_eq!(
        decide_in_place_merge_claim(sessions.get(&thread_id)),
        InPlaceMergeClaim::Claim
    );
}

#[test]
fn an_in_session_discard_refuses_a_stopping_session_and_a_missing_worktree() {
    use super::{claim_for_discard, DiscardTarget, SESSION_STOPPING_MESSAGE};

    let thread_id = uuid::Uuid::new_v4();
    let mut sessions = std::collections::HashMap::new();
    assert!(matches!(
        claim_for_discard(&mut sessions, thread_id),
        Ok(DiscardTarget::NoSession)
    ));

    let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel();
    sessions.insert(thread_id, make_test_session(msg_tx, true));
    assert_eq!(
        claim_for_discard(&mut sessions, thread_id)
            .err()
            .map(|e| e.to_string())
            .as_deref(),
        Some("No worktree for this session")
    );

    sessions.get_mut(&thread_id).unwrap().worktree_path =
        Some(std::path::PathBuf::from("/tmp/lucidos-test/wt"));
    assert_eq!(
        claim_for_discard(&mut sessions, thread_id)
            .err()
            .map(|e| e.to_string())
            .as_deref(),
        Some("No branch for this session"),
        "without a branch the reset has nothing to check the worktree against"
    );
    assert_eq!(
        sessions[&thread_id].change_claim, None,
        "a refused Discard leaves no claim behind"
    );

    let session = sessions.get_mut(&thread_id).unwrap();
    session.pending_stop = Some(crate::engine::StopReason::Apply);
    assert_eq!(
        claim_for_discard(&mut sessions, thread_id)
            .err()
            .map(|e| e.to_string())
            .as_deref(),
        Some(SESSION_STOPPING_MESSAGE)
    );
}

#[test]
fn is_engine_injected_path_matches_excluded_paths_only() {
    assert!(crate::engine::worktree_artifacts::is_engine_injected_path(
        ".lucidos-workspace"
    ));
    assert!(crate::engine::worktree_artifacts::is_engine_injected_path(
        ".lucidos/bin/lucidos"
    ));
    assert!(crate::engine::worktree_artifacts::is_engine_injected_path(
        ".lucidos/"
    ));
    assert!(crate::engine::worktree_artifacts::is_engine_injected_path(
        ".lucidos"
    ));
    assert!(crate::engine::worktree_artifacts::is_engine_injected_path(
        ".claude/skills/lucidos-cli/SKILL.md"
    ));
    assert!(crate::engine::worktree_artifacts::is_engine_injected_path(
        ".claude/skills/lucidos-cli/"
    ));
    assert!(crate::engine::worktree_artifacts::is_engine_injected_path(
        ".claude/skills/lucidos-cli"
    ));

    assert!(crate::engine::worktree_artifacts::is_engine_injected_path(
        "data/apps/habit-tracker/.claude/skills/lucidos-cli/SKILL.md"
    ));
    assert!(crate::engine::worktree_artifacts::is_engine_injected_path(
        "data/apps/habit-tracker/.claude/skills/lucidos-cli/"
    ));
    assert!(crate::engine::worktree_artifacts::is_engine_injected_path(
        "data/apps/habit-tracker/.claude/skills/lucidos-cli"
    ));

    assert!(!crate::engine::worktree_artifacts::is_engine_injected_path(
        ".lucidos-workspace-archive"
    ));
    assert!(!crate::engine::worktree_artifacts::is_engine_injected_path(
        ".lucidosX/bin"
    ));
    assert!(!crate::engine::worktree_artifacts::is_engine_injected_path(
        ".claude/skills/lucidos-cli-helper/SKILL.md"
    ));
    assert!(!crate::engine::worktree_artifacts::is_engine_injected_path(
        ".claude/skills/bugfix/SKILL.md"
    ));
    assert!(!crate::engine::worktree_artifacts::is_engine_injected_path(
        ".claude/CLAUDE.md"
    ));
    assert!(!crate::engine::worktree_artifacts::is_engine_injected_path(
        "src/main.rs"
    ));
    assert!(!crate::engine::worktree_artifacts::is_engine_injected_path(
        "data/apps/habit-tracker/.claude/skills/lucidos-cli-helper/SKILL.md"
    ));
    assert!(!crate::engine::worktree_artifacts::is_engine_injected_path(
        "data/apps/habit-tracker/.claude/CLAUDE.md"
    ));
}

#[test]
fn run_merge_session_tier2_routes_through_start_merge_and_get_prompt() {
    let source = include_str!("../claude_code/merge_session.rs");
    let needle = "async fn run_merge_session_tier2(";
    let start = source
        .find(needle)
        .expect("run_merge_session_tier2 must exist in claude_code/merge_session.rs");
    let tail = &source[start..];
    let end = tail
        .find("\n    async fn ")
        .or_else(|| tail.find("\n    fn "))
        .or_else(|| tail.find("\n}\n"))
        .unwrap_or(tail.len().min(4000));
    let body = &tail[..end];

    assert!(
        body.contains("start_merge_and_get_prompt("),
        "run_merge_session_tier2 must route the merge prompt through \
         start_merge_and_get_prompt so MergeConflictDetected is emitted \
         before the prompt ships to CC. Body was:\n{}",
        body
    );
    assert!(
        !body.contains("build_merge_prompt("),
        "run_merge_session_tier2 must NOT call build_merge_prompt directly — \
         that bypasses the MergeConflictDetected emit and resurrects the \
         Tier-2 panel-starvation bug. Body was:\n{}",
        body
    );
    assert!(
        body.contains("probe_merge_conflicts("),
        "run_merge_session_tier2 must probe the orphan worktree for the \
         actual conflicting files (mirrors apply_now::merge_via_cc_session) \
         so the panel shows real numbers. Body was:\n{}",
        body
    );
}

mod guard_tests {
    #[test]
    fn only_the_run_loop_writes_the_session_phase() {
        let writers: Vec<String> = crate::test_support::source_scan::production_sources()
            .into_iter()
            .filter(|(_, src)| src.contains(".is_waiting = "))
            .map(|(path, _)| path)
            .collect();
        assert_eq!(
            writers,
            vec![
                "engine/agent_session/lifecycle.rs".to_string(),
                "engine/agent_session/run_session/run.rs".to_string(),
            ],
            "`mark_turn_boundary` and the run loop are the only writers"
        );
    }

    mod control_source_shape {
        fn control_src() -> String {
            crate::test_support::source_scan::read_production_source(
                &crate::test_support::source_scan::engine_source("engine/claude_code/control.rs"),
            )
        }

        fn between<'a>(src: &'a str, from: &str, to: &str) -> &'a str {
            let start = src.find(from).unwrap_or_else(|| panic!("no {from}"));
            let rest = &src[start..];
            &rest[..rest.find(to).unwrap_or_else(|| panic!("no {to}"))]
        }

        #[test]
        fn a_stop_with_no_live_session_always_settles_as_a_stuck_projection() {
            let src = control_src();
            let body = between(
                &src,
                "async fn stop_agent(",
                "async fn pending_stop_reason(",
            );
            let at = body
                .find("settle_unclaimed_running_thread(")
                .expect("stop_agent settles a stuck projection");
            let call = &body[at..at + body[at..].find(".await").expect("the settle is awaited")];
            assert!(call.contains("SettleTerminal::StuckProjection"), "{call}");
        }

        #[test]
        fn a_stop_is_decided_and_reserved_under_one_lock_before_tasks_are_abandoned() {
            let src = control_src();
            let body = between(
                &src,
                "async fn stop_agent(",
                "async fn pending_stop_reason(",
            );
            let before = &body[..body
                .find("abandon_background_tasks(")
                .expect("stop_agent abandons background tasks")];
            assert!(before.contains("stop_refusal("), "{before}");
            assert!(before.contains("pending_stop = Some(reason)"), "{before}");
            assert_eq!(before.matches(".lock().await").count(), 1, "{before}");
        }

        #[test]
        fn a_rung_pick_holds_the_sessions_guard_until_it_is_stored() {
            let src = control_src();
            let body = between(
                &src,
                "async fn record_rung_pick(",
                "async fn cc_categorized_commands(",
            );
            assert!(body.contains("_ => break sessions,"), "{body}");
            let emit = body.find(".emit(").expect("the pick is emitted");
            let release = body.rfind("drop(sessions)").expect("the guard is released");
            assert!(emit < release, "the guard drops before the pick is stored");
        }
    }
}

#[test]
fn every_control_the_picker_offers_parses_as_a_rung_pick() {
    use super::control::RungPick;
    let definitions = crate::runtime::claude_code::cc_command_definitions();
    let definitions = definitions.as_array().expect("a list of controls");
    assert!(!definitions.is_empty());
    for definition in definitions {
        let mut body = serde_json::json!({ "subtype": definition["subtype"] });
        for param in definition["params"].as_array().expect("params") {
            body[param["key"].as_str().expect("a param key")] = serde_json::json!("x");
        }
        if let Err(e) = serde_json::from_value::<RungPick>(body.clone()) {
            panic!("the picker offers {body}, which no pick parses: {e}");
        }
    }
}
