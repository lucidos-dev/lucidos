use super::*;
use crate::engine::event_bus::test_rows::{emit_row, row_frame};
use crate::engine::event_bus::HistoricalReplay;
use crate::engine::thread_events::{EventChannel, MessageOrigin};
use crate::test_support::{
    message_received, offline_event_bus, seed_chat_thread, seed_thread_event, setup_test_db,
    teardown_test_db,
};

fn types() -> Option<Vec<&'static str>> {
    Some(vec!["MessageReceived"])
}

fn seqs(frames: &[EmittedEvent]) -> Vec<i64> {
    frames.iter().filter_map(|f| f.seq).collect()
}

fn thread_of(frame: &EmittedEvent) -> Uuid {
    match &frame.typed {
        BusEvent::Thread { thread_id, .. } => *thread_id,
        BusEvent::System(_) => panic!("expected a thread frame"),
    }
}

async fn seq_of_last_row(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT MAX(sequence) FROM events")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// A replay position at this floor with no append in flight, for the tests
/// that need no database.
fn at_floor(floor: i64) -> LagReplay {
    LagReplay::opened(
        ReplayScope::Workspace,
        None,
        UnbroadcastRows::default(),
        AppendTickets::default(),
        ReplayStart {
            floor: Some(floor),
            in_flight: Vec::new(),
        },
    )
}

/// A window over `(after, through]` with nothing skipped or owed.
fn window(after: i64, through: i64) -> ReplayWindow {
    ReplayWindow {
        after,
        through,
        skip: vec![],
        floor: after,
        owed: vec![],
    }
}

#[tokio::test]
async fn a_replay_returns_the_rows_the_subscriber_never_saw() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut replay = LagReplay::subscribe(&bus, ReplayScope::Workspace, types()).1;

    let thread_id = seed_chat_thread(&bus, "hello").await;

    let missed = replay.missed(&pool).await;
    assert_eq!(missed.len(), 1, "one dropped row, got {:?}", seqs(&missed));
    assert_eq!(thread_of(&missed[0]), thread_id);
    assert!(matches!(
        &missed[0].typed,
        BusEvent::Thread {
            event: ThreadEvent::MessageReceived { text, .. },
            ..
        } if text == "hello"
    ));

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The memory index bills one extraction per event, and the voice call speaks
/// one answer per event. So a row acted on live must not come back.
#[tokio::test]
async fn a_replay_skips_what_was_acted_on_live_and_live_skips_what_was_replayed() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut replay = LagReplay::subscribe(&bus, ReplayScope::Workspace, types()).1;

    seed_chat_thread(&bus, "seen live").await;
    let seen = seq_of_last_row(&pool).await;
    assert!(replay.first_sight(seen));
    seed_chat_thread(&bus, "dropped").await;
    let dropped = seq_of_last_row(&pool).await;

    assert_eq!(seqs(&replay.missed(&pool).await), vec![dropped]);
    assert!(
        !replay.first_sight(dropped),
        "the replay returned it, so its late live frame is a duplicate"
    );

    seed_chat_thread(&bus, "after").await;
    let after = seq_of_last_row(&pool).await;
    assert!(replay.first_sight(after), "a row above the replay is new");
    assert!(
        replay.missed(&pool).await.is_empty(),
        "nothing new was dropped"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_thread_scoped_replay_reads_only_its_thread_and_its_types() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mine = seed_chat_thread(&bus, "mine").await;
    let mut replay = LagReplay::subscribe(&bus, ReplayScope::Thread(mine), types()).1;

    seed_chat_thread(&bus, "another thread").await;
    seed_thread_event(
        &bus,
        mine,
        ThreadEvent::ResponseFailed {
            error: "not a replayed type".into(),
        },
    )
    .await;
    seed_thread_event(&bus, mine, message_received("mine again")).await;

    let missed = replay.missed(&pool).await;
    assert_eq!(missed.len(), 1, "got {:?}", seqs(&missed));
    assert_eq!(thread_of(&missed[0]), mine);

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The branch-work net reads `actor` and the memory indexer reads `channel`
/// off the frame, so both must survive the round trip through the row.
#[tokio::test]
async fn a_replayed_frame_carries_the_meta_it_was_emitted_with() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut replay = LagReplay::subscribe(&bus, ReplayScope::Workspace, types()).1;
    let actor = MessageOrigin::Device {
        device_id: "test-device".into(),
    };
    bus.emit(BusEvent::Thread {
        thread_id: Uuid::new_v4(),
        event: message_received("with meta"),
        meta: EventMeta {
            channel: Some(EventChannel::Chat),
            actor: Some(actor.clone()),
            ..EventMeta::NONE
        },
    })
    .await
    .unwrap();

    let missed = replay.missed(&pool).await;
    let BusEvent::Thread { meta, .. } = &missed[0].typed else {
        panic!("expected a thread frame");
    };
    assert_eq!(meta.channel, Some(EventChannel::Chat));
    assert_eq!(meta.actor, Some(actor));

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A replayed frame dispatches as its live frame did, so the trigger matcher
/// reads the same chain depth and drops the same emitting trigger.
#[tokio::test]
async fn a_replayed_frame_carries_its_stored_depth_and_emitting_trigger() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut replay = LagReplay::subscribe(&bus, ReplayScope::Workspace, types()).1;
    seed_chat_thread(&bus, "from a trigger").await;
    let seq = seq_of_last_row(&pool).await;
    sqlx::query(
        "UPDATE events SET trigger_chain_depth = 3, emitting_trigger_id = 'daily-digest' \
         WHERE sequence = $1",
    )
    .bind(seq)
    .execute(&pool)
    .await
    .unwrap();

    let missed = replay.missed(&pool).await;
    assert_eq!(missed.len(), 1, "got {:?}", seqs(&missed));
    assert_eq!(missed[0].trigger_chain_depth(), 3);
    assert_eq!(
        missed[0].emitting_trigger_id.as_deref(),
        Some("daily-digest")
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_row_that_does_not_read_back_is_skipped_and_the_rest_still_replay() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut replay = LagReplay::subscribe(&bus, ReplayScope::Workspace, types()).1;

    let broken = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, thread_id) \
         VALUES ($1, 'thread', $2, 'MessageReceived', '{}'::jsonb, $3)",
    )
    .bind(Uuid::new_v4())
    .bind(broken.to_string())
    .bind(broken)
    .execute(&pool)
    .await
    .unwrap();
    let good = seed_chat_thread(&bus, "readable").await;

    let missed = replay.missed(&pool).await;
    assert_eq!(missed.len(), 1, "got {:?}", seqs(&missed));
    assert_eq!(thread_of(&missed[0]), good);

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A failed read is not a retry: nothing runs on its own. It only keeps the
/// floor, so the next lag's replay covers both windows.
#[tokio::test]
async fn a_failed_read_keeps_the_floor_for_the_next_replay() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let unreachable = offline_event_bus();
    let mut replay = LagReplay::subscribe(&bus, ReplayScope::Workspace, types()).1;

    seed_chat_thread(&bus, "first window").await;
    assert!(replay.missed(unreachable.pool()).await.is_empty());
    seed_chat_thread(&bus, "second window").await;

    assert_eq!(replay.missed(&pool).await.len(), 2);

    pool.close().await;
    teardown_test_db(&db).await;
}

