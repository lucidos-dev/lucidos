use super::*;
use serde_json::json;

use crate::engine::home_thread::ensure_home_thread;
use crate::engine::widgets::{WidgetChange, WidgetInstance};
use crate::test_support::{setup_test_db, teardown_test_db};
use WidgetSize::{Large, Medium, Small};

fn pin(app_id: &str) -> ThreadEvent {
    ThreadEvent::WidgetPinned {
        app_id: app_id.into(),
        params: None,
        label: None,
        shown_event_id: None,
    }
}

fn unpin(app_id: &str) -> ThreadEvent {
    ThreadEvent::WidgetUnpinned {
        app_id: app_id.into(),
        params: None,
        label: None,
    }
}

fn tile(app_id: &str, size: WidgetSize) -> ArrangedTile {
    ArrangedTile {
        app_id: app_id.into(),
        params: None,
        size,
        hidden: false,
    }
}

fn arranged(layout: LayoutClass, tiles: Vec<ArrangedTile>) -> ThreadEvent {
    ThreadEvent::HomeBoardArranged { layout, tiles }
}

/// (app id, size, hidden) per tile of one layout, in board order.
fn shape(board: &FoldedBoard, layout: LayoutClass) -> Vec<(String, Option<WidgetSize>, bool)> {
    board
        .layout(layout)
        .iter()
        .map(|t| (t.app_id.clone(), t.size, t.hidden))
        .collect()
}

fn ids(board: &FoldedBoard, layout: LayoutClass) -> Vec<String> {
    shape(board, layout).into_iter().map(|t| t.0).collect()
}

/// Invariant: the tiles are exactly Home's pins. A pin appends at the default
/// size and an unpin removes the tile.
#[test]
fn a_pin_appends_a_tile_at_its_default_size_and_an_unpin_removes_it() {
    let board = fold_home_board(&[pin("diet"), pin("inbox"), pin("clock"), unpin("inbox")]);
    assert_eq!(
        shape(&board, LayoutClass::Desktop),
        [("diet".into(), None, false), ("clock".into(), None, false)]
    );
}

/// A snapshot entry for an instance that is not pinned is ignored, so no
/// arrangement can add a tile.
#[test]
fn a_snapshot_for_an_unpinned_instance_is_ignored() {
    let board = fold_home_board(&[
        pin("diet"),
        arranged(
            LayoutClass::Desktop,
            vec![tile("ghost", Small), tile("diet", Large)],
        ),
    ]);
    assert_eq!(
        shape(&board, LayoutClass::Desktop),
        [("diet".into(), Some(Large), false)]
    );
}

/// The newest snapshot per layout wins, whole.
#[test]
fn the_last_snapshot_wins() {
    let board = fold_home_board(&[
        pin("a"),
        pin("b"),
        arranged(
            LayoutClass::Desktop,
            vec![tile("b", Large), tile("a", Small)],
        ),
        arranged(
            LayoutClass::Desktop,
            vec![tile("a", Medium), tile("b", Small)],
        ),
    ]);
    assert_eq!(
        shape(&board, LayoutClass::Desktop),
        [
            ("a".into(), Some(Medium), false),
            ("b".into(), Some(Small), false)
        ]
    );
}

/// The phone layout copies desktop until a phone arranges it. From then on
/// each layout keeps its own order, sizes and hidden tiles.
#[test]
fn the_phone_layout_copies_desktop_until_arranged() {
    let desktop_only = [
        pin("a"),
        pin("b"),
        arranged(
            LayoutClass::Desktop,
            vec![tile("b", Large), tile("a", Small)],
        ),
    ];
    let board = fold_home_board(&desktop_only);
    assert_eq!(
        shape(&board, LayoutClass::Phone),
        shape(&board, LayoutClass::Desktop)
    );

    let mut events = desktop_only.to_vec();
    events.push(arranged(
        LayoutClass::Phone,
        vec![
            tile("a", Small),
            ArrangedTile {
                hidden: true,
                ..tile("b", Medium)
            },
        ],
    ));
    events.push(pin("c"));
    let board = fold_home_board(&events);
    assert_eq!(
        shape(&board, LayoutClass::Phone),
        [
            ("a".into(), Some(Small), false),
            ("b".into(), Some(Medium), true),
            ("c".into(), None, false),
        ]
    );
    assert_eq!(ids(&board, LayoutClass::Desktop), ["b", "a", "c"]);
}

