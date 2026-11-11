//! The widget fit check: what it renders, where, and what the agent reads.

use super::*;
use crate::core::apps::WidgetSize::{Large, Medium, Small, Xl};
use crate::core::DevicePresenceStore;
use crate::engine::event_bus::EmittedEvent;
use crate::engine::pending_answers::{resolve_answer, AnswerRefusal};
use crate::engine::thread_events::MessageOrigin;
use crate::test_support::{
    seed_device, seed_preference, seed_preference_for_device, setup_test_db, teardown_test_db,
};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::broadcast::Receiver;

const PHONE: &str = "device-phone";
const LAPTOP: &str = "device-laptop";
const APP: &str = "trip";

fn measured(
    surface: FitSurface,
    size: WidgetSize,
    findings: Vec<FitFinding>,
    total: usize,
) -> FitBoxReport {
    FitBoxReport {
        surface,
        size,
        outcome: FitOutcome::Measured {
            frame: FrameSize {
                width: 320.0,
                height: 320.4,
            },
            findings,
            total,
        },
    }
}

fn clean(sizes: &[WidgetSize]) -> Vec<FitBoxReport> {
    FitSurface::ALL
        .into_iter()
        .flat_map(|surface| {
            sizes
                .iter()
                .map(move |&size| measured(surface, size, vec![], 0))
        })
        .collect()
}

fn ellipsis(text: &str) -> FitFinding {
    FitFinding {
        kind: FitKind::Ellipsis,
        edge: FitEdge::Right,
        px: 41.6,
        path: "div.flight".into(),
        text: text.into(),
    }
}

fn findings_of(note: &str) -> Vec<&str> {
    note.lines().filter(|line| line.starts_with("- ")).collect()
}

// ---- The size list ----

#[test]
fn checks_the_declared_sizes_smallest_first() {
    assert_eq!(fit_sizes(&[Large, Small]), vec![Small, Large]);
    assert_eq!(fit_sizes(&[Xl, Medium]), vec![Medium, Xl]);
}

#[test]
fn checks_the_undeclared_sizes_for_a_widget_that_declares_none() {
    assert_eq!(
        fit_sizes(&WidgetSize::UNDECLARED),
        vec![Small, Medium, Large]
    );
}

// ---- The scale ----

#[test]
fn renders_at_the_workspace_scale_when_no_device_has_its_own() {
    assert_eq!(
        largest_scale(112.5, &[]),
        FitScale {
            percent: 112.5,
            device_id: None
        }
    );
}

#[test]
fn renders_at_a_device_scale_above_the_workspace_one() {
    let devices = [(PHONE.into(), "125".into()), (LAPTOP.into(), "100".into())];
    assert_eq!(
        largest_scale(112.5, &devices),
        FitScale {
            percent: 125.0,
            device_id: Some(PHONE.into())
        }
    );
}

#[test]
fn keeps_the_workspace_scale_over_device_scales_below_it() {
    let devices = [(PHONE.into(), "100".into())];
    assert_eq!(largest_scale(150.0, &devices).percent, 150.0);
}

/// The pre-grid names paint what the SDK paints for them.
#[test]
fn reads_a_pre_grid_scale_name_as_the_sdk_does() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../packages/lucidos-sdk/src/appearance.ts");
    let sdk = std::fs::read_to_string(&path).expect("the SDK's appearance.ts");
    let table = sdk
        .split("export const LEGACY_UI_SCALES: Record<string, number> = {")
        .nth(1)
        .and_then(|rest| rest.split("};").next())
        .expect("LEGACY_UI_SCALES is declared in appearance.ts");
    let names: Vec<(&str, f64)> = table
        .lines()
        .filter_map(|line| line.trim().trim_end_matches(',').split_once(": "))
        .map(|(name, value)| (name, value.parse().unwrap()))
        .collect();
    assert_eq!(names.len(), 3, "{table}");
    for (name, percent) in names {
        assert_eq!(parse_scale(name), Some(percent), "{name}");
    }
}