/// I2. A row the bus sent before the subscription never reached this
/// subscriber, so no replay offers it.
#[tokio::test]
async fn a_row_sent_before_subscribing_is_never_replayed() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    seed_chat_thread(&bus, "history").await;
    let (mut rx, mut replay) = LagReplay::subscribe(&bus, ReplayScope::Workspace, types());

    let next = seed_chat_thread(&bus, "after subscribing").await;
    crate::test_support::flood_past_bus_capacity(&bus);
    let frames = replay.read(rx.recv().await, &pool).await.unwrap();
    assert_eq!(frames.len(), 1, "got {:?}", seqs(&frames));
    assert_eq!(thread_of(&frames[0]), next);

    pool.close().await;
    teardown_test_db(&db).await;
}

/// The one loop shape every replaying subscriber runs: a live frame passes
/// once, a lag hands back what it dropped, and a closed bus ends the loop.
#[tokio::test]
async fn a_read_passes_live_frames_once_and_turns_a_lag_into_the_dropped_rows() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let (mut rx, mut replay) = LagReplay::subscribe(&bus, ReplayScope::Workspace, types());

    let live = seed_chat_thread(&bus, "live").await;
    let frames = replay.read(rx.recv().await, &pool).await.unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!(thread_of(&frames[0]), live);

    let dropped = seed_chat_thread(&bus, "dropped").await;
    crate::test_support::flood_past_bus_capacity(&bus);
    let frames = replay.read(rx.recv().await, &pool).await.unwrap();
    assert_eq!(frames.len(), 1, "got {:?}", seqs(&frames));
    assert_eq!(thread_of(&frames[0]), dropped);

    let flood_frame = replay.read(rx.recv().await, &pool).await.unwrap();
    assert!(
        flood_frame.is_empty(),
        "a type outside the list is not handed on"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// A system row that has committed and not yet broadcast, as an append sits
/// between its commit and its send. A default-shaped variant, so the replay
/// reads it back as its event.
async fn commit_unsent_row(bus: &EventBus) -> super::super::CommittedSystemEvent {
    let event = SystemEvent::ArtifactCreated {
        artifact_path: "notes.md".into(),
        commit: "abc123".into(),
        source: None,
        writer_thread_id: None,
    };
    let append = bus.begin_append().await.unwrap();
    bus.commit_system_event(append, Uuid::new_v4(), event, 0, None)
        .await
        .unwrap()
}

/// I1. The row committed before the subscription opened, so it sits at or
/// below the subscription's floor. Its frame still reached this subscriber,
/// so the lag that dropped it owes it.
#[tokio::test]
async fn a_row_committed_before_subscribing_and_sent_after_is_replayed() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let unsent = commit_unsent_row(&bus).await;
    let seq = unsent.seq;

    let (mut rx, mut replay) = LagReplay::subscribe(&bus, ReplayScope::Workspace, None);
    bus.publish_system_event(unsent);
    crate::test_support::flood_past_bus_capacity(&bus);

    let frames = replay.read(rx.recv().await, &pool).await.unwrap();
    assert_eq!(seqs(&frames), vec![seq]);

    pool.close().await;
    teardown_test_db(&db).await;
}