/// A pinned instance an arrangement leaves out keeps its place after the
/// named ones, with its size.
#[test]
fn an_omitted_tile_keeps_its_size_after_the_named_ones() {
    let board = fold_home_board(&[
        pin("a"),
        pin("b"),
        pin("c"),
        arranged(
            LayoutClass::Desktop,
            vec![tile("a", Small), tile("b", Large), tile("c", Small)],
        ),
        arranged(LayoutClass::Desktop, vec![tile("c", Medium)]),
    ]);
    assert_eq!(
        shape(&board, LayoutClass::Desktop),
        [
            ("c".into(), Some(Medium), false),
            ("a".into(), Some(Small), false),
            ("b".into(), Some(Large), false),
        ]
    );
}

/// Invariant: a new pin starts at the default size. Unpinned and pinned again,
/// a tile is a new pin: it appends at the default, whatever an older snapshot
/// said about it.
#[test]
fn a_pin_after_an_unpin_is_a_new_tile_at_the_default_size() {
    let board = fold_home_board(&[
        pin("a"),
        pin("b"),
        arranged(
            LayoutClass::Desktop,
            vec![tile("a", Large), tile("b", Small)],
        ),
        unpin("a"),
        pin("a"),
    ]);
    assert_eq!(
        shape(&board, LayoutClass::Desktop),
        [("b".into(), Some(Small), false), ("a".into(), None, false)]
    );
}

/// Params are part of a tile's identity, in any key order.
#[test]
fn two_instances_of_one_widget_are_two_tiles() {
    let with = |clip: &str| ThreadEvent::WidgetPinned {
        app_id: "player".into(),
        params: Some(serde_json::from_value(json!({ "clip": clip })).unwrap()),
        label: None,
        shown_event_id: None,
    };
    let board = fold_home_board(&[with("a.mp3"), with("b.mp3"), with("a.mp3")]);
    assert_eq!(board.layout(LayoutClass::Desktop).len(), 2);
}

fn write_widget(ws: &std::path::Path, id: &str, manifest: Value) {
    let dir = ws.join("data/apps").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
}

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    crate::test_support::seed_thread_event(bus, thread_id, event).await;
}