#[test]
fn skips_a_device_value_that_is_no_scale() {
    let devices = [(PHONE.into(), "huge".into()), (LAPTOP.into(), "900".into())];
    assert_eq!(largest_scale(100.0, &devices).device_id, None);
}

// ---- The note ----

#[test]
fn a_pass_names_every_size_both_surfaces_and_the_scale() {
    let sizes = [Small, Medium, Large];
    let note = fit_note(APP, &sizes, 125.0, &[], &clean(&sizes));
    assert_eq!(
        note,
        "[WIDGET FIT] trip: fits small, medium and large on the desk tile and the phone \
         column, at 125% UI scale."
    );
}

/// Invariant: an unknown is never a pass. Without the params it requires,
/// the check measured a face the widget never shows a user.
#[test]
fn a_widget_that_requires_a_param_never_reads_as_a_plain_pass() {
    let sizes = [Small, Large];
    let note = fit_note(APP, &sizes, 100.0, &["clip"], &clean(&sizes));
    assert_eq!(
        note,
        "[WIDGET FIT] trip: not checked with params: it requires clip, so the check drew \
         its face without them. That face fits small and large on the desk tile and the \
         phone column, at 100% UI scale."
    );

    let mut boxes = clean(&sizes);
    boxes[0] = measured(
        FitSurface::DeskTile,
        Small,
        vec![ellipsis("No clip given")],
        1,
    );
    let note = fit_note(APP, &sizes, 100.0, &["clip", "start"], &boxes);
    let second = note.lines().nth(1).unwrap_or_default();
    assert_eq!(
        second, "It requires clip and start, so the check drew its face without them.",
        "{note}"
    );
}

#[test]
fn a_finding_names_its_size_surface_frame_and_text() {
    let sizes = [Small, Large];
    let mut boxes = clean(&sizes);
    boxes[3] = measured(
        FitSurface::PhoneColumn,
        Large,
        vec![ellipsis("SK 1467 Oslo Gardermoen to Stockholm Arlanda")],
        3,
    );
    let note = fit_note(APP, &sizes, 125.0, &[], &boxes);
    assert!(
        note.starts_with(
            "[WIDGET FIT] trip: content is cut off in 1 of 4 boxes, at 125% UI scale."
        ),
        "{note}"
    );
    assert_eq!(
        findings_of(&note),
        vec![
            "- large, phone column (320x320 px): an ellipsis cuts \"SK 1467 Oslo Gardermoen to \
             Stockholm Arlanda\" (div.flight) by 42 px",
            "- large, phone column: 2 more cut off",
        ]
    );
    assert!(note.ends_with(BOX_RULES), "{note}");
}

#[test]
fn each_kind_of_cut_reads_as_what_happened() {
    let finding = |kind, text: &str| FitFinding {
        kind,
        edge: FitEdge::Bottom,
        px: 12.0,
        path: "p".into(),
        text: text.into(),
    };
    assert_eq!(
        finding_line(&finding(FitKind::PastEdge, "Gate C12")),
        "\"Gate C12\" (p) runs 12 px past the bottom edge"
    );
    assert_eq!(
        finding_line(&finding(FitKind::Clipped, "Gate C12")),
        "\"Gate C12\" (p) is clipped 12 px at its bottom edge by an overflow: hidden ancestor"
    );
    assert_eq!(
        finding_line(&finding(FitKind::Media, "")),
        "media (p) runs 12 px past the bottom edge"
    );
}

#[test]
fn a_quote_keeps_whole_characters() {
    let long = "å".repeat(QUOTE_CHARS + 10);
    let line = finding_line(&ellipsis(&long));
    assert!(
        line.contains(&format!("\"{}\"", "å".repeat(QUOTE_CHARS))),
        "{line}"
    );
}

