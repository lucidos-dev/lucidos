use super::compute_turn_gap_note;
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

/// Emit a thread event and return the persisted event id.
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

/// The real user-message event on a coding-agent thread (the engine emits
/// `MessageReceived`, NOT `CodingAgentUserMessageSent`, for user turns). Returns
/// the event id so a test can pass it as the current-turn origin.
async fn emit_message_received(bus: &EventBus, thread_id: Uuid, text: &str) -> Uuid {
    emit(
        bus,
        thread_id,
        ThreadEvent::MessageReceived {
            voice_session_id: None,
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
        },
    )
    .await
}

async fn emit_engine_prompt(bus: &EventBus, thread_id: Uuid, text: &str) -> Uuid {
    emit(
        bus,
        thread_id,
        ThreadEvent::CodingAgentPromptSent {
            text: text.into(),
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            origin: None,
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
            change_id: Uuid::new_v4().to_string(),
            requires_restart: false,
            client_update: false,
            commits: commits.into_iter().map(|s| s.to_string()).collect(),
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: post_merge_sha.map(|s| s.to_string()),
            path: String::new(),
        },
    )
    .await;
}

/// Emit a `ChangeProposed` so the `changes` projection carries the branch and
/// description the note looks up. Returns the change id.
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

/// The common setup: a CC session on `branch` with a proposed change and a turn
/// boundary before it. Returns the change id.
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

// -------------------- Applied (carried over from applied_changes) --------------------

/// 1. prev message, idled, ChangeApplied, current message: note is Some, names
///    the commit subject, the short SHA, and uses the word "applied".
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
    // Shortened to 8 chars: the rest of the SHA must not leak.
    assert!(
        !note.note.contains("78e105d2e9ab"),
        "sha not shortened: {}",
        note.note
    );
    // The three facts the pre-merge applied-change note carried in its header.
    // Losing the "not pending" clause would undo the bug that note was written
    // to fix, which is the agent saying the work is still awaiting Apply.
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

/// 2. No covered event in the gap before the current turn produces no note.
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

/// 3. The note self-clears: it fires once on the turn after the apply, then NOT
///    on the following turn, even though no engine-synthesized prompt was
///    emitted in between (user CC turns are `MessageReceived`, not
///    `CodingAgentUserMessageSent`).
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

    // Turn 2: the apply is newer than turn 1's boundary, so the note fires.
    let turn2 = emit_message_received(&bus, thread_id, "turn 2").await;
    assert!(
        compute_turn_gap_note(&pool, thread_id, turn2, Some(BRANCH))
            .await
            .is_some(),
        "note must fire on the first turn after the apply"
    );

    // Turn 3, a plain turn with NO engine prompt in between: the apply now sits
    // below turn 2's boundary, so the note self-clears.
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

/// 4. Live-apply ordering regression guard: ChangeApplied then a TRAILING
///    CodingAgentIdled (the Tier-1 live-apply path emits an idle after the
///    apply). Keyed off turn boundaries (not the last idle), so the note still
///    fires.
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
    // Tier-1 live-apply emits a trailing idle AFTER ChangeApplied. Keying off
    // the last idle would make the note miss the apply; keying off the turn
    // boundary does not.
    emit_idled(&bus, thread_id).await;
    let current = emit_message_received(&bus, thread_id, "continue").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .expect("trailing idle must not suppress the note");
    assert!(note.note.contains("feat: landed work"), "{}", note.note);

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

/// 5. Multiple events in the gap are listed oldest-first and capped at
///    MAX_LINES. Also exercises `CodingAgentPromptSent` as a valid boundary and
///    mixed event kinds keeping event order.
#[tokio::test]
async fn multiple_events_listed_oldest_first_and_capped() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    // Engine-synthesized prompt (hardening) is also a valid turn boundary.
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

    // Capping: a single apply with > MAX_LINES commits truncates.
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
            change_id: Uuid::new_v4().to_string(),
            requires_restart: false,
            client_update: false,
            commits,
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: Some("deadbeef".into()),
            path: String::new(),
        },
    )
    .await;
    let current = emit_message_received(&bus, thread_id, "next").await;

    let note = compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
        .await
        .unwrap()
        .note;
    // 60 commits against `MAX_LINES_PER_EVENT` = 20, so 40 truncated. The
    // per-event cap binds before the overall `MAX_LINES` = 50, which is the
    // point: one big Apply must leave room for whatever follows it.
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

