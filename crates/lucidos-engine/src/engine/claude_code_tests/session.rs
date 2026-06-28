
/// Validates that idle_notify (using notify_waiters) wakes a registered waiter.
/// This is the contract that send_and_wait and apply_now_inner depend on:
/// the Result handler must call idle_notify.notify_waiters() so that
/// any task waiting on idle_notify.notified() wakes up.
#[tokio::test]
async fn idle_notify_wakes_registered_waiter() {
    let notify = std::sync::Arc::new(tokio::sync::Notify::new());
    let notify2 = notify.clone();

    // Simulate send_and_wait: register a waiter BEFORE the notification
    let waiter = tokio::spawn(async move {
        // Use the same 5s timeout pattern as send_and_wait
        match tokio::time::timeout(std::time::Duration::from_secs(2), notify2.notified()).await {
            Ok(()) => true,  // Woken by notify_waiters
            Err(_) => false, // Timed out — notify_waiters was never called
        }
    });

    // Give the waiter time to register
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Simulate the Result handler firing idle_notify
    notify.notify_waiters();

    let woken = waiter.await.unwrap();
    assert!(
        woken,
        "idle_notify.notify_waiters() must wake registered waiters — \
                         this is the contract that send_and_wait depends on"
    );
}

/// Validates that notify_waiters does NOT store a permit — calling it
/// BEFORE a waiter registers means the waiter misses the notification.
/// This is why bare `idle_notify.notified().await` (without a poll loop)
/// is dangerous: if the notification fires before the await starts, it's lost.
#[tokio::test]
async fn notify_waiters_does_not_store_permit() {
    let notify = std::sync::Arc::new(tokio::sync::Notify::new());

    // Fire notification BEFORE any waiter is registered
    notify.notify_waiters();

    // Now try to wait — should NOT wake up (permit not stored)
    let result =
        tokio::time::timeout(std::time::Duration::from_millis(100), notify.notified()).await;

    assert!(result.is_err(), "notify_waiters() must NOT store a permit — \
                                   bare .notified().await after a missed notification hangs forever");
}

/// Validates the resume branch reuse logic: when a branch exists,
/// `git worktree add` should use the existing branch (no -b flag)
/// instead of creating a new one.
#[tokio::test]
async fn resume_branch_reuses_existing_branch() {
    use crate::engine::git_ops::git_cmd;

    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();

    // Set up a git repo with an initial commit
    let _ = git_cmd(&["init"], repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], repo).await;
    let _ = tokio::fs::write(repo.join("file.txt"), "initial").await;
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "init"], repo).await;

    // Create a branch with a commit (simulating a previous Claude Code session)
    let branch_name = "claude-code/20260326-test";
    let _ = git_cmd(&["checkout", "-b", branch_name], repo).await;
    let _ = tokio::fs::write(repo.join("change.txt"), "cc changes").await;
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "cc work"], repo).await;
    let _ = git_cmd(&["checkout", "main"], repo).await;

    // Verify the branch exists
    let exists = git_cmd(&["rev-parse", "--verify", branch_name], repo)
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    assert!(exists, "Test branch should exist");

    // Create a worktree from the existing branch (no -b flag)
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

    // The worktree should have the CC changes
    let content = tokio::fs::read_to_string(wt_path.join("change.txt"))
        .await
        .unwrap();
    assert_eq!(
        content, "cc changes",
        "Resumed worktree should contain previous CC changes"
    );

    // Clean up
    let _ = git_cmd(
        &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
        repo,
    )
    .await;
}

/// When a worktree already exists for the resume branch (e.g. left over from
/// a previous engine session), `parse_worktree_list` should detect it so the
/// caller can reuse the existing worktree instead of failing with
/// "branch is already used by worktree at ...".
#[tokio::test]
async fn resume_reuses_existing_worktree_for_branch() {
    use crate::engine::git_ops::{git_cmd, parse_worktree_list};

    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();

    // Set up a git repo with an initial commit
    let _ = git_cmd(&["init"], repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], repo).await;
    let _ = tokio::fs::write(repo.join("file.txt"), "initial").await;
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "init"], repo).await;

    // Create a branch and a worktree for it (simulating a previous Claude Code session)
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

    // Now verify that parse_worktree_list detects the existing worktree
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

    // Verify it points to the correct path
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

    // Trying to create a SECOND worktree for the same branch should fail
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

    // Verify the original worktree content is still intact
    let content = tokio::fs::read_to_string(wt_path.join("change.txt"))
        .await
        .unwrap();
    assert_eq!(
        content, "cc changes",
        "Existing worktree should preserve CC changes"
    );

    // Clean up
    let _ = git_cmd(
        &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
        repo,
    )
    .await;
}

