use super::*;
use serde_json::json;
use uuid::Uuid;

use crate::core::devices::DeviceStore;
use crate::engine::home_thread::ensure_home_thread;
use crate::engine::widgets::WidgetChange;
use crate::test_support::{setup_test_db, teardown_test_db};
use Dashboard::{Desktop, Phone};
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

fn legacy_tile(app_id: &str, size: WidgetSize) -> LegacyArrangedTile {
    LegacyArrangedTile {
        app_id: app_id.into(),
        params: None,
        size,
        hidden: false,
    }
}

fn hidden(app_id: &str, size: WidgetSize) -> LegacyArrangedTile {
    LegacyArrangedTile {
        hidden: true,
        ..legacy_tile(app_id, size)
    }
}

fn legacy_arranged(layout: Dashboard, tiles: Vec<LegacyArrangedTile>) -> ThreadEvent {
    ThreadEvent::HomeBoardArranged { layout, tiles }
}

fn tile(app_id: &str, size: WidgetSize) -> ArrangedTile {
    sized_tile(app_id, size.into())
}

fn sized_tile(app_id: &str, size: TileSize) -> ArrangedTile {
    ArrangedTile {
        app_id: app_id.into(),
        params: None,
        size,
    }
}

fn added(dashboard: Dashboard, app_id: &str) -> SystemEvent {
    SystemEvent::DashboardTileAdded {
        dashboard,
        app_id: app_id.into(),
        params: None,
        label: None,
        actor: None,
    }
}

fn removed(dashboard: Dashboard, app_id: &str) -> SystemEvent {
    SystemEvent::DashboardTileRemoved {
        dashboard,
        app_id: app_id.into(),
        params: None,
        actor: None,
    }
}

fn arranged(dashboard: Dashboard, tiles: Vec<ArrangedTile>) -> SystemEvent {
    SystemEvent::DashboardArranged {
        dashboard,
        tiles,
        actor: None,
    }
}

/// (app id, size) per tile of one dashboard, in order.
fn shape(dashboards: &FoldedDashboards, which: Dashboard) -> Vec<(String, Option<WidgetSize>)> {
    dashboards
        .tiles(which)
        .iter()
        .map(|t| (t.app_id.clone(), t.size.and_then(TileSize::widget_size)))
        .collect()
}

fn ids(dashboards: &FoldedDashboards, which: Dashboard) -> Vec<String> {
    shape(dashboards, which).into_iter().map(|t| t.0).collect()
}

/// Seed: Home's old pins are each dashboard's tiles at the default size, and
/// an old unpin removed the tile from both.
#[test]
fn the_seed_holds_homes_old_pins_on_both_dashboards() {
    let seeded = seed_dashboards(&[pin("diet"), pin("inbox"), pin("clock"), unpin("inbox")]);
    for which in Dashboard::ALL {
        assert_eq!(
            shape(&seeded, which),
            [("diet".into(), None), ("clock".into(), None)]
        );
    }
}

/// Seed: an old snapshot entry for an instance that was not pinned is
/// ignored, and the newest snapshot per layout wins, whole.
#[test]
fn the_seed_reads_the_newest_snapshot_and_ignores_unpinned_entries() {
    let seeded = seed_dashboards(&[
        pin("a"),
        pin("b"),
        legacy_arranged(
            Desktop,
            vec![legacy_tile("ghost", Small), legacy_tile("b", Large)],
        ),
        legacy_arranged(
            Desktop,
            vec![legacy_tile("a", Medium), legacy_tile("b", Small)],
        ),
    ]);
    assert_eq!(
        shape(&seeded, Desktop),
        [("a".into(), Some(Medium)), ("b".into(), Some(Small))]
    );
}

/// Invariant: after upgrade each dashboard shows what its layout showed. A
/// phone never arranged copies the desktop. A tile hidden on one layout
/// seeds only the other dashboard.
#[test]
fn each_dashboard_starts_with_what_its_layout_showed() {
    let desktop_only = [
        pin("a"),
        pin("b"),
        legacy_arranged(Desktop, vec![legacy_tile("b", Large), hidden("a", Small)]),
    ];
    let seeded = seed_dashboards(&desktop_only);
    assert_eq!(shape(&seeded, Desktop), [("b".into(), Some(Large))]);
    assert_eq!(
        shape(&seeded, Phone),
        shape(&seeded, Desktop),
        "an unarranged phone copied the desktop, hidden tile included"
    );

    let mut events = desktop_only.to_vec();
    events.push(legacy_arranged(
        Phone,
        vec![legacy_tile("a", Small), hidden("b", Medium)],
    ));
    events.push(pin("c"));
    let seeded = seed_dashboards(&events);
    assert_eq!(
        shape(&seeded, Phone),
        [("a".into(), Some(Small)), ("c".into(), None)]
    );
    assert_eq!(ids(&seeded, Desktop), ["b", "c"]);
}

