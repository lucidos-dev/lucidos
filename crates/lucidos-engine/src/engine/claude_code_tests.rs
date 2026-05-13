use super::{
    append_allowed_tool_pattern, cc_allowed_tools, derive_allow_pattern, hardening_succeeded,
    lookup_repo_commands_in_cache, read_allowed_tools_file, settle_stuck_running_thread,
    write_allowed_tools_file, AllowScope, CC_ALLOWED_TOOLS_HEADER, DEFAULT_CC_ALLOWED_TOOLS,
};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
use crate::engine::types::CcCommandsInfo;
use crate::test_support::{setup_test_db, teardown_test_db};
use std::collections::HashMap;
use uuid::Uuid;

fn cache_with(entries: &[(&str, &[&str])]) -> HashMap<String, CcCommandsInfo> {
    entries
        .iter()
        .map(|(path, skills)| {
            (
                (*path).to_string(),
                CcCommandsInfo {
                    builtin_commands: vec![],
                    skill_commands: skills.iter().map(|s| (*s).to_string()).collect(),
                },
            )
        })
        .collect()
}

#[test]
fn lookup_returns_cached_entry_for_matching_repo() {
    let cache = cache_with(&[("/repo/a", &["skill-a"]), ("/repo/b", &["skill-b"])]);
    let info = lookup_repo_commands_in_cache(&cache, "/repo/a");
    assert_eq!(info.skill_commands, vec!["skill-a".to_string()]);
}

/// Regression test for the bug where the compose-view menu returned
/// `cache.values().next()` — i.e., an arbitrary other repo's skills —
/// when the requested repo had no cache entry. Skills from a non-selected
/// repo must NEVER surface.
#[test]
fn lookup_returns_empty_for_unknown_repo_never_falls_back_to_other_repos() {
    let cache = cache_with(&[("/repo/a", &["skill-a"]), ("/repo/b", &["skill-b"])]);
    let info = lookup_repo_commands_in_cache(&cache, "/repo/never-cached");
    assert!(
        info.skill_commands.is_empty(),
        "must not leak other repos' skills"
    );
    assert!(info.builtin_commands.is_empty());
}

#[test]
fn lookup_returns_empty_for_empty_cache() {
    let cache: HashMap<String, CcCommandsInfo> = HashMap::new();
    let info = lookup_repo_commands_in_cache(&cache, "/any/path");
    assert!(info.skill_commands.is_empty());
    assert!(info.builtin_commands.is_empty());
}

/// Cancellation produces `Ok(_)` from the run loop, so the predicate must
/// gate on the marker (single source of truth that `/harden` finished)
/// regardless of how the session ended.
#[test]
fn hardening_succeeded_requires_marker_and_no_cancel() {
    assert!(hardening_succeeded(false, true));
    assert!(!hardening_succeeded(true, true));
    assert!(!hardening_succeeded(false, false));
    assert!(!hardening_succeeded(true, false));
}