/// I3. A row still unsent when a lag comes is not owed yet: it arrives live,
/// once, and a later lag does not replay it.
#[tokio::test]
async fn an_unsent_row_is_not_replayed_and_arrives_live_once() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let unsent = commit_unsent_row(&bus).await;
    let seq = unsent.seq;
    let (mut rx, mut replay) = LagReplay::subscribe(&bus, ReplayScope::Workspace, None);

    crate::test_support::flood_past_bus_capacity(&bus);
    let lagged = replay.read(rx.recv().await, &pool).await.unwrap();
    assert!(lagged.is_empty(), "got {:?}", seqs(&lagged));

    bus.publish_system_event(unsent);
    let live = drain_to(&mut rx, &mut replay, &pool, seq).await;
    assert_eq!(seqs(&live), vec![seq]);

    crate::test_support::flood_past_bus_capacity(&bus);
    let lagged = replay.read(rx.recv().await, &pool).await.unwrap();
    assert!(lagged.is_empty(), "got {:?}", seqs(&lagged));

    pool.close().await;
    teardown_test_db(&db).await;
}

/// I3. A row the replay returned is skipped when its own frame arrives late
/// from the buffer.
#[tokio::test]
async fn an_owed_row_the_replay_returned_is_skipped_when_it_arrives_late() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let unsent = commit_unsent_row(&bus).await;
    let seq = unsent.seq;
    let (mut rx, mut replay) = LagReplay::subscribe(&bus, ReplayScope::Workspace, None);

    crate::test_support::flood_past_bus_capacity(&bus);
    bus.publish_system_event(unsent);
    let replayed = replay.read(rx.recv().await, &pool).await.unwrap();
    assert_eq!(seqs(&replayed), vec![seq], "a sent row is owed at once");

    let late = drain_to(&mut rx, &mut replay, &pool, seq).await;
    assert!(seqs(&late).is_empty(), "got {:?}", seqs(&late));

    pool.close().await;
    teardown_test_db(&db).await;
}