/// Seed: an instance an old snapshot left out keeps its place after the named
/// ones, with its size. Pinned again after an unpin, it is a new tile.
#[test]
fn the_seed_keeps_omitted_tiles_and_repins_at_the_default_size() {
    let seeded = seed_dashboards(&[
        pin("a"),
        pin("b"),
        pin("c"),
        legacy_arranged(
            Desktop,
            vec![
                legacy_tile("a", Small),
                legacy_tile("b", Large),
                legacy_tile("c", Small),
            ],
        ),
        legacy_arranged(Desktop, vec![legacy_tile("c", Medium)]),
        unpin("a"),
        pin("a"),
    ]);
    assert_eq!(
        shape(&seeded, Desktop),
        [
            ("c".into(), Some(Medium)),
            ("b".into(), Some(Large)),
            ("a".into(), None),
        ]
    );
}

/// Invariant: old stored rows still read. A pin from before the board, a
/// legacy `WidgetRestored`, seeds a tile with its label.
#[test]
fn a_legacy_pin_row_seeds_a_tile_with_its_label() {
    let legacy = ThreadEvent::from_stored(
        "WidgetRestored",
        json!({"app_id": "fares", "label": "Fares to Oslo"}),
    )
    .unwrap();
    let stored_snapshot = ThreadEvent::from_stored(
        "HomeBoardArranged",
        json!({"layout": "phone", "tiles": [{"app_id": "fares", "size": "large"}]}),
    )
    .unwrap();
    let seeded = seed_dashboards(&[legacy, pin("diet"), stored_snapshot]);
    assert_eq!(ids(&seeded, Desktop), ["fares", "diet"]);
    assert_eq!(
        shape(&seeded, Phone),
        [("fares".into(), Some(Large)), ("diet".into(), None)]
    );
    assert_eq!(
        seeded.tiles(Phone)[0].label.as_deref(),
        Some("Fares to Oslo")
    );
}

/// Params are part of a tile's identity, in any key order.
#[test]
fn two_instances_of_one_widget_are_two_tiles() {
    let with = |clip: &str| SystemEvent::DashboardTileAdded {
        dashboard: Desktop,
        app_id: "player".into(),
        params: Some(serde_json::from_value(json!({ "clip": clip })).unwrap()),
        label: None,
        actor: None,
    };
    let folded = fold_dashboards(&[], &[with("a.mp3"), with("b.mp3"), with("a.mp3")]);
    assert_eq!(folded.tiles(Desktop).len(), 2);
}

/// Invariant: each dashboard's tiles are independent. Adding, removing or
/// arranging on one never changes the other.
#[test]
fn the_two_dashboards_change_independently() {
    let seed = [pin("shared")];
    let folded = fold_dashboards(
        &seed,
        &[
            added(Phone, "steps"),
            added(Desktop, "inbox"),
            removed(Phone, "shared"),
            arranged(Desktop, vec![tile("inbox", Large), tile("shared", Small)]),
        ],
    );
    assert_eq!(shape(&folded, Phone), [("steps".into(), None)]);
    assert_eq!(
        shape(&folded, Desktop),
        [
            ("inbox".into(), Some(Large)),
            ("shared".into(), Some(Small))
        ]
    );
}

/// An arrangement names only tiles on its dashboard. A tile added after a
/// remove is new, at the default size.
#[test]
fn an_arrangement_cannot_add_a_tile_and_a_re_add_is_new() {
    let folded = fold_dashboards(
        &[],
        &[
            added(Desktop, "a"),
            added(Desktop, "b"),
            arranged(
                Desktop,
                vec![tile("ghost", Small), tile("a", Large), tile("b", Small)],
            ),
            removed(Desktop, "a"),
            added(Desktop, "a"),
        ],
    );
    assert_eq!(
        shape(&folded, Desktop),
        [("b".into(), Some(Small)), ("a".into(), None)]
    );
}