/// Invariant: an unknown is never a pass.
#[test]
fn a_box_the_page_did_not_report_is_never_a_pass() {
    let sizes = [Small, Medium];
    let mut boxes = clean(&sizes);
    boxes.pop();
    let note = fit_note(APP, &sizes, 100.0, &[], &boxes);
    assert!(
        note.starts_with("[WIDGET FIT] trip: not checked: 1 of 4 boxes could not be measured"),
        "{note}"
    );
    assert_eq!(
        findings_of(&note),
        vec!["- medium, phone column: not checked: the page sent no report"]
    );
}

#[test]
fn a_box_that_failed_to_load_is_not_checked() {
    let sizes = [Small];
    let mut boxes = clean(&sizes);
    boxes[0].outcome = FitOutcome::Failed {
        error: "the widget did not answer".into(),
    };
    let note = fit_note(APP, &sizes, 100.0, &[], &boxes);
    assert!(note.contains("not checked"), "{note}");
    assert!(
        findings_of(&note).contains(&"- small, desk tile: not checked: the widget did not answer")
    );
}

#[test]
fn a_fractional_scale_reads_as_it_is() {
    let note = fit_note(APP, &[Small], 112.5, &[], &clean(&[Small]));
    assert!(note.contains("at 112.5% UI scale"), "{note}");
}

#[test]
fn a_long_note_counts_the_lines_it_leaves_out() {
    let sizes = [Small, Medium, Large, Xl];
    let boxes: Vec<FitBoxReport> = FitSurface::ALL
        .into_iter()
        .flat_map(|surface| {
            sizes
                .iter()
                .map(move |&size| measured(surface, size, vec![ellipsis("a"), ellipsis("b")], 2))
        })
        .collect();
    let lines = findings_of(&fit_note(APP, &sizes, 100.0, &[], &boxes)).len();
    assert_eq!(lines, MAX_NOTE_LINES);
    assert!(fit_note(APP, &sizes, 100.0, &[], &boxes).contains("- 5 more lines not shown"));
}

/// Pins the page's wire shape (`widgetFitCheck.ts`).
#[test]
fn reads_the_report_a_page_posts() {
    let boxes: Vec<FitBoxReport> = serde_json::from_value(json!([
        { "surface": "phone-column", "size": "large", "box": { "width": 320, "height": 320 },
          "findings": [{ "kind": "ellipsis", "edge": "right", "px": 42, "path": "div", "text": "SK" }],
          "total": 1 },
        { "surface": "desk-tile", "size": "small", "error": "the widget did not answer" },
    ]))
    .unwrap();
    assert!(matches!(
        &boxes[0].outcome,
        FitOutcome::Measured { total: 1, .. }
    ));
    assert!(
        matches!(&boxes[1].outcome, FitOutcome::Failed { error } if error == "the widget did not answer")
    );
}

// ---- The round trip ----

struct Bench {
    bus: Arc<EventBus>,
    events: Receiver<EmittedEvent>,
    pool: PgPool,
    db: String,
    pending: Arc<PendingWidgetFits>,
    workspace: Arc<tempfile::TempDir>,
}

async fn bench() -> Bench {
    let (pool, db) = setup_test_db().await;
    seed_device(&pool, PHONE, Some("iPhone Safari/604.1"), Some("My iPhone")).await;
    seed_device(&pool, LAPTOP, Some("Macintosh Chrome"), Some("My MacBook")).await;
    let (bus, _rx) = EventBus::new(pool.clone());
    let events = bus.subscribe();
    let workspace = tempfile::tempdir().unwrap();
    write_app(
        workspace.path(),
        APP,
        json!({ "name": "Trip", "kind": "widget", "sizes": ["large", "small"] }),
    );
    write_app(workspace.path(), "notes", json!({ "name": "Notes" }));
    Bench {
        bus: Arc::new(bus),
        events,
        pool,
        db,
        pending: Arc::default(),
        workspace: Arc::new(workspace),
    }
}

