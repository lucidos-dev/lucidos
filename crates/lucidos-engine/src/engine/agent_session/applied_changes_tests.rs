use super::compute_applied_change_note;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{ActorMode, EventChannel, EventMeta, ThreadEvent};
use crate::test_support::start_cc_session;
use uuid::Uuid;

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

/// 1. prev message → idled → ChangeApplied → current message: note is Some,
///    names the commit subject, the short SHA, and uses the word "applied".
#[tokio::test]
async fn applied_change_after_last_prompt_produces_note() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/applied", None).await;

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

    let note = compute_applied_change_note(&pool, thread_id, current).await;
    let note = note.expect("expected an applied-change note");
    assert!(note.to_lowercase().contains("applied"), "note: {note}");
    assert!(
        note.contains("feat(notifications): instrument SW push/notificationclick"),
        "note: {note}"
    );
    assert!(note.contains("78e105d2"), "short sha missing in: {note}");
    // Shortened to 8 chars — the rest of the SHA must not leak.
    assert!(!note.contains("78e105d2e9ab"), "sha not shortened: {note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

/// 2. No ChangeApplied in the gap before the current turn → None.
#[tokio::test]
async fn no_applied_change_after_last_prompt_is_none() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/applied", None).await;

    emit_message_received(&bus, thread_id, "do the thing").await;
    emit_idled(&bus, thread_id).await;
    let current = emit_message_received(&bus, thread_id, "now do more").await;

    assert!(compute_applied_change_note(&pool, thread_id, current)
        .await
        .is_none());

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

/// 3. The note self-clears: it fires once on the turn after the apply, then
///    NOT on the following turn — even though no engine-synthesized prompt was
///    emitted in between (the regression the plan's SQL would have hit, since
///    user CC turns are `MessageReceived`, not `CodingAgentUserMessageSent`).
#[tokio::test]
async fn note_self_clears_on_next_plain_turn() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/applied", None).await;

    emit_message_received(&bus, thread_id, "turn 1").await;
    emit_idled(&bus, thread_id).await;
    emit_change_applied(&bus, thread_id, vec!["feat: landed work"], Some("aaaaaaaa11")).await;

    // Turn 2: the apply is newer than turn 1's boundary → note fires.
    let turn2 = emit_message_received(&bus, thread_id, "turn 2").await;
    assert!(
        compute_applied_change_note(&pool, thread_id, turn2)
            .await
            .is_some(),
        "note must fire on the first turn after the apply"
    );

    // Turn 3, a plain turn with NO engine prompt in between: the apply now sits
    // below turn 2's boundary → note self-clears.
    let turn3 = emit_message_received(&bus, thread_id, "turn 3").await;
    assert!(
        compute_applied_change_note(&pool, thread_id, turn3)
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
    start_cc_session(&bus, thread_id, "claude-code/applied", None).await;

    emit_message_received(&bus, thread_id, "do the thing").await;
    emit_idled(&bus, thread_id).await;
    emit_change_applied(&bus, thread_id, vec!["feat: landed work"], Some("bbbbbbbb22")).await;
    // Tier-1 live-apply emits a trailing idle AFTER ChangeApplied. Keying off
    // the last idle would make the note miss the apply — keying off the turn
    // boundary does not.
    emit_idled(&bus, thread_id).await;
    let current = emit_message_received(&bus, thread_id, "continue").await;

    let note = compute_applied_change_note(&pool, thread_id, current).await;
    let note = note.expect("trailing idle must not suppress the note");
    assert!(note.contains("feat: landed work"), "note: {note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

/// 5. Multiple ChangeApplied in the gap → all listed oldest-first, capped at
///    MAX_LINES. Also exercises `CodingAgentPromptSent` as a valid boundary.
#[tokio::test]
async fn multiple_applies_listed_oldest_first_and_capped() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/applied", None).await;

    // Engine-synthesized prompt (hardening) is also a valid turn boundary.
    emit_engine_prompt(&bus, thread_id, "/harden").await;
    emit_change_applied(&bus, thread_id, vec!["feat: alpha"], Some("11111111")).await;
    emit_change_applied(&bus, thread_id, vec!["feat: beta"], Some("22222222")).await;
    let current = emit_message_received(&bus, thread_id, "next").await;

    let note = compute_applied_change_note(&pool, thread_id, current)
        .await
        .unwrap();
    let alpha = note.find("feat: alpha").expect("alpha present");
    let beta = note.find("feat: beta").expect("beta present");
    assert!(alpha < beta, "oldest-first ordering violated: {note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;

    // Capping: a single apply with > MAX_LINES commits truncates.
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/applied", None).await;
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

    let note = compute_applied_change_note(&pool, thread_id, current)
        .await
        .unwrap();
    // 60 commits, MAX_LINES = 50 → 10 truncated.
    assert!(note.contains("and 10 more"), "expected truncation note: {note}");
    assert!(note.contains("commit 0"), "first commit present: {note}");
    assert!(!note.contains("commit 59"), "tail commit must be truncated: {note}");

    pool.close().await;
    crate::test_support::teardown_test_db(&db_name).await;
}

/// 6. No-op apply (empty commits) → graceful wording, no panic, no empty
///    bullet.
#[tokio::test]
async fn noop_apply_renders_gracefully() {
    let (pool, db_name) = crate::test_support::setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    start_cc_session(&bus, thread_id, "claude-code/applied", None).await;

    emit_message_received(&bus, thread_id, "do the thing").await;
    emit_change_applied(&bus, thread_id, vec![], None).await;
    let current = emit_message_received(&bus, thread_id, "next").await;

    let note = compute_applied_change_note(&pool, thread_id, current)
        .await
        .unwrap();
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