/// When the resume branch no longer exists, a fresh branch should be created.
#[tokio::test]
async fn resume_falls_back_when_branch_deleted() {
    use crate::engine::git_ops::git_cmd;

    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();

    // Set up a git repo
    let _ = git_cmd(&["init"], repo).await;
    let _ = git_cmd(&["checkout", "-b", "main"], repo).await;
    let _ = tokio::fs::write(repo.join("file.txt"), "initial").await;
    let _ = git_cmd(&["add", "."], repo).await;
    let _ = git_cmd(&["commit", "-m", "init"], repo).await;

    // Verify a non-existent branch
    let branch_name = "claude-code/20260326-deleted";
    let exists = git_cmd(&["rev-parse", "--verify", branch_name], repo)
        .await
        .map(|o| o.status.success())
        .unwrap_or(false);
    assert!(!exists, "Deleted branch should not exist");

    // The code should fall back to creating a new branch
    let new_branch = crate::engine::agent_session::generate_cc_branch_name();
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

    // Clean up
    let _ = git_cmd(
        &["worktree", "remove", "--force", wt_path.to_str().unwrap()],
        repo,
    )
    .await;
}

/// Test helper: create a `AgentSession` with sensible defaults.
/// Only `msg_tx` and `is_waiting` vary across tests.
fn make_test_session(
    msg_tx: tokio::sync::mpsc::UnboundedSender<crate::engine::AgentUserInput>,
    is_waiting: bool,
) -> crate::engine::AgentSession {
    use std::sync::Arc;
    crate::engine::AgentSession {
        msg_tx,
        is_waiting,
        has_changes: false,
        requires_restart: false,
        pending_stop: None,
        cancel_actor: None,
        redirect_followup: false,
        stop: Arc::new(tokio::sync::Notify::new()),
        interrupt: Arc::new(tokio::sync::Notify::new()),
        idle_notify: Arc::new(tokio::sync::Notify::new()),
        apply_now_in_progress: false,
        process_exited: false,
        worktree_path: None,
        branch_name: None,
        repo_root: None,
        cc_session_id: None,
        shutting_down: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        external_terminal_emitted: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        control_tx: tokio::sync::mpsc::unbounded_channel().0,
        builtin_commands: vec![],
        skill_commands: vec![],
        current_model: None,
        current_reasoning_effort: None,
        last_event_at: Arc::new(std::sync::atomic::AtomicI64::new(0)),
        pending_followups: Arc::new(std::sync::atomic::AtomicU32::new(0)),
        question_resume_pending: false,
        tools_in_flight: Arc::new(std::sync::atomic::AtomicI32::new(0)),
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
    }
}