/// A phone's user agent picks the phone dashboard; anything else, an iPad and
/// an Android tablet included, picks the desktop.
#[test]
fn a_user_agent_picks_its_dashboard() {
    let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) Safari/604.1";
    let android_phone =
        "Mozilla/5.0 (Linux; Android 15; Pixel 9) Chrome/130.0 Mobile Safari/537.36";
    let android_tablet = "Mozilla/5.0 (Linux; Android 15; Pixel Tablet) Chrome/130.0 Safari/537.36";
    let ipad = "Mozilla/5.0 (iPad; CPU OS 18_0 like Mac OS X) Safari/604.1";
    let mac = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) Safari/605.1.15";
    assert_eq!(Dashboard::for_user_agent(iphone), Phone);
    assert_eq!(Dashboard::for_user_agent(android_phone), Phone);
    for ua in [android_tablet, ipad, mac] {
        assert_eq!(Dashboard::for_user_agent(ua), Desktop, "{ua}");
    }
}

/// The widget id and size of a tile that must be a widget, pinned.
fn widget_tile(tile: &DashboardTile) -> (&str, WidgetSize) {
    match tile {
        DashboardTile::Widget { widget, size } => {
            assert!(widget.pinned);
            (widget.app_id.as_str(), *size)
        }
        DashboardTile::App(app) => panic!("'{}' is a shortcut, not a widget", app.app_id),
    }
}

fn write_widget(ws: &std::path::Path, id: &str, manifest: Value) {
    let dir = ws.join("data/apps").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
}

async fn emit(bus: &EventBus, thread_id: Uuid, event: ThreadEvent) {
    crate::test_support::seed_thread_event(bus, thread_id, event).await;
}

fn instance(app_id: &str) -> WidgetInstance {
    WidgetInstance {
        app_id: app_id.into(),
        ..Default::default()
    }
}

async fn add(
    pool: &PgPool,
    manager: &AppManager,
    bus: &EventBus,
    which: Dashboard,
    app_id: &str,
) -> bool {
    let plan = check_dashboard_change(
        pool,
        manager,
        DashboardChange::Add,
        which,
        &instance(app_id),
    )
    .await
    .unwrap();
    apply_dashboard_change(
        manager,
        bus,
        plan,
        DashboardChange::Add,
        which,
        instance(app_id),
        None,
    )
    .await
    .unwrap()
}

fn assert_refused<T: std::fmt::Debug>(r: Result<T, WidgetCheckFailed>, needle: &str) {
    match r {
        Err(WidgetCheckFailed::Refused(reason)) => {
            assert!(reason.contains(needle), "{needle}: {reason}")
        }
        other => panic!("expected a refusal naming {needle}, got {other:?}"),
    }
}

/// The read resolves each tile's size against the manifest: a new tile at the
/// first declared size, and an arranged size the widget no longer declares at
/// the nearest smaller one. A widget that no longer lists drops off.
#[tokio::test]
async fn the_read_resolves_sizes_against_the_manifest() {
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
    emit(&bus, home, pin("gone")).await;
    add(&pool, &manager, &bus, Desktop, "inbox").await;

    let view = dashboard(&pool, &manager, Desktop).await.unwrap();
    let sizes: Vec<(&str, WidgetSize)> = view.tiles.iter().map(widget_tile).collect();
    assert_eq!(sizes, [("clock", Small), ("inbox", Medium)]);

    let tiles = vec![tile("clock", Large), tile("inbox", Medium)];
    check_arrangement(&pool, &manager, Desktop, &tiles)
        .await
        .unwrap();
    emit_arrangement(&bus, Desktop, tiles, None).await.unwrap();
    write_widget(
        tmp.path(),
        "clock",
        json!({"name": "Clock", "kind": "widget", "reusable": true, "sizes": ["small", "medium"]}),
    );
    let view = dashboard(&pool, &manager, Desktop).await.unwrap();
    assert_eq!(
        widget_tile(&view.tiles[0]).1,
        Medium,
        "large fits down to medium"
    );
    let phone = dashboard(&pool, &manager, Phone).await.unwrap();
    assert_eq!(
        phone
            .tiles
            .iter()
            .map(|t| widget_tile(t).0)
            .collect::<Vec<_>>(),
        ["clock"],
        "the desktop's add stays on the desktop"
    );
    teardown_test_db(&db).await;
}

