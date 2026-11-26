#[tokio::test]
async fn try_wait_detects_dead_cc_process() {
    use tokio::process::Command;

    let mut child = Command::new("echo")
        .arg("done")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn 'echo'");

    let _stdin = child.stdin.take();
    let _stdout = child.stdout.take();

    let _ = child.wait().await;

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

    let _ = child.kill().await;
}

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
            content: Default::default(),
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

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ChangeApplied {
            content: Default::default(),
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
    let final_text = build_resume_prompt_text(
        &pool,
        thread_id,
        origin_id,
        user_message,
        "",
        ResumeSpawnContext {
            worktree_path: None,
            last_idle_sha: None,
            branch_note: None,
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
            content: Default::default(),
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
        "",
        ResumeSpawnContext {
            worktree_path: None,
            last_idle_sha: None,
            branch_note: None,
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
        branch_note: note,
        session_branch: None,
    };

    let noted = build_resume_prompt_text(
        &pool,
        Uuid::new_v4(),
        Uuid::new_v4(),
        user_message,
        "",
        spawn(Some(
            "[Note from engine: APPLIED: fix(app): the composer's Diff is an icon]",
        )),
    )
    .await;

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

    let bare = build_resume_prompt_text(
        &pool,
        Uuid::new_v4(),
        Uuid::new_v4(),
        user_message,
        "",
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
        "",
        ResumeSpawnContext {
            worktree_path: None,
            last_idle_sha: None,
            branch_note: None,
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

    let follow_up = "and Saturday?";
    let origin = emit(message_received(follow_up)).await;
    let sent = emit(prompt_sent(follow_up, origin)).await;
    let text = build_followup_prompt_text(
        &pool,
        thread_id,
        Some(origin),
        Some(sent),
        follow_up,
        Default::default(),
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
            Default::default(),
            Some(branch)
        )
        .await,
        second,
        "the report rode the previous prompt, so this one goes bare"
    );

    let respawn = "one more thing";
    let origin = emit(message_received(respawn)).await;
    let text = build_resume_prompt_text(
        &pool,
        thread_id,
        origin,
        respawn,
        "",
        ResumeSpawnContext {
            worktree_path: None,
            last_idle_sha: None,
            branch_note: None,
            session_branch: Some(branch),
        },
    )
    .await;
    assert_eq!(text, respawn, "a respawn must not repeat the report");

    pool.close().await;
    teardown_test_db(&db_name).await;
}

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

    let text = build_followup_prompt_text(
        &pool,
        thread_id,
        None,
        Some(sent),
        "",
        Default::default(),
        Some(branch),
    )
    .await;
    assert!(
        text.contains("free on Friday"),
        "the image-only prompt must carry the filed report: {text}"
    );

    pool.close().await;
    teardown_test_db(&db_name).await;
}

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

use super::run::HeldBlocks;
use crate::engine::agent_context::SentDeviceContexts;

fn held(sent: &SentDeviceContexts) -> HeldBlocks<'_> {
    HeldBlocks {
        sent,
        conversation: None,
        unread: &[],
    }
}

#[test]
fn only_a_claude_code_conversation_is_trusted_to_hold_the_block() {
    use crate::runtime::CodingAgent;
    let sent = SentDeviceContexts::default();
    let held_by = |agent| HeldBlocks::for_agent(&sent, agent, Some("session-1"), &[]).conversation;
    assert_eq!(held_by(CodingAgent::ClaudeCode), Some("session-1"));
    assert_eq!(held_by(CodingAgent::Codex), None);
}

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

#[tokio::test]
async fn a_spawn_sends_a_slash_command_verbatim() {
    use super::run::{first_prompt_for_agent, PromptFor, ResumeSpawnContext};
    let branch = "claude-code/verbatim-spawn";
    let (pool, db_name, bus, thread_id) = thread_with_context_to_prepend(branch).await;
    let event_store = crate::core::EventStore::new(pool.clone());
    let spawn = || ResumeSpawnContext {
        worktree_path: None,
        last_idle_sha: None,
        branch_note: None,
        session_branch: Some(branch),
    };
    let sent = SentDeviceContexts::default();

    let command = emit_on(&bus, thread_id, user_message("/cost")).await;
    let text = first_prompt_for_agent(
        &pool,
        &event_store,
        thread_id,
        command,
        PromptFor::Turn("/cost"),
        spawn(),
        held(&sent),
    )
    .await
    .text;
    assert_eq!(text, "/cost");

    let nudge = emit_on(&bus, thread_id, user_message("keep warm")).await;
    let text = first_prompt_for_agent(
        &pool,
        &event_store,
        thread_id,
        nudge,
        PromptFor::KeepAlive("keep warm"),
        spawn(),
        held(&sent),
    )
    .await
    .text;
    assert_eq!(text, "keep warm");

    let prompt = emit_on(&bus, thread_id, user_message("what changed?")).await;
    let text = first_prompt_for_agent(
        &pool,
        &event_store,
        thread_id,
        prompt,
        PromptFor::Turn("what changed?"),
        spawn(),
        held(&sent),
    )
    .await
    .text;
    assert!(
        text.starts_with("[USER DEVICE & PREFERENCES]")
            && text.contains("free on Friday")
            && text.ends_with("what changed?"),
        "a prompt keeps its device block and note: {text}"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_follow_up_sends_a_slash_command_verbatim() {
    use super::run::follow_up_for_agent;
    use crate::engine::{AgentInputKind, AgentUserInput};
    let branch = "claude-code/verbatim-follow-up";
    let (pool, db_name, bus, thread_id) = thread_with_context_to_prepend(branch).await;
    let event_store = crate::core::EventStore::new(pool.clone());
    let memory = SentDeviceContexts::default();
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
        held(&memory),
    )
    .await
    .text;
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
        held(&memory),
    )
    .await
    .text;
    assert!(
        text.starts_with("[USER DEVICE & PREFERENCES]")
            && text.contains("free on Friday")
            && text.ends_with("what changed?"),
        "a prompt keeps its device block and note: {text}"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

fn message_from(
    text: &str,
    mode: crate::engine::thread_events::ActorMode,
    origin: crate::engine::thread_events::MessageOrigin,
) -> crate::engine::thread_events::ThreadEvent {
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
        mode,
        model: None,
        reasoning_effort: None,
        origin: Some(origin),
    }
}

async fn prompts_for_turns_opened_by(
    mode: crate::engine::thread_events::ActorMode,
    origin: crate::engine::thread_events::MessageOrigin,
) -> (String, String) {
    use super::run::{first_prompt_for_agent, follow_up_for_agent, PromptFor, ResumeSpawnContext};
    use crate::engine::{AgentInputKind, AgentUserInput};
    let branch = "claude-code/standing-note";
    let (pool, db_name, bus, thread_id) = thread_with_context_to_prepend(branch).await;
    let event_store = crate::core::EventStore::new(pool.clone());
    let sent = SentDeviceContexts::default();

    let first = emit_on(&bus, thread_id, message_from("start", mode, origin.clone())).await;
    let first_text = first_prompt_for_agent(
        &pool,
        &event_store,
        thread_id,
        first,
        PromptFor::Turn("start"),
        ResumeSpawnContext {
            worktree_path: None,
            last_idle_sha: None,
            branch_note: None,
            session_branch: Some(branch),
        },
        held(&sent),
    )
    .await
    .text;

    let next = emit_on(&bus, thread_id, message_from("next", mode, origin)).await;
    let follow_up_text = follow_up_for_agent(
        &pool,
        &event_store,
        thread_id,
        &AgentUserInput {
            text: "next".into(),
            images: None,
            origin_event_id: Some(next),
            kind: AgentInputKind::User,
            completion: None,
        },
        None,
        Some(branch),
        held(&sent),
    )
    .await
    .text;

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
    (first_text, follow_up_text)
}

#[tokio::test]
async fn every_turn_another_thread_opened_carries_the_owner_approval_note() {
    use crate::engine::thread_events::{ActorMode, MessageOrigin, ThreadDirection};
    let spawner = MessageOrigin::ThreadLink {
        thread_id: uuid::Uuid::new_v4(),
        title: None,
        spawning_event_id: None,
        mode: ActorMode::Agent,
        direction: ThreadDirection::Parent,
    };
    let (first, follow_up) = prompts_for_turns_opened_by(ActorMode::Agent, spawner).await;
    for (label, text) in [("first prompt", &first), ("follow-up", &follow_up)] {
        assert!(
            text.contains("THE OWNER DID NOT OPEN THIS TURN"),
            "{label}: {text}"
        );
        assert!(text.contains("lucidos ask-permission"), "{label}");
        assert!(text.contains("409"), "{label}");
        assert!(!text.contains("create-top-thread"), "{label}");
        assert!(text.contains("needs no authority"), "{label}");
    }
}

#[tokio::test]
async fn no_turn_the_owner_opened_carries_the_owner_approval_note() {
    use crate::engine::thread_events::{ActorMode, MessageOrigin};
    let owner = MessageOrigin::Device {
        device_id: "device-abc".into(),
    };
    let (first, follow_up) = prompts_for_turns_opened_by(ActorMode::Human, owner).await;
    for (label, text) in [("first prompt", &first), ("follow-up", &follow_up)] {
        assert!(
            !text.contains("THE OWNER DID NOT OPEN THIS TURN"),
            "{label}: {text}"
        );
        assert!(text.ends_with(if label == "first prompt" {
            "start"
        } else {
            "next"
        }));
    }
}

#[test]
fn a_forwarded_redelivery_is_withdrawable_and_its_read_names_the_message() {
    use super::run::owed_input_event_ids;
    use crate::engine::agent_session::input_ledger::{InputLedger, ReadInput, Settled};
    use uuid::Uuid;

    let id = Uuid::new_v4();
    let prompt_sent = Uuid::new_v4();
    let input = crate::runtime::AgentInput {
        defer_on_steer_failure: false,
        text: "also add a test".into(),
        images: Vec::new(),
        uuid: Uuid::new_v4(),
    };
    let mut inputs = InputLedger::new();
    inputs.forwarded(
        owed_input_event_ids(Some(id), Some(prompt_sent)),
        &input,
        None,
    );

    assert_eq!(inputs.withdrawable(id), Some(input.uuid));
    assert_eq!(
        inputs.observe(&crate::runtime::AgentEvent::InputRead { input: input.uuid }),
        Settled::Read(vec![ReadInput {
            event_id: id,
            verbatim: false
        }])
    );
}

#[tokio::test]
async fn a_redelivered_follow_up_tells_the_agent_it_is_late() {
    use super::run::{follow_up_for_agent, RESUME_NOTE_REFERENT_GUARD};
    use crate::engine::{AgentInputKind, AgentUserInput};
    let branch = "claude-code/redelivered";
    let (pool, db_name, bus, thread_id) = thread_with_context_to_prepend(branch).await;
    let event_store = crate::core::EventStore::new(pool.clone());
    let sent = SentDeviceContexts::default();
    let sent_at = chrono::DateTime::parse_from_rfc3339("2026-10-17T09:00:50Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let redelivered = |text: &str, origin| AgentUserInput {
        text: text.into(),
        images: None,
        origin_event_id: Some(origin),
        kind: AgentInputKind::Redelivered {
            sent_at,
            input: crate::engine::RedeliveredInput::Message {
                sender: crate::engine::thread_events::ActorMode::Human,
            },
        },
        completion: None,
    };
    let note = crate::engine::agent_session::stranded_follow_ups::late_delivery_note(
        crate::engine::RedeliveredInput::Message {
            sender: crate::engine::thread_events::ActorMode::Human,
        },
        sent_at,
    );

    let message = emit_on(&bus, thread_id, user_message("also add a test")).await;
    let text = follow_up_for_agent(
        &pool,
        &event_store,
        thread_id,
        &redelivered("also add a test", message),
        None,
        Some(branch),
        held(&sent),
    )
    .await
    .text;
    let at_note = text.find(&note).expect("the late note is there");
    let at_guard = text
        .find(RESUME_NOTE_REFERENT_GUARD)
        .expect("the guard closes the notes");
    assert!(at_note < at_guard, "{text}");
    assert!(text.ends_with("also add a test"), "{text}");

    let command = emit_on(&bus, thread_id, user_message("/cost")).await;
    let text = follow_up_for_agent(
        &pool,
        &event_store,
        thread_id,
        &redelivered("/cost", command),
        None,
        Some(branch),
        held(&sent),
    )
    .await
    .text;
    assert_eq!(text, "/cost");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

const DEVICE_BLOCK: &str = "[USER DEVICE & PREFERENCES]";

#[tokio::test]
async fn a_resume_skips_the_device_block_its_conversation_already_holds() {
    use super::run::{first_prompt_for_agent, PromptFor, ResumeSpawnContext};
    let branch = "claude-code/held-first-prompt";
    let (pool, db_name, bus, thread_id) = thread_with_context_to_prepend(branch).await;
    let event_store = crate::core::EventStore::new(pool.clone());
    let sent = SentDeviceContexts::default();
    let prompt = |conversation| {
        let (pool, event_store, bus, sent) = (&pool, &event_store, &bus, &sent);
        async move {
            let origin = emit_on(bus, thread_id, user_message("go on")).await;
            first_prompt_for_agent(
                pool,
                event_store,
                thread_id,
                origin,
                PromptFor::Turn("go on"),
                ResumeSpawnContext {
                    worktree_path: None,
                    last_idle_sha: None,
                    branch_note: None,
                    session_branch: Some(branch),
                },
                HeldBlocks {
                    sent,
                    conversation,
                    unread: &[],
                },
            )
            .await
        }
    };

    let spawned = prompt(None).await;
    assert!(spawned.text.starts_with(DEVICE_BLOCK), "{}", spawned.text);
    sent.record(
        thread_id,
        "session-1",
        spawned.device_block.expect("the block is in flight"),
    );

    let resumed = prompt(Some("session-1")).await;
    assert!(!resumed.text.contains(DEVICE_BLOCK), "{}", resumed.text);
    assert!(resumed.text.ends_with("go on"), "{}", resumed.text);
    assert!(resumed.device_block.is_none());

    let other = prompt(Some("session-2")).await;
    assert!(other.text.starts_with(DEVICE_BLOCK), "{}", other.text);

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_follow_up_repeats_the_device_block_only_when_it_changed() {
    use super::run::follow_up_for_agent;
    use crate::engine::{AgentInputKind, AgentUserInput};
    let branch = "claude-code/held-follow-up";
    let (pool, db_name, bus, thread_id) = thread_with_context_to_prepend(branch).await;
    let event_store = crate::core::EventStore::new(pool.clone());
    let sent = SentDeviceContexts::default();
    let follow_up = |text: &str, origin| AgentUserInput {
        text: text.into(),
        images: None,
        origin_event_id: Some(origin),
        kind: AgentInputKind::User,
        completion: None,
    };
    let send = |text: &'static str| {
        let (pool, event_store, bus, sent) = (&pool, &event_store, &bus, &sent);
        async move {
            let origin = emit_on(bus, thread_id, user_message(text)).await;
            follow_up_for_agent(
                pool,
                event_store,
                thread_id,
                &follow_up(text, origin),
                None,
                Some(branch),
                HeldBlocks {
                    sent,
                    conversation: Some("session-1"),
                    unread: &[],
                },
            )
            .await
        }
    };

    let first = send("one").await;
    assert!(first.text.contains(DEVICE_BLOCK), "{}", first.text);
    let fingerprint = first.device_block.expect("the block is in flight");
    let queued = send("one and a half").await;
    assert!(
        queued.text.contains(DEVICE_BLOCK),
        "nothing counts as read yet: {}",
        queued.text
    );

    sent.record(thread_id, "session-1", fingerprint);
    let unchanged = send("two").await;
    assert!(!unchanged.text.contains(DEVICE_BLOCK), "{}", unchanged.text);
    assert!(unchanged.text.ends_with("two"), "{}", unchanged.text);
    assert!(unchanged.device_block.is_none());

    crate::test_support::seed_preference(&pool, crate::core::prefs::THEME_MODE.key(), "light")
        .await
        .unwrap();
    let changed = send("three").await;
    assert!(
        changed.text.starts_with(DEVICE_BLOCK) && changed.text.contains("- theme-mode: light"),
        "{}",
        changed.text
    );
    sent.record(
        thread_id,
        "session-1",
        changed.device_block.expect("the block is in flight"),
    );
    assert!(!send("four").await.text.contains(DEVICE_BLOCK));

    sent.forget(thread_id);
    assert!(send("five").await.text.starts_with(DEVICE_BLOCK));

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

mod run_loop_rule_tests {
    const RUN_SRC: &str = include_str!("run_session/run.rs");

    fn at(needle: &str) -> usize {
        let mut hits = RUN_SRC.match_indices(needle);
        let (first, _) = hits
            .next()
            .unwrap_or_else(|| panic!("run.rs lost `{needle}`"));
        assert!(
            hits.next().is_none(),
            "`{needle}` must stay unique in run.rs"
        );
        first
    }

    fn after(start: usize, needle: &str) -> usize {
        start
            + RUN_SRC[start..]
                .find(needle)
                .unwrap_or_else(|| panic!("`{needle}` no longer follows its anchor"))
    }

    fn slice(from: &str, to: &str) -> &'static str {
        let start = at(from);
        &RUN_SRC[start..after(start, to)]
    }

    fn squashed(text: &str) -> String {
        text.split_whitespace().collect()
    }

    fn has(text: &str, needle: &str) -> bool {
        squashed(text).contains(&squashed(needle))
    }

    #[test]
    fn the_idle_exit_reads_the_handoff_before_it_takes_the_sessions_lock() {
        let body = slice(
            "async fn exit_idle_subprocess_unless_owed(",
            "TerminateDecision::Terminate =>",
        );
        let read = body
            .find("handoff_pending(")
            .expect("the idle exit reads the handoff");
        let lock = body
            .find("self.agent_sessions.lock().await")
            .expect("the idle exit takes the sessions lock");
        assert!(read < lock, "a query must not run while the lock is held");
    }

    #[test]
    fn the_idle_terminate_marks_the_session_exited_before_it_cancels() {
        let before_cancel = slice(
            "TerminateDecision::Terminate => {",
            "agent_cancel.cancel();",
        );
        assert!(
            before_cancel.contains("s.process_exited = true;"),
            "a follow-up in the shutdown window must see process_exited"
        );
    }

    #[test]
    fn every_run_passes_the_spawn_gate_before_it_spawns() {
        let start = slice(
            "async fn run_agent_session(",
            "let spawned = match sandbox {",
        );
        assert!(
            start.contains("self.gate_coding_agent_spawn(thread_id, origin_id).await?;"),
            "a resume and a follow-up spawn pass the gate too"
        );
    }

    #[test]
    fn the_session_entry_is_published_before_the_worktree_is_set_up() {
        assert!(
            at("session.current_reasoning_effort = effort.clone();")
                < at(".resolve_run_worktree_context("),
            "the commands API reads the entry while the worktree is being set up"
        );
    }

    #[test]
    fn a_keep_alive_spawn_records_no_rung() {
        for needle in [
            "let records_rung = !self.event_bus.in_keep_alive_turn(thread_id);",
            "model: normalized_model.clone().filter(|_| records_rung),",
            "reasoning_effort: rung_effort.clone().filter(|_| records_rung),",
            "if records_rung && (normalized_model.is_some() || cc_reasoning_effort.is_some()) {",
        ] {
            assert!(RUN_SRC.contains(needle), "run.rs lost `{needle}`");
        }
    }

    #[test]
    fn a_keep_alive_turn_sends_its_nudge_with_no_notes() {
        assert!(has(
            RUN_SRC,
            "if self.event_bus.in_keep_alive_turn(thread_id) { PromptFor::KeepAlive(first_message) } else { PromptFor::Turn(first_message) }"
        ));
    }

    #[test]
    fn a_failed_app_lookup_or_a_missing_folder_refuses_the_spawn() {
        let lookup = slice(
            "\"SELECT coding_agent_kind, coding_agent_folder FROM thread_summaries WHERE thread_id = $1\",",
            "let is_app_spawn = app_spawn_id.is_some();",
        );
        assert!(
            has(lookup, ".fetch_optional(&self.pool) .await? {"),
            "a failed read must refuse, not read as `not an app`"
        );
        assert!(
            has(lookup, "Some((Some(k), folder)) if k == \"app\" => {"),
            "an app thread with a NULL folder must reach the refusal below"
        );
        assert!(lookup.contains("return Err("));
    }

    #[test]
    fn the_account_pin_is_computed_once_replayed_and_recorded_at_init() {
        at("crate::engine::agent_session::lookup_pinned_cc_config_dir(self.pool(), thread_id)");
        for needle in [
            "account_pin: injected_pin.as_ref(),",
            "let effective_pin = injected_pin.clone().or_else(|| live_account.clone());",
            "claude_config_dir: effective_pin.as_ref().map(|pin| pin.dir().to_string()),",
        ] {
            assert!(RUN_SRC.contains(needle), "run.rs lost `{needle}`");
        }
    }

    #[test]
    fn the_keep_awake_hold_lives_as_long_as_the_run() {
        at("let _awake = crate::core::keep_awake::hold(");
    }

    #[test]
    fn a_single_turn_session_still_reads_the_head_it_started_from() {
        at("let adoption_anchor_sha = match worktree_path.as_deref() {");
        at("crate::engine::agent_session::external_edits::git_head_sha(wt)");
    }

    #[test]
    fn the_startup_permit_is_held_until_init() {
        let held = at("let mut startup_permit = Some(startup_permit);");
        let init = at("AgentEvent::Init { session_id: cc_sid,");
        let between = &RUN_SRC[held + "let mut startup_permit = Some(startup_permit);".len()..init];
        assert!(!between.contains("startup_permit"), "released before Init");
        assert!(RUN_SRC[init..].contains("startup_permit.take()"));
    }

    #[test]
    fn a_stop_during_startup_is_rechecked_in_the_hold_that_marks_the_session_running() {
        let block = slice(
            "let canceled_during_spawn = {",
            "if canceled_during_spawn {",
        );
        assert_eq!(block.matches(".lock().await").count(), 1);
        let recheck = block
            .find("startup_cancel.is_cancelled()")
            .expect("the re-check");
        let running = block
            .find("SessionPhase::Running {")
            .expect("the transition");
        assert!(recheck < running);
    }

    #[test]
    fn the_claim_outlives_the_process_torn_down_by_a_startup_stop() {
        let arm = slice(
            "if canceled_during_spawn {",
            "return Ok(self.settle_canceled_startup(claim, request_id, &meta).await);",
        );
        let cancel = arm
            .find("agent_cancel.cancel();")
            .expect("the process is killed");
        let teardown = arm
            .find("await_process_teardown(&mut events_rx, thread_id, \"a Stop during startup\").await;")
            .expect("the teardown is awaited before the claim is released");
        assert!(cancel < teardown);
    }

    #[test]
    fn the_hang_watchdog_counter_is_the_one_the_session_entry_shares() {
        at("let tools_in_flight_shared = claim.entry.tools_in_flight.clone();");
        at("let tools_in_flight = tools_in_flight_shared;");
    }

    #[test]
    fn a_model_call_is_proven_by_every_usage_event_and_only_by_one() {
        assert_eq!(
            RUN_SRC
                .matches("api_calls_seen = api_calls_seen.saturating_add(1);")
                .count(),
            2,
            "one for Usage, one for UsageUnscoped"
        );
    }

    #[test]
    fn init_records_the_attached_session_before_anything_else() {
        let init = at("AgentEvent::Init { session_id: cc_sid,");
        let record = after(init, "init_session_id.replace(");
        assert!(
            !RUN_SRC[init..record].contains("cc_model"),
            "the stale-resume veto reads this before the model is looked at"
        );
        at("resume_attach_confirmed: resume_session_id.is_some()");
    }

    #[test]
    fn init_reconciles_the_model_against_the_supplied_alias() {
        at("let norm = crate::runtime::claude_code::reconcile_cc_model(");
        assert!(has(RUN_SRC, "normalized_model = Some(norm);"));
    }

    #[test]
    fn the_commands_cache_is_updated_outside_the_sessions_lock() {
        let locked = slice("let cache_update = {", "if let Some(info) = cache_update {");
        assert!(!locked.contains("upsert_cc_commands_cache"));
    }

    #[test]
    fn init_persists_the_session_id_whether_or_not_the_model_changed() {
        let needle = "cc_session_id: Some(cc_sid.clone()),";
        let emit = at(needle);
        let guard = RUN_SRC[..emit]
            .rfind("if ")
            .expect("an enclosing statement");
        assert!(
            RUN_SRC[guard..].starts_with("if let Err(e) = self.event_bus.emit("),
            "the emit must not sit behind a model-changed condition"
        );
    }

    #[test]
    fn post_terminal_stragglers_are_dropped_by_each_event_arm() {
        assert!(has(
            RUN_SRC,
            "AgentEvent::Message { text, .. } | AgentEvent::ProgressNote { text } if turn_ended =>"
        ));
        assert!(RUN_SRC.contains("AgentEvent::Thought { .. } if turn_ended => {}"));
        let tool_use_guard = "AgentEvent::ToolUse { name, id, input, .. } if turn_ended => {";
        let tool_use = slice(
            tool_use_guard,
            "AgentEvent::ToolUse { name, input, id, parent_tool_use_id, api_call_id } => {",
        );
        assert!(
            !tool_use.contains("tools_in_flight"),
            "a post-terminal turn has no live watchdog to disarm"
        );
        let tool_result = slice(
            "AgentEvent::ToolResult { id, .. } if turn_ended => {",
            "AgentEvent::ToolResult { output, status: _, id, parent_tool_use_id } => {",
        );
        assert!(tool_result.contains("release_tool_slot(&tools_in_flight);"));
        assert!(tool_result.contains("tool_names.take(&id);"));
    }

    #[test]
    fn every_tool_call_disarms_the_watchdog_even_a_question() {
        let arm = slice(
            "AgentEvent::ToolUse { name, input, id, parent_tool_use_id, api_call_id } => {",
            "if crate::runtime::is_user_question_tool(&name) {",
        );
        assert!(arm.contains("tools_in_flight.fetch_add(1, std::sync::atomic::Ordering::Relaxed);"));
    }

    #[test]
    fn tool_arguments_are_redacted_before_the_description_is_built() {
        assert!(
            at("crate::core::redact_subprocess_output_in_json(&mut input, &tool_result_secrets);")
                < at("let description = crate::core::describe_cc_tool(&name, &input);")
        );
    }

    #[test]
    fn only_an_external_terminal_makes_result_text_the_teardown_reply() {
        at("let result_text_is_live = !external_terminal_emitted");
        assert!(RUN_SRC.matches("result_text_is_live").count() >= 3);
    }

    #[test]
    fn a_stale_resume_ends_the_same_way_before_and_after_the_first_prompt() {
        let stale = slice(
            "if is_stale_resume_signal(stale_inputs)",
            "return Err(STALE_RESUME_ERROR.into());",
        );
        assert!(has(stale, "|| explicit_session_not_found"));
        assert!(stale.contains("SessionPhase::Starting {"));
        assert!(stale.contains("claim.entry_guard.reap_only();"));
        assert!(
            stale.contains(
                "await_process_teardown(&mut events_rx, thread_id, \"a stale resume\").await;"
            ),
            "the old process group must be gone before the retry is signalled"
        );
        assert_eq!(RUN_SRC.matches("STALE_RESUME_ERROR.into()").count(), 2);
        assert_eq!(
            RUN_SRC
                .matches("reason: SessionEndReason::StaleResume,")
                .count(),
            2
        );
    }

    #[test]
    fn a_resume_settle_result_is_skipped_before_the_stale_check() {
        let settle = slice(
            "if is_resume_settle_result(stale_inputs, api_calls_seen == 0) {",
            "if api_calls_seen > 0",
        );
        assert!(settle.contains("continue 'event_loop;"));
        assert!(
            at("if is_resume_settle_result(stale_inputs, api_calls_seen == 0) {")
                < at("if is_stale_resume_signal(stale_inputs)")
        );
    }

    #[test]
    fn the_result_arm_reads_shutdown_once() {
        let arm = slice(
            "if is_stale_resume_signal(stale_inputs)",
            "interrupt_escalate_at = None;",
        );
        assert_eq!(arm.matches("session_is_shutting_down(").count(), 1);
    }

    #[test]
    fn the_cancel_device_is_cleared_after_the_terminal_that_names_it() {
        let arm = slice(
            "let clears = terminal_clears_user_hit_stop(&terminal_kind);",
            "interrupt_escalate_at = None;",
        );
        let emit = arm
            .find("\"[AgentSession] terminal event (Result classify)\"")
            .expect("the terminal emit");
        let clear = arm.find("meta.actor = None;").expect("the actor clear");
        assert!(
            emit < clear,
            "the cancel terminal must still carry the device"
        );
    }

    #[test]
    fn the_session_keeps_one_branch_name_after_an_adoption() {
        let boundary = slice(
            "crate::engine::agent_session::lifecycle::mark_turn_boundary(s);",
            "s.idle_notify.notify_waiters();",
        );
        assert!(boundary.contains("s.branch_name = Some(branch_name.clone());"));
    }

    #[test]
    fn the_idle_proposal_gate_has_no_diff_term() {
        let gate = slice("let idle_write = idle_change_write(", "let reentry_head");
        assert!(!gate.contains("wt_has_changes"));
    }

    #[test]
    fn an_unanswered_diff_probe_changes_nothing_and_an_empty_diff_never_discards() {
        let probe = slice(
            "match changed_files.as_deref() {",
            "Some(_) if !write.may_create()",
        );
        assert!(probe.contains("None => {"));
        assert!(probe.contains("Skipping propose/reconcile at idle"));
        let empty = slice("Some([]) => {", "Some(_) if !write.may_create()");
        assert!(empty.contains("self.reconcile_emptied_pending_change("));
        assert!(!empty.to_lowercase().contains("discard"));
    }

    #[test]
    fn the_idle_arm_emits_releases_and_re_enters_in_order() {
        let idle = at("if emit_idle {");
        let emit = at("self.emit_coding_agent_idled(");
        let mark = at("self.event_bus.mark_keep_alive_turn(thread_id, false).await;");
        let release = at("auto_resume_holds().release(thread_id)");
        let ceiling = at("crate::engine::context_ceiling::ceiling_crossing(");
        let exit = at("IdleAction::ExitSubprocess =>");
        assert!(idle < emit, "no idle is emitted outside `emit_idle`");
        assert!(emit < mark, "the keep-alive mark clears after the idle");
        assert!(emit < release, "the hold is released after both emits");
        assert!(release < ceiling && mark < ceiling);
        assert!(ceiling < exit, "before terminate_decision sees the queue");
    }

    #[test]
    fn the_approval_bridge_never_waits_on_the_user_in_the_loop() {
        let arm = slice("perm_req = async {", "Some(mut user_input) = msg_rx.recv()");
        let spawn = arm
            .find("tokio::spawn(")
            .expect("a waiter task per request");
        let request = arm.find("Some(req) => {").expect("the request arm");
        assert!(!arm[request..spawn].contains(".await"));
        assert!(arm.contains("let session_worktree = worktree_path.clone();"));
        assert!(RUN_SRC.contains("session.worktree_path = worktree_path.clone();"));
        let disarm = arm.find("tools.fetch_add(1, std::sync::atomic::Ordering::Relaxed);");
        let prompt = arm.find("prompt_coding_agent_permission(");
        let release = arm.find("release_tool_slot(&tools);");
        assert!(disarm.is_some() && disarm < prompt && prompt < release);
    }

    #[test]
    fn a_follow_up_writes_its_prompt_sent_before_it_forwards_and_never_with_the_block() {
        let arm = slice(
            "Some(mut user_input) = msg_rx.recv()",
            "Some(withdraw) = withdraw_rx.recv()",
        );
        let prompt = arm
            .find("ThreadEvent::CodingAgentPromptSent {")
            .expect("the audit event");
        let send = arm.find("agent_input_tx.send(").expect("the forward");
        assert!(prompt < send);
        assert!(arm[prompt..send].contains("text: user_input.text.clone(),"));
    }

    #[test]
    fn a_withdraw_reads_what_is_unforwarded_under_the_sessions_lock() {
        let arm = slice(
            "Some(withdraw) = withdraw_rx.recv() => {",
            "Some(take) = child_reports.take_rx.recv()",
        );
        assert!(has(
            arm,
            ".lock().await .get(&thread_id) .is_some_and(|s| s.unforwarded_inputs.contains(&withdraw.input_event_id));"
        ));
    }

    #[test]
    fn the_interrupt_arm_withdraws_cancels_arms_the_fallback_and_stamps_the_device() {
        let arm = slice("_ = interrupt.notified() => {", "_ = stop.notified() => {");
        let withdraw = arm
            .find("withdraw_ceiling_before_stop(")
            .expect("the ceiling withdrawal");
        let owes = arm.find("if !inputs.owes(uuid)").expect("the settle check");
        assert!(
            arm[withdraw..owes].contains("agent_cancel.cancel();"),
            "a refused withdrawal must still cancel"
        );
        assert!(arm.contains("meta.actor = Some(cancel_actor);"));
        assert!(
            arm.find("ControlRequest::Interrupt") < arm.find("INTERRUPT_ESCALATE_AFTER"),
            "the escalation timer is armed once the interrupt is forwarded"
        );
        assert!(arm.matches("INTERRUPT_ESCALATE_AFTER").count() == 1);
    }

    #[test]
    fn the_escalation_stamps_a_user_stop_and_drops_the_withheld_error() {
        let escalation = slice(
            "escalating to hard stop for thread {}",
            "self.record_usage_flushed_at_stop(",
        );
        assert!(escalation.contains("TerminalKind::Canceled("));
        assert!(
            escalation.contains("unwrap_or(crate::engine::thread_events::CancelCause::UserStop)")
        );
        assert!(escalation.contains("withheld_api_error = None;"));
    }

    #[test]
    fn a_landed_result_disarms_the_escalation_fallback() {
        let arm = slice(
            "let clears = terminal_clears_user_hit_stop(&terminal_kind);",
            "claude_text_buf.clear();",
        );
        assert!(arm.contains("interrupt_escalate_at = None;"));
    }

    #[test]
    fn only_the_watchdog_tick_marks_a_hang() {
        let tick = slice(
            "WatchdogGate::FirePastCeiling(tif) => {",
            "self.finalize_direct_agent(",
        );
        assert!(tick.contains("watchdog_fired = true;"));
        assert!(!RUN_SRC.contains("watchdog_fired = agent_cancel"));
        assert!(has(
            RUN_SRC,
            "emitted_terminal_event, watchdog_fired, exit_facts,"
        ));
    }

    #[test]
    fn a_tool_past_the_ceiling_is_killed_only_while_the_thread_is_running() {
        let ceiling = slice("WatchdogGate::FirePastCeiling(tif) => {", "_ => false,");
        assert!(ceiling.contains("thread_is_running("));
        assert_eq!(ceiling.matches("let still_running").count(), 1);
        assert!(squashed(ceiling).ends_with("still_running}"));
    }

    #[test]
    fn a_billing_observation_is_taken_before_any_turn_is_started() {
        assert!(
            at("if let AgentEvent::BillingObserved { attribution } = &ev {")
                < at("let new_turn_after_terminal = {")
        );
    }

    #[test]
    fn a_read_is_announced_with_the_new_turn_decision() {
        assert!(at("let new_turn_after_terminal = {") < at("announce_reads("));
        at("new_turn_after_terminal && read_owed_input,");
    }

    #[test]
    fn the_idle_return_resumes_after_the_follow_up_drain() {
        let exit = slice(
            "let orphans = lost_followups_to_orphans(drain_lost_followups(msg_rx));",
            "return Ok(ProcessResult {",
        );
        assert!(exit.contains("self.maybe_auto_resume_after_api_error("));
        assert!(exit.contains("!orphans.is_empty(),"));
        assert!(
            at("idle_exit(&mut guard, thread_id, &external_terminal_emitted)")
                < at("let orphans = lost_followups_to_orphans(")
        );
    }

    #[test]
    fn a_compaction_forgets_the_device_block_and_only_a_read_records_one() {
        let compacted = slice(
            "AgentEvent::Compacted { trigger, pre_tokens, post_tokens } => {",
            "AgentEvent::OutputEnded => unreachable!",
        );
        assert!(compacted.contains("self.sent_device_contexts.forget(thread_id);"));
        assert_eq!(
            RUN_SRC.matches("self.sent_device_contexts.record(").count(),
            1
        );
        let read = slice(
            "AgentEvent::InputRead { input } => {",
            "AgentEvent::InputRejected",
        );
        assert!(read.contains("self.sent_device_contexts.record("));
    }

    #[test]
    fn a_run_owing_a_lane_batch_answers_it_whatever_way_it_ended() {
        assert!(has(
            RUN_SRC,
            "if let Some(batch_id) = lane_batch.filter(|_| !lane_answered) {"
        ));
        assert!(RUN_SRC.contains("self.report_lane_thread_end(batch_id, thread_id, end);"));
    }

    #[test]
    fn the_edit_note_is_told_what_the_turn_gap_note_explains() {
        let gap =
            at("let gap_note = crate::engine::agent_session::turn_gap::compute_turn_gap_note(");
        let edit = at("gap_note.as_ref().is_some_and(|n| n.explains_worktree_reset),");
        assert!(gap < edit, "the turn-gap note is computed first");
    }
}