/// Emit MessageReceived for a CC-channel thread → status='running'.
/// Mirrors what `spawn_agent_thread` does before kicking off the bg task.
async fn seed_running_cc_thread(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            text: "do the thing".into(),
            user_image_hashes: vec![],
            device_id: None,
            device: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Agent,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: EventMeta {
            channel: Some(EventChannel::CodingAgent),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

async fn read_status(pool: &sqlx::PgPool, thread_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT status FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

fn user_device_actor() -> crate::engine::thread_events::MessageOrigin {
    crate::engine::thread_events::MessageOrigin::Device {
        device_id: "test-device".into(),
        label: "Test Device".into(),
    }
}

/// User clicks Stop / Apply / Discard / Archive / Interrupt on a CC thread
/// that's stuck at status='running' (the background spawn task errored before
/// any terminal event could fire, or the CC subprocess hadn't yet registered
/// in agent_sessions when the user pressed the button). The settle helper
/// emits `ResponseAborted` with `AbortCause::StaleSettle` and the user actor:
///   - `Aborted` (not `Canceled`) because no live response existed to cancel
///     — this is system-driven cleanup of stuck projection state.
///   - `cause=StaleSettle` so the frontend renders "Settled stuck response"
///     instead of "Restarted" (device actor's default abort summary) or
///     "Response interrupted" (system actor's default abort summary).
///   - User actor so the chip reads "You" (the user *did* push the button)
///     rather than "⚙ System".
///   - Thread status lands at `idle` (not `failed`): the projection branches
///     on `cause=StaleSettle` to use the cancel-style status mapping, so the
///     thread list doesn't show a red error indicator on a thread the user
///     just deliberately settled.
#[tokio::test]
async fn settle_stuck_running_thread_emits_aborted_stale_settle_with_user_actor() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    seed_running_cc_thread(&bus, thread_id).await;
    assert_eq!(
        read_status(&pool, thread_id).await.as_deref(),
        Some("running")
    );

    let did_emit = settle_stuck_running_thread(&pool, &bus, thread_id, Some(user_device_actor()))
        .await
        .unwrap();
    assert!(did_emit, "stuck running thread should be settled");

    // Exactly one ResponseAborted, zero ResponseCanceled — moving stale-settle
    // to abort means no ghost "Canceled the response" appears on a thread that
    // wasn't actually mid-response.
    let aborted_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ResponseAborted'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        aborted_count, 1,
        "exactly one ResponseAborted must be persisted"
    );

    let canceled_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ResponseCanceled'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        canceled_count, 0,
        "stale-settle is an abort, not a cancel — no live response existed to cancel"
    );

    let payload: serde_json::Value = sqlx::query_scalar(
        "SELECT payload FROM events \
             WHERE aggregate_id = $1 AND event_type = 'ResponseAborted'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        payload["cause"], "stale_settle",
        "cause must be stale_settle so the summary reads 'Settled stuck response'"
    );
    assert_eq!(
        payload["actor"]["kind"],
        "device",
        "actor.kind must be 'device' (user from a known device) so the chip reads 'You'"
    );
    assert_eq!(payload["actor"]["device_id"], "test-device");

    // Thread status: stale-settle must land at `idle`, not the default
    // ResponseAborted bucket of `failed`. Otherwise the thread list shows a
    // red error indicator on a thread the user just deliberately settled.
    assert_eq!(
        read_status(&pool, thread_id).await.as_deref(),
        Some("idle"),
        "stale-settle must use the cancel-style status mapping (idle), not the \
             default ResponseAborted bucket (failed)"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Idempotency: settling a thread that's already non-running is a no-op
/// (so that double-clicks on the stop button don't pile up events).
#[tokio::test]
async fn settle_stuck_running_thread_no_op_when_already_settled() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    seed_running_cc_thread(&bus, thread_id).await;
    // First settle transitions running → failed.
    assert!(
        settle_stuck_running_thread(&pool, &bus, thread_id, Some(user_device_actor()))
            .await
            .unwrap()
    );
    // Second settle should be a no-op.
    assert!(
        !settle_stuck_running_thread(&pool, &bus, thread_id, Some(user_device_actor()))
            .await
            .unwrap()
    );

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM events WHERE aggregate_id = $1 AND event_type = 'ResponseAborted'",
    )
    .bind(thread_id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "second settle must not emit a duplicate event");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Contrast test: a real (non-stale-settle) `ResponseAborted` still lands in
/// the `failed` bucket. The stale-settle special case in the projection
/// (`ResponseAborted { cause: StaleSettle }` → idle) must not over-apply to
/// other abort causes — engine shutdowns, safety-net crashes, etc. still
/// surface the red error indicator on the thread list.
#[tokio::test]
async fn response_aborted_non_stale_settle_lands_in_failed_status() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();

    seed_running_cc_thread(&bus, thread_id).await;

    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event: crate::engine::thread_events::ThreadEvent::ResponseAborted {
            text: String::new(),
            images: vec![],
            model: None,
            reasoning_effort: None,
            cause: crate::engine::thread_events::AbortCause::SafetyNet,
        },
        meta: crate::engine::thread_events::EventMeta::NONE,
    })
    .await
    .unwrap();

    assert_eq!(
        read_status(&pool, thread_id).await.as_deref(),
        Some("failed"),
        "non-stale-settle aborts must keep the default 'failed' bucket"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A thread that the projection never knew about (no thread_summaries row)
/// is also a no-op — interrupt of an unknown id should not emit phantom
/// events for non-existent threads.
#[tokio::test]
async fn settle_stuck_running_thread_no_op_for_unknown_thread() {
    let (pool, db_name) = setup_test_db().await;
    let (bus, _callback_rx) = EventBus::new(pool.clone());

    let did_emit =
        settle_stuck_running_thread(&pool, &bus, Uuid::new_v4(), Some(user_device_actor()))
            .await
            .unwrap();
    assert!(!did_emit);

    pool.close().await;
    teardown_test_db(&db_name).await;
}

#[test]
fn cc_allowed_tools_returns_default_when_user_dir_missing() {
    assert_eq!(cc_allowed_tools(None), DEFAULT_CC_ALLOWED_TOOLS.join(","));
}

#[test]
fn cc_allowed_tools_seeds_empty_default_file_on_first_use() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cc-allowed-tools");
    assert!(!path.exists());

    let result = cc_allowed_tools(Some(dir.path()));

    assert_eq!(result, "", "default allowlist must be empty");
    assert!(path.exists(), "seed file should have been written");
    let seeded = std::fs::read_to_string(&path).unwrap();
    assert_eq!(seeded, CC_ALLOWED_TOOLS_HEADER);
}

