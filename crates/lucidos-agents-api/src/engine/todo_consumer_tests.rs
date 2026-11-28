use super::*;
use crate::core::event_subscription::EventSubscription;
use crate::engine::thread_events::{
    ChildCompletionStatus, EventMeta, EventWaitCancelCause, TodoStatus, WakeMode,
};
use crate::engine::tools::todo::todo_tool_impl;
use crate::test_support::{setup_test_db, teardown_test_db};
use serde_json::json;
use tokio::sync::broadcast::Receiver;
use uuid::Uuid;

async fn setup() -> (EventBus, Receiver<EmittedEvent>, sqlx::PgPool, String) {
    let (pool, db) = setup_test_db().await;
    let (bus, _parent_rx) = EventBus::new(pool.clone());
    let rx = bus.subscribe();
    (bus, rx, pool, db)
}

fn drain(rx: &mut Receiver<EmittedEvent>) {
    while rx.try_recv().is_ok() {}
}

async fn dispatch_next_terminator(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    rx: &mut Receiver<EmittedEvent>,
    thread_id: Uuid,
) {
    dispatch_next_terminator_with_background(bus, pool, rx, thread_id, false).await
}

fn is_terminator(event: &ThreadEvent) -> bool {
    matches!(
        event,
        ThreadEvent::ResponseGenerated { .. }
            | ThreadEvent::ResponseCanceled { .. }
            | ThreadEvent::ResponseAborted { .. }
            | ThreadEvent::ResponseFailed { .. }
    )
}

async fn dispatch_next_terminator_with_background(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    rx: &mut Receiver<EmittedEvent>,
    thread_id: Uuid,
    holds_background_work: bool,
) {
    let ev = next_matching(rx, thread_id, is_terminator).await;
    handle_event(bus, pool, &ev, holds_background_work).await;
}

async fn next_matching(
    rx: &mut Receiver<EmittedEvent>,
    thread_id: Uuid,
    pick: impl Fn(&ThreadEvent) -> bool,
) -> EmittedEvent {
    loop {
        let ev = rx.recv().await.expect("broadcast channel should not close");
        if let BusEvent::Thread {
            thread_id: tid,
            event,
            ..
        } = &ev.typed
        {
            if *tid == thread_id && pick(event) {
                return ev;
            }
        }
    }
}

async fn next_todo_items(
    rx: &mut Receiver<EmittedEvent>,
    thread_id: Uuid,
) -> Vec<super::super::thread_events::TodoItem> {
    let found = tokio::time::timeout(tokio::time::Duration::from_secs(5), async {
        loop {
            let ev = rx.recv().await.expect("broadcast channel should not close");
            if let BusEvent::Thread {
                thread_id: tid,
                event: ThreadEvent::TodoListWritten { items, .. },
                ..
            } = ev.typed
            {
                if tid == thread_id {
                    return items;
                }
            }
        }
    })
    .await;
    found.expect("expected a TodoListWritten settle for the thread, none arrived")
}

async fn seed_in_progress_list(bus: &EventBus, pool: &sqlx::PgPool, thread_id: Uuid) {
    let args = json!({
        "todos": [
            { "content": "a", "active_form": "doing a", "status": "in_progress" },
        ]
    });
    todo_tool_impl(bus, pool, &args, thread_id)
        .await
        .expect("seed write should succeed");
}

async fn emit_response_generated(bus: &EventBus, thread_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "done".to_string(),
            images: vec![],
            model: None,
            reasoning_effort: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("emit response");
}

async fn emit_response_canceled(bus: &EventBus, pool: &sqlx::PgPool, thread_id: Uuid) {
    use crate::engine::thread_events::{emit_response_canceled, CancelCause};
    emit_response_canceled(
        bus,
        pool,
        thread_id,
        CancelCause::UserStop,
        String::new(),
        vec![],
        None,
        None,
        EventMeta::NONE,
        "[TodoConsumerTest] ResponseCanceled",
    )
    .await;
}

fn watching() -> Vec<EventSubscription> {
    vec![EventSubscription {
        event_type: "ChangeProposed".into(),
        condition: None,
    }]
}