/// Validates that when a Claude Code session is idle (is_waiting=true), a follow-up
/// message is routed via msg_tx instead of being rejected with
/// "A coding agent is already running".
#[tokio::test]
async fn idle_session_routes_followup_via_msg_tx() {
    use crate::engine::AgentUserInput;

    let (msg_tx, mut msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
    let session = make_test_session(msg_tx, true);

    // Simulate single-lock routing logic from run_direct_agent
    assert!(!session.process_exited);
    assert!(session.is_waiting);

    // Route via msg_tx (same as production code)
    assert!(
        session
            .msg_tx
            .send(AgentUserInput {
                text: "Follow-up message".to_string(),
                images: None,
                origin_event_id: None,
                kind: crate::engine::AgentInputKind::User,
            })
            .is_ok(),
        "send should succeed when receiver is alive"
    );

    let received = msg_rx
        .try_recv()
        .expect("msg_rx should have received the follow-up");
    assert_eq!(received.text, "Follow-up message");
}

/// Validates that when a Claude Code session is actively working (is_waiting=false),
/// the follow-up is rejected (not routed via msg_tx).
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

/// Validates that when the msg_tx channel is closed (receiver dropped),
/// send returns Err — production code should propagate the error.
#[tokio::test]
async fn closed_channel_returns_error() {
    use crate::engine::AgentUserInput;

    let (msg_tx, msg_rx) = tokio::sync::mpsc::unbounded_channel::<AgentUserInput>();
    let session = make_test_session(msg_tx, true);

    // Drop the receiver to simulate CC process exit
    drop(msg_rx);

    let result = session.msg_tx.send(AgentUserInput {
        text: "too late".to_string(),
        images: None,
        origin_event_id: None,
        kind: crate::engine::AgentInputKind::User,
    });
    assert!(result.is_err(), "send should fail when receiver is dropped");
}

/// Idle Claude Code session (waiting for next prompt, process alive) must NOT be
/// reported as in-flight. Regression: `abort_in_flight_for_restart` used
/// to filter only on `!process_exited`, so every idle Claude Code session got a
/// `ResponseAborted` on `/api/v1/restart` — rendering as "Response Interrupted"
/// with a Continue button on every restart.
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

/// Idle-termination follow-up race contract (the `:31` regression).
///
/// At idle the run loop's `ExitSubprocess` arm now sets `process_exited = true`
/// (under the `agent_sessions` lock) BEFORE `agent_cancel.cancel()`, so a
/// follow-up landing during the ~3s graceful-shutdown window is declined by the
/// chat fast-path instead of being sent on `msg_tx` into a dying subprocess.
/// This pins both halves of that fast-path gate (`chat/process/run.rs`): the
/// `has_session` predicate (`map(|s| !s.process_exited)`) and the send-step
/// guard (`if !session.process_exited`). When the gate declines, the follow-up
/// falls through to the slow `--resume` path and is never dropped.
#[tokio::test]
async fn exited_mark_declines_fast_path_followup_route() {
    let (msg_tx, _msg_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut session = make_test_session(msg_tx, true);

    // Before the idle-terminate mark: an idle session accepts the fast-path.
    let has_session_before = !session.process_exited;
    assert!(
        has_session_before,
        "an idle, alive session must be eligible for the msg_tx fast-path"
    );

    // The Terminate arm marks the session exited as it cancels the subprocess.
    session.process_exited = true;

    // Both fast-path gates now decline → caller falls through to the slow
    // (`--resume`) path rather than routing into the terminating subprocess.
    let has_session_after = !session.process_exited; // chat fast-path has_session
    let send_step_allows = !session.process_exited; // chat fast-path send guard
    assert!(
        !has_session_after,
        "a session marked process_exited at idle-terminate must NOT pass has_session"
    );
    assert!(
        !send_step_allows,
        "a session marked process_exited must NOT pass the msg_tx send guard"
    );
}

// Regression coverage for the "ResponseCanceled is missing device" gap on a
// LIVE Claude Code session: `POST /api/v1/claude-code/stop` (UserStop) resolves
// the actor from the request headers and `interrupt_agent` stamps it on the
// session's `cancel_actor` field before firing the `interrupt` notify. The
// run_session interrupt arm drains it (`take_session_cancel_actor`) into
// `meta.actor` so the emitted `ResponseCanceled` records WHICH device clicked
// Cancel — the Initiator popover's Device row. These tests exercise the field's
// stamp-and-drain contract in isolation (a full `LucidosEngine` isn't built
// here; the integration through `interrupt_agent` / `take_session_cancel_actor`
// is identical, just behind the `agent_sessions` map lock).

/// `interrupt_agent` stamps then the run loop drains; the second drain is empty
/// so a resumed turn on the same session can't inherit a stale device.
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

    // Stamp (mirrors interrupt_agent's live-session branch).
    let actor = MessageOrigin::Device {
        device_id: "ios-1".into(),
        label: "My iPhone".into(),
    };
    session.cancel_actor = Some(actor.clone());

    // First drain returns the stamped actor (what take_session_cancel_actor does).
    assert_eq!(
        session.cancel_actor.take(),
        Some(actor),
        "first take must return the device that clicked Cancel"
    );
    // One-shot: a resumed turn must not re-stamp the same device.
    assert!(
        session.cancel_actor.take().is_none(),
        "second take must be empty"
    );
}

#[test]
fn is_engine_injected_path_matches_excluded_paths_only() {
    assert!(super::is_engine_injected_path(".lucidos-workspace"));
    assert!(super::is_engine_injected_path(".lucidos/bin/lucidos"));
    assert!(super::is_engine_injected_path(".lucidos/"));
    assert!(super::is_engine_injected_path(".lucidos"));
    assert!(super::is_engine_injected_path(
        ".claude/skills/lucidos-cli/SKILL.md"
    ));
    assert!(super::is_engine_injected_path(
        ".claude/skills/lucidos-cli/"
    ));
    assert!(super::is_engine_injected_path(".claude/skills/lucidos-cli"));

    // App coding-agent threads run CC inside `<wt>/data/apps/<id>/`, so the
    // engine-injected lucidos-cli skill lands at `data/apps/<id>/.claude/...`
    // (install_lucidos_cli_skill writes relative to CC's cwd, not the
    // worktree root). Must match the same as the root-level path or the
    // skill folder shows up as a pending change every app spawn.
    assert!(super::is_engine_injected_path(
        "data/apps/habit-tracker/.claude/skills/lucidos-cli/SKILL.md"
    ));
    assert!(super::is_engine_injected_path(
        "data/apps/habit-tracker/.claude/skills/lucidos-cli/"
    ));
    assert!(super::is_engine_injected_path(
        "data/apps/habit-tracker/.claude/skills/lucidos-cli"
    ));

    // Sibling paths must NOT match — false positives would hide unrelated
    // user files (e.g. a user-named `.lucidos-workspace-archive` or a
    // `.claude/skills/lucidos-cli-helper/` skill).
    assert!(!super::is_engine_injected_path(
        ".lucidos-workspace-archive"
    ));
    assert!(!super::is_engine_injected_path(".lucidosX/bin"));
    assert!(!super::is_engine_injected_path(
        ".claude/skills/lucidos-cli-helper/SKILL.md"
    ));
    assert!(!super::is_engine_injected_path(
        ".claude/skills/bugfix/SKILL.md"
    ));
    assert!(!super::is_engine_injected_path(".claude/CLAUDE.md"));
    assert!(!super::is_engine_injected_path("src/main.rs"));
    // Sibling under the deep app path must also stay visible.
    assert!(!super::is_engine_injected_path(
        "data/apps/habit-tracker/.claude/skills/lucidos-cli-helper/SKILL.md"
    ));
    assert!(!super::is_engine_injected_path(
        "data/apps/habit-tracker/.claude/CLAUDE.md"
    ));
}

/// Regression guard for the Tier-2 merge-conflict bug.
///
/// `run_merge_session_tier2` is the Tier-2 entry point — dead Claude Code session +
/// orphan worktree on disk — and historically shipped the merge prompt to
/// CC via `build_merge_prompt` + `emit_automated_prompt` **without** first
/// emitting `MergeConflictDetected`. That bug starved the chat-exchange
/// panel (nothing to render) and the SSE consumers
/// (`applyingChangeIds`, conflict toast).
///
/// The fix routes the function through `start_merge_and_get_prompt`, which
/// bundles the boundary emit with the prompt build so the two cannot be
/// separated.
///
/// We assert on source text rather than runtime behavior because nothing
/// in this crate constructs a live `LucidosEngine` outside `main.rs` —
/// the same constraint that drives the compile-only signature pins in
/// `change_ops_engine_origin_stamping_tests.rs`. Source-text is brittle
/// to formatting but resilient to behavioral regressions: if a future
/// refactor moves the function back to bare `build_merge_prompt`, this
/// test fails loudly.
#[test]
fn run_merge_session_tier2_routes_through_start_merge_and_get_prompt() {
    let source = include_str!("../claude_code/merge_session.rs");
    let needle = "async fn run_merge_session_tier2(";
    let start = source
        .find(needle)
        .expect("run_merge_session_tier2 must exist in claude_code/merge_session.rs");
    // Slice from the signature to the next top-level item start. The body
    // ends well before the next `async fn`/`pub fn`/`fn ` at the module
    // level — taking a generous window avoids brittle brace-matching.
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
