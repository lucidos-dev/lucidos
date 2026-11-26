use super::*;
use crate::engine::event_bus::EventBus;
use crate::engine::thread_events::{ActorMode, AnswerKind, EventChannel, EventMeta};
use crate::test_support::{setup_test_db, start_cc_session, teardown_test_db};
use sqlx::PgPool;

fn no_sessions() -> AgentSessions {
    tokio::sync::Mutex::new(std::collections::HashMap::new())
}

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent, channel: EventChannel) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta {
            channel: Some(channel),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();
}

async fn message(bus: &EventBus, thread_id: Uuid) {
    emit(
        bus,
        thread_id,
        ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "do the thing".into(),
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
        EventChannel::Chat,
    )
    .await;
}

async fn request(bus: &EventBus, thread_id: Uuid) {
    crate::engine::chat::agent_archive::record_archive_request(bus, thread_id)
        .await
        .unwrap();
}

async fn reply(bus: &EventBus, thread_id: Uuid, channel: EventChannel) {
    emit(
        bus,
        thread_id,
        ThreadEvent::ResponseGenerated {
            text: "done".into(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        channel,
    )
    .await;
}

async fn cc_idled(bus: &EventBus, thread_id: Uuid) {
    emit(
        bus,
        thread_id,
        ThreadEvent::CodingAgentIdled {
            has_changes: false,
            is_external_repo: false,
            requires_restart: false,
            cc_session_id: None,
            coding_agent: crate::runtime::CodingAgent::ClaudeCode,
            reason: None,
            worktree_path: None,
            worktree_head_sha: None,
            bg_bash_pending: false,
            turn_end: None,
        },
        EventChannel::ClaudeCode,
    )
    .await;
}

fn change_id(thread_id: Uuid) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, thread_id.as_bytes())
}

async fn propose(bus: &EventBus, thread_id: Uuid) {
    emit(
        bus,
        thread_id,
        ThreadEvent::ChangeProposed {
            content: Default::default(),
            change_id: change_id(thread_id).to_string(),
            description: Some("work".into()),
            files: vec!["a.rs".into()],
            requires_restart: false,
            origin: None,
            commit_sha: None,
            branch_name: format!("claude-code/test-{thread_id}"),
            repo_root: "/tmp".into(),
            hardened: true,
            incomplete: false,
            set_aside: false,
            path: String::new(),
            diff: String::new(),
        },
        EventChannel::ClaudeCode,
    )
    .await;
}

async fn apply(bus: &EventBus, thread_id: Uuid) {
    emit(
        bus,
        thread_id,
        ThreadEvent::ChangeApplied {
            content: Default::default(),
            change_id: change_id(thread_id).to_string(),
            requires_restart: false,
            client_update: false,
            commits: vec![],
            thread_title: None,
            actor: None,
            pre_merge_sha: None,
            post_merge_sha: None,
            landing_merge_sha: None,
            path: String::new(),
        },
        EventChannel::ClaudeCode,
    )
    .await;
}

async fn verdict(pool: &PgPool, thread_id: Uuid) -> RequestVerdict {
    let facts = read_request_facts(pool, &no_sessions(), thread_id)
        .await
        .expect("facts read")
        .expect("thread exists");
    request_verdict(&facts)
}

fn settled_facts() -> RequestFacts {
    RequestFacts {
        open: true,
        archived: false,
        status: ThreadStatus::Idle,
        parked_on_question: false,
        live_event_waits: false,
        turn_settle: TurnSettle::Settled,
        pending_change: false,
        unproposed_work: false,
        blocking_descendants: false,
        pinned: false,
    }
}

#[test]
fn the_verdict_archives_only_a_settled_thread_nothing_refuses() {
    assert_eq!(request_verdict(&settled_facts()), RequestVerdict::Archive);

    let closed: [fn(&mut RequestFacts); 2] = [|f| f.open = false, |f| f.archived = true];
    for close in closed {
        let mut facts = settled_facts();
        close(&mut facts);
        assert_eq!(request_verdict(&facts), RequestVerdict::Closed, "{facts:?}");
    }

    let holds: [fn(&mut RequestFacts); 12] = [
        |f| f.status = ThreadStatus::Running,
        |f| f.status = ThreadStatus::WaitingForUserAnswer,
        |f| f.status = ThreadStatus::Paused,
        |f| f.status = ThreadStatus::Failed,
        |f| f.parked_on_question = true,
        |f| f.live_event_waits = true,
        |f| f.turn_settle = TurnSettle::InFlight,
        |f| f.turn_settle = TurnSettle::CutOff,
        |f| f.pending_change = true,
        |f| f.unproposed_work = true,
        |f| f.blocking_descendants = true,
        |f| f.pinned = true,
    ];
    for hold in holds {
        let mut facts = settled_facts();
        hold(&mut facts);
        assert_eq!(request_verdict(&facts), RequestVerdict::Wait, "{facts:?}");
    }
}

