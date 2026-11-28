use super::{compute_turn_gap_note, compute_turn_gap_note_until};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ActorMode, ChildCompletionStatus, EventChannel, EventMeta, ThreadEvent,
};
use crate::test_support::start_cc_session;
use uuid::Uuid;

const BRANCH: &str = "claude-code/turn-gap";

fn cc_meta() -> EventMeta {
    EventMeta {
        channel: Some(EventChannel::ClaudeCode),
        ..EventMeta::NONE
    }
}

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) -> Uuid {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: cc_meta(),
    })
    .await
    .unwrap()
    .expect("emit produced no EmitResult")
    .event_id
}

async fn emit_message_received(bus: &EventBus, thread_id: Uuid, text: &str) -> Uuid {
    emit(
        bus,
        thread_id,
        ThreadEvent::MessageReceived {
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
        },
    )
    .await
}

async fn emit_engine_prompt(bus: &EventBus, thread_id: Uuid, text: &str) -> Uuid {
    emit_delivery(bus, thread_id, text, None).await
}

async fn emit_read(bus: &EventBus, thread_id: Uuid, input_event_id: Uuid) {
    emit(
        bus,
        thread_id,
        ThreadEvent::CodingAgentInputRead {
            input_event_id,
            started_turn: false,
            verbatim: false,
        },
    )
    .await;
}

async fn emit_delivery(
    bus: &EventBus,
    thread_id: Uuid,
    text: &str,
    input_event_id: Option<Uuid>,
) -> Uuid {
    emit(
        bus,
        thread_id,
        ThreadEvent::CodingAgentPromptSent {
            text: text.into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            origin: None,
            input_event_id,
        },
    )
    .await
}

async fn emit_idled(bus: &EventBus, thread_id: Uuid) {
    emit(
        bus,
        thread_id,
        ThreadEvent::CodingAgentIdled {
            has_changes: true,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: Some("sid".into()),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
            turn_end: None,
        },
    )
    .await;
}

async fn emit_change_applied(
    bus: &EventBus,
    thread_id: Uuid,
    commits: Vec<&str>,
    post_merge_sha: Option<&str>,
) {
    emit(
        bus,
        thread_id,
        ThreadEvent::ChangeApplied {
            content: Default::default(),
            change_id: Uuid::new_v4().to_string(),
            requires_restart: false,
            client_update: false,
            commits: commits.into_iter().map(|s| s.to_string()).collect(),
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: post_merge_sha.map(|s| s.to_string()),
            landing_merge_sha: None,
            path: String::new(),
        },
    )
    .await;
}