#[test]
fn derive_allow_pattern_skill_narrow_uses_plugin_glob() {
    let input = serde_json::json!({ "skill": "code-review:code-review" });
    assert_eq!(
        derive_allow_pattern("Skill", &input, AllowScope::Narrow).as_deref(),
        Some("Skill(code-review:*)"),
    );
}

#[test]
fn derive_allow_pattern_skill_narrow_with_no_colon_uses_full_name() {
    let input = serde_json::json!({ "skill": "loop" });
    assert_eq!(
        derive_allow_pattern("Skill", &input, AllowScope::Narrow).as_deref(),
        Some("Skill(loop:*)"),
    );
}

#[test]
fn derive_allow_pattern_skill_broad_returns_bare_tool_name() {
    let input = serde_json::json!({ "skill": "code-review:code-review" });
    assert_eq!(
        derive_allow_pattern("Skill", &input, AllowScope::Broad).as_deref(),
        Some("Skill"),
    );
}

#[test]
fn derive_allow_pattern_bash_narrow_uses_first_token() {
    let input = serde_json::json!({ "command": "git status --short" });
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Narrow).as_deref(),
        Some("Bash(git:*)"),
    );
}

#[test]
fn derive_allow_pattern_bash_broad_returns_bare_tool_name() {
    let input = serde_json::json!({ "command": "ls" });
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Broad).as_deref(),
        Some("Bash"),
    );
}

#[test]
fn derive_allow_pattern_other_tool_narrow_returns_none() {
    let input = serde_json::json!({ "file_path": "/tmp/x" });
    assert_eq!(
        derive_allow_pattern("Read", &input, AllowScope::Narrow),
        None
    );
}

#[test]
fn derive_allow_pattern_other_tool_broad_returns_bare_name() {
    let input = serde_json::json!({});
    assert_eq!(
        derive_allow_pattern("Read", &input, AllowScope::Broad).as_deref(),
        Some("Read"),
    );
}

#[test]
fn derive_allow_pattern_skill_missing_input_returns_none() {
    let input = serde_json::json!({});
    assert_eq!(
        derive_allow_pattern("Skill", &input, AllowScope::Narrow),
        None
    );
}

/// CC's `--permission-mode acceptEdits` routes parametric file-path tools
/// (Edit/Write/NotebookEdit) through `--permission-prompt-tool` for any
/// out-of-cwd path **regardless** of bare entries in `--allowedTools`.
/// Persisting bare `Edit` (etc.) silently does nothing for the very paths
/// that surfaced the prompt — so the engine must refuse to write them.
/// In-cwd paths never reach this card (acceptEdits auto-approves), so no
/// legitimate caller is denied by this guard.
#[test]
fn derive_allow_pattern_broad_returns_none_for_acceptedits_routed_tools() {
    let input = serde_json::json!({"file_path": "/x", "old_string": "a", "new_string": "b"});
    assert_eq!(
        derive_allow_pattern("Edit", &input, AllowScope::Broad),
        None,
        "broad Edit must be suppressed — bare entry doesn't bypass acceptEdits routing"
    );
    assert_eq!(
        derive_allow_pattern("Write", &input, AllowScope::Broad),
        None,
        "broad Write must be suppressed for the same reason"
    );
    assert_eq!(
        derive_allow_pattern("NotebookEdit", &input, AllowScope::Broad),
        None,
        "broad NotebookEdit must be suppressed for the same reason"
    );
}