/// Invariant: a widget takes only sizes it declares, and an arrangement names
/// only its own dashboard's tiles, each once.
#[tokio::test]
async fn an_arrangement_refuses_an_undeclared_size_another_dashboards_tile_and_a_repeat() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let manager = AppManager::new(tmp.path()).unwrap();
    for (id, sizes) in [("clock", json!(["small"])), ("inbox", json!(["medium"]))] {
        write_widget(
            tmp.path(),
            id,
            json!({"name": id, "kind": "widget", "reusable": true, "sizes": sizes}),
        );
    }
    add(&pool, &manager, &bus, Phone, "clock").await;
    add(&pool, &manager, &bus, Desktop, "inbox").await;

    assert_refused(
        check_arrangement(&pool, &manager, Phone, &[tile("clock", Large)]).await,
        "does not take the size 'large'",
    );
    assert_refused(
        check_arrangement(&pool, &manager, Phone, &[tile("inbox", Medium)]).await,
        "not on the phone dashboard",
    );
    assert_refused(
        check_arrangement(
            &pool,
            &manager,
            Phone,
            &[tile("clock", Small), tile("clock", Small)],
        )
        .await,
        "named twice",
    );
    check_arrangement(&pool, &manager, Phone, &[tile("clock", Small)])
        .await
        .unwrap();
    teardown_test_db(&db).await;
}

/// Invariant: a tile names a reusable widget. Adding a thread's own widget
/// makes it reusable first, and a repeat is refused. Stop reusing is refused
/// while a dashboard holds it, and allowed once it is removed.
#[tokio::test]
async fn adding_a_threads_widget_makes_it_reusable_and_guards_stop_reusing() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let manager = AppManager::new(tmp.path()).unwrap();
    let origin = Uuid::new_v4();
    crate::test_support::seed_thread_event(
        &bus,
        origin,
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
    write_widget(
        tmp.path(),
        "fares",
        json!({"name": "Fares", "kind": "widget", "origin_thread_id": origin.to_string()}),
    );

    assert!(
        add(&pool, &manager, &bus, Phone, "fares").await,
        "made reusable"
    );
    assert!(manager.get_app("fares").unwrap().reusable);
    assert_refused(
        check_dashboard_change(
            &pool,
            &manager,
            DashboardChange::Add,
            Phone,
            &instance("fares"),
        )
        .await,
        "already on the phone dashboard",
    );
    assert_refused(
        widgets::check_set_reusable(&pool, &manager, "fares", false).await,
        "on the phone dashboard",
    );

    let plan = check_dashboard_change(
        &pool,
        &manager,
        DashboardChange::Remove,
        Phone,
        &instance("fares"),
    )
    .await
    .unwrap();
    apply_dashboard_change(
        &manager,
        &bus,
        plan,
        DashboardChange::Remove,
        Phone,
        instance("fares"),
        None,
    )
    .await
    .unwrap();
    widgets::check_set_reusable(&pool, &manager, "fares", false)
        .await
        .unwrap();
    assert_refused(
        check_dashboard_change(
            &pool,
            &manager,
            DashboardChange::Remove,
            Phone,
            &instance("fares"),
        )
        .await,
        "not on the phone dashboard",
    );
    teardown_test_db(&db).await;
}

/// Invariant: nothing writes a legacy board event. A pin or an unpin on the
/// Home thread is refused, so Home's old pins stay the seed. The dashboard
/// events land on no thread, so Home starts no turn.
#[tokio::test]
async fn the_home_thread_takes_no_pins_and_dashboard_events_land_on_no_thread() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let manager = AppManager::new(tmp.path()).unwrap();
    let home = ensure_home_thread(&bus, &pool).await.unwrap();
    write_widget(
        tmp.path(),
        "clock",
        json!({"name": "Clock", "kind": "widget", "reusable": true}),
    );
    emit(&bus, home, pin("clock")).await;

    for change in [
        WidgetChange::Pin {
            shown_event_id: None,
        },
        WidgetChange::Unpin,
    ] {
        assert_refused(
            widgets::check_widget_change(&pool, &manager, change, &instance("clock"), home).await,
            "Home has no shelf",
        );
    }

    let plan = check_dashboard_change(
        &pool,
        &manager,
        DashboardChange::Remove,
        Desktop,
        &instance("clock"),
    )
    .await
    .unwrap();
    apply_dashboard_change(
        &manager,
        &bus,
        plan,
        DashboardChange::Remove,
        Desktop,
        instance("clock"),
        None,
    )
    .await
    .unwrap();
    let types: Vec<String> =
        sqlx::query_scalar("SELECT event_type FROM events WHERE thread_id = $1 ORDER BY sequence")
            .bind(home)
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        types,
        ["HomeThreadCreated", "WidgetPinned"],
        "no turn started"
    );
    let folded = folded_dashboards(&pool).await.unwrap();
    assert!(folded.tiles(Desktop).is_empty());
    assert_eq!(ids(&folded, Phone), ["clock"], "the seed is untouched");
    teardown_test_db(&db).await;
}