async fn emit_wait_started(bus: &EventBus, thread_id: Uuid, wait_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitStarted {
            background_timing: None,
            wait_id,
            tool_use_id: format!("toolu_{wait_id}"),
            on: watching(),
            reason: "watching for the change to land".into(),
            armed_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
            watermark: 0,
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("emit EventWaitStarted");
}

async fn emit_wait_canceled(bus: &EventBus, thread_id: Uuid, wait_id: Uuid) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitCanceled {
            wait_id,
            cause: EventWaitCancelCause::UserStop,
            on: watching(),
            reason: "watching for the change to land".into(),
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("emit EventWaitCanceled");
}

async fn dispatch_next_cancel(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    rx: &mut Receiver<EmittedEvent>,
    thread_id: Uuid,
) {
    let ev = next_matching(rx, thread_id, |e| {
        matches!(e, ThreadEvent::EventWaitCanceled { .. })
    })
    .await;
    handle_event(bus, pool, &ev, true).await;
}

async fn assert_nothing_settles(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    rx: &mut Receiver<EmittedEvent>,
    thread_id: Uuid,
) {
    use tokio::time::{timeout, Duration};
    let deadline = tokio::time::Instant::now() + Duration::from_millis(150);
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return;
        }
        match timeout(remaining, rx.recv()).await {
            Ok(Ok(ev)) => {
                handle_event(bus, pool, &ev, false).await;
                if let BusEvent::Thread {
                    thread_id: tid,
                    event: ThreadEvent::TodoListWritten { items, .. },
                    ..
                } = &ev.typed
                {
                    if *tid == thread_id {
                        panic!("unexpected settle, got {:?}", items);
                    }
                }
            }
            _ => return,
        }
    }
}

async fn park_on_a_wait(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    rx: &mut Receiver<EmittedEvent>,
    thread_id: Uuid,
    wait_ids: &[Uuid],
) {
    seed_in_progress_list(bus, pool, thread_id).await;
    for wait_id in wait_ids {
        emit_wait_started(bus, thread_id, *wait_id).await;
    }
    drain(rx);

    emit_response_generated(bus, thread_id).await;
    dispatch_next_terminator(bus, pool, rx, thread_id).await;
    let parked = next_todo_items(rx, thread_id).await;
    assert_eq!(
        parked[0].status,
        TodoStatus::Waiting,
        "precondition: the terminator parks the list, got {:?}",
        parked[0].status,
    );
}

async fn seed_thread_status(pool: &sqlx::PgPool, thread_id: Uuid, status: &str) {
    sqlx::query(
        "INSERT INTO thread_summaries (thread_id, status) VALUES ($1, $2) \
         ON CONFLICT (thread_id) DO UPDATE SET status = EXCLUDED.status",
    )
    .bind(thread_id)
    .bind(status)
    .execute(pool)
    .await
    .expect("seed thread_summaries row");
}

async fn emit_response_aborted(bus: &EventBus, thread_id: Uuid) {
    use crate::engine::thread_events::{emit_response_aborted, AbortCause};
    emit_response_aborted(
        bus,
        thread_id,
        AbortCause::EngineShutdown,
        String::new(),
        vec![],
        None,
        None,
        EventMeta::NONE,
        "[TodoConsumerTest] ResponseAborted",
    )
    .await;
}