/// CC always routes `ExitPlanMode` through `--permission-prompt-tool`
/// regardless of `--allowedTools` — plan-mode exit is the user's plan
/// approval step, not a regular tool call. Persisting bare `ExitPlanMode`
/// to the allowlist would mislead the user into thinking the prompt would
/// stop coming back; suppress so the UI hides the broad button.
#[test]
fn derive_allow_pattern_broad_returns_none_for_exit_plan_mode() {
    let input = serde_json::json!({ "plan": "Step 1: do thing" });
    assert_eq!(
        derive_allow_pattern("ExitPlanMode", &input, AllowScope::Broad),
        None,
        "broad ExitPlanMode must be suppressed — CC always prompts for plan approval"
    );
}

/// Narrow scope for the same tools is still None (no narrow pattern is
/// generated for path-tools today). Documented here so a future addition
/// of `Edit(<glob>)` patterns is an intentional change, not a side-effect.
#[test]
fn derive_allow_pattern_narrow_remains_none_for_path_tools() {
    let input = serde_json::json!({"file_path": "/x"});
    assert_eq!(
        derive_allow_pattern("Edit", &input, AllowScope::Narrow),
        None
    );
    assert_eq!(
        derive_allow_pattern("Write", &input, AllowScope::Narrow),
        None
    );
    assert_eq!(
        derive_allow_pattern("NotebookEdit", &input, AllowScope::Narrow),
        None
    );
}

#[test]
fn derive_allow_pattern_session_edit_uses_per_file_scope() {
    let input = serde_json::json!({
        "file_path": "/Users/me/repo/.claude/commands/harden.md",
        "old_string": "x",
        "new_string": "y",
    });
    assert_eq!(
        derive_allow_pattern("Edit", &input, AllowScope::Session).as_deref(),
        Some("Edit(/Users/me/repo/.claude/commands/harden.md)"),
    );
}

#[test]
fn derive_allow_pattern_session_write_uses_per_file_scope() {
    let input = serde_json::json!({
        "file_path": "/tmp/new.txt",
        "content": "hello",
    });
    assert_eq!(
        derive_allow_pattern("Write", &input, AllowScope::Session).as_deref(),
        Some("Write(/tmp/new.txt)"),
    );
}

#[test]
fn derive_allow_pattern_session_notebookedit_uses_notebook_path() {
    let input = serde_json::json!({
        "notebook_path": "/tmp/nb.ipynb",
        "new_source": "print(1)",
    });
    assert_eq!(
        derive_allow_pattern("NotebookEdit", &input, AllowScope::Session).as_deref(),
        Some("NotebookEdit(/tmp/nb.ipynb)"),
    );
}

#[test]
fn derive_allow_pattern_session_two_edits_to_same_file_match() {
    // Different `old_string`/`new_string` payloads must derive the same
    // session pattern so the second prompt auto-resolves against the first.
    let a = serde_json::json!({"file_path": "/x", "old_string": "a", "new_string": "b"});
    let b = serde_json::json!({"file_path": "/x", "old_string": "c", "new_string": "d"});
    assert_eq!(
        derive_allow_pattern("Edit", &a, AllowScope::Session),
        derive_allow_pattern("Edit", &b, AllowScope::Session),
    );
}

#[test]
fn derive_allow_pattern_session_edit_missing_path_returns_none() {
    let input = serde_json::json!({"old_string": "x"});
    assert_eq!(
        derive_allow_pattern("Edit", &input, AllowScope::Session),
        None
    );
}

#[test]
fn derive_allow_pattern_session_bash_reuses_narrow_pattern() {
    let input = serde_json::json!({"command": "git push origin main"});
    assert_eq!(
        derive_allow_pattern("Bash", &input, AllowScope::Session).as_deref(),
        Some("Bash(git:*)"),
    );
}

#[test]
fn derive_allow_pattern_session_skill_reuses_narrow_pattern() {
    let input = serde_json::json!({"skill": "superpowers:test-driven-development"});
    assert_eq!(
        derive_allow_pattern("Skill", &input, AllowScope::Session).as_deref(),
        Some("Skill(superpowers:*)"),
    );
}

/// Bare-tool fallback for tools without a narrow sub-scope. Session scope
/// is engine-side, so `BROAD_ALLOW_INEFFECTIVE` does not apply — the user
/// gets to remember any prompt for the rest of the thread.
#[test]
fn derive_allow_pattern_session_other_tool_falls_back_to_bare_name() {
    let input = serde_json::json!({"pattern": "foo"});
    assert_eq!(
        derive_allow_pattern("Read", &input, AllowScope::Session).as_deref(),
        Some("Read"),
    );
}