/// The agent's default: a named dashboard wins, else the device's, else a
/// refusal that names the choice.
#[tokio::test]
async fn an_unnamed_dashboard_is_the_devices() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    DeviceStore::register(
        &pool,
        &bus,
        "phone-1",
        Some("Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) Safari/604.1"),
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        resolve_dashboard(&pool, Some("desktop"), Some("phone-1")).await,
        Ok(Desktop)
    );
    assert_eq!(
        resolve_dashboard(&pool, None, Some("phone-1")).await,
        Ok(Phone)
    );
    let refused = resolve_dashboard(&pool, None, None).await.unwrap_err();
    assert!(refused.contains("phone or desktop"), "{refused}");
    assert!(resolve_dashboard(&pool, Some("tablet"), None)
        .await
        .is_err());
    teardown_test_db(&db).await;
}

fn write_app(ws: &std::path::Path, id: &str) {
    write_widget(ws, id, json!({"name": id}));
}

fn app_tile(tile: &DashboardTile) -> &AppShortcut {
    match tile {
        DashboardTile::App(app) => app,
        DashboardTile::Widget { widget, .. } => {
            panic!("'{}' is a widget, not a shortcut", widget.app_id)
        }
    }
}

/// Invariants: an app joins a dashboard as a quarter shortcut, and its kind
/// is read from the manifest at read time. The same add reads as a widget
/// tile once the manifest says widget, with no event between.
#[tokio::test]
async fn an_app_is_a_quarter_shortcut_and_its_kind_comes_from_the_manifest() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let manager = AppManager::new(tmp.path()).unwrap();
    write_app(tmp.path(), "notes");
    assert!(
        !add(&pool, &manager, &bus, Desktop, "notes").await,
        "an app is never made reusable"
    );

    let view = dashboard(&pool, &manager, Desktop).await.unwrap();
    let shortcut = app_tile(&view.tiles[0]);
    assert_eq!(
        (shortcut.app_id.as_str(), shortcut.name.as_str()),
        ("notes", "notes")
    );
    assert_eq!(
        serde_json::to_value(&view.tiles[0]).unwrap(),
        json!({"kind": "app", "app_id": "notes", "name": "notes", "size": "quarter"})
    );

    write_widget(
        tmp.path(),
        "notes",
        json!({"name": "Notes", "kind": "widget", "reusable": true}),
    );
    let view = dashboard(&pool, &manager, Desktop).await.unwrap();
    assert_eq!(widget_tile(&view.tiles[0]), ("notes", Medium));
    let wire = serde_json::to_value(&view.tiles[0]).unwrap();
    assert_eq!(wire["kind"], "widget");
    assert_eq!(wire["size"], "medium");

    let kinds: Vec<String> = sqlx::query_scalar(
        "SELECT payload::text FROM events WHERE aggregate = 'dashboard' ORDER BY sequence",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(kinds.iter().all(|payload| !payload.contains("\"kind\"")));
    teardown_test_db(&db).await;
}