#[tokio::test]
async fn a_request_made_mid_turn_lands_after_the_turn_ends() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();

    message(&bus, thread).await;
    request(&bus, thread).await;
    assert_eq!(verdict(&pool, thread).await, RequestVerdict::Wait);

    reply(&bus, thread, EventChannel::Chat).await;
    assert_eq!(verdict(&pool, thread).await, RequestVerdict::Archive);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_pin_holds_the_request_until_it_is_unpinned() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();

    message(&bus, thread).await;
    request(&bus, thread).await;
    reply(&bus, thread, EventChannel::Chat).await;
    emit(&bus, thread, ThreadEvent::ThreadSaved, EventChannel::Chat).await;
    assert_eq!(verdict(&pool, thread).await, RequestVerdict::Wait);

    emit(&bus, thread, ThreadEvent::ThreadUnsaved, EventChannel::Chat).await;
    assert_eq!(verdict(&pool, thread).await, RequestVerdict::Archive);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_new_message_closes_the_request() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();

    message(&bus, thread).await;
    request(&bus, thread).await;
    reply(&bus, thread, EventChannel::Chat).await;
    message(&bus, thread).await;
    reply(&bus, thread, EventChannel::Chat).await;

    assert_eq!(verdict(&pool, thread).await, RequestVerdict::Closed);
    assert!(threads_with_open_requests(&pool).await.unwrap().is_empty());

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn restore_expires_archived_requests_but_accepts_fresh_requests() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();
    message(&bus, thread).await;
    request(&bus, thread).await;
    reply(&bus, thread, EventChannel::Chat).await;
    assert!(request_is_open(&pool, thread).await.unwrap());
    sqlx::query("INSERT INTO snapshot_restore_boundary (restore_id, snapshot, restored_at, event_sequence) SELECT $1, $2, now(), MAX(sequence) FROM events")
        .bind(Uuid::new_v4()).bind(crate::core::snapshot::handle::Handle::generate().as_str())
        .execute(&pool).await.unwrap();
    assert!(!request_is_open(&pool, thread).await.unwrap());
    assert!(threads_with_open_requests(&pool).await.unwrap().is_empty());
    assert_eq!(verdict(&pool, thread).await, RequestVerdict::Closed);
    reply(&bus, thread, EventChannel::Chat).await;
    assert!(!request_is_open(&pool, thread).await.unwrap());
    request(&bus, thread).await;
    assert!(request_is_open(&pool, thread).await.unwrap());
    assert_eq!(
        threads_with_open_requests(&pool).await.unwrap(),
        HashSet::from([thread])
    );
    assert_eq!(verdict(&pool, thread).await, RequestVerdict::Archive);
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_archive_closes_the_request() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();

    message(&bus, thread).await;
    request(&bus, thread).await;
    reply(&bus, thread, EventChannel::Chat).await;
    assert_eq!(
        threads_with_open_requests(&pool).await.unwrap(),
        HashSet::from([thread])
    );
    emit(
        &bus,
        thread,
        ThreadEvent::ThreadArchived,
        EventChannel::Chat,
    )
    .await;

    assert_eq!(verdict(&pool, thread).await, RequestVerdict::Closed);
    assert!(threads_with_open_requests(&pool).await.unwrap().is_empty());

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_question_holds_the_request_until_the_turn_after_it_ends() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();

    message(&bus, thread).await;
    request(&bus, thread).await;
    emit(
        &bus,
        thread,
        ThreadEvent::UserQuestionAsked {
            tool_use_id: "tu-1".into(),
            cc_session_id: String::new(),
            question: "Which one?".into(),
            options: vec![],
            worktree_path: None,
            multi_select: false,
            owner_approval: None,
            permission_request: None,
        },
        EventChannel::Chat,
    )
    .await;
    assert_eq!(verdict(&pool, thread).await, RequestVerdict::Wait);

    emit(
        &bus,
        thread,
        ThreadEvent::UserQuestionAnswered {
            compose: None,
            tool_use_id: "tu-1".into(),
            quiet: false,
            answer: AnswerKind::FreeText {
                text: "the first".into(),
                image_hashes: vec![],
            },
        },
        EventChannel::Chat,
    )
    .await;
    reply(&bus, thread, EventChannel::Chat).await;
    assert_eq!(verdict(&pool, thread).await, RequestVerdict::Archive);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_pending_change_holds_the_request_until_it_is_applied() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();

    start_cc_session(&bus, thread, "claude-code/test", None).await;
    request(&bus, thread).await;
    reply(&bus, thread, EventChannel::ClaudeCode).await;
    propose(&bus, thread).await;
    cc_idled(&bus, thread).await;
    assert_eq!(verdict(&pool, thread).await, RequestVerdict::Wait);

    apply(&bus, thread).await;
    assert_eq!(verdict(&pool, thread).await, RequestVerdict::Archive);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_turn_between_its_terminal_and_its_idle_holds_the_request() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();

    start_cc_session(&bus, thread, "claude-code/test", None).await;
    request(&bus, thread).await;
    reply(&bus, thread, EventChannel::ClaudeCode).await;
    assert_eq!(verdict(&pool, thread).await, RequestVerdict::Wait);

    cc_idled(&bus, thread).await;
    assert_eq!(verdict(&pool, thread).await, RequestVerdict::Archive);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_thread_with_no_request_is_closed() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();

    message(&bus, thread).await;
    reply(&bus, thread, EventChannel::Chat).await;
    assert_eq!(verdict(&pool, thread).await, RequestVerdict::Closed);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_external_repo_change_does_not_hold_the_request() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();

    start_cc_session(&bus, thread, "claude-code/test", None).await;
    request(&bus, thread).await;
    reply(&bus, thread, EventChannel::ClaudeCode).await;
    propose(&bus, thread).await;
    cc_idled(&bus, thread).await;
    assert_eq!(verdict(&pool, thread).await, RequestVerdict::Wait);

    sqlx::query(
        "UPDATE thread_summaries SET coding_agent_is_external_repo = TRUE WHERE thread_id = $1",
    )
    .bind(thread)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(verdict(&pool, thread).await, RequestVerdict::Archive);

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_recorded_request_names_the_agent_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let thread = Uuid::new_v4();

    message(&bus, thread).await;
    request(&bus, thread).await;

    let (mode, source): (String, String) = sqlx::query_as(
        "SELECT payload->'actor'->>'mode', payload->'actor'->>'source_thread_id' \
           FROM events WHERE thread_id = $1 AND event_type = 'ThreadArchiveRequested'",
    )
    .bind(thread)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(mode, "agent");
    assert_eq!(source, thread.to_string());

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_child_that_stops_blocking_wakes_its_parent() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let parent = Uuid::new_v4();
    message(&bus, parent).await;
    reply(&bus, parent, EventChannel::Chat).await;

    let child = Uuid::new_v4();
    emit(
        &bus,
        child,
        ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "a sub-task".into(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            parent_thread_id: Some(parent),
            spawning_event_id: None,
            mode: ActorMode::Agent,
            model: None,
            reasoning_effort: None,
            origin: None,
        },
        EventChannel::Chat,
    )
    .await;

    let mut rx = bus.subscribe();
    reply(&bus, child, EventChannel::Chat).await;

    let mut woke = false;
    while let Ok(emitted) = rx.try_recv() {
        if let BusEvent::Thread {
            thread_id,
            event: ThreadEvent::ChildrenCountChanged { .. },
            ..
        } = &emitted.typed
        {
            woke |= *thread_id == parent && thread_to_resolve(&emitted.typed) == Some(parent);
        }
    }
    assert!(
        woke,
        "the child's settle must reach the parent as a thread event"
    );

    teardown_test_db(&db).await;
}