#[test]
fn append_allowed_tool_pattern_creates_file_with_header() {
    let dir = tempfile::tempdir().unwrap();
    append_allowed_tool_pattern(Some(dir.path()), "Skill(code-review:*)").unwrap();
    let body = std::fs::read_to_string(dir.path().join("cc-allowed-tools")).unwrap();
    assert!(body.starts_with(CC_ALLOWED_TOOLS_HEADER));
    assert!(body.trim_end().ends_with("Skill(code-review:*)"));
}

#[test]
fn append_allowed_tool_pattern_skips_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    append_allowed_tool_pattern(Some(dir.path()), "Skill").unwrap();
    append_allowed_tool_pattern(Some(dir.path()), "Skill").unwrap();
    let body = std::fs::read_to_string(dir.path().join("cc-allowed-tools")).unwrap();
    assert_eq!(body.matches("Skill\n").count(), 1);
}

#[test]
fn append_allowed_tool_pattern_appends_to_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("cc-allowed-tools"),
        "# header\nBash\nRead\n",
    )
    .unwrap();
    append_allowed_tool_pattern(Some(dir.path()), "Skill(code-review:*)").unwrap();
    let body = std::fs::read_to_string(dir.path().join("cc-allowed-tools")).unwrap();
    assert_eq!(body, "# header\nBash\nRead\nSkill(code-review:*)\n");
}

#[test]
fn append_allowed_tool_pattern_treats_existing_pattern_as_present_even_with_indent() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("cc-allowed-tools"),
        "# header\n  Skill(code-review:*)  \n",
    )
    .unwrap();
    append_allowed_tool_pattern(Some(dir.path()), "Skill(code-review:*)").unwrap();
    let body = std::fs::read_to_string(dir.path().join("cc-allowed-tools")).unwrap();
    assert_eq!(body, "# header\n  Skill(code-review:*)  \n");
}

#[test]
fn append_allowed_tool_pattern_no_op_when_user_dir_none() {
    // Should not panic and should not error.
    append_allowed_tool_pattern(None, "Bash").unwrap();
}

#[test]
fn read_allowed_tools_file_returns_header_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let body = read_allowed_tools_file(dir.path()).unwrap();
    assert_eq!(body, CC_ALLOWED_TOOLS_HEADER);
}

#[test]
fn write_then_read_allowed_tools_file_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let payload = "# notes\nBash\nSkill(meta:*)\n";
    write_allowed_tools_file(dir.path(), payload).unwrap();
    assert_eq!(read_allowed_tools_file(dir.path()).unwrap(), payload);
}

#[test]
fn default_cc_allowed_tools_is_empty() {
    assert_eq!(DEFAULT_CC_ALLOWED_TOOLS, &[] as &[&str]);
}

#[test]
fn cc_allowed_tools_parses_user_file_strips_comments_and_blanks() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("cc-allowed-tools"),
        "# header comment\n\nBash\nRead\n  Edit  \n# inline comment\nSkill(superpowers:*)\n\n",
    )
    .unwrap();

    assert_eq!(
        cc_allowed_tools(Some(dir.path())),
        "Bash,Read,Edit,Skill(superpowers:*)",
    );
}

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

    // Create a branch with a commit (simulating a previous CC session)
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

    // Create a branch and a worktree for it (simulating a previous CC session)
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
    }
}

/// Validates that when a CC session is idle (is_waiting=true), a follow-up
/// message is routed via msg_tx instead of being rejected with
/// "Claude Code is already running".
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
            })
            .is_ok(),
        "send should succeed when receiver is alive"
    );

    let received = msg_rx
        .try_recv()
        .expect("msg_rx should have received the follow-up");
    assert_eq!(received.text, "Follow-up message");
}

/// Validates that when a CC session is actively working (is_waiting=false),
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
    });
    assert!(result.is_err(), "send should fail when receiver is dropped");
}

/// Idle CC session (waiting for next prompt, process alive) must NOT be
/// reported as in-flight. Regression: `abort_in_flight_for_restart` used
/// to filter only on `!process_exited`, so every idle CC session got a
/// `ResponseAborted` on `/api/restart` — rendering as "Response Interrupted"
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
}