/// Read until the frame of the row at `seq` has passed, and return every frame
/// handed on along the way.
async fn drain_to(
    rx: &mut broadcast::Receiver<EmittedEvent>,
    replay: &mut LagReplay,
    pool: &PgPool,
    seq: i64,
) -> Vec<EmittedEvent> {
    let mut handed_on = Vec::new();
    loop {
        let received = rx.recv().await;
        let passed = matches!(&received, Ok(frame) if frame.seq == Some(seq));
        handed_on.extend(replay.read(received, pool).await.unwrap());
        if passed {
            return handed_on;
        }
    }
}

/// I5. A subscription opened before the bus learned any sequence owes every
/// row the bus sends, so its first lag replays them all.
#[tokio::test]
async fn a_first_lag_with_no_starting_position_replays_what_it_dropped() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let (mut rx, mut replay) = LagReplay::subscribe(&bus, ReplayScope::Workspace, types());
    assert_eq!(replay.floor, None, "the bus has appended nothing yet");

    let dropped = seed_chat_thread(&bus, "dropped").await;
    crate::test_support::flood_past_bus_capacity(&bus);

    let frames = replay.read(rx.recv().await, &pool).await.unwrap();
    assert_eq!(frames.len(), 1, "got {:?}", seqs(&frames));
    assert_eq!(thread_of(&frames[0]), dropped);

    pool.close().await;
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn a_read_of_a_closed_bus_ends_the_loop() {
    let bus = offline_event_bus();
    let pool = bus.pool().clone();
    let (mut rx, mut replay) = LagReplay::subscribe(&bus, ReplayScope::Workspace, types());
    drop(bus);
    assert!(replay.read(rx.recv().await, &pool).await.is_none());
}

#[test]
fn the_floor_moves_over_sequences_received_without_a_gap() {
    let mut replay = at_floor(10);
    replay.received(Some(12));
    assert_eq!(replay.floor, Some(10), "11 has not arrived");
    replay.received(Some(11));
    assert_eq!(replay.floor, Some(12));
    assert!(replay.received.is_empty());
}

#[test]
fn out_of_order_frames_merge_into_ranges() {
    let mut replay = at_floor(10);
    for seq in [15, 13, 14, 20, 21] {
        replay.received(Some(seq));
    }
    replay.received(Some(14));
    replay.received(Some(9));
    replay.received(None);
    assert_eq!(
        replay
            .received
            .iter()
            .map(|(s, e)| (*s, *e))
            .collect::<Vec<_>>(),
        vec![(13, 15), (20, 21)]
    );
}

#[test]
fn a_covered_window_marks_late_frames_and_keeps_what_lies_above() {
    let mut replay = at_floor(10);
    replay.received(Some(19));
    replay.received(Some(20));
    replay.received(Some(21));
    replay.covered(&window(10, 20));
    assert!(replay.is_covered(Some(20)));
    assert!(!replay.is_covered(Some(21)));
    assert!(
        !replay.is_covered(None),
        "a transient frame is never covered"
    );
    assert_eq!(replay.floor, Some(21), "21 sits right above the new floor");
}

/// A row that reached the subscriber live is not offered again, so a replay
/// never re-decides an event against a registry that changed since.
#[tokio::test]
async fn a_window_skips_rows_received_live() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut replay = LagReplay::subscribe(&bus, ReplayScope::Workspace, None).1;

    let gap = emit_row(&bus).await;
    let seen = emit_row(&bus).await;
    replay.received(Some(seen));
    let missed = emit_row(&bus).await;
    let window = replay.window(&pool).await.expect("rows to replay");

    assert!(window.includes(gap));
    assert!(!window.includes(seen));
    assert!(window.includes(missed));
    assert_eq!(window.through, missed);
    teardown_test_db(&db).await;
}