async fn summary_row(pool: &PgPool, thread_id: Uuid) -> Value {
    sqlx::query_scalar(
        "SELECT to_jsonb(t) - 'updated_at' FROM thread_summaries t WHERE thread_id = $1",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// The board read resolves each tile's size against the manifest: a new pin
/// at the first declared size, and an arranged size that the widget no longer
/// declares at the nearest smaller one.
#[tokio::test]
async fn the_board_read_resolves_sizes_against_the_manifest() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let manager = AppManager::new(tmp.path()).unwrap();
    let home = ensure_home_thread(&bus, &pool).await.unwrap();
    write_widget(
        tmp.path(),
        "clock",
        json!({"name": "Clock", "kind": "widget", "reusable": true, "sizes": ["small", "large"]}),
    );
    write_widget(
        tmp.path(),
        "inbox",
        json!({"name": "Inbox", "kind": "widget", "reusable": true}),
    );
    emit(&bus, home, pin("clock")).await;
    emit(&bus, home, pin("inbox")).await;
    emit(&bus, home, pin("gone")).await;

    let board = home_board(&pool, &manager, LayoutClass::Desktop)
        .await
        .unwrap();
    let sizes: Vec<(&str, WidgetSize)> = board
        .tiles
        .iter()
        .map(|t| (t.widget.app_id.as_str(), t.size))
        .collect();
    assert_eq!(sizes, [("clock", Small), ("inbox", Medium)]);

    emit(
        &bus,
        home,
        arranged(LayoutClass::Desktop, vec![tile("clock", Large)]),
    )
    .await;
    write_widget(
        tmp.path(),
        "clock",
        json!({"name": "Clock", "kind": "widget", "reusable": true, "sizes": ["small", "medium"]}),
    );
    let board = home_board(&pool, &manager, LayoutClass::Desktop)
        .await
        .unwrap();
    assert_eq!(board.tiles[0].size, Medium, "large fits down to medium");
    teardown_test_db(&db).await;
}

/// Invariant: a widget only takes sizes it declares, and an arrangement names
/// only Home's pins, each once.
#[tokio::test]
async fn an_arrangement_refuses_an_undeclared_size_an_unpinned_instance_and_a_repeat() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let manager = AppManager::new(tmp.path()).unwrap();
    let home = ensure_home_thread(&bus, &pool).await.unwrap();
    write_widget(
        tmp.path(),
        "clock",
        json!({"name": "Clock", "kind": "widget", "reusable": true, "sizes": ["small"]}),
    );
    write_widget(
        tmp.path(),
        "inbox",
        json!({"name": "Inbox", "kind": "widget", "reusable": true}),
    );
    emit(&bus, home, pin("clock")).await;

    let refused = |r: Result<Uuid, WidgetCheckFailed>, needle: &str| match r {
        Err(WidgetCheckFailed::Refused(reason)) => {
            assert!(reason.contains(needle), "{needle}: {reason}")
        }
        other => panic!("expected a refusal naming {needle}, got {other:?}"),
    };
    refused(
        check_arrangement(&pool, &manager, &[tile("clock", Large)]).await,
        "does not take the size 'large'",
    );
    refused(
        check_arrangement(&pool, &manager, &[tile("inbox", Medium)]).await,
        "not pinned to Home",
    );
    refused(
        check_arrangement(
            &pool,
            &manager,
            &[tile("clock", Small), tile("clock", Small)],
        )
        .await,
        "named twice",
    );
    assert_eq!(
        check_arrangement(&pool, &manager, &[tile("clock", Small)])
            .await
            .unwrap(),
        home
    );
    teardown_test_db(&db).await;
}

/// Invariant: Home stays an unused Home. An arrange starts no turn, changes
/// no summary column and gives the summary tree nothing to summarize.
#[tokio::test]
async fn an_arrange_starts_no_turn_and_leaves_homes_summary_alone() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let manager = AppManager::new(tmp.path()).unwrap();
    let home = ensure_home_thread(&bus, &pool).await.unwrap();
    write_widget(
        tmp.path(),
        "clock",
        json!({"name": "Clock", "kind": "widget", "reusable": true, "sizes": ["small", "medium"]}),
    );
    emit(&bus, home, pin("clock")).await;
    let before = summary_row(&pool, home).await;

    let tiles = vec![tile("clock", Small)];
    check_arrangement(&pool, &manager, &tiles).await.unwrap();
    emit_arrangement(&bus, home, LayoutClass::Phone, tiles, None)
        .await
        .unwrap();

    assert_eq!(summary_row(&pool, home).await, before, "no badge, no move");
    let types: Vec<String> =
        sqlx::query_scalar("SELECT event_type FROM events WHERE thread_id = $1 ORDER BY sequence")
            .bind(home)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        types,
        ["HomeThreadCreated", "WidgetPinned", "HomeBoardArranged"],
        "no turn started"
    );
    let log = crate::engine::summary_tree::store::load_thread_log_now(
        &pool,
        home,
        crate::engine::summary_tree::log::ThreadKind::LucidosAgent,
    )
    .await
    .unwrap();
    assert!(
        log.entries.is_empty() && log.turns.is_empty(),
        "nothing to summarize"
    );
    let board = home_board(&pool, &manager, LayoutClass::Phone)
        .await
        .unwrap();
    assert_eq!(board.tiles[0].size, Small);
    teardown_test_db(&db).await;
}