/// A widget's parameterized instances name no shortcut once its manifest says
/// app. Each would draw as the same tile, and no remove could name it.
#[tokio::test]
async fn a_widget_instance_with_params_drops_off_once_the_manifest_says_app() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let manager = AppManager::new(tmp.path()).unwrap();
    write_widget(
        tmp.path(),
        "player",
        json!({"name": "Player", "kind": "widget", "reusable": true}),
    );
    for clip in ["a.mp3", "b.mp3"] {
        let params = WidgetParams::from([("clip".to_string(), json!(clip))]);
        bus.emit(BusEvent::System(SystemEvent::DashboardTileAdded {
            dashboard: Desktop,
            app_id: "player".into(),
            params: Some(params),
            label: None,
            actor: None,
        }))
        .await
        .unwrap();
    }
    assert_eq!(
        dashboard(&pool, &manager, Desktop)
            .await
            .unwrap()
            .tiles
            .len(),
        2
    );

    write_app(tmp.path(), "player");
    assert!(dashboard(&pool, &manager, Desktop)
        .await
        .unwrap()
        .tiles
        .is_empty());
    teardown_test_db(&db).await;
}

/// Invariant: a shortcut is always a quarter and a widget never one. A
/// stored size on an app tile reads back as a quarter all the same.
#[tokio::test]
async fn only_an_app_takes_the_quarter_size() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let manager = AppManager::new(tmp.path()).unwrap();
    write_app(tmp.path(), "notes");
    write_widget(
        tmp.path(),
        "clock",
        json!({"name": "Clock", "kind": "widget", "reusable": true}),
    );
    add(&pool, &manager, &bus, Desktop, "notes").await;
    add(&pool, &manager, &bus, Desktop, "clock").await;

    assert_refused(
        check_arrangement(&pool, &manager, Desktop, &[tile("notes", Small)]).await,
        "'notes' does not take the size 'small'. It takes: quarter",
    );
    assert_refused(
        check_arrangement(
            &pool,
            &manager,
            Desktop,
            &[sized_tile("clock", TileSize::Quarter)],
        )
        .await,
        "'clock' does not take the size 'quarter'",
    );
    let tiles = vec![sized_tile("notes", TileSize::Quarter), tile("clock", Large)];
    check_arrangement(&pool, &manager, Desktop, &tiles)
        .await
        .unwrap();

    emit_arrangement(&bus, Desktop, vec![tile("notes", Large)], None)
        .await
        .unwrap();
    let view = dashboard(&pool, &manager, Desktop).await.unwrap();
    assert_eq!(
        serde_json::to_value(&view.tiles[0]).unwrap()["size"],
        "quarter",
        "a stored size on an app reads as a quarter"
    );
    teardown_test_db(&db).await;
}

/// Invariant: every add path refuses the same things. An app takes no
/// params, an unknown id is refused, and a repeat is refused. A tile whose
/// app is deleted drops off the view.
#[tokio::test]
async fn an_app_add_refuses_params_an_unknown_id_and_a_repeat_and_a_deleted_app_drops_off() {
    let (pool, db) = setup_test_db().await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let tmp = tempfile::tempdir().unwrap();
    let manager = AppManager::new(tmp.path()).unwrap();
    write_app(tmp.path(), "notes");

    let with_params = WidgetInstance {
        params: Some([("tab".to_string(), json!("a"))].into()),
        ..instance("notes")
    };
    assert_refused(
        check_dashboard_change(&pool, &manager, DashboardChange::Add, Desktop, &with_params).await,
        "takes no params",
    );
    assert_refused(
        check_dashboard_change(
            &pool,
            &manager,
            DashboardChange::Add,
            Desktop,
            &instance("missing"),
        )
        .await,
        "No widget or app 'missing' exists",
    );

    add(&pool, &manager, &bus, Desktop, "notes").await;
    assert_refused(
        check_dashboard_change(
            &pool,
            &manager,
            DashboardChange::Add,
            Desktop,
            &instance("notes"),
        )
        .await,
        "App 'notes' with these params is already on the desktop dashboard",
    );

    std::fs::remove_dir_all(tmp.path().join("data/apps/notes")).unwrap();
    assert!(dashboard(&pool, &manager, Desktop)
        .await
        .unwrap()
        .tiles
        .is_empty());
    teardown_test_db(&db).await;
}

/// Invariant: a `DashboardArranged` written before app shortcuts reads back
/// unchanged, since the new size type is a superset of the old one.
#[test]
fn an_old_arranged_tile_reads_into_the_tile_size() {
    for (name, size) in [("small", Small), ("medium", Medium), ("large", Large)] {
        let tile: ArrangedTile =
            serde_json::from_value(json!({"app_id": "clock", "size": name})).unwrap();
        assert_eq!(tile.size, TileSize::from(size));
    }
    assert_eq!(TileSize::Quarter.widget_size(), None);
}