fn write_app(ws: &std::path::Path, id: &str, manifest: serde_json::Value) {
    let dir = ws.join("data/apps").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("manifest.json"), manifest.to_string()).unwrap();
}

async fn visible(pool: &PgPool, device: &str) {
    DevicePresenceStore::record_visible(pool, device)
        .await
        .unwrap();
}

/// Run the check on its own task, as a tool call would, so the test can answer.
fn start(
    b: &Bench,
    app_id: &'static str,
    preferred: Option<&'static str>,
    box_budget: Duration,
) -> tokio::task::JoinHandle<String> {
    let (bus, pool, pending, workspace) = (
        b.bus.clone(),
        b.pool.clone(),
        b.pending.clone(),
        b.workspace.clone(),
    );
    tokio::spawn(async move {
        let apps = AppManager::new(workspace.path()).unwrap();
        let cx = WidgetFitContext {
            bus: &bus,
            pool: &pool,
            apps: &apps,
            pending: &pending,
        };
        check_widget_fit(&cx, app_id, Uuid::nil(), preferred, box_budget).await
    })
}

/// The next `WidgetFitRequested`: its request id, device, sizes and scale.
async fn next_request(
    rx: &mut Receiver<EmittedEvent>,
) -> (String, Option<String>, Vec<WidgetSize>, f64, Option<String>) {
    loop {
        let ev = rx.recv().await.expect("broadcast channel should not close");
        if let BusEvent::Thread {
            event:
                ThreadEvent::WidgetFitRequested {
                    request_id,
                    sizes,
                    ui_scale,
                    scale_device_id,
                    ..
                },
            meta,
            ..
        } = ev.typed
        {
            let device = match meta.actor {
                Some(MessageOrigin::Device { device_id }) => Some(device_id),
                _ => None,
            };
            return (request_id, device, sizes, ui_scale, scale_device_id);
        }
    }
}

fn no_request_sent(rx: &mut Receiver<EmittedEvent>) -> bool {
    std::iter::from_fn(|| rx.try_recv().ok()).all(|ev| {
        !matches!(
            ev.typed,
            BusEvent::Thread {
                event: ThreadEvent::WidgetFitRequested { .. },
                ..
            }
        )
    })
}

/// Invariant: every size on both surfaces, at the largest scale any device uses.
#[tokio::test]
async fn resolves_the_largest_scale_among_registered_devices() {
    let b = bench().await;
    seed_preference(&b.pool, prefs::UI_SCALE.key(), "112.5")
        .await
        .unwrap();
    seed_preference_for_device(&b.pool, prefs::UI_SCALE.key(), "125", PHONE)
        .await
        .unwrap();
    seed_preference_for_device(&b.pool, prefs::UI_SCALE.key(), "100", LAPTOP)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO preferences (key, value, device_id) VALUES ($1, '200', 'device-gone')",
    )
    .bind(prefs::UI_SCALE.key())
    .execute(&b.pool)
    .await
    .unwrap();
    assert_eq!(
        resolve_fit_scale(&b.pool).await.unwrap(),
        FitScale {
            percent: 125.0,
            device_id: Some(PHONE.into())
        }
    );
    teardown_test_db(&b.db).await;
}

/// A device whose scale row predates `ui-scale` paints at it, so the check
/// renders at it too. Its rows sit over the workspace's, as the shell reads.
#[tokio::test]
async fn reads_a_device_scale_stored_under_a_pre_grid_key() {
    let b = bench().await;
    seed_preference_for_device(&b.pool, "text-size", "large", PHONE)
        .await
        .unwrap();
    seed_preference_for_device(&b.pool, "font-size", "112.5", LAPTOP)
        .await
        .unwrap();
    assert_eq!(
        resolve_fit_scale(&b.pool).await.unwrap(),
        FitScale {
            percent: 125.0,
            device_id: Some(PHONE.into())
        }
    );

    seed_preference(&b.pool, prefs::UI_SCALE.key(), "100")
        .await
        .unwrap();
    assert_eq!(
        resolve_fit_scale(&b.pool).await.unwrap(),
        FitScale {
            percent: 100.0,
            device_id: None
        },
        "the shell reads `ui-scale` before an older key, wherever it sits"
    );
    teardown_test_db(&b.db).await;
}

