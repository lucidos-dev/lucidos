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
/// agent's input channel (built by `build_resume_prompt_text`, the spawn-path
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
        provider: None,
        voice_session_id: None,
        wake_mode: None,
        text: text.into(),
        user_image_hashes: vec![],
        device_id: None,
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
            set_aside: false,
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
            landing_merge_sha: None,
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
        provider: None,
        voice_session_id: None,
        wake_mode: None,
        text: text.into(),
        user_image_hashes: vec![],
        device_id: None,
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
            set_aside: false,
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

/// The regression. A note quoting "the composer's Diff is an icon" sat directly
/// above "It needs more height. Three lines maybe?", and the pronoun bound to
/// the note instead of to the glyph the user meant. So a non-empty block closes
/// by disclaiming the topic, and a turn carrying no note gains nothing.
#[tokio::test]
async fn the_note_block_disclaims_the_topic_of_the_message_below_it() {
    use super::run::{build_resume_prompt_text, ResumeSpawnContext, RESUME_NOTE_REFERENT_GUARD};
    use crate::test_support::{setup_test_db, teardown_test_db};
    use uuid::Uuid;

    let (pool, db_name) = setup_test_db().await;
    let user_message = "It needs more height. Three lines maybe?";

    let spawn = |note: Option<&'static str>| ResumeSpawnContext {
        worktree_path: None,
        last_idle_sha: None,
        adoption_note: note,
        session_branch: None,
    };

    // One note is enough: the guard is a property of the block, not of which
    // note filled it. An empty thread contributes no turn-gap note, and no
    // worktree means no external-edit note.
    let noted = build_resume_prompt_text(
        &pool,
        Uuid::new_v4(),
        Uuid::new_v4(),
        user_message,
        spawn(Some(
            "[Note from engine: APPLIED: fix(app): the composer's Diff is an icon]",
        )),
    )
    .await;

    // Matched against the constant, not against a copy of its wording, so the
    // guard can be reworded without breaking a test about where it sits. An
    // emptied constant still fails: `find("")` returns 0, below `note_at`.
    let guard_at = noted
        .find(RESUME_NOTE_REFERENT_GUARD)
        .unwrap_or_else(|| panic!("note block must disclaim the topic: {noted}"));
    let note_at = noted
        .find("the composer's Diff")
        .expect("the note itself must survive");
    let message_at = noted
        .find(user_message)
        .expect("the user's message must survive");
    assert!(
        note_at < guard_at && guard_at < message_at,
        "the guard belongs between the notes and the message: {noted}"
    );
    assert!(
        noted.ends_with(user_message),
        "prompt must still end with the user message: {noted}"
    );

    // Nothing happened in the gap, so there is no block and nothing to
    // disclaim. The user's words reach the agent alone, as before.
    let bare = build_resume_prompt_text(
        &pool,
        Uuid::new_v4(),
        Uuid::new_v4(),
        user_message,
        spawn(None),
    )
    .await;
    assert_eq!(
        bare, user_message,
        "a turn with no note must carry no guard"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The reproduction, end to end on the path it took. A switch restarted the
/// engine mid-build, and the spawn consumer resumed the session with the bare
/// continue message. The agent answered "You've stopped a Rust build twice, so
/// I won't start another one". The resumed prompt must name the real cause
/// before it says to continue.
#[tokio::test]
async fn a_switch_resume_is_told_the_restart_stopped_the_call() {
    use super::run::{build_resume_prompt_text, ResumeSpawnContext, RESUME_NOTE_REFERENT_GUARD};
    use crate::engine::agent_recovery::{
        AUTO_RESUME_AFTER_SWITCH_REASON, CONTINUE_RESUME_USER_MESSAGE,
    };
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{
        emit_response_aborted, AbortCause, ActorMode, EventChannel, EventMeta, ThreadEvent,
    };
    use crate::test_support::{setup_test_db, start_cc_session, teardown_test_db};
    use uuid::Uuid;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let branch = "claude-code/restart-resume";
    let cc_meta = EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    };

    start_cc_session(&bus, thread_id, branch, None).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "build the preview gateway".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: None,
            spawning_event_id: None,
            mode: ActorMode::Human,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        meta: cc_meta.clone(),
    })
    .await
    .unwrap();

    // The teardown boundary `abort_in_flight_for_restart` lands mid-build.
    emit_response_aborted(
        &bus,
        thread_id,
        AbortCause::EngineShutdown,
        String::new(),
        vec![],
        None,
        None,
        cc_meta.clone(),
        "[test] teardown ResponseAborted",
    )
    .await;

    let origin_id = bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ContinuationRequested {
                reason: AUTO_RESUME_AFTER_SWITCH_REASON.into(),
            },
            meta: cc_meta,
        })
        .await
        .unwrap()
        .expect("emit produced no EmitResult")
        .event_id;

    let final_text = build_resume_prompt_text(
        &pool,
        thread_id,
        origin_id,
        CONTINUE_RESUME_USER_MESSAGE,
        ResumeSpawnContext {
            worktree_path: None,
            last_idle_sha: None,
            adoption_note: None,
            session_branch: Some(branch),
        },
    )
    .await;

    let restart_at = final_text
        .find("ENGINE RESTART")
        .unwrap_or_else(|| panic!("the resume must name the restart: {final_text}"));
    let guard_at = final_text
        .find(RESUME_NOTE_REFERENT_GUARD)
        .expect("a note block closes with the referent guard");
    assert!(
        restart_at < guard_at,
        "the restart line belongs in the note block: {final_text}"
    );
    assert!(
        final_text.contains("nobody refused or interrupted anything"),
        "the resume must clear the user of the transcript's rejection: {final_text}"
    );
    assert!(
        final_text.ends_with(CONTINUE_RESUME_USER_MESSAGE),
        "the continue message still ends the prompt: {final_text}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// A kept-alive session takes its next prompt without a respawn. A report its
/// child filed during the previous turn rides that prompt, and no later one:
/// not the next follow-up, and not the turn after the session ends.
#[tokio::test]
async fn a_report_filed_mid_turn_reaches_the_kept_alive_follow_up_once() {
    use super::run::RESUME_NOTE_REFERENT_GUARD;
    use super::run::{build_followup_prompt_text, build_resume_prompt_text, ResumeSpawnContext};
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{
        ActorMode, ChildCompletionStatus, EventChannel, EventMeta, ThreadEvent,
    };
    use crate::test_support::{setup_test_db, start_cc_session, teardown_test_db};
    use uuid::Uuid;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let branch = "claude-code/kept-alive";
    let emit = |event: ThreadEvent| {
        let bus = &bus;
        async move {
            bus.emit(BusEvent::Thread {
                thread_id,
                event,
                meta: EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    ..EventMeta::NONE
                },
            })
            .await
            .unwrap()
            .expect("emit produced no EmitResult")
            .event_id
        }
    };
    let message_received = |text: &str| ThreadEvent::MessageReceived {
        provider: None,
        voice_session_id: None,
        wake_mode: None,
        text: text.into(),
        user_image_hashes: vec![],
        device_id: None,
        image_description: None,
        parent_thread_id: None,
        spawning_event_id: None,
        mode: ActorMode::Human,
        model: None,
        reasoning_effort: None,
        origin: None,
    };
    let prompt_sent = |text: &str, input: Uuid| ThreadEvent::CodingAgentPromptSent {
        input_event_id: Some(input),
        text: text.into(),
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        origin: None,
    };
    // The agent's own report that it took an input in.
    let read = |input: Uuid| ThreadEvent::CodingAgentInputRead {
        input_event_id: input,
        started_turn: false,
        verbatim: false,
    };

    start_cc_session(&bus, thread_id, branch, None).await;
    let spawn = emit(message_received("spawn the venue check")).await;
    emit(read(spawn)).await;
    emit(ThreadEvent::ChildReportFiled {
        child_thread_id: Uuid::new_v4(),
        child_thread_title: Some("Check the venue".into()),
        status: ChildCompletionStatus::Success,
        summary: "The venue is free on Friday.".into(),
        pending_change_ids: vec![],
        sub_thread_pending_changes: vec![],
        integrated_branch: None,
    })
    .await;

    // The session is kept alive, so the follow-up is delivered in-session.
    let follow_up = "and Saturday?";
    let origin = emit(message_received(follow_up)).await;
    let sent = emit(prompt_sent(follow_up, origin)).await;
    let text = build_followup_prompt_text(
        &pool,
        thread_id,
        Some(origin),
        Some(sent),
        follow_up,
        Some(branch),
    )
    .await;
    assert!(
        text.contains("[CHILD THREAD REPORT FILED]") && text.contains("free on Friday"),
        "the kept-alive prompt must carry the filed report: {text}"
    );
    assert!(
        text.ends_with(&format!("{RESUME_NOTE_REFERENT_GUARD}\n\n{follow_up}")),
        "the note block closes with the guard, then the message: {text}"
    );
    emit(read(origin)).await;

    let second = "thanks";
    let origin = emit(message_received(second)).await;
    let sent = emit(prompt_sent(second, origin)).await;
    assert_eq!(
        build_followup_prompt_text(
            &pool,
            thread_id,
            Some(origin),
            Some(sent),
            second,
            Some(branch)
        )
        .await,
        second,
        "the report rode the previous prompt, so this one goes bare"
    );

    // The session ends, and the next message respawns it.
    let respawn = "one more thing";
    let origin = emit(message_received(respawn)).await;
    let text = build_resume_prompt_text(
        &pool,
        thread_id,
        origin,
        respawn,
        ResumeSpawnContext {
            worktree_path: None,
            last_idle_sha: None,
            adoption_note: None,
            session_branch: Some(branch),
        },
    )
    .await;
    assert_eq!(text, respawn, "a respawn must not repeat the report");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// Which events bound a live session's note, per input. A forward that wrote a
/// `CodingAgentPromptSent` ends at it; one that did not ends at its origin.
#[test]
fn a_follow_up_note_ends_at_its_delivery_when_it_has_one() {
    use super::run::followup_gap_bounds;
    use uuid::Uuid;

    let origin = Uuid::new_v4();
    let sent = Uuid::new_v4();
    assert_eq!(
        followup_gap_bounds(Some(origin), Some(sent)),
        Some((origin, sent))
    );
    assert_eq!(followup_gap_bounds(None, Some(sent)), Some((sent, sent)));
    assert_eq!(
        followup_gap_bounds(Some(origin), None),
        Some((origin, origin))
    );
    assert_eq!(followup_gap_bounds(None, None), None);
}

/// An image-only follow-up has no text, but it still writes the boundary its
/// note ends at. So it carries the note, or the gap is lost behind it.
#[tokio::test]
async fn an_image_only_follow_up_still_carries_the_note() {
    use super::run::build_followup_prompt_text;
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{
        ChildCompletionStatus, EventChannel, EventMeta, ThreadEvent,
    };
    use crate::test_support::{setup_test_db, start_cc_session, teardown_test_db};
    use uuid::Uuid;

    let (pool, db_name) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let branch = "claude-code/image-only";
    let emit = |event: ThreadEvent| {
        let bus = &bus;
        async move {
            bus.emit(BusEvent::Thread {
                thread_id,
                event,
                meta: EventMeta {
                    channel: Some(EventChannel::ClaudeCode),
                    ..EventMeta::NONE
                },
            })
            .await
            .unwrap()
            .expect("emit produced no EmitResult")
            .event_id
        }
    };

    start_cc_session(&bus, thread_id, branch, None).await;
    emit(ThreadEvent::ChildReportFiled {
        child_thread_id: Uuid::new_v4(),
        child_thread_title: Some("Check the venue".into()),
        status: ChildCompletionStatus::Success,
        summary: "The venue is free on Friday.".into(),
        pending_change_ids: vec![],
        sub_thread_pending_changes: vec![],
        integrated_branch: None,
    })
    .await;
    let sent = emit(ThreadEvent::CodingAgentPromptSent {
        input_event_id: None,
        text: String::new(),
        coding_agent: crate::runtime::CodingAgent::ClaudeCode,
        origin: None,
    })
    .await;

    let text =
        build_followup_prompt_text(&pool, thread_id, None, Some(sent), "", Some(branch)).await;
    assert!(
        text.contains("free on Friday"),
        "the image-only prompt must carry the filed report: {text}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

/// The read of a forwarded input must name an event the turn gap checks: its
/// origin, or for an engine prompt with none its `CodingAgentPromptSent`.
#[test]
fn a_forwarded_input_owes_its_read_to_its_origin_or_its_delivery() {
    use super::run::owed_input_event_ids;
    use uuid::Uuid;

    let origin = Uuid::new_v4();
    let sent = Uuid::new_v4();
    assert_eq!(owed_input_event_ids(Some(origin), Some(sent)), vec![origin]);
    assert_eq!(owed_input_event_ids(None, Some(sent)), vec![sent]);
    assert_eq!(owed_input_event_ids(Some(origin), None), vec![origin]);
    assert!(owed_input_event_ids(None, None).is_empty());
}

/// A thread whose next prompt has something to put in front of it: a device
/// block (a stored preference) and a turn-gap note (a report filed after the
/// last read). Returns the pool, the database name, the bus and the thread.
async fn thread_with_context_to_prepend(
    branch: &str,
) -> (
    sqlx::PgPool,
    String,
    crate::engine::event_bus::EventBus,
    uuid::Uuid,
) {
    use crate::engine::event_bus::{BusEvent, EventBus};
    use crate::engine::thread_events::{ChildCompletionStatus, EventMeta, ThreadEvent};
    use crate::test_support::{seed_preference, setup_test_db, start_cc_session};

    let (pool, db_name) = setup_test_db().await;
    seed_preference(&pool, "timezone", "Europe/Oslo")
        .await
        .unwrap();
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = uuid::Uuid::new_v4();
    start_cc_session(&bus, thread_id, branch, None).await;
    let first = emit_on(&bus, thread_id, user_message("turn 1")).await;
    emit_on(
        &bus,
        thread_id,
        ThreadEvent::CodingAgentInputRead {
            input_event_id: first,
            started_turn: false,
            verbatim: false,
        },
    )
    .await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChildReportFiled {
            child_thread_id: uuid::Uuid::new_v4(),
            child_thread_title: Some("Check the venue".into()),
            status: ChildCompletionStatus::Success,
            summary: "The venue is free on Friday.".into(),
            pending_change_ids: vec![],
            sub_thread_pending_changes: vec![],
            integrated_branch: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .unwrap();
    (pool, db_name, bus, thread_id)
}

fn user_message(text: &str) -> crate::engine::thread_events::ThreadEvent {
    crate::engine::thread_events::ThreadEvent::MessageReceived {
        provider: None,
        voice_session_id: None,
        wake_mode: None,
        text: text.into(),
        user_image_hashes: vec![],
        device_id: None,
        image_description: None,
        parent_thread_id: None,
        spawning_event_id: None,
        mode: crate::engine::thread_events::ActorMode::Human,
        model: None,
        reasoning_effort: None,
        origin: None,
    }
}

async fn emit_on(
    bus: &crate::engine::event_bus::EventBus,
    thread_id: uuid::Uuid,
    event: crate::engine::thread_events::ThreadEvent,
) -> uuid::Uuid {
    bus.emit(crate::engine::event_bus::BusEvent::Thread {
        thread_id,
        event,
        meta: crate::engine::thread_events::EventMeta {
            channel: Some(crate::engine::thread_events::EventChannel::ClaudeCode),
            ..crate::engine::thread_events::EventMeta::NONE
        },
    })
    .await
    .unwrap()
    .expect("emit produced no EmitResult")
    .event_id
}

/// Claude Code runs a slash command only when the input starts with `/`. So
/// a spawn's first prompt sends a command exactly as typed, with no note and no
/// device block, while a prompt keeps both.
#[tokio::test]
async fn a_spawn_sends_a_slash_command_verbatim() {
    use super::run::{first_prompt_for_agent, PromptFor, ResumeSpawnContext};
    let branch = "claude-code/verbatim-spawn";
    let (pool, db_name, bus, thread_id) = thread_with_context_to_prepend(branch).await;
    let event_store = crate::core::EventStore::new(pool.clone());
    let spawn = || ResumeSpawnContext {
        worktree_path: None,
        last_idle_sha: None,
        adoption_note: None,
        session_branch: Some(branch),
    };

    let command = emit_on(&bus, thread_id, user_message("/cost")).await;
    let text = first_prompt_for_agent(
        &pool,
        &event_store,
        thread_id,
        command,
        PromptFor::Turn("/cost"),
        spawn(),
    )
    .await;
    assert_eq!(text, "/cost");

    // A keep-alive's nudge carries no device block and no notes.
    let nudge = emit_on(&bus, thread_id, user_message("keep warm")).await;
    let text = first_prompt_for_agent(
        &pool,
        &event_store,
        thread_id,
        nudge,
        PromptFor::KeepAlive("keep warm"),
        spawn(),
    )
    .await;
    assert_eq!(text, "keep warm");

    let prompt = emit_on(&bus, thread_id, user_message("what changed?")).await;
    let text = first_prompt_for_agent(
        &pool,
        &event_store,
        thread_id,
        prompt,
        PromptFor::Turn("what changed?"),
        spawn(),
    )
    .await;
    assert!(
        text.starts_with("[USER DEVICE & PREFERENCES]")
            && text.contains("free on Friday")
            && text.ends_with("what changed?"),
        "a prompt keeps its device block and note: {text}"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

/// The same for a follow-up to a live session.
#[tokio::test]
async fn a_follow_up_sends_a_slash_command_verbatim() {
    use super::run::follow_up_for_agent;
    use crate::engine::{AgentInputKind, AgentUserInput};
    let branch = "claude-code/verbatim-follow-up";
    let (pool, db_name, bus, thread_id) = thread_with_context_to_prepend(branch).await;
    let event_store = crate::core::EventStore::new(pool.clone());
    let follow_up = |text: &str, origin| AgentUserInput {
        text: text.into(),
        images: None,
        origin_event_id: Some(origin),
        kind: AgentInputKind::User,
        completion: None,
    };
    let prompt_sent =
        |text: &str, input| crate::engine::thread_events::ThreadEvent::CodingAgentPromptSent {
            text: text.into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            origin: None,
            input_event_id: Some(input),
        };

    let command = emit_on(&bus, thread_id, user_message("/cost")).await;
    let sent = emit_on(&bus, thread_id, prompt_sent("/cost", command)).await;
    let text = follow_up_for_agent(
        &pool,
        &event_store,
        thread_id,
        &follow_up("/cost", command),
        Some(sent),
        Some(branch),
    )
    .await;
    assert_eq!(text, "/cost");

    let prompt = emit_on(&bus, thread_id, user_message("what changed?")).await;
    let sent = emit_on(&bus, thread_id, prompt_sent("what changed?", prompt)).await;
    let text = follow_up_for_agent(
        &pool,
        &event_store,
        thread_id,
        &follow_up("what changed?", prompt),
        Some(sent),
        Some(branch),
    )
    .await;
    assert!(
        text.starts_with("[USER DEVICE & PREFERENCES]")
            && text.contains("free on Friday")
            && text.ends_with("what changed?"),
        "a prompt keeps its device block and note: {text}"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}