#[tokio::test]
async fn handle_event_marks_abandoned_on_response_generated() {
    let (bus, mut rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();

    seed_in_progress_list(&bus, &pool, thread_id).await;
    drain(&mut rx);

    emit_response_generated(&bus, thread_id).await;
    dispatch_next_terminator(&bus, &pool, &mut rx, thread_id).await;

    let flipped = next_todo_items(&mut rx, thread_id).await;
    assert_eq!(flipped.len(), 1, "list kept, got {:?}", flipped);
    assert_eq!(
        flipped[0].status,
        super::super::thread_events::TodoStatus::Abandoned,
        "in_progress flipped to abandoned, got {:?}",
        flipped[0].status,
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn handle_event_marks_abandoned_on_response_canceled() {
    let (bus, mut rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();

    seed_in_progress_list(&bus, &pool, thread_id).await;
    drain(&mut rx);

    emit_response_canceled(&bus, &pool, thread_id).await;
    dispatch_next_terminator(&bus, &pool, &mut rx, thread_id).await;

    let flipped = next_todo_items(&mut rx, thread_id).await;
    assert_eq!(flipped.len(), 1, "list kept, got {:?}", flipped);
    assert_eq!(
        flipped[0].status,
        super::super::thread_events::TodoStatus::Abandoned,
        "in_progress flipped to abandoned, got {:?}",
        flipped[0].status,
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn handle_event_marks_abandoned_on_response_aborted() {
    let (bus, mut rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();

    seed_in_progress_list(&bus, &pool, thread_id).await;
    drain(&mut rx);

    emit_response_aborted(&bus, thread_id).await;
    dispatch_next_terminator(&bus, &pool, &mut rx, thread_id).await;

    let flipped = next_todo_items(&mut rx, thread_id).await;
    assert_eq!(flipped.len(), 1, "list kept, got {:?}", flipped);
    assert_eq!(
        flipped[0].status,
        super::super::thread_events::TodoStatus::Abandoned,
        "in_progress flipped to abandoned, got {:?}",
        flipped[0].status,
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn handle_event_marks_abandoned_on_response_failed() {
    let (bus, mut rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();

    seed_in_progress_list(&bus, &pool, thread_id).await;
    drain(&mut rx);

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseFailed {
            error: "upstream 500".to_string(),
            spend_cap: None,
            agent_exit: None,
            provider_error: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("emit failed");
    dispatch_next_terminator(&bus, &pool, &mut rx, thread_id).await;

    let flipped = next_todo_items(&mut rx, thread_id).await;
    assert_eq!(flipped.len(), 1, "list kept, got {:?}", flipped);
    assert_eq!(
        flipped[0].status,
        super::super::thread_events::TodoStatus::Abandoned,
        "in_progress flipped to abandoned, got {:?}",
        flipped[0].status,
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn handle_event_settles_to_waiting_when_the_thread_holds_background_work() {
    let (bus, mut rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();

    seed_in_progress_list(&bus, &pool, thread_id).await;
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::ResponseGenerated {
            text: "Phase A is building, I will report when it lands".into(),
            model: None,
            reasoning_effort: None,
            images: vec![],
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("emit failed");
    dispatch_next_terminator_with_background(&bus, &pool, &mut rx, thread_id, true).await;

    let flipped = next_todo_items(&mut rx, thread_id).await;
    assert_eq!(flipped.len(), 1, "list kept, got {:?}", flipped);
    assert_eq!(
        flipped[0].status,
        super::super::thread_events::TodoStatus::Waiting,
        "a thread owning a running background task is parked, not abandoned; got {:?}",
        flipped[0].status,
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn handle_event_settles_to_waiting_when_the_thread_holds_a_live_event_wait() {
    let (bus, mut rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();

    seed_in_progress_list(&bus, &pool, thread_id).await;
    emit_wait_started(&bus, thread_id, Uuid::new_v4()).await;
    drain(&mut rx);

    emit_response_generated(&bus, thread_id).await;
    dispatch_next_terminator(&bus, &pool, &mut rx, thread_id).await;

    let settled = next_todo_items(&mut rx, thread_id).await;
    assert_eq!(settled.len(), 1, "list kept, got {:?}", settled);
    assert_eq!(
        settled[0].status,
        super::super::thread_events::TodoStatus::Waiting,
        "in_progress parked rather than abandoned, got {:?}",
        settled[0].status,
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn handle_event_ignores_unrelated_events() {
    let (bus, mut rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();

    seed_in_progress_list(&bus, &pool, thread_id).await;
    drain(&mut rx);

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode: None,
            text: "hi".to_string(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            mode: crate::engine::thread_events::ActorMode::Human,
            model: None,
            reasoning_effort: None,
            parent_thread_id: None,
            spawning_event_id: None,
            origin: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("emit message");

    assert_nothing_settles(&bus, &pool, &mut rx, thread_id).await;

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_canceled_last_subscription_settles_the_list_to_abandoned() {
    let (bus, mut rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();
    let wait_id = Uuid::new_v4();

    park_on_a_wait(&bus, &pool, &mut rx, thread_id, &[wait_id]).await;

    emit_wait_canceled(&bus, thread_id, wait_id).await;
    dispatch_next_cancel(&bus, &pool, &mut rx, thread_id).await;

    let settled = next_todo_items(&mut rx, thread_id).await;
    assert_eq!(settled.len(), 1, "list kept, got {:?}", settled);
    assert_eq!(
        settled[0].status,
        TodoStatus::Abandoned,
        "nothing will re-open the thread now, so the item is abandoned; got {:?}",
        settled[0].status,
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_cascade_of_cancels_settles_once_when_the_last_wait_goes() {
    let (bus, mut rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();
    let first = Uuid::new_v4();
    let last = Uuid::new_v4();

    park_on_a_wait(&bus, &pool, &mut rx, thread_id, &[first, last]).await;

    emit_wait_canceled(&bus, thread_id, first).await;
    dispatch_next_cancel(&bus, &pool, &mut rx, thread_id).await;
    assert_nothing_settles(&bus, &pool, &mut rx, thread_id).await;

    emit_wait_canceled(&bus, thread_id, last).await;
    dispatch_next_cancel(&bus, &pool, &mut rx, thread_id).await;

    let settled = next_todo_items(&mut rx, thread_id).await;
    assert_eq!(
        settled[0].status,
        TodoStatus::Abandoned,
        "the last cancel settles, got {:?}",
        settled[0].status,
    );
    assert_nothing_settles(&bus, &pool, &mut rx, thread_id).await;

    pool.close().await;
    teardown_test_db(&db).await;
}

async fn assert_a_turn_owning_the_list_blocks_the_settle(status: &str) {
    let (bus, mut rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();
    let wait_id = Uuid::new_v4();

    park_on_a_wait(&bus, &pool, &mut rx, thread_id, &[wait_id]).await;
    seed_thread_status(&pool, thread_id, status).await;

    emit_wait_canceled(&bus, thread_id, wait_id).await;
    dispatch_next_cancel(&bus, &pool, &mut rx, thread_id).await;
    assert_nothing_settles(&bus, &pool, &mut rx, thread_id).await;

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_cancel_settles_nothing_while_the_thread_is_running() {
    assert_a_turn_owning_the_list_blocks_the_settle("running").await;
}

#[tokio::test]
async fn a_cancel_settles_nothing_while_the_thread_awaits_an_answer() {
    assert_a_turn_owning_the_list_blocks_the_settle("waiting_for_user_answer").await;
}

#[tokio::test]
async fn a_cancel_settles_nothing_while_the_thread_is_paused() {
    assert_a_turn_owning_the_list_blocks_the_settle("paused").await;
}

#[tokio::test]
async fn a_delivered_or_expired_wait_is_left_to_the_turn_it_wakes() {
    let (bus, mut rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();
    let delivered = Uuid::new_v4();
    let expired = Uuid::new_v4();

    park_on_a_wait(&bus, &pool, &mut rx, thread_id, &[delivered, expired]).await;

    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitDelivered {
            wait_id: delivered,
            event_id: Uuid::new_v4(),
            event_type: "ChangeProposed".into(),
            payload: json!({}),
            matched_index: 0,
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("emit EventWaitDelivered");
    bus.emit(BusEvent::Thread {
        thread_id,
        event: ThreadEvent::EventWaitExpired { wait_id: expired },
        meta: EventMeta::NONE,
    })
    .await
    .expect("emit EventWaitExpired");

    assert_nothing_settles(&bus, &pool, &mut rx, thread_id).await;

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_failed_turn_state_probe_settles_nothing() {
    let (bus, mut rx, pool, db) = setup().await;
    let thread_id = Uuid::new_v4();
    let wait_id = Uuid::new_v4();

    park_on_a_wait(&bus, &pool, &mut rx, thread_id, &[wait_id]).await;
    emit_wait_canceled(&bus, thread_id, wait_id).await;

    let cancel = next_matching(&mut rx, thread_id, |e| {
        matches!(e, ThreadEvent::EventWaitCanceled { .. })
    })
    .await;
    pool.close().await;

    handle_event(&bus, &pool, &cancel, false).await;
    assert_nothing_settles(&bus, &pool, &mut rx, thread_id).await;

    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_terminator_lost_to_a_lag_still_settles_the_list() {
    use crate::engine::event_bus::{LagReplay, ReplayScope};
    use crate::test_support::flood_past_bus_capacity;

    let (bus, _setup_rx, pool, db) = setup().await;
    let (mut rx, mut replay) =
        LagReplay::subscribe(&bus, ReplayScope::Workspace, Some(settle_event_types()));
    let thread_id = Uuid::new_v4();

    seed_in_progress_list(&bus, &pool, thread_id).await;
    emit_response_generated(&bus, thread_id).await;
    flood_past_bus_capacity(&bus);
    let mut watch = bus.subscribe();

    let open = settle_after(rx.recv().await, &mut replay, &bus, &pool, |_| async {
        false
    })
    .await;
    assert!(open, "a lag must not end the consumer");
    let settled = next_todo_items(&mut watch, thread_id).await;
    assert_eq!(
        settled[0].status,
        TodoStatus::Abandoned,
        "the dropped terminator settles the list on replay, got {:?}",
        settled[0].status,
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[test]
fn the_settle_list_names_the_canceled_subscription() {
    let cancel = ThreadEvent::EventWaitCanceled {
        wait_id: Uuid::new_v4(),
        cause: EventWaitCancelCause::UserStop,
        on: watching(),
        reason: String::new(),
    };
    assert!(settle_event_types().contains(&cancel.event_type()));
}

async fn spawn_child(bus: &EventBus, parent: Uuid, child: Uuid, wake_mode: Option<WakeMode>) {
    bus.emit(BusEvent::Thread {
        thread_id: child,
        event: ThreadEvent::MessageReceived {
            provider: None,
            voice_session_id: None,
            wake_mode,
            text: "child task".to_string(),
            user_image_hashes: vec![],
            device_id: None,
            image_description: None,
            mode: crate::engine::thread_events::ActorMode::Human,
            model: None,
            reasoning_effort: None,
            parent_thread_id: Some(parent),
            spawning_event_id: None,
            origin: None,
        },
        meta: EventMeta::NONE,
    })
    .await
    .expect("emit the child's MessageReceived");
}

async fn finish_child(pool: &sqlx::PgPool, child: Uuid) {
    seed_thread_status(pool, child, "idle").await;
}

fn child_completed(child_thread_id: Uuid) -> ThreadEvent {
    ThreadEvent::ChildThreadCompleted {
        child_thread_id,
        child_thread_title: None,
        status: ChildCompletionStatus::Success,
        summary: "done".to_string(),
        pending_change_ids: vec![],
        sub_thread_pending_changes: vec![],
        integrated_branch: None,
    }
}

fn child_report_filed(child_thread_id: Uuid) -> ThreadEvent {
    ThreadEvent::ChildReportFiled {
        child_thread_id,
        child_thread_title: None,
        status: ChildCompletionStatus::Success,
        summary: "done".to_string(),
        pending_change_ids: vec![],
        sub_thread_pending_changes: vec![],
        integrated_branch: None,
    }
}

fn child_stopped(child_thread_id: Uuid) -> ThreadEvent {
    ThreadEvent::ChildThreadStopped {
        child_thread_id,
        child_thread_title: None,
    }
}

fn child_detached(child_thread_id: Uuid) -> ThreadEvent {
    ThreadEvent::ChildThreadDetached {
        child_thread_id,
        child_thread_title: None,
    }
}

async fn emit_on(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    bus.emit(BusEvent::Thread {
        thread_id,
        event,
        meta: EventMeta::NONE,
    })
    .await
    .expect("emit on the parent");
}

fn is_child_unpark(event: &ThreadEvent) -> bool {
    matches!(
        event,
        ThreadEvent::ChildReportFiled { .. }
            | ThreadEvent::ChildThreadStopped { .. }
            | ThreadEvent::ChildThreadDetached { .. }
    )
}

async fn dispatch_next_child_unpark(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    rx: &mut Receiver<EmittedEvent>,
    parent: Uuid,
) {
    let ev = next_matching(rx, parent, is_child_unpark).await;
    handle_event(bus, pool, &ev, false).await;
}

async fn settle_at_terminator(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    rx: &mut Receiver<EmittedEvent>,
    parent: Uuid,
) -> TodoStatus {
    emit_response_generated(bus, parent).await;
    dispatch_next_terminator(bus, pool, rx, parent).await;
    next_todo_items(rx, parent).await[0].status
}

async fn park_on_children(
    bus: &EventBus,
    pool: &sqlx::PgPool,
    rx: &mut Receiver<EmittedEvent>,
    parent: Uuid,
    children: &[Uuid],
) {
    seed_in_progress_list(bus, pool, parent).await;
    for child in children {
        spawn_child(bus, parent, *child, None).await;
    }
    drain(rx);
    let status = settle_at_terminator(bus, pool, rx, parent).await;
    assert_eq!(
        status,
        TodoStatus::Waiting,
        "precondition: a live child parks the list"
    );
}

#[tokio::test]
async fn a_live_child_at_the_terminator_parks_the_list() {
    let (bus, mut rx, pool, db) = setup().await;
    park_on_children(&bus, &pool, &mut rx, Uuid::new_v4(), &[Uuid::new_v4()]).await;
    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_live_no_wake_child_parks_the_list_too() {
    let (bus, mut rx, pool, db) = setup().await;
    let parent = Uuid::new_v4();

    seed_in_progress_list(&bus, &pool, parent).await;
    spawn_child(&bus, parent, Uuid::new_v4(), Some(WakeMode::NoWake)).await;
    drain(&mut rx);

    let status = settle_at_terminator(&bus, &pool, &mut rx, parent).await;
    assert_eq!(status, TodoStatus::Waiting);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_queued_spawn_with_no_row_parks_the_list() {
    let (bus, mut rx, pool, db) = setup().await;
    let parent = Uuid::new_v4();

    seed_in_progress_list(&bus, &pool, parent).await;
    sqlx::query(
        "INSERT INTO thread_queue (id, kind, thread_id, request, status) \
         VALUES ($1, 'sub-thread', $2, jsonb_build_object('parent_thread_id', $3::text), \
                 'queued')",
    )
    .bind(Uuid::new_v4())
    .bind(Uuid::new_v4())
    .bind(parent.to_string())
    .execute(&pool)
    .await
    .expect("seed a queued spawn");
    drain(&mut rx);

    let status = settle_at_terminator(&bus, &pool, &mut rx, parent).await;
    assert_eq!(status, TodoStatus::Waiting);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_queued_child_moved_to_top_level_does_not_park_the_list() {
    let (bus, mut rx, pool, db) = setup().await;
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();

    seed_in_progress_list(&bus, &pool, parent).await;
    spawn_child(&bus, parent, child, None).await;
    finish_child(&pool, child).await;
    sqlx::query(
        "INSERT INTO thread_queue (id, kind, thread_id, request, status) \
         VALUES ($1, 'sub-thread', $2, jsonb_build_object('parent_thread_id', $3::text), \
                 'queued')",
    )
    .bind(Uuid::new_v4())
    .bind(child)
    .bind(parent.to_string())
    .execute(&pool)
    .await
    .expect("seed the child's queue entry");
    emit_on(&bus, parent, child_detached(child)).await;
    drain(&mut rx);

    let status = settle_at_terminator(&bus, &pool, &mut rx, parent).await;
    assert_eq!(status, TodoStatus::Abandoned);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_child_that_finished_before_the_terminator_does_not_park_the_list() {
    let (bus, mut rx, pool, db) = setup().await;
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();

    seed_in_progress_list(&bus, &pool, parent).await;
    spawn_child(&bus, parent, child, None).await;
    finish_child(&pool, child).await;
    emit_on(&bus, parent, child_completed(child)).await;
    drain(&mut rx);

    let status = settle_at_terminator(&bus, &pool, &mut rx, parent).await;
    assert_eq!(status, TodoStatus::Abandoned);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_child_resolving_after_the_terminator_still_parks_the_list() {
    let (bus, mut rx, pool, db) = setup().await;
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();

    seed_in_progress_list(&bus, &pool, parent).await;
    spawn_child(&bus, parent, child, None).await;
    drain(&mut rx);

    emit_response_generated(&bus, parent).await;
    let terminator = next_matching(&mut rx, parent, is_terminator).await;
    finish_child(&pool, child).await;
    emit_on(&bus, parent, child_report_filed(child)).await;

    handle_event(&bus, &pool, &terminator, false).await;
    let settled = next_todo_items(&mut rx, parent).await;
    assert_eq!(settled[0].status, TodoStatus::Waiting);

    pool.close().await;
    teardown_test_db(&db).await;
}

async fn assert_the_last_child_unpark_abandons(
    resolve: impl FnOnce(Uuid) -> ThreadEvent,
    child_idles: bool,
) {
    let (bus, mut rx, pool, db) = setup().await;
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();

    park_on_children(&bus, &pool, &mut rx, parent, &[child]).await;
    if child_idles {
        finish_child(&pool, child).await;
    }
    emit_on(&bus, parent, resolve(child)).await;
    dispatch_next_child_unpark(&bus, &pool, &mut rx, parent).await;

    let settled = next_todo_items(&mut rx, parent).await;
    assert_eq!(settled[0].status, TodoStatus::Abandoned);
    assert_nothing_settles(&bus, &pool, &mut rx, parent).await;

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn the_last_child_filing_its_report_abandons_the_list() {
    assert_the_last_child_unpark_abandons(child_report_filed, true).await;
}

#[tokio::test]
async fn the_last_child_stopping_abandons_the_list() {
    assert_the_last_child_unpark_abandons(child_stopped, true).await;
}

#[tokio::test]
async fn the_last_child_detached_abandons_the_list() {
    assert_the_last_child_unpark_abandons(child_detached, false).await;
}

#[tokio::test]
async fn one_of_two_children_filing_changes_nothing_and_the_last_settles_once() {
    let (bus, mut rx, pool, db) = setup().await;
    let parent = Uuid::new_v4();
    let first = Uuid::new_v4();
    let last = Uuid::new_v4();

    park_on_children(&bus, &pool, &mut rx, parent, &[first, last]).await;

    finish_child(&pool, first).await;
    emit_on(&bus, parent, child_report_filed(first)).await;
    dispatch_next_child_unpark(&bus, &pool, &mut rx, parent).await;
    assert_nothing_settles(&bus, &pool, &mut rx, parent).await;

    finish_child(&pool, last).await;
    emit_on(&bus, parent, child_report_filed(last)).await;
    dispatch_next_child_unpark(&bus, &pool, &mut rx, parent).await;
    let settled = next_todo_items(&mut rx, parent).await;
    assert_eq!(settled[0].status, TodoStatus::Abandoned);
    assert_nothing_settles(&bus, &pool, &mut rx, parent).await;

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_completed_child_is_left_to_the_turn_it_wakes() {
    let (bus, mut rx, pool, db) = setup().await;
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();

    park_on_children(&bus, &pool, &mut rx, parent, &[child]).await;
    finish_child(&pool, child).await;
    emit_on(&bus, parent, child_completed(child)).await;
    assert_nothing_settles(&bus, &pool, &mut rx, parent).await;

    pool.close().await;
    teardown_test_db(&db).await;
}

async fn assert_a_turn_owning_the_list_blocks_a_child_unpark(status: &str) {
    let (bus, mut rx, pool, db) = setup().await;
    let parent = Uuid::new_v4();
    let child = Uuid::new_v4();

    park_on_children(&bus, &pool, &mut rx, parent, &[child]).await;
    seed_thread_status(&pool, parent, status).await;
    finish_child(&pool, child).await;
    emit_on(&bus, parent, child_report_filed(child)).await;
    dispatch_next_child_unpark(&bus, &pool, &mut rx, parent).await;
    assert_nothing_settles(&bus, &pool, &mut rx, parent).await;

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_child_unpark_settles_nothing_while_the_parent_is_running() {
    assert_a_turn_owning_the_list_blocks_a_child_unpark("running").await;
}

#[tokio::test]
async fn a_child_unpark_settles_nothing_while_the_parent_awaits_an_answer() {
    assert_a_turn_owning_the_list_blocks_a_child_unpark("waiting_for_user_answer").await;
}

#[tokio::test]
async fn a_child_unpark_settles_nothing_while_the_parent_is_paused() {
    assert_a_turn_owning_the_list_blocks_a_child_unpark("paused").await;
}

#[test]
fn the_settle_list_names_the_child_unparks_and_not_the_completion() {
    let child = Uuid::new_v4();
    let types = settle_event_types();
    for unpark in [
        child_report_filed(child),
        child_stopped(child),
        child_detached(child),
    ] {
        assert!(types.contains(&unpark.event_type()), "{unpark:?}");
    }
    assert!(!types.contains(&child_completed(child).event_type()));
}