/// A backfill row the bus wrote without a broadcast was never meant to reach
/// a subscriber, so a replay does not offer it either.
#[tokio::test]
async fn a_window_skips_rows_written_without_a_broadcast() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut replay = LagReplay::subscribe(&bus, ReplayScope::Workspace, None).1;

    let payload = serde_json::json!({});
    let backfilled = bus
        .replay_historical_event(HistoricalReplay {
            event_id: uuid::Uuid::new_v4(),
            aggregate: "test",
            aggregate_id: "backfill",
            event_type: "BuildObserved",
            payload: &payload,
            thread_id: None,
            created: None,
            broadcast: false,
        })
        .await
        .unwrap()
        .expect("inserted");
    let missed = emit_row(&bus).await;
    let window = replay.window(&pool).await.expect("a row to replay");

    assert!(!window.includes(backfilled));
    assert!(window.includes(missed));
    teardown_test_db(&db).await;
}

#[tokio::test]
async fn an_uncovered_window_is_read_again_by_the_next_lag() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut replay = LagReplay::subscribe(&bus, ReplayScope::Workspace, None).1;
    let first = emit_row(&bus).await;
    let failed = replay.window(&pool).await.expect("one row to replay");

    let second = emit_row(&bus).await;
    let next = replay.window(&pool).await.expect("two rows to replay");

    assert_eq!(next.after, failed.after, "the floor did not move");
    assert!(next.includes(first) && next.includes(second));
    teardown_test_db(&db).await;
}

/// A floor set at the first lag keeps what arrived live before it. Nothing
/// was replayed, so no late frame may be skipped as covered.
#[tokio::test]
async fn a_floor_set_at_the_first_lag_skips_live_rows_and_covers_none() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let (_receiver, mut replay) = LagReplay::subscribe(&bus, ReplayScope::Workspace, None);
    let seq = emit_row(&bus).await;
    assert!(replay.first_sight(seq));

    assert_eq!(replay.window(&pool).await, None);
    assert_eq!(replay.floor, Some(seq));
    assert!(!replay.is_covered(Some(seq)));
    teardown_test_db(&db).await;
}

/// I6. A failed read keeps a known floor and the appends still owed, so the
/// next lag replays both windows.
#[tokio::test]
async fn a_failed_read_keeps_a_known_floor_and_what_is_owed() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let unreachable = offline_event_bus();
    seed_chat_thread(&bus, "history").await;
    let unsent = commit_unsent_row(&bus).await;
    let owed = unsent.seq;
    let (_receiver, mut replay) = LagReplay::subscribe(&bus, ReplayScope::Workspace, None);
    let floor = replay.floor;
    bus.publish_system_event(unsent);
    let after = emit_row(&bus).await;

    assert!(replay.missed(unreachable.pool()).await.is_empty());
    assert_eq!(replay.floor, floor);
    assert_eq!(replay.owed(), vec![owed]);
    assert_eq!(seqs(&replay.missed(&pool).await), vec![owed]);
    assert_eq!(
        replay.floor,
        Some(after),
        "the domain row itself does not read back"
    );

    pool.close().await;
    teardown_test_db(&db).await;
}

/// I5. A thread-scoped subscription opened before the bus learned anything
/// sets its floor from a workspace horizon, and replays its thread's rows.
#[tokio::test]
async fn a_thread_scoped_first_lag_replays_what_it_dropped() {
    let (pool, db) = setup_test_db().await;
    let (bus, _callbacks) = EventBus::new(pool.clone());
    let thread_id = Uuid::new_v4();
    let (_receiver, mut replay) =
        LagReplay::subscribe(&bus, ReplayScope::Thread(thread_id), types());
    assert_eq!(replay.floor, None);

    seed_chat_thread(&bus, "another thread").await;
    seed_thread_event(&bus, thread_id, message_received("mine")).await;
    let missed = replay.missed(&pool).await;
    assert_eq!(missed.len(), 1, "got {:?}", seqs(&missed));
    assert_eq!(thread_of(&missed[0]), thread_id);

    pool.close().await;
    teardown_test_db(&db).await;
}

