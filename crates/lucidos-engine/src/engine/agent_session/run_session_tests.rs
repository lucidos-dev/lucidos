/// Verifies that `try_wait()` detects a dead child process even when
/// the stdout pipe hasn't produced EOF. This is the watchdog that
/// prevents threads from getting stuck in RUNNING state after the CC
/// process is killed (e.g. macOS sleep killing the process).
#[tokio::test]
async fn try_wait_detects_dead_cc_process() {
    use tokio::process::Command;

    // Spawn a short-lived process
    let mut child = Command::new("echo")
        .arg("done")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn 'echo'");

    // Take stdin/stdout (same as the real CC code does)
    let _stdin = child.stdin.take();
    let _stdout = child.stdout.take();

    // Wait for process to exit (with explicit wait, not just sleep)
    let _ = child.wait().await;

    // try_wait should report the exit status
    let status = child.try_wait().expect("try_wait should not error");
    assert!(
        status.is_some(),
        "try_wait must detect dead process even after stdin/stdout are taken"
    );
    assert!(
        status.unwrap().success(),
        "process should have exited successfully"
    );
}

/// Verifies that `try_wait()` returns None for a still-running process.
/// This ensures the watchdog doesn't false-positive on healthy Claude Code sessions.
#[tokio::test]
async fn try_wait_returns_none_for_running_process() {
    use tokio::process::Command;

    let mut child = Command::new("sleep")
        .arg("10")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn 'sleep'");

    let _stdin = child.stdin.take();
    let _stdout = child.stdout.take();

    let status = child.try_wait().expect("try_wait should not error");
    assert!(
        status.is_none(),
        "try_wait must return None for a running process"
    );

    // Clean up
    let _ = child.kill().await;
}

/// Integration: propose → apply → resume. The text handed to the resumed
/// agent's input channel (built by `build_resume_prompt_text`, the single
/// injection point used by both CC and Codex) must be prefixed with the
/// applied-change note and end with the user's message.
#[tokio::test]
async fn resume_prompt_is_prefixed_with_applied_change_note() {
    use super::run::{build_resume_prompt_text, ResumeSpawnContext};
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
    use crate::test_support::{setup_test_db, start_cc_session, teardown_test_db};
    use uuid::Uuid;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let branch = "claude-code/applied-resume";

    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };

    let message_received = |text: &str| ThreadEvent::MessageReceived {
        text: text.into(),
        user_image_hashes: vec![],
        device_id: None,
        device: None,
        image_description: None,
        parent_thread_id: None,
        spawning_event_id: None,
        mode: ActorMode::Human,
        model: None,
        reasoning_effort: None,
        origin: None,
    };

    start_cc_session(&bus, thread_id, branch, None).await;

    // The agent's prior turn: user message (MessageReceived, the real CC
    // user-message event), then a proposed change.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: message_received("add the feature"),
        meta: cc_meta.clone(),
    })
    .await
    .unwrap();
    let change_id = Uuid::new_v4().to_string();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: change_id.clone(),
            description: Some("the feature".into()),
            files: vec!["src/feature.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: branch.into(),
            repo_root: "/tmp/repo".into(),
            hardened: true,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: cc_meta.clone(),
    })
    .await
    .unwrap();

    // The user clicks Apply: the change is merged to main.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
            change_id,
            requires_restart: false,
            client_update: false,
            commits: vec!["feat: the feature".into()],
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: Some("abc12345def".into()),
            path: String::new(),
        },
        meta: cc_meta.clone(),
    })
    .await
    .unwrap();

    // Resume with a non-empty user message. The engine persists the current
    // turn's MessageReceived BEFORE building the prompt (see chat/process/run.rs
    // "Emit MessageReceived FIRST"), so emit it and use its id as the origin.
    let user_message = "now also add tests";
    let origin_id = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: message_received(user_message),
            meta: cc_meta.clone(),
        })
        .await
        .unwrap()
        .expect("emit produced no EmitResult")
        .event_id;
    // No worktree/adoption notes here — we isolate the applied-change path.
    let final_text = build_resume_prompt_text(
        &pool,
        thread_id,
        origin_id,
        user_message,
        ResumeSpawnContext {
            worktree_path: None,
            last_idle_sha: None,
            adoption_note: None,
            session_branch: Some(branch),
        },
    )
    .await;

    assert!(
        final_text.starts_with("[Note from engine:"),
        "prompt not prefixed with note: {final_text}"
    );
    assert!(
        final_text.to_lowercase().contains("applied"),
        "note missing 'applied': {final_text}"
    );
    assert!(
        final_text.contains("feat: the feature"),
        "note missing commit subject: {final_text}"
    );
    assert!(
        final_text.contains("abc12345"),
        "note missing short sha: {final_text}"
    );
    assert!(
        final_text.ends_with(user_message),
        "prompt must end with the user message: {final_text}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Integration: propose → discard → resume. The mirror of the applied-change
/// case, and the gap this work closes: the replayed conversation still believes
/// the change is pending, so the prompt must carry the discard note.
#[tokio::test]
async fn resume_prompt_is_prefixed_with_discarded_change_note() {
    use super::run::{build_resume_prompt_text, ResumeSpawnContext};
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
    use crate::test_support::{setup_test_db, start_cc_session, teardown_test_db};
    use uuid::Uuid;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let branch = "claude-code/discarded-resume";

    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };

    let message_received = |text: &str| ThreadEvent::MessageReceived {
        text: text.into(),
        user_image_hashes: vec![],
        device_id: None,
        device: None,
        image_description: None,
        parent_thread_id: None,
        spawning_event_id: None,
        mode: ActorMode::Human,
        model: None,
        reasoning_effort: None,
        origin: None,
    };

    start_cc_session(&bus, thread_id, branch, None).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: message_received("add the feature"),
        meta: cc_meta.clone(),
    })
    .await
    .unwrap();
    let change_id = Uuid::new_v4().to_string();
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeProposed {
            change_id: change_id.clone(),
            description: Some("the feature".into()),
            files: vec!["src/feature.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: branch.into(),
            repo_root: "/tmp/repo".into(),
            hardened: true,
            incomplete: false,
            path: String::new(),
            diff: String::new(),
        },
        meta: cc_meta.clone(),
    })
    .await
    .unwrap();

    // The user clicks Discard: the branch is reset to main HEAD and the
    // worktree cleaned, so the change's commits are gone.
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeDiscarded {
            change_id,
            actor: None,
            path: String::new(),
        },
        meta: cc_meta.clone(),
    })
    .await
    .unwrap();

    let user_message = "so where did that end up?";
    let origin_id = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: message_received(user_message),
            meta: cc_meta.clone(),
        })
        .await
        .unwrap()
        .expect("emit produced no EmitResult")
        .event_id;
    let final_text = build_resume_prompt_text(
        &pool,
        thread_id,
        origin_id,
        user_message,
        ResumeSpawnContext {
            worktree_path: None,
            last_idle_sha: None,
            adoption_note: None,
            session_branch: Some(branch),
        },
    )
    .await;

    assert!(
        final_text.starts_with("[Note from engine:"),
        "prompt not prefixed with note: {final_text}"
    );
    assert!(
        final_text.contains("DISCARDED"),
        "note missing the discard: {final_text}"
    );
    assert!(
        final_text.contains(branch),
        "note must name the branch that was reset: {final_text}"
    );
    assert!(
        final_text.contains("do not offer to Apply"),
        "note must stop the agent re-offering Apply: {final_text}"
    );
    assert!(
        final_text.ends_with(user_message),
        "prompt must end with the user message: {final_text}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}