/// Invariant: an unknown is never a pass, and nothing waits for nobody.
#[tokio::test]
async fn with_no_visible_window_it_says_not_checked_and_sends_nothing() {
    let mut b = bench().await;
    let note = start(&b, APP, Some(PHONE), WIDGET_FIT_BOX_BUDGET)
        .await
        .unwrap();
    assert_eq!(
        note,
        format!(
            "[WIDGET FIT] trip: not checked: no Lucidos window is open to render it.\n{BOX_RULES}"
        )
    );
    assert!(no_request_sent(&mut b.events));
    teardown_test_db(&b.db).await;
}

/// Invariant: only the routed device's page answers, and the engine takes only that answer.
#[tokio::test]
async fn takes_only_the_routed_device_answer() {
    let mut b = bench().await;
    visible(&b.pool, PHONE).await;
    seed_preference_for_device(&b.pool, prefs::UI_SCALE.key(), "125", PHONE)
        .await
        .unwrap();
    let check = start(&b, APP, Some(PHONE), WIDGET_FIT_BOX_BUDGET);
    let (request_id, device, sizes, scale, scale_device) = next_request(&mut b.events).await;
    assert_eq!(device.as_deref(), Some(PHONE));
    assert_eq!(sizes, vec![Small, Large]);
    assert_eq!((scale, scale_device.as_deref()), (125.0, Some(PHONE)));

    assert_eq!(
        resolve_answer(&b.pending, &request_id, Some(LAPTOP), vec![]),
        Err(AnswerRefusal::OtherDevice)
    );
    resolve_answer(&b.pending, &request_id, Some(PHONE), clean(&sizes)).unwrap();
    assert_eq!(
        check.await.unwrap(),
        "[WIDGET FIT] trip: fits small and large on the desk tile and the phone column, at 125% UI scale."
    );
    teardown_test_db(&b.db).await;
}

#[tokio::test]
async fn renders_on_a_visible_window_when_the_preferred_device_has_none() {
    let mut b = bench().await;
    visible(&b.pool, LAPTOP).await;
    let check = start(&b, APP, Some(PHONE), WIDGET_FIT_BOX_BUDGET);
    let (request_id, device, sizes, ..) = next_request(&mut b.events).await;
    assert_eq!(device.as_deref(), Some(LAPTOP));
    resolve_answer(&b.pending, &request_id, Some(LAPTOP), clean(&sizes)).unwrap();
    assert!(check.await.unwrap().contains(": fits "));
    teardown_test_db(&b.db).await;
}

/// Invariant: the engine outwaits the page. Each surface walks its sizes in
/// turn, so four sizes need four budgets before the round trip's.
#[test]
fn the_wait_gives_each_declared_size_a_budget_plus_the_round_trip() {
    let budget = WIDGET_FIT_BOX_BUDGET;
    assert_eq!(fit_wait(budget, 1), budget * 2);
    assert_eq!(fit_wait(budget, WidgetSize::ALL.len()), budget * 5);
}

/// Invariant: a check is bounded, and a silent page is not checked.
#[tokio::test]
async fn a_page_that_never_answers_is_not_checked_at_the_timeout() {
    let mut b = bench().await;
    visible(&b.pool, PHONE).await;
    let check = start(&b, APP, Some(PHONE), Duration::from_millis(200));
    let (request_id, ..) = next_request(&mut b.events).await;
    let note = check.await.unwrap();
    assert!(
        note.starts_with("[WIDGET FIT] trip: not checked: the Lucidos window on \"My iPhone\" did not answer within 0 s."),
        "{note}"
    );
    assert_eq!(
        resolve_answer(&b.pending, &request_id, Some(PHONE), vec![]),
        Err(AnswerRefusal::Unknown),
        "a timed-out check takes no late answer"
    );
    teardown_test_db(&b.db).await;
}