/// I6. A horizon read that fails never advances the floor.
#[tokio::test]
async fn a_failed_horizon_read_keeps_the_floor() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let mut replay = LagReplay::subscribe(&bus, ReplayScope::Workspace, None).1;
    let floor = replay.floor;
    pool.close().await;

    assert_eq!(replay.window(&pool).await, None);
    assert_eq!(replay.floor, floor);
    teardown_test_db(&db).await;
}

/// A frame still buffered from before the first window was never replayed,
/// so it is not skipped as covered when it arrives.
#[test]
fn a_frame_below_the_first_window_is_not_covered() {
    let mut replay = at_floor(10);
    replay.covered(&window(10, 20));
    assert!(!replay.is_covered(Some(9)));
    assert!(replay.is_covered(Some(11)));
}

/// I10. A replay that stopped among the owed rows below the floor keeps the
/// floor, and owes the rest to the next lag.
#[test]
fn a_partial_replay_among_owed_rows_keeps_the_floor() {
    let mut replay = at_floor(10);
    let window = ReplayWindow {
        after: 6,
        through: 20,
        skip: vec![8..=8, 10..=10],
        floor: 10,
        owed: vec![7, 9],
    };
    assert!(window.includes(7) && window.includes(9) && window.includes(11));
    assert!(!window.includes(8) && !window.includes(10));

    replay.covered(&window.up_to(7));
    assert!(replay.is_covered(Some(7)));
    assert!(!replay.is_covered(Some(9)));
    assert_eq!(replay.floor, Some(10));
}

/// I3. A live frame at or below the opening floor is an in-flight append's
/// row. The subscriber acts on it once, and no lag owes it afterwards.
#[test]
fn a_live_row_below_the_opening_floor_is_acted_on_once_and_not_owed() {
    let (event_tx, _keep_open) = broadcast::channel(16);
    let tickets = AppendTickets::default();
    let in_flight = tickets.open();
    in_flight.learned(9);
    let ended = tickets.open();
    ended.learned(10);
    drop(ended);
    let (_receiver, start) = tickets.subscribe(&event_tx);
    let mut replay = LagReplay::opened(
        ReplayScope::Workspace,
        None,
        UnbroadcastRows::default(),
        tickets.clone(),
        start,
    );

    in_flight.send(&event_tx, row_frame(9));
    assert!(replay.live(&row_frame(9)), "a first sight is acted on");
    assert!(replay.owed().is_empty());
    assert!(!replay.live(&row_frame(9)), "and only once");
}

/// I5. An append can draw a lower sequence and learn it after another
/// append's higher one. So a floor set at the first lag never sits above the
/// committed horizon, and that lower row stays owed.
#[test]
fn a_floor_set_at_the_first_lag_never_sits_above_the_committed_horizon() {
    let (event_tx, _keep_open) = broadcast::channel(16);
    let tickets = AppendTickets::default();
    let (_receiver, start) = tickets.subscribe(&event_tx);
    let mut replay = LagReplay::opened(
        ReplayScope::Workspace,
        None,
        UnbroadcastRows::default(),
        tickets.clone(),
        start,
    );
    let higher = tickets.open();
    higher.learned(50);

    assert_eq!(replay.first_floor(10), Some(10));
    assert_eq!(replay.floor, Some(10));
}

/// The gaps a window skips between the owed rows and the floor.
#[test]
fn the_rows_between_owed_rows_and_the_floor_are_skipped() {
    assert_eq!(gaps_between(6, &[7, 9], 10), vec![8..=8, 10..=10]);
    assert_eq!(gaps_between(10, &[], 10), Vec::<RangeInclusive<i64>>::new());
    assert_eq!(gaps_between(3, &[4], 8), vec![5..=8]);
}

/// A replay that stopped part way covers only what it offered, and the next
/// lag reads the rest.
#[test]
fn a_partial_replay_covers_only_what_it_offered() {
    let mut replay = at_floor(10);
    let window = window(10, 20);
    replay.covered(&window.up_to(15));
    assert!(replay.is_covered(Some(15)));
    assert!(!replay.is_covered(Some(16)));
    assert_eq!(replay.floor, Some(15));
}