async fn emit_change_proposed(
    bus: &EventBus,
    thread_id: Uuid,
    branch: &str,
    description: &str,
) -> String {
    let change_id = Uuid::new_v4().to_string();
    emit(
        bus,
        thread_id,
        ThreadEvent::ChangeProposed {
            content: Default::default(),
            change_id: change_id.clone(),
            description: Some(description.into()),
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
    )
    .await;
    change_id
}

async fn emit_change_discarded(bus: &EventBus, thread_id: Uuid, change_id: &str) {
    emit(
        bus,
        thread_id,
        ThreadEvent::ChangeDiscarded {
            change_id: change_id.to_string(),
            actor: None,
            path: String::new(),
        },
    )
    .await;
}

async fn session_with_proposed_change(
    bus: &EventBus,
    thread_id: Uuid,
    branch: &str,
    description: &str,
) -> String {
    start_cc_session(bus, thread_id, branch, None).await;
    emit_message_received(bus, thread_id, "do the thing").await;
    let change_id = emit_change_proposed(bus, thread_id, branch, description).await;
    emit_idled(bus, thread_id).await;
    change_id
}

#[tokio::test]
async fn applied_change_after_last_prompt_produces_note() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "do the thing").await;
    emit_idled(&bus, thread_id).await;
    emit_change_applied(
        &bus,
        thread_id,
        vec!["feat(notifications): instrument SW push/notificationclick"],
        Some("78e105d2e9ab"),
    )
    .await;
    let current = emit_message_received(&bus, thread_id, "now do more").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .expect("expected a turn-gap note");
    assert!(
        note.note.to_lowercase().contains("applied"),
        "{}",
        note.note
    );
    assert!(
        note.note
            .contains("feat(notifications): instrument SW push/notificationclick"),
        "{}",
        note.note
    );
    assert!(note.note.contains("78e105d2"), "short sha: {}", note.note);
    assert!(
        !note.note.contains("78e105d2e9ab"),
        "sha not shortened: {}",
        note.note
    );
    assert!(
        note.note.contains("worktree as reset to match main"),
        "note must say the worktree was reset: {}",
        note.note
    );
    assert!(
        note.note.contains("NOT pending") && note.note.contains("awaiting Apply"),
        "note must stop the agent calling applied work pending: {}",
        note.note
    );
    assert!(
        note.explains_worktree_reset,
        "an apply resets the worktree to main"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn no_gap_event_after_last_prompt_is_none() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "do the thing").await;
    emit_idled(&bus, thread_id).await;
    let current = emit_message_received(&bus, thread_id, "now do more").await;

    assert!(
        compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
            .await
            .is_none()
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn note_self_clears_on_next_plain_turn() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "turn 1").await;
    emit_idled(&bus, thread_id).await;
    emit_change_applied(
        &bus,
        thread_id,
        vec!["feat: landed work"],
        Some("aaaaaaaa11"),
    )
    .await;

    let turn2 = emit_message_received(&bus, thread_id, "turn 2").await;
    assert!(
        compute_turn_gap_note(&pool, thread_id, turn2, Some(BRANCH))
            .await
            .is_some(),
        "note must fire on the first turn after the apply"
    );

    let turn3 = emit_message_received(&bus, thread_id, "turn 3").await;
    assert!(
        compute_turn_gap_note(&pool, thread_id, turn3, Some(BRANCH))
            .await
            .is_none(),
        "note must NOT re-fire on the following plain turn"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn trailing_idle_after_apply_still_produces_note() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "do the thing").await;
    emit_idled(&bus, thread_id).await;
    emit_change_applied(
        &bus,
        thread_id,
        vec!["feat: landed work"],
        Some("bbbbbbbb22"),
    )
    .await;
    emit_idled(&bus, thread_id).await;
    let current = emit_message_received(&bus, thread_id, "continue").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .expect("trailing idle must not suppress the note");
    assert!(note.note.contains("feat: landed work"), "{}", note.note);

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn multiple_events_listed_oldest_first_and_capped() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_engine_prompt(&bus, thread_id, "/harden").await;
    emit_change_applied(&bus, thread_id, vec!["feat: alpha"], Some("11111111")).await;
    let stale = emit_change_proposed(&bus, thread_id, "claude-code/stale", "the stale one").await;
    emit_change_discarded(&bus, thread_id, &stale).await;
    emit_change_applied(&bus, thread_id, vec!["feat: beta"], Some("22222222")).await;
    let current = emit_message_received(&bus, thread_id, "next").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .unwrap()
        .note;
    let alpha = note.find("feat: alpha").expect("alpha present");
    let discarded = note.find("DISCARDED").expect("discard present");
    let beta = note.find("feat: beta").expect("beta present");
    assert!(
        alpha < discarded && discarded < beta,
        "oldest-first ordering violated: {note}"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;

    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    emit_message_received(&bus, thread_id, "big batch").await;
    let commits: Vec<String> = (0..60).map(|i| format!("commit {i}")).collect();
    emit(
        &bus,
        thread_id,
        ThreadEvent::ChangeApplied {
            content: Default::default(),
            change_id: Uuid::new_v4().to_string(),
            requires_restart: false,
            client_update: false,
            commits,
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: Some("deadbeef".into()),
            landing_merge_sha: None,
            path: String::new(),
        },
    )
    .await;
    let current = emit_message_received(&bus, thread_id, "next").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .unwrap()
        .note;
    assert!(
        note.contains("and 40 more"),
        "expected truncation note: {note}"
    );
    assert!(note.contains("commit 0"), "first commit present: {note}");
    assert!(
        !note.contains("commit 59"),
        "tail commit must be truncated: {note}"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn noop_apply_renders_gracefully() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "do the thing").await;
    emit_change_applied(&bus, thread_id, vec![], None).await;
    let current = emit_message_received(&bus, thread_id, "next").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .unwrap()
        .note;
    assert!(note.to_lowercase().contains("applied"), "note: {note}");
    assert!(
        note.contains("no commits") || note.contains("already merged"),
        "no-op wording missing: {note}"
    );
    assert!(!note.contains("\n- \n"), "empty bullet rendered: {note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn child_completion_origin_advances_boundary() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "turn 1").await;
    emit_change_applied(
        &bus,
        thread_id,
        vec!["feat: landed work"],
        Some("cccccccc33"),
    )
    .await;

    let turn2 = emit(
        &bus,
        thread_id,
        ThreadEvent::ChildThreadCompleted {
            child_thread_id: Uuid::new_v4(),
            child_thread_title: Some("child".into()),
            status: ChildCompletionStatus::Success,
            summary: "child did the thing".into(),
            pending_change_ids: vec![],
            sub_thread_pending_changes: vec![],
            integrated_branch: None,
        },
    )
    .await;
    assert!(
        compute_turn_gap_note(&pool, thread_id, turn2, Some(BRANCH))
            .await
            .is_some(),
        "note must fire on the child-woken turn after the apply"
    );

    let turn3 = emit_message_received(&bus, thread_id, "turn 3").await;
    assert!(
        compute_turn_gap_note(&pool, thread_id, turn3, Some(BRANCH))
            .await
            .is_none(),
        "note must NOT re-fire after a child-woken turn already carried it"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn discard_produces_note() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let change_id = session_with_proposed_change(&bus, thread_id, BRANCH, "add the feature").await;

    emit_change_discarded(&bus, thread_id, &change_id).await;
    let current = emit_message_received(&bus, thread_id, "what happened?").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .expect("a discard must produce a note");
    assert!(note.note.contains("DISCARDED"), "{}", note.note);
    assert!(
        note.note.contains("add the feature"),
        "note must name the change: {}",
        note.note
    );
    assert!(
        note.note.contains(BRANCH),
        "note must name the branch: {}",
        note.note
    );
    assert!(
        note.note.contains("commits are gone"),
        "note must say the commits are gone: {}",
        note.note
    );
    assert!(
        note.note.contains("reset that branch to main"),
        "note must say the branch was reset to main: {}",
        note.note
    );
    assert!(
        note.note.contains("NOT pending") && note.note.contains("do not offer to Apply"),
        "note must forbid re-offering Apply: {}",
        note.note
    );
    assert!(
        note.explains_worktree_reset,
        "a discard resets the worktree to main"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn discard_note_self_clears() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let change_id = session_with_proposed_change(&bus, thread_id, BRANCH, "add the feature").await;

    emit_change_discarded(&bus, thread_id, &change_id).await;

    let turn2 = emit_message_received(&bus, thread_id, "turn 2").await;
    assert!(
        compute_turn_gap_note(&pool, thread_id, turn2, Some(BRANCH))
            .await
            .is_some(),
        "note must fire on the first turn after the discard"
    );

    let turn3 = emit_message_received(&bus, thread_id, "turn 3").await;
    assert!(
        compute_turn_gap_note(&pool, thread_id, turn3, Some(BRANCH))
            .await
            .is_none(),
        "discard note must NOT re-fire on the following turn"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn other_branch_says_current_work_untouched() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    session_with_proposed_change(&bus, thread_id, BRANCH, "the live work").await;

    let stale = emit_change_proposed(&bus, thread_id, "claude-code/stale", "an old change").await;
    emit_change_discarded(&bus, thread_id, &stale).await;
    let current = emit_message_received(&bus, thread_id, "carry on").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .unwrap();
    assert!(note.note.contains("claude-code/stale"), "{}", note.note);
    assert!(
        note.note.contains("NOT your current branch") && note.note.contains(BRANCH),
        "note must contrast with the session branch: {}",
        note.note
    );
    assert!(
        note.note.contains("your current work is untouched"),
        "note must reassure about live work: {}",
        note.note
    );
    assert!(
        !note.explains_worktree_reset,
        "a discard on another branch did not move this worktree"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn later_boundary_after_the_origin_does_not_swallow_the_gap() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let change_id = session_with_proposed_change(&bus, thread_id, BRANCH, "add the feature").await;

    emit_change_discarded(&bus, thread_id, &change_id).await;

    let origin = emit_message_received(&bus, thread_id, "what happened?").await;
    emit_message_received(&bus, thread_id, "also, do X next").await;

    let note = compute_turn_gap_note(&pool, thread_id, origin, Some(BRANCH))
        .await
        .expect("a message arriving after the origin must not hide the discard");
    assert!(note.note.contains("DISCARDED"), "{}", note.note);

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn event_after_the_origin_is_carried_to_the_next_turn() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let change_id = session_with_proposed_change(&bus, thread_id, BRANCH, "add the feature").await;

    let origin = emit_message_received(&bus, thread_id, "status?").await;
    emit_change_discarded(&bus, thread_id, &change_id).await;

    assert!(
        compute_turn_gap_note(&pool, thread_id, origin, Some(BRANCH))
            .await
            .is_none(),
        "an event after this turn's origin is not in this turn's gap"
    );

    let next = emit_message_received(&bus, thread_id, "and now?").await;
    let note = compute_turn_gap_note(&pool, thread_id, next, Some(BRANCH))
        .await
        .expect("the discard must surface on the following turn instead of vanishing");
    assert!(note.note.contains("DISCARDED"), "{}", note.note);

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn revert_produces_note() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let change_id = session_with_proposed_change(&bus, thread_id, BRANCH, "the feature").await;

    emit(
        &bus,
        thread_id,
        ThreadEvent::ChangeReverted {
            change_id: change_id.clone(),
            actor: None,
            path: String::new(),
        },
    )
    .await;
    let current = emit_message_received(&bus, thread_id, "and now?").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .expect("a revert must produce a note");
    assert!(note.note.contains("REVERTED"), "{}", note.note);
    assert!(note.note.contains("the feature"), "{}", note.note);
    assert!(
        note.note.contains("main no longer contains that work"),
        "note must correct the 'it is in main' belief: {}",
        note.note
    );
    assert!(
        !note.explains_worktree_reset,
        "a revert runs in the main repo and must not silence the external-edit note"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn apply_failed_quotes_engine_error() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let change_id = session_with_proposed_change(&bus, thread_id, BRANCH, "the feature").await;

    let error = "Hardening did not complete (no marker recorded). Click Apply again to retry.";
    emit(
        &bus,
        thread_id,
        ThreadEvent::ChangeApplyFailed {
            pre_merge_sha: None,
            post_merge_sha: None,
            landing_merge_sha: None,
            change_id: change_id.clone(),
            error: error.into(),
            actor: None,
        },
    )
    .await;
    let current = emit_message_received(&bus, thread_id, "why did that fail?").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .expect("an apply failure must produce a note");
    assert!(note.note.contains("APPLY FAILED"), "{}", note.note);
    assert!(
        note.note.contains("STILL PENDING"),
        "the change really is still pending: {}",
        note.note
    );
    assert!(
        note.note
            .contains(&format!("showed the user this message: \"{}\"", error)),
        "the error must be quoted and attributed to the engine, not addressed to the agent: {}",
        note.note
    );
    assert!(
        !note.explains_worktree_reset,
        "a failed apply moves nothing"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_stopped_child_reaches_the_resumed_parent_as_alive() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    emit_message_received(&bus, thread_id, "turn 1").await;

    emit(
        &bus,
        thread_id,
        ThreadEvent::ChildThreadStopped {
            child_thread_id: child_id,
            child_thread_title: Some("Fix the ticket".into()),
        },
    )
    .await;
    let current = emit_message_received(&bus, thread_id, "what now?").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .expect("a stopped child must produce a note");
    for needle in [
        "CHILD STOPPED",
        "Fix the ticket",
        "NOT finished",
        "do not roll back",
    ] {
        assert!(
            note.note.contains(needle),
            "missing {needle:?}: {}",
            note.note
        );
    }
    assert!(note.note.contains(&child_id.to_string()), "{}", note.note);
    assert!(
        !note.explains_worktree_reset,
        "a child's Stop moves nothing in the parent's worktree"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

fn filed(child_thread_id: Uuid, summary: &str) -> ThreadEvent {
    ThreadEvent::ChildReportFiled {
        child_thread_id,
        child_thread_title: Some("Check the venue".into()),
        status: crate::engine::thread_events::ChildCompletionStatus::Success,
        summary: summary.into(),
        pending_change_ids: vec![],
        sub_thread_pending_changes: vec![],
        integrated_branch: None,
    }
}

#[tokio::test]
async fn each_filed_report_reaches_the_next_turn_once() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    emit_message_received(&bus, thread_id, "turn 1").await;

    emit(&bus, thread_id, filed(child_id, "First finding.")).await;
    let second_turn = emit_message_received(&bus, thread_id, "turn 2").await;
    let note = compute_turn_gap_note(&pool, thread_id, second_turn, Some(BRANCH))
        .await
        .expect("a filed report must reach the next turn")
        .note;
    for needle in [
        "[CHILD THREAD REPORT FILED]",
        "First finding.",
        "did not wake you",
    ] {
        assert!(note.contains(needle), "missing {needle:?}: {note}");
    }

    emit(&bus, thread_id, filed(child_id, "Second finding.")).await;
    let third_turn = emit_message_received(&bus, thread_id, "turn 3").await;
    let note = compute_turn_gap_note(&pool, thread_id, third_turn, Some(BRANCH))
        .await
        .expect("the second report must reach the turn after it")
        .note;
    assert!(note.contains("Second finding."), "{note}");
    assert!(
        !note.contains("First finding."),
        "a report the previous turn carried must not repeat: {note}"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_event_landing_before_the_delivery_rides_that_follow_up_once() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    emit_message_received(&bus, thread_id, "turn 1").await;

    let follow_up = emit_message_received(&bus, thread_id, "follow-up").await;
    emit(&bus, thread_id, filed(child_id, "Landed in the window.")).await;
    let delivered = emit_engine_prompt(&bus, thread_id, "follow-up").await;
    let note = compute_turn_gap_note_until(&pool, thread_id, follow_up, delivered, Some(BRANCH))
        .await
        .expect("the window event must ride the follow-up")
        .note;
    assert!(note.contains("Landed in the window."), "{note}");

    let next = emit_message_received(&bus, thread_id, "next").await;
    let next_delivered = emit_engine_prompt(&bus, thread_id, "next").await;
    assert!(
        compute_turn_gap_note_until(&pool, thread_id, next, next_delivered, Some(BRANCH))
            .await
            .is_none(),
        "the follow-up carried it, so the next prompt must not"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_withdrawn_follow_up_leaves_its_gap_to_the_next_prompt() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    let turn1 = emit_message_received(&bus, thread_id, "turn 1").await;
    emit_read(&bus, thread_id, turn1).await;
    emit(&bus, thread_id, filed(child_id, "Filed mid-turn.")).await;

    let withdrawn = emit_message_received(&bus, thread_id, "never read").await;
    emit_delivery(&bus, thread_id, "never read", Some(withdrawn)).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::QueuedMessageRemoved {
            removed_message_id: withdrawn,
            restore_to_compose: false,
            compose: None,
        },
    )
    .await;

    let next = emit_message_received(&bus, thread_id, "next").await;
    let delivered = emit_delivery(&bus, thread_id, "next", Some(next)).await;
    let note = compute_turn_gap_note_until(&pool, thread_id, next, delivered, Some(BRANCH))
        .await
        .expect("the withdrawn follow-up's gap must ride the next prompt")
        .note;
    assert!(note.contains("Filed mid-turn."), "{note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_delivery_the_agent_never_read_moves_no_bound() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    let turn1 = emit_message_received(&bus, thread_id, "turn 1").await;
    emit_read(&bus, thread_id, turn1).await;
    emit(&bus, thread_id, filed(child_id, "Filed mid-turn.")).await;

    let lost = emit_message_received(&bus, thread_id, "lost in the send").await;
    emit_delivery(&bus, thread_id, "lost in the send", Some(lost)).await;

    let next = emit_message_received(&bus, thread_id, "next").await;
    let delivered = emit_delivery(&bus, thread_id, "next", Some(next)).await;
    let note = compute_turn_gap_note_until(&pool, thread_id, next, delivered, Some(BRANCH))
        .await
        .expect("an unread delivery must not hide the report")
        .note;
    assert!(note.contains("Filed mid-turn."), "{note}");
    let respawned = compute_turn_gap_note(&pool, thread_id, next, Some(BRANCH))
        .await
        .expect("a respawn must carry it too")
        .note;
    assert!(respawned.contains("Filed mid-turn."), "{respawned}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_delivery_the_agent_read_moves_the_bound() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    let turn1 = emit_message_received(&bus, thread_id, "turn 1").await;
    emit_read(&bus, thread_id, turn1).await;
    emit(&bus, thread_id, filed(child_id, "Filed mid-turn.")).await;

    let follow_up = emit_message_received(&bus, thread_id, "follow-up").await;
    emit_delivery(&bus, thread_id, "follow-up", Some(follow_up)).await;
    emit_read(&bus, thread_id, follow_up).await;

    let next = emit_message_received(&bus, thread_id, "next").await;
    let delivered = emit_delivery(&bus, thread_id, "next", Some(next)).await;
    assert!(
        compute_turn_gap_note_until(&pool, thread_id, next, delivered, Some(BRANCH))
            .await
            .is_none(),
        "the read follow-up carried the report, so the next prompt must not"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_assumed_read_moves_no_bound() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    let turn1 = emit_message_received(&bus, thread_id, "turn 1").await;
    emit_read(&bus, thread_id, turn1).await;
    emit(&bus, thread_id, filed(child_id, "Filed mid-turn.")).await;

    let settled = emit_message_received(&bus, thread_id, "settled unread").await;
    emit_delivery(&bus, thread_id, "settled unread", Some(settled)).await;
    let old_read = serde_json::json!({ "input_event_id": settled, "assumed": true });
    assert!(matches!(
        ThreadEvent::from_stored("CodingAgentInputRead", old_read.clone()),
        Ok(ThreadEvent::CodingAgentInputRead { input_event_id, .. }) if input_event_id == settled
    ));
    sqlx::query(
        "INSERT INTO events (id, event_type, payload, thread_id, aggregate, aggregate_id) \
             VALUES ($1, 'CodingAgentInputRead', $2, $3, 'thread', $3::text)",
    )
    .bind(Uuid::new_v4())
    .bind(old_read)
    .bind(thread_id)
    .execute(&pool)
    .await
    .expect("seed an old assumed read");

    let next = emit_message_received(&bus, thread_id, "next").await;
    let note = compute_turn_gap_note(&pool, thread_id, next, Some(BRANCH))
        .await
        .expect("an assumed read must not hide the report")
        .note;
    assert!(note.contains("Filed mid-turn."), "{note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_verbatim_read_moves_no_bound() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    let turn1 = emit_message_received(&bus, thread_id, "turn 1").await;
    emit_read(&bus, thread_id, turn1).await;
    emit(
        &bus,
        thread_id,
        filed(Uuid::new_v4(), "Before the command."),
    )
    .await;

    let command = emit_message_received(&bus, thread_id, "/cost").await;
    emit_delivery(&bus, thread_id, "/cost", Some(command)).await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::CodingAgentInputRead {
            input_event_id: command,
            started_turn: false,
            verbatim: true,
        },
    )
    .await;
    emit(&bus, thread_id, filed(Uuid::new_v4(), "After the command.")).await;

    let next = emit_message_received(&bus, thread_id, "next").await;
    let delivered = emit_delivery(&bus, thread_id, "next", Some(next)).await;
    let note = compute_turn_gap_note_until(&pool, thread_id, next, delivered, Some(BRANCH))
        .await
        .expect("a verbatim command must not hide the gap")
        .note;
    assert!(note.contains("Before the command."), "{note}");
    assert!(note.contains("After the command."), "{note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_boundary_older_than_the_first_read_still_counts() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    emit_message_received(&bus, thread_id, "turn 1").await;
    emit(&bus, thread_id, filed(child_id, "Long since read.")).await;
    emit_message_received(&bus, thread_id, "turn 2").await;

    let other = Uuid::new_v4();
    start_cc_session(&bus, other, "claude-code/other", None).await;
    let elsewhere = emit_message_received(&bus, other, "elsewhere").await;
    emit_read(&bus, other, elsewhere).await;

    let next = emit_message_received(&bus, thread_id, "turn 3").await;
    assert!(
        compute_turn_gap_note(&pool, thread_id, next, Some(BRANCH))
            .await
            .is_none(),
        "turn 2 predates reads, so it still bounds the gap"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_queued_follow_up_does_not_swallow_the_gap_of_the_one_before() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    emit_message_received(&bus, thread_id, "turn 1").await;
    emit(&bus, thread_id, filed(child_id, "Filed mid-turn.")).await;

    let first = emit_message_received(&bus, thread_id, "first").await;
    emit_message_received(&bus, thread_id, "queued second").await;
    let first_delivered = emit_engine_prompt(&bus, thread_id, "first").await;
    let note = compute_turn_gap_note_until(&pool, thread_id, first, first_delivered, Some(BRANCH))
        .await
        .expect("the queued message must not hide the report")
        .note;
    assert!(note.contains("Filed mid-turn."), "{note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_origin_less_engine_prompt_carries_the_gap_once() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    let turn1 = emit_message_received(&bus, thread_id, "turn 1").await;
    emit_read(&bus, thread_id, turn1).await;
    emit(&bus, thread_id, filed(child_id, "Filed before harden.")).await;

    let harden = emit_engine_prompt(&bus, thread_id, "run /harden").await;
    let note = compute_turn_gap_note_until(&pool, thread_id, harden, harden, Some(BRANCH))
        .await
        .expect("the engine prompt must carry the report")
        .note;
    assert!(note.contains("Filed before harden."), "{note}");
    emit_read(&bus, thread_id, harden).await;

    let next = emit_message_received(&bus, thread_id, "next").await;
    assert!(
        compute_turn_gap_note(&pool, thread_id, next, Some(BRANCH))
            .await
            .is_none(),
        "the engine prompt carried it, so the next turn must not"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_moved_out_child_reaches_the_resumed_parent() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let child_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    emit_message_received(&bus, thread_id, "turn 1").await;
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, parent_thread_id, depth) VALUES ($1, $2, 1)",
    )
    .bind(child_id)
    .bind(thread_id)
    .execute(&pool)
    .await
    .unwrap();

    emit(
        &bus,
        thread_id,
        ThreadEvent::ChildThreadDetached {
            child_thread_id: child_id,
            child_thread_title: Some("Fix the ticket".into()),
        },
    )
    .await;
    let current = emit_message_received(&bus, thread_id, "what now?").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .expect("a moved-out child must produce a note");
    for needle in [
        "CHILD MOVED OUT",
        "Fix the ticket",
        "no longer your child",
        "Do not wait for it",
    ] {
        assert!(
            note.note.contains(needle),
            "missing {needle:?}: {}",
            note.note
        );
    }
    assert!(!note.explains_worktree_reset);

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn tier_1_clean_reports_artifacts_without_claiming_a_reset() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    emit_message_received(&bus, thread_id, "turn 1").await;

    emit(
        &bus,
        thread_id,
        ThreadEvent::WorktreeCleaned {
            tier: 1,
            freed_bytes: 3 * 1024 * 1024 * 1024,
            branch_deleted: false,
        },
    )
    .await;
    let current = emit_message_received(&bus, thread_id, "carry on").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .expect("a clean must produce a note");
    assert!(note.note.contains("WORKTREE CLEANED"), "{}", note.note);
    assert!(note.note.contains("3.0 GB"), "{}", note.note);
    assert!(
        note.note.contains("next build starts cold"),
        "{}",
        note.note
    );
    assert!(
        !note.explains_worktree_reset,
        "tier 1 strips build artifacts and moves no ref"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[test]
fn tier_0_clean_reports_full_removal() {
    let event = super::GapEvent::WorktreeCleaned {
        tier: 0,
        freed_bytes: 512 * 1024 * 1024,
        branch_deleted: false,
    };
    assert!(event.moved_the_worktree(BRANCH, Some(BRANCH)));
    assert!(super::worktree_cleaned_line(0, 512 * 1024 * 1024, false)
        .contains("removed your whole worktree"));
}

#[tokio::test]
async fn tier_2_clean_explains_the_reset() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    emit_message_received(&bus, thread_id, "turn 1").await;

    emit(
        &bus,
        thread_id,
        ThreadEvent::WorktreeCleaned {
            tier: 2,
            freed_bytes: 512 * 1024 * 1024,
            branch_deleted: true,
        },
    )
    .await;
    let current = emit_message_received(&bus, thread_id, "carry on").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .unwrap();
    assert!(note.note.contains("512 MB"), "{}", note.note);
    assert!(
        note.note
            .contains(crate::engine::worktree_cleanup::SNAPSHOT_COMMIT_MESSAGE),
        "{}",
        note.note
    );
    assert!(
        note.note.contains("branch was deleted as fully merged"),
        "{}",
        note.note
    );
    assert!(
        note.explains_worktree_reset,
        "tier 2 removes and recreates the worktree"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn unknown_change_id_renders_gracefully() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    emit_message_received(&bus, thread_id, "turn 1").await;

    emit(
        &bus,
        thread_id,
        ThreadEvent::ChangeDiscarded {
            change_id: String::new(),
            actor: None,
            path: "legacy/path".into(),
        },
    )
    .await;
    let current = emit_message_received(&bus, thread_id, "what happened?").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .expect("an unresolvable change must still be reported")
        .note;
    assert!(note.contains("DISCARDED"), "{note}");
    assert!(note.contains("change ?"), "fallback label missing: {note}");
    assert!(!note.contains("\n- \n"), "empty bullet rendered: {note}");
    assert!(!note.contains("branch ``"), "empty branch rendered: {note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn missing_session_branch_still_renders() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let change_id = session_with_proposed_change(&bus, thread_id, BRANCH, "the feature").await;

    emit_change_discarded(&bus, thread_id, &change_id).await;
    let current = emit_message_received(&bus, thread_id, "what happened?").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, None)
        .await
        .unwrap()
        .note;
    assert!(note.contains("DISCARDED"), "{note}");
    assert!(note.contains(BRANCH), "branch still named: {note}");
    assert!(
        !note.contains("NOT your current branch"),
        "no comparison without a session branch: {note}"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_background_completion_nobody_delivered_reaches_the_resumed_agent() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "run the suite").await;
    emit(
        &bus,
        thread_id,
        completion("task-finished", "cargo test --lib", Some(0), false),
    )
    .await;
    emit(
        &bus,
        thread_id,
        completion("task-lost", "./scripts/e2e.sh", None, true),
    )
    .await;
    let current = emit_message_received(&bus, thread_id, "how did it go?").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .expect("an abandoned background task must reach the resumed agent")
        .note;
    assert!(note.contains("BACKGROUND TASK LOST"), "{note}");
    assert!(note.contains("./scripts/e2e.sh"), "{note}");
    assert!(
        note.contains("BACKGROUND TASK FINISHED"),
        "an undelivered ordinary completion is news too: {note}"
    );
    assert!(note.contains("cargo test --lib"), "{note}");
    assert!(
        note.contains("exit code 0"),
        "the ordinary line reports the verdict: {note}"
    );
    assert!(
        note.contains("task task-lost"),
        "the id must be usable: {note}"
    );
    assert!(
        note.contains("task task-finished"),
        "the id must be usable: {note}"
    );
    assert!(
        !note.contains("./scripts/e2e.sh` (task task-los) ended"),
        "an abandoned task has no verdict to report: {note}"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_completion_read_inside_the_turn_is_not_repeated() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "run the suite").await;
    emit(
        &bus,
        thread_id,
        completion("task-polled", "cargo test --lib", Some(0), false),
    )
    .await;
    emit_idled(&bus, thread_id).await;
    emit(
        &bus,
        thread_id,
        completion("task-after", "./scripts/e2e.sh", Some(0), false),
    )
    .await;
    let next = emit_message_received(&bus, thread_id, "and now?").await;

    let note = compute_turn_gap_note(&pool, thread_id, next, Some(BRANCH))
        .await
        .expect("the completion after the idle is still news")
        .note;
    assert!(!note.contains("task-polled"), "{note}");
    assert!(note.contains("task task-after"), "{note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_continuation_request_that_never_ran_does_not_swallow_the_note() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "run the suite").await;
    emit(
        &bus,
        thread_id,
        completion("task-lost", "./scripts/e2e.sh", None, true),
    )
    .await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::ContinuationRequested {
            reason: crate::engine::agent_recovery::AUTO_RESUME_AFTER_SWITCH_REASON.into(),
        },
    )
    .await;

    let next = emit_message_received(&bus, thread_id, "what now?").await;
    let note = compute_turn_gap_note(&pool, thread_id, next, Some(BRANCH))
        .await
        .expect("an unactuated resume request must not consume the note")
        .note;
    assert!(note.contains("BACKGROUND TASK LOST"), "{note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_background_completion_an_event_wait_delivered_is_not_repeated() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "run the suite").await;
    let completed = emit(
        &bus,
        thread_id,
        completion("task-woke", "cargo test --lib", Some(0), false),
    )
    .await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::EventWaitDelivered {
            wait_id: Uuid::new_v4(),
            event_id: completed,
            event_type: "BackgroundBashCompleted".into(),
            payload: serde_json::json!({"task_id": "task-woke"}),
            matched_index: 0,
        },
    )
    .await;
    let current = emit_message_received(&bus, thread_id, "how did it go?").await;

    assert!(
        compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
            .await
            .is_none(),
        "the delivery already told the agent, so the gap holds nothing to say"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_task_the_thread_stopped_itself_gets_no_line() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "lint it").await;
    let watching = |task_id: &str| crate::core::event_subscription::EventSubscription {
        event_type: "BackgroundBashCompleted".into(),
        condition: Some(serde_json::json!({ "task_id": task_id })),
    };
    emit(
        &bus,
        thread_id,
        ThreadEvent::EventWaitCanceled {
            wait_id: Uuid::new_v4(),
            cause: crate::engine::thread_events::EventWaitCancelCause::AgentStandDown,
            on: vec![watching("task-stopped")],
            reason: "make lint to finish".into(),
        },
    )
    .await;
    for task_id in ["task-stopped", "task-killed-elsewhere"] {
        let mut killed = completion(task_id, "make lint", None, false);
        if let ThreadEvent::BackgroundBashCompleted {
            killed: flag,
            signal,
            ..
        } = &mut killed
        {
            *flag = true;
            *signal = Some(15);
        }
        emit(&bus, thread_id, killed).await;
    }
    let current = emit_message_received(&bus, thread_id, "ready?").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .unwrap()
        .note;
    assert!(
        !note.contains("task-stopped"),
        "a self stop is not news: {note}"
    );
    assert!(
        note.contains("task-killed-elsewhere"),
        "a kill whose watch was never stood down still reports: {note}"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_old_stand_down_does_not_hide_a_later_kill() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "lint it").await;
    let canceled = emit(
        &bus,
        thread_id,
        ThreadEvent::EventWaitCanceled {
            wait_id: Uuid::new_v4(),
            cause: crate::engine::thread_events::EventWaitCancelCause::AgentStandDown,
            on: vec![crate::core::event_subscription::EventSubscription {
                event_type: "BackgroundBashCompleted".into(),
                condition: Some(serde_json::json!({ "task_id": "task-dropped" })),
            }],
            reason: "make lint to finish".into(),
        },
    )
    .await;
    sqlx::query("UPDATE events SET created = created - interval '2 hours' WHERE id = $1")
        .bind(canceled)
        .execute(&pool)
        .await
        .unwrap();
    let mut killed = completion("task-dropped", "make lint", None, false);
    if let ThreadEvent::BackgroundBashCompleted {
        killed: flag,
        signal,
        ..
    } = &mut killed
    {
        *flag = true;
        *signal = Some(15);
    }
    emit(&bus, thread_id, killed).await;
    let current = emit_message_received(&bus, thread_id, "ready?").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .unwrap()
        .note;
    assert!(note.contains("task-dropped"), "{note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_ordinary_line_names_how_the_task_ended() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "run the suite").await;
    let mut timed_out = completion("task-slow", "cargo test --lib", None, false);
    if let ThreadEvent::BackgroundBashCompleted {
        timed_out: flag,
        signal,
        ..
    } = &mut timed_out
    {
        *flag = true;
        *signal = Some(9);
    }
    emit(&bus, thread_id, timed_out).await;
    let current = emit_message_received(&bus, thread_id, "how did it go?").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .unwrap()
        .note;
    assert!(
        note.contains("blowing its own timeout"),
        "a deadline is not a plain SIGKILL: {note}"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_large_apply_does_not_bury_the_background_task_line() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "land it").await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::ChangeApplied {
            content: Default::default(),
            change_id: Uuid::new_v4().to_string(),
            requires_restart: false,
            client_update: false,
            commits: (0..60).map(|i| format!("commit {i}")).collect(),
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: Some("deadbeef".into()),
            landing_merge_sha: None,
            path: String::new(),
        },
    )
    .await;
    emit(
        &bus,
        thread_id,
        completion("task-lost", "./scripts/e2e.sh", None, true),
    )
    .await;
    let current = emit_message_received(&bus, thread_id, "how did it go?").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .unwrap()
        .note;
    assert!(
        note.contains("BACKGROUND TASK LOST"),
        "a 60-commit apply must not spend the whole budget: {note}"
    );
    assert!(
        note.contains("commit 0"),
        "the apply is still reported: {note}"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_gap_full_of_applies_does_not_bury_the_background_task_line() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "land them all").await;
    for batch in 0..3 {
        emit(
            &bus,
            thread_id,
            ThreadEvent::ChangeApplied {
                content: Default::default(),
                change_id: Uuid::new_v4().to_string(),
                requires_restart: false,
                client_update: false,
                commits: (0..25)
                    .map(|i| format!("batch {batch} commit {i}"))
                    .collect(),
                thread_title: None,
                actor: None,
                pre_merge_sha: None,
                post_merge_sha: Some("deadbeef".into()),
                landing_merge_sha: None,
                path: String::new(),
            },
        )
        .await;
    }
    emit(
        &bus,
        thread_id,
        completion("task-lost", "./scripts/e2e.sh", None, true),
    )
    .await;
    let current = emit_message_received(&bus, thread_id, "how did it go?").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .unwrap()
        .note;
    assert!(
        note.contains("BACKGROUND TASK LOST"),
        "the only channel a lost task has must survive the budget: {note}"
    );
    assert!(note.contains("./scripts/e2e.sh"), "{note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

fn completion(
    task_id: &str,
    command: &str,
    exit_code: Option<i32>,
    abandoned: bool,
) -> ThreadEvent {
    ThreadEvent::BackgroundBashCompleted {
        task_id: task_id.into(),
        command: command.into(),
        exit_code,
        signal: None,
        stdout: String::new(),
        stderr: String::new(),
        started_at: chrono::Utc::now(),
        finished_at: chrono::Utc::now(),
        timed_out: false,
        killed: false,
        abandoned,
    }
}

const RESTART_LINE: &str = "ENGINE RESTART";

async fn emit_abort(
    bus: &EventBus,
    thread_id: Uuid,
    cause: crate::engine::thread_events::AbortCause,
    channel: Option<EventChannel>,
) {
    crate::engine::thread_events::emit_response_aborted(
        bus,
        thread_id,
        cause,
        String::new(),
        vec![],
        None,
        None,
        EventMeta {
            channel,
            ..EventMeta::NONE
        },
        "[test] ResponseAborted",
    )
    .await;
}

async fn emit_generated(bus: &EventBus, thread_id: Uuid) {
    emit(
        bus,
        thread_id,
        ThreadEvent::ResponseGenerated {
            text: "done".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
    )
    .await;
}

async fn emit_switch_continuation(bus: &EventBus, thread_id: Uuid) -> Uuid {
    emit(
        bus,
        thread_id,
        ThreadEvent::ContinuationRequested {
            reason: crate::engine::agent_recovery::AUTO_RESUME_AFTER_SWITCH_REASON.into(),
        },
    )
    .await
}

#[tokio::test]
async fn a_restart_that_cut_off_the_last_turn_is_named_on_resume() {
    use crate::engine::thread_events::AbortCause;

    for cause in [AbortCause::EngineShutdown, AbortCause::RecoveryAfterRestart] {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, BRANCH, None).await;

        emit_message_received(&bus, thread_id, "run the engine tests").await;
        emit_abort(&bus, thread_id, cause, Some(EventChannel::ClaudeCode)).await;
        let current = emit_switch_continuation(&bus, thread_id).await;

        let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
            .await
            .unwrap_or_else(|| panic!("{cause:?}: a restart abort must produce a note"));
        for must in [
            RESTART_LINE,
            "doesn't want to proceed",
            "[Request interrupted by user for tool use]",
            "nobody refused or interrupted anything",
            "did not complete",
            "safe to run it again",
        ] {
            assert!(
                note.note.contains(must),
                "{cause:?}: missing {must:?}: {}",
                note.note
            );
        }
        assert!(
            !note.explains_worktree_reset,
            "a restart moves no ref, so it must not excuse a HEAD move"
        );

        pool.close().await;
        crate::test_support::teardown_test_db(&db_name).await;
    }
}

#[tokio::test]
async fn two_restarts_in_one_gap_are_one_line() {
    use crate::engine::thread_events::AbortCause;

    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "run the engine tests").await;
    let cc = Some(EventChannel::ClaudeCode);
    emit_abort(&bus, thread_id, AbortCause::EngineShutdown, cc).await;
    emit_switch_continuation(&bus, thread_id).await;
    emit_abort(&bus, thread_id, AbortCause::EngineShutdown, cc).await;
    let current = emit_switch_continuation(&bus, thread_id).await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .expect("a restart abort must produce a note")
        .note;
    assert_eq!(note.matches(RESTART_LINE).count(), 1, "{note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_turn_that_finished_after_the_restart_carries_no_restart_line() {
    use crate::engine::thread_events::AbortCause;

    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "run the engine tests").await;
    emit_abort(
        &bus,
        thread_id,
        AbortCause::EngineShutdown,
        Some(EventChannel::ClaudeCode),
    )
    .await;
    emit_switch_continuation(&bus, thread_id).await;
    emit_generated(&bus, thread_id).await;
    let current = emit_message_received(&bus, thread_id, "thanks, what next?").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH)).await;
    assert!(
        note.is_none(),
        "a finished resume leaves nothing to report: {:?}",
        note.map(|n| n.note)
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn only_a_coding_agent_restart_abort_names_a_restart() {
    use crate::engine::thread_events::{AbortCause, CancelCause};

    enum Ending {
        Aborted(AbortCause, Option<EventChannel>),
        UserStop,
    }
    let cc = Some(EventChannel::ClaudeCode);
    for ending in [
        Ending::Aborted(AbortCause::ProcessKilled, cc),
        Ending::Aborted(AbortCause::SafetyNet, cc),
        Ending::Aborted(AbortCause::SessionDropped, cc),
        Ending::Aborted(AbortCause::StaleSettle, cc),
        Ending::Aborted(AbortCause::EngineShutdown, None),
        Ending::UserStop,
    ] {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread_id = Uuid::new_v4();
        start_cc_session(&bus, thread_id, BRANCH, None).await;
        emit_message_received(&bus, thread_id, "run the engine tests").await;

        let label = match ending {
            Ending::Aborted(cause, channel) => {
                emit_abort(&bus, thread_id, cause, channel).await;
                format!("{cause:?} on {channel:?}")
            }
            Ending::UserStop => {
                crate::engine::thread_events::emit_response_canceled(
                    &bus,
                    &pool,
                    thread_id,
                    CancelCause::UserStop,
                    String::new(),
                    vec![],
                    None,
                    None,
                    cc_meta(),
                    "[test] ResponseCanceled",
                )
                .await;
                "a user Stop".to_string()
            }
        };
        let current = emit_message_received(&bus, thread_id, "carry on").await;

        let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH)).await;
        assert!(
            note.is_none(),
            "{label} must not be reported as a restart: {:?}",
            note.map(|n| n.note)
        );

        pool.close().await;
        crate::test_support::teardown_test_db(&db_name).await;
    }
}

fn user_only_message(text: &str, device_id: &str) -> ThreadEvent {
    ThreadEvent::UserOnlyMessagePosted {
        text: text.into(),
        user_image_hashes: vec![],
        device_id: Some(device_id.into()),
        mentioned_user_ids: vec![],
    }
}

async fn emit_user_only_message(bus: &EventBus, thread_id: Uuid, text: &str, device_id: &str) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: user_only_message(text, device_id),
        meta: EventMeta::with_actor(Some(crate::engine::thread_events::MessageOrigin::Device {
            device_id: device_id.into(),
        })),
    })
    .await
    .unwrap()
    .expect("emit produced no EmitResult");
}

#[tokio::test]
async fn user_only_messages_reach_the_next_woken_prompt_once() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    emit_message_received(&bus, thread_id, "turn 1").await;

    emit_user_only_message(&bus, thread_id, "the copy feels long", "dev-alex").await;
    emit_user_only_message(&bus, thread_id, "agreed, cut it", "dev-sam").await;
    let woken = emit_message_received(&bus, thread_id, "cut the copy").await;
    let note = compute_turn_gap_note(&pool, thread_id, woken, Some(BRANCH))
        .await
        .expect("the paused talk must reach the woken prompt")
        .note;
    let first = note.find("the copy feels long").expect(&note);
    let second = note.find("agreed, cut it").expect(&note);
    assert!(first < second, "oldest first: {note}");
    assert!(note.contains("[User-only messages"), "{note}");

    let next = emit_message_received(&bus, thread_id, "and the title").await;
    assert!(
        compute_turn_gap_note(&pool, thread_id, next, Some(BRANCH))
            .await
            .is_none(),
        "the woken prompt carried the talk, so the next one must not"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[test]
fn the_people_block_names_each_speaker_and_keeps_the_newest() {
    use crate::gateway_users::{User, UserSnapshot};
    let user = |id: &str, name: &str, device: &str| User {
        id: id.into(),
        name: name.into(),
        emails: vec![],
        device_ids: vec![device.into()],
    };
    let team = UserSnapshot {
        users: vec![
            user("op-sam", "Sam", "dev-sam"),
            user("op-alex", "Alex", "dev-alex"),
        ],
        relayed: vec![],
    };
    let payload = |text: &str, device: &str| {
        serde_json::json!({
            "text": text,
            "actor": {"kind": "device", "device_id": device},
        })
    };
    let block = super::user_talk_block(
        &[
            payload("too long", "dev-alex"),
            payload("cut it", "dev-sam"),
        ],
        &team,
    )
    .expect("two messages make a block");
    assert!(block.contains("\nAlex: too long\nSam: cut it]"), "{block}");

    let many: Vec<_> = (0..super::MAX_USER_ONLY_MESSAGES + 2)
        .map(|i| payload(&format!("line {i}"), "dev-sam"))
        .collect();
    let block = super::user_talk_block(&many, &team).expect("a block");
    assert!(block.contains("2 earlier message(s) not shown"), "{block}");
    assert!(!block.contains("Sam: line 1\n"), "{block}");
    assert!(block.contains(&format!("Sam: line {}]", super::MAX_USER_ONLY_MESSAGES + 1)));
    assert!(super::user_talk_block(&[], &team).is_none());
}

#[test]
fn every_turn_origin_type_is_a_turn_boundary() {
    let boundary = super::boundary_event_types();
    for origin in crate::engine::thread_events::CC_ORIGINATING_EVENT_TYPES {
        assert!(
            boundary.iter().any(|b| b == origin),
            "{origin} originates a turn but is not a boundary"
        );
    }
    assert!(boundary.iter().any(|b| b == "CodingAgentPromptSent"));
}

#[tokio::test]
async fn a_long_filed_report_reaches_the_next_turn_whole() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    emit_message_received(&bus, thread_id, "turn 1").await;

    let summary = format!("{} END-OF-REPORT", "finding ".repeat(1000));
    emit(&bus, thread_id, filed(Uuid::new_v4(), &summary)).await;
    let next = emit_message_received(&bus, thread_id, "turn 2").await;
    let note = compute_turn_gap_note(&pool, thread_id, next, Some(BRANCH))
        .await
        .expect("a filed report must reach the next turn")
        .note;
    assert!(note.contains(&summary), "the report was cut short");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_background_task_line_carries_the_date_the_task_was_spawned() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    emit_message_received(&bus, thread_id, "run the suite").await;

    let spawned = chrono::DateTime::parse_from_rfc3339("2026-01-02T03:04:05Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    for event in [
        completion("task-lost", "./scripts/e2e.sh", None, true),
        completion("task-done", "cargo test --lib", Some(0), false),
    ] {
        let mut event = event;
        if let ThreadEvent::BackgroundBashCompleted { started_at, .. } = &mut event {
            *started_at = spawned;
        }
        emit(&bus, thread_id, event).await;
    }
    let current = emit_message_received(&bus, thread_id, "how did it go?").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .unwrap()
        .note;
    for task in ["task-lost", "task-done"] {
        assert!(
            note.contains(&format!("(task {task}, spawned 2026-01-02T03:04:05")),
            "{task} must carry its spawn date: {note}"
        );
    }

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn a_task_ended_by_bash_kill_says_so() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    emit_message_received(&bus, thread_id, "run the suite").await;

    let mut killed = completion("task-killed", "cargo test --lib", None, false);
    if let ThreadEvent::BackgroundBashCompleted {
        killed: flag,
        signal,
        ..
    } = &mut killed
    {
        *flag = true;
        *signal = Some(9);
    }
    emit(&bus, thread_id, killed).await;
    let current = emit_message_received(&bus, thread_id, "how did it go?").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .unwrap()
        .note;
    assert!(note.contains("because bash_kill stopped it"), "{note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn the_status_phrase_is_the_one_bash_output_describes() {
    use crate::core::shell::TaskOutcome;

    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    emit_message_received(&bus, thread_id, "run the suite").await;

    emit(
        &bus,
        thread_id,
        completion("task-exit", "cargo test --lib", Some(3), false),
    )
    .await;
    let mut crashed = completion("task-signal", "cargo build", None, false);
    if let ThreadEvent::BackgroundBashCompleted { signal, .. } = &mut crashed {
        *signal = Some(11);
    }
    emit(&bus, thread_id, crashed).await;
    let current = emit_message_received(&bus, thread_id, "how did it go?").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .unwrap()
        .note;
    for (exit_code, signal) in [(Some(3), None), (None, Some(11))] {
        let phrase = TaskOutcome::from_persisted(exit_code, signal).describe();
        assert!(note.contains(&format!("with {phrase}")), "{phrase}: {note}");
    }

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_unresolved_change_branch_counts_as_this_sessions_worktree() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    emit_message_received(&bus, thread_id, "turn 1").await;
    emit(
        &bus,
        thread_id,
        ThreadEvent::ChangeDiscarded {
            change_id: String::new(),
            actor: None,
            path: "legacy/path".into(),
        },
    )
    .await;
    let current = emit_message_received(&bus, thread_id, "what happened?").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .unwrap();
    assert!(note.note.contains("DISCARDED"), "{}", note.note);
    assert!(
        note.explains_worktree_reset,
        "the note says a reset happened, so the flag must agree"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

#[tokio::test]
async fn an_origin_that_was_never_persisted_loses_no_note() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;
    emit_message_received(&bus, thread_id, "do the thing").await;
    emit_change_applied(&bus, thread_id, vec!["feat: landed"], Some("abcdef012345")).await;

    let note = compute_turn_gap_note(&pool, thread_id, Uuid::new_v4(), Some(BRANCH))
        .await
        .expect("an unresolved bound must not hide the gap")
        .note;
    assert!(note.contains("feat: landed"), "{note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}