/// 6. No-op apply (empty commits): graceful wording, no panic, no empty bullet.
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
    // No empty "- " bullet line.
    assert!(!note.contains("\n- \n"), "empty bullet rendered: {note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

// -------------------- Boundary set --------------------

/// 7. A turn woken by a finished child thread is a real turn boundary, so the
///    note must self-clear across it. `ChildThreadCompleted` is a CC turn ORIGIN
///    (`CC_ORIGINATING_EVENT_TYPES`) but was missing from the hand-listed
///    boundary set, so the apply stayed above the threshold and the note fired
///    a second time on the next plain turn.
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

    // Turn 2 is woken by a child thread finishing, not by a user message.
    let turn2 = emit(
        &bus,
        thread_id,
        ThreadEvent::ChildThreadCompleted {
            child_thread_id: Uuid::new_v4(),
            child_thread_title: Some("child".into()),
            status: ChildCompletionStatus::Success,
            summary: "child did the thing".into(),
            pending_change_ids: vec![],
        },
    )
    .await;
    assert!(
        compute_turn_gap_note(&pool, thread_id, turn2, Some(BRANCH))
            .await
            .is_some(),
        "note must fire on the child-woken turn after the apply"
    );

    // Turn 3: the apply now sits below turn 2's boundary, so it self-clears.
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

// -------------------- Discarded --------------------

/// 8. The gap this module exists to close: the user clicked Discard.
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

/// 9. A discard must self-clear exactly like an apply.
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

/// 10. The reconcile path (`discard_pending_for_thread_except`) discards STALE
///     siblings on other branches. That must not read as "your work is gone".
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
    // A discard elsewhere reset THAT branch, not this worktree. Claiming
    // otherwise would silence the external-edit note about a HEAD move nothing
    // in this gap explains.
    assert!(
        !note.explains_worktree_reset,
        "a discard on another branch did not move this worktree"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

/// 10b. Two messages racing one spawn: `cc_spawn_coalesce` runs a single turn
///      whose origin is the FIRST message, while the second is already
///      persisted. Bounding the gap by the origin's id alone let `MAX(sequence)`
///      pick that later message as the "previous" boundary and swallow the whole
///      gap, so the user who typed twice after a Discard was told nothing.
#[tokio::test]
async fn later_boundary_after_the_origin_does_not_swallow_the_gap() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let change_id = session_with_proposed_change(&bus, thread_id, BRANCH, "add the feature").await;

    emit_change_discarded(&bus, thread_id, &change_id).await;

    // The turn the engine actually runs is anchored on the first message...
    let origin = emit_message_received(&bus, thread_id, "what happened?").await;
    // ...but a second message lands before the spawn builds its prompt.
    emit_message_received(&bus, thread_id, "also, do X next").await;

    let note = compute_turn_gap_note(&pool, thread_id, origin, Some(BRANCH))
        .await
        .expect("a message arriving after the origin must not hide the discard");
    assert!(note.note.contains("DISCARDED"), "{}", note.note);

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

/// 10c. The flip side of 10b: an event that lands AFTER the current origin
///      belongs to the next turn's gap, not this one, and must still be
///      surfaced there rather than lost between the two.
#[tokio::test]
async fn event_after_the_origin_is_carried_to_the_next_turn() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let change_id = session_with_proposed_change(&bus, thread_id, BRANCH, "add the feature").await;

    // The user sends a message, THEN clicks Discard while the spawn is starting.
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

// -------------------- Reverted --------------------

/// 11. A revert undoes work in `main`, which the applied note told the agent to
///     build on. The note must correct that, and must NOT claim the worktree
///     moved (revert runs in the main repo).
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

// -------------------- ApplyFailed --------------------

/// 12. A failed Apply leaves the change pending. The engine's error is
///     user-facing toast copy, so it must be quoted and attributed, never
///     rendered as an instruction to the agent.
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

// -------------------- WorktreeCleaned --------------------

/// 13. Tier 1 strips build artifacts from a worktree that can still resume. No
///     ref moved, so it must not silence the external-edit note.
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

/// 14. Tier 2 removes the worktree and can delete the branch, which recreates it
///     at `main`: the second way to produce the HEAD-moved misattribution.
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
        note.note.contains("untracked file you left there is gone"),
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

// -------------------- Degradation --------------------

/// 15. A legacy payload with no `change_id` (the pre-`change_id` shape carried
///     `path` only) has no `changes` row to resolve. The note must still render
///     the event, with no panic and no empty bullet.
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
    // With no branch known, the note must not invent one.
    assert!(!note.contains("branch ``"), "empty branch rendered: {note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

/// 16. With no session branch in hand (a spawn that could not resolve one), a
///     discard still renders and simply doesn't draw the comparison.
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

/// 17. A background completion nobody delivered reaches the resumed agent,
///     and the two endings read differently. Abandoned means the engine
///     stopped and there is no verdict; ordinary means the task finished and
///     only the wake went missing. Both leave the agent believing its build is
///     still running if the note stays silent.
///
///     Nothing filters by kind here, because the boundary set does it: a wake
///     the watcher DID deliver emits `CodingAgentPromptSent`, which is a
///     boundary, so that completion falls outside the next gap. Test 18 pins
///     that half.
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
    // The line tells the agent to drain, and every bash_output lookup is an
    // exact match, so a shortened id comes back as `unknown task_id`.
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

/// 19. **A `ContinuationRequested` must NOT advance the boundary.** It is
///     persisted when a resume is REQUESTED, and several ordinary paths never
///     actuate it: a racing user message takes the session first, a later
///     request supersedes it, two Continue clicks emit two. Counting it lost
///     the note permanently in each of those, on the crash-restart path this
///     coverage exists for.
///
///     The accepted residual is the opposite: a resume that DID deliver the
///     note records nothing, so it repeats once on the next message. That
///     costs a re-read; the other costs the agent believing its build runs.
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
    // Requested, and then never actuated: the user's own message won the
    // session, so no turn ever built a note from this.
    emit(
        &bus,
        thread_id,
        ThreadEvent::ContinuationRequested {
            reason: "auto_resume_after_switch".into(),
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

/// 18. The other half: a completion whose wake WAS delivered is not repeated.
///     The delivery emits `CodingAgentPromptSent`, which is a turn boundary, so
///     the completion falls before the next gap and needs no filter of its own.
#[tokio::test]
async fn a_background_completion_whose_wake_landed_is_not_repeated() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "run the suite").await;
    emit(
        &bus,
        thread_id,
        completion("task-woke", "cargo test --lib", Some(0), false),
    )
    .await;
    // What the watcher's wake becomes once `run_session` consumes it.
    emit_engine_prompt(&bus, thread_id, "Background task task-woke finished").await;
    let current = emit_message_received(&bus, thread_id, "how did it go?").await;

    assert!(
        compute_turn_gap_note(&pool, thread_id, current, Some(BRANCH))
            .await
            .is_none(),
        "the agent already heard about it, so the gap holds nothing to say"
    );

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

/// 20. The ordinary line names `timed_out` and `killed`, for the same reason
///     the wake it replaces leads with them. Without it a deadline, a
///     cancellation and a segfault all read as "killed by SIGKILL".
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

/// 21. **A big Apply must not bury the lost-task line.** The note truncates its
///     tail, an Apply renders a line per merged commit, and a completion
///     always sorts after the Apply that preceded it. So the one channel an
///     undelivered background completion has was the line that got dropped.
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
            change_id: Uuid::new_v4().to_string(),
            requires_restart: false,
            client_update: false,
            commits: (0..60).map(|i| format!("commit {i}")).collect(),
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: Some("deadbeef".into()),
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

/// 22. **The overall budget cannot bury it either.** A per-event cap alone
///     still lost the line once enough events shared the gap, and the boot
///     sweep makes that reachable: it settles every historical unsettled task
///     on a thread at once. Background lines are rendered first for that
///     reason, whatever their sequence.
#[tokio::test]
async fn a_gap_full_of_applies_does_not_bury_the_background_task_line() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, BRANCH, None).await;

    emit_message_received(&bus, thread_id, "land them all").await;
    // Three applies at the per-event cap already exceed MAX_LINES on their own.
    for batch in 0..3 {
        emit(
            &bus,
            thread_id,
            ThreadEvent::ChangeApplied {
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

/// A background completion, ordinary or abandoned.
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