#[tokio::test]
async fn a_plain_app_is_not_checked() {
    let mut b = bench().await;
    visible(&b.pool, PHONE).await;
    let note = start(&b, "notes", Some(PHONE), WIDGET_FIT_BOX_BUDGET)
        .await
        .unwrap();
    assert!(
        note.starts_with("[WIDGET FIT] notes: not checked: it is not a widget."),
        "{note}"
    );
    assert!(no_request_sent(&mut b.events));
    teardown_test_db(&b.db).await;
}

// ---- What a write checks ----

fn bench_apps(b: &Bench) -> AppManager {
    AppManager::new(b.workspace.path()).unwrap()
}

/// Invariant: only widget apps, only their own files, once per widget per call.
#[tokio::test]
async fn a_write_checks_each_widget_it_touches_once() {
    let b = bench().await;
    let apps = bench_apps(&b);
    let written = [
        "apps/trip/index.html",
        "apps/trip/style.css",
        "apps/trip/manifest.json",
        "apps/trip/widget.js",
    ];
    assert_eq!(written_widgets(&apps, written), vec![APP.to_string()]);
    teardown_test_db(&b.db).await;
}

#[tokio::test]
async fn a_write_outside_a_widgets_own_files_checks_nothing() {
    let b = bench().await;
    let apps = bench_apps(&b);
    for path in [
        "apps/notes/index.html",
        "apps/notes/manifest.json",
        "artifacts/x.html",
        "apps/trip/scripts/fetch.py",
        "apps/trip/data.json",
        "apps/trip/sub/manifest.json",
        "apps/missing/index.html",
    ] {
        assert!(written_widgets(&apps, [path]).is_empty(), "{path}");
    }
    teardown_test_db(&b.db).await;
}

/// Invariant: the check never refuses a write. A check that cannot run is a
/// note, one per widget, after a blank line.
#[tokio::test]
async fn a_check_that_cannot_run_is_one_not_checked_note() {
    let mut b = bench().await;
    let apps = bench_apps(&b);
    let cx = WidgetFitContext {
        bus: &b.bus,
        pool: &b.pool,
        apps: &apps,
        pending: &b.pending,
    };
    let written = [
        "apps/trip/index.html",
        "apps/trip/style.css",
        "apps/notes/index.html",
    ];
    let widgets = written_widgets(&apps, written);
    let notes = widget_fit_notes(&cx, &widgets, Uuid::nil(), None, WIDGET_FIT_BOX_BUDGET).await;
    assert_eq!(
        notes,
        format!(
            "\n\n[WIDGET FIT] trip: not checked: no Lucidos window is open to render it.\n{BOX_RULES}"
        )
    );
    assert!(no_request_sent(&mut b.events));
    teardown_test_db(&b.db).await;
}

#[tokio::test]
async fn a_write_with_no_widget_file_adds_no_note() {
    let mut b = bench().await;
    visible(&b.pool, PHONE).await;
    let apps = bench_apps(&b);
    let cx = WidgetFitContext {
        bus: &b.bus,
        pool: &b.pool,
        apps: &apps,
        pending: &b.pending,
    };
    let widgets = written_widgets(&apps, ["apps/notes/index.html", "artifacts/x.html"]);
    let notes = widget_fit_notes(
        &cx,
        &widgets,
        Uuid::nil(),
        Some(PHONE),
        WIDGET_FIT_BOX_BUDGET,
    )
    .await;
    assert_eq!(notes, "");
    assert!(no_request_sent(&mut b.events));
    teardown_test_db(&b.db).await;
}