/// Pin to Home on another thread's own widget makes it reusable in the same
/// action, so deleting that thread never silently empties the tile. A pin to
/// any other thread is still refused.
#[tokio::test]
async fn pin_to_home_makes_another_threads_widget_reusable_first() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let manager = AppManager::new(tmp.path()).unwrap();
    let home = ensure_home_thread(&bus, &pool).await.unwrap();
    let origin = Uuid::new_v4();
    let elsewhere = Uuid::new_v4();
    for thread in [origin, elsewhere] {
        emit(
            &bus,
            thread,
            ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                wake_mode: None,
                text: "compare the fares".into(),
                user_image_hashes: vec![],
                device_id: None,
                image_description: None,
                parent_thread_id: None,
                spawning_event_id: None,
                mode: crate::engine::thread_events::ActorMode::Human,
                model: None,
                reasoning_effort: None,
                origin: None,
            },
        )
        .await;
    }
    write_widget(
        tmp.path(),
        "fares",
        json!({"name": "Fares", "kind": "widget", "origin_thread_id": origin.to_string()}),
    );
    let fares = WidgetInstance {
        app_id: "fares".into(),
        ..Default::default()
    };

    let refused = widgets::check_widget_change(
        &pool,
        &manager,
        WidgetChange::Pin {
            shown_event_id: None,
        },
        &fares,
        elsewhere,
    )
    .await;
    assert!(matches!(refused, Err(WidgetCheckFailed::Refused(_))));

    let plan = widgets::check_widget_change(
        &pool,
        &manager,
        WidgetChange::Pin {
            shown_event_id: None,
        },
        &fares,
        home,
    )
    .await
    .unwrap();
    assert!(plan.make_reusable);
    let made = widgets::apply_widget_change(
        &manager,
        &bus,
        plan,
        WidgetChange::Pin {
            shown_event_id: None,
        },
        fares,
        home,
        None,
    )
    .await
    .unwrap();
    assert!(made);
    assert!(manager.get_app("fares").unwrap().reusable);
    let board = home_board(&pool, &manager, LayoutClass::Desktop)
        .await
        .unwrap();
    assert_eq!(board.tiles.len(), 1);

    // A second pin racing the first is refused as a repeat, never a failure.
    let again = WidgetInstance {
        app_id: "fares".into(),
        ..Default::default()
    };
    let refused = widgets::check_widget_change(
        &pool,
        &manager,
        WidgetChange::Pin {
            shown_event_id: None,
        },
        &again,
        home,
    )
    .await;
    assert!(matches!(refused, Err(WidgetCheckFailed::Refused(_))));

    // While Home pins it, Stop reusing is refused: the tile would empty.
    let stop = widgets::check_set_reusable(&pool, &manager, "fares", false).await;
    assert!(
        matches!(&stop, Err(WidgetCheckFailed::Refused(r)) if r.contains("pinned to Home")),
        "got: {stop:?}"
    );

    // Unpinned, reuse stopped, then pinned again: it is made reusable again.
    emit(&bus, home, unpin("fares")).await;
    widgets::check_set_reusable(&pool, &manager, "fares", false)
        .await
        .unwrap();
    manager
        .set_widget_reusable(&bus, "fares", false, None)
        .await
        .unwrap();
    let plan = widgets::check_widget_change(
        &pool,
        &manager,
        WidgetChange::Pin {
            shown_event_id: None,
        },
        &again,
        home,
    )
    .await
    .unwrap();
    assert!(plan.make_reusable, "a pin again still guards the tile");
    teardown_test_db(&db).await;
}

/// Migration: a Home pin made before the board existed is a tile at its
/// default size, a legacy `WidgetRestored` row included. No data moves.
#[test]
fn a_pin_made_before_the_board_is_a_tile_at_its_default_size() {
    let legacy = ThreadEvent::from_stored("WidgetRestored", json!({"app_id": "fares"})).unwrap();
    let board = fold_home_board(&[legacy, pin("diet")]);
    assert_eq!(
        shape(&board, LayoutClass::Desktop),
        [("fares".into(), None, false), ("diet".into(), None, false)]
    );
    assert_eq!(
        shape(&board, LayoutClass::Phone),
        shape(&board, LayoutClass::Desktop)
    );
}