#[test]
fn a_request_held_by_a_change_claim_lands_after_the_claim_is_released() {
    crate::test_support::run_on_engine_runtime(
        "engine::archive_request::tests::a_request_held_by_a_change_claim_lands_after_the_claim_is_released",
        claim_release_case,
    );
}

async fn archive_state(pool: &PgPool, thread_id: Uuid) -> String {
    sqlx::query_scalar("SELECT archive_state FROM thread_summaries WHERE thread_id = $1")
        .bind(thread_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn claim_release_case() {
    use crate::engine::types::{AgentSession, ChangeClaim};

    let (pool, db) = setup_test_db().await;
    let (engine, _workspace) = crate::test_support::boot_mock_engine(&db).await;
    let thread = Uuid::new_v4();
    message(&engine.event_bus, thread).await;
    reply(&engine.event_bus, thread, EventChannel::Chat).await;
    let (mut session, _rx) = AgentSession::for_test();
    session.change_claim = Some(ChangeClaim::Apply);
    engine.agent_sessions.lock().await.insert(thread, session);

    start_archive_request_resolver(&engine);
    request(&engine.event_bus, thread).await;

    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        archive_state(&pool, thread).await,
        "inbox",
        "a held claim must hold the request"
    );

    engine.agent_sessions.lock().await.remove(&thread);
    let waited = tokio::time::Instant::now();
    while archive_state(&pool, thread).await != "archived" {
        assert!(
            waited.elapsed() < CLAIM_RETRY * 4,
            "the request never landed after the claim was released: no event wakes it, so only the retry can"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    teardown_test_db(&db).await;
}
