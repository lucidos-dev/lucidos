//! A thread's widgets, its shelf, and the verbs that change them (ADRs 0402,
//! 0407).
//!
//! A thread's widgets are derived on read from its widget events plus each
//! widget's app manifest. No column caches them, so a reload, another device
//! or a compaction cannot lose them. Each verb checks first and then writes:
//! show, pin and unpin write one thread event and touch no file.

use std::sync::LazyLock;

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::{App, AppManager, AppReveal};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// One widget shown in a thread. Only a pinned one has a chip on the shelf;
/// the transcript's card reads every one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThreadWidget {
    pub app_id: String,
    pub name: String,
    /// The validated `App::icon`, a path inside the widget's folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub reusable: bool,
    /// When the frame's cover lifts, from the manifest, as for any app.
    pub reveal: AppReveal,
    /// Pinned to the shelf, so it has a chip there. The thread's card shows
    /// either way.
    pub pinned: bool,
    /// The newest `WidgetShown` for this widget in this thread: where "Show
    /// in thread" scrolls to.
    pub shown_event_id: Uuid,
}

/// A widget event as the fold reads it, in thread order.
#[derive(Debug, Clone)]
pub struct WidgetEventRow {
    pub event_id: Uuid,
    pub event_type: String,
    pub app_id: String,
}

/// What the events alone say about one widget shown in a thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoldedWidget {
    pub app_id: String,
    pub pinned: bool,
    pub shown_event_id: Uuid,
}

/// Fold a thread's widget events into its shown widgets, ordered by first
/// showing. Only a pin puts a widget on the shelf and only an unpin takes it
/// off; showing again changes neither. A pin or unpin before any showing is
/// ignored.
pub fn fold_thread_widgets(rows: &[WidgetEventRow]) -> Vec<FoldedWidget> {
    let mut widgets: Vec<FoldedWidget> = Vec::new();
    for row in rows {
        let Some(change) = WidgetChange::from_event_type(&row.event_type) else {
            continue;
        };
        let existing = widgets.iter_mut().find(|w| w.app_id == row.app_id);
        match (change, existing) {
            (WidgetChange::Show, Some(widget)) => widget.shown_event_id = row.event_id,
            (WidgetChange::Show, None) => widgets.push(FoldedWidget {
                app_id: row.app_id.clone(),
                pinned: false,
                shown_event_id: row.event_id,
            }),
            (WidgetChange::Pin, Some(widget)) => widget.pinned = true,
            (WidgetChange::Unpin, Some(widget)) => widget.pinned = false,
            (_, None) => {}
        }
    }
    widgets
}

async fn widget_event_rows(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Vec<WidgetEventRow>, BoxError> {
    let rows: Vec<(Uuid, String, Option<String>)> = sqlx::query_as(
        "SELECT id, event_type, payload->>'app_id' FROM events \
         WHERE thread_id = $1 AND event_type = ANY($2) ORDER BY sequence",
    )
    .bind(thread_id)
    .bind(STORED_WIDGET_EVENT_TYPES.as_slice())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(event_id, event_type, app_id)| {
            Some(WidgetEventRow {
                event_id,
                event_type,
                app_id: app_id?,
            })
        })
        .collect())
}

/// Every widget shown in a thread, unpinned ones included. A widget whose
/// folder is gone drops off: there is nothing left to open. One whose manifest
/// does not parse drops off as it does from the apps list. Any other read
/// failure fails the read, since it says nothing about whether the widget
/// exists.
pub async fn thread_widgets(
    pool: &PgPool,
    app_manager: &AppManager,
    thread_id: Uuid,
) -> Result<Vec<ThreadWidget>, BoxError> {
    let folded = fold_thread_widgets(&widget_event_rows(pool, thread_id).await?);
    let mut widgets = Vec::with_capacity(folded.len());
    for widget in folded {
        let app = match app_manager.get_app(&widget.app_id) {
            Ok(app) => app,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                log!(
                    "[Widgets] Skipping {} in the thread's widgets: {}",
                    widget.app_id,
                    e
                );
                continue;
            }
            Err(e) => return Err(format!("reading widget '{}': {e}", widget.app_id).into()),
        };
        widgets.push(ThreadWidget {
            app_id: widget.app_id,
            name: app.name,
            icon: app.icon,
            reusable: app.reusable,
            reveal: app.reveal,
            pinned: widget.pinned,
            shown_event_id: widget.shown_event_id,
        });
    }
    Ok(widgets)
}

/// The widget this id names, or why it cannot be used. Only a missing folder
/// or manifest means there is no such widget.
fn widget(app_manager: &AppManager, app_id: &str) -> Result<App, WidgetCheckFailed> {
    let app = app_manager.get_app(app_id).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput => {
            Refused(format!("No widget '{}' exists", app_id))
        }
        _ => ReadFailed(format!("reading widget '{app_id}': {e}").into()),
    })?;
    if !app.is_widget() {
        return Err(Refused(format!(
            "'{}' is an app, not a widget. Open an app with navigate_ui.",
            app_id
        )));
    }
    Ok(app)
}

async fn thread_exists(pool: &PgPool, thread_id: Uuid) -> Result<bool, BoxError> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM thread_summaries WHERE thread_id = $1)",
    )
    .bind(thread_id)
    .fetch_one(pool)
    .await?)
}

/// A widget change in a thread that the user or an agent asked for: showing a
/// widget there, or pinning or unpinning its chip on the shelf.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetChange {
    Show,
    Pin,
    Unpin,
}

/// Every `event_type` a stored widget event may carry: the current names plus
/// the legacy ones their `serde(alias)` still reads.
static STORED_WIDGET_EVENT_TYPES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    ThreadEvent::RESERVED_TYPE_NAMES
        .iter()
        .chain(ThreadEvent::LEGACY_TYPE_NAME_ALIASES)
        .copied()
        .filter(|name| WidgetChange::from_event_type(name).is_some())
        .collect()
});

impl WidgetChange {
    /// The change a stored row records. Goes through `ThreadEvent`'s own
    /// deserializer, so a legacy name reads as the variant it was renamed to.
    fn from_event_type(name: &str) -> Option<Self> {
        let event = ThreadEvent::from_stored(name, serde_json::json!({ "app_id": "" })).ok()?;
        match event {
            ThreadEvent::WidgetShown { .. } => Some(Self::Show),
            ThreadEvent::WidgetPinned { .. } => Some(Self::Pin),
            ThreadEvent::WidgetUnpinned { .. } => Some(Self::Unpin),
            _ => None,
        }
    }

    fn event(self, app_id: String) -> ThreadEvent {
        match self {
            Self::Show => ThreadEvent::WidgetShown { app_id },
            Self::Pin => ThreadEvent::WidgetPinned { app_id },
            Self::Unpin => ThreadEvent::WidgetUnpinned { app_id },
        }
    }
}

/// Why a widget verb did not run. The API answers a refusal with 409 and a
/// failed read with 500, which is why the two stay apart.
#[derive(Debug)]
pub enum WidgetCheckFailed {
    /// The request itself is wrong: the reason is for the caller.
    Refused(String),
    ReadFailed(BoxError),
}

impl std::fmt::Display for WidgetCheckFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(reason) => f.write_str(reason),
            Self::ReadFailed(e) => write!(f, "checking the widget failed: {e}"),
        }
    }
}

use WidgetCheckFailed::{ReadFailed, Refused};

/// Whether a widget change can happen.
pub async fn check_widget_change(
    pool: &PgPool,
    app_manager: &AppManager,
    change: WidgetChange,
    app_id: &str,
    thread_id: Uuid,
) -> Result<(), WidgetCheckFailed> {
    let app = widget(app_manager, app_id)?;
    if !thread_exists(pool, thread_id).await.map_err(ReadFailed)? {
        return Err(Refused(format!("No thread {} exists", thread_id)));
    }
    if change == WidgetChange::Show {
        if !app.reusable && app.origin_thread_id != Some(thread_id) {
            return Err(Refused(format!(
                "Widget '{}' belongs to another thread. Make it reusable first, then show it here.",
                app_id
            )));
        }
        return Ok(());
    }
    let widgets = fold_thread_widgets(
        &widget_event_rows(pool, thread_id)
            .await
            .map_err(ReadFailed)?,
    );
    let Some(widget) = widgets.iter().find(|w| w.app_id == app_id) else {
        return Err(Refused(format!(
            "Widget '{}' was never shown in this thread",
            app_id
        )));
    };
    match (change, widget.pinned) {
        (WidgetChange::Pin, true) => Err(Refused(format!(
            "Widget '{}' is already pinned to this thread's shelf",
            app_id
        ))),
        (WidgetChange::Unpin, false) => Err(Refused(format!(
            "Widget '{}' is not pinned to this thread's shelf",
            app_id
        ))),
        _ => Ok(()),
    }
}

/// Whether a widget may be made reusable, or stop being reusable. Stopping
/// hands the widget back to its origin thread, so that thread must exist:
/// otherwise nothing could show the widget or ever remove it.
pub async fn check_set_reusable(
    pool: &PgPool,
    app_manager: &AppManager,
    app_id: &str,
    reusable: bool,
) -> Result<(), WidgetCheckFailed> {
    let app = widget(app_manager, app_id)?;
    if app.reusable == reusable {
        return Err(Refused(format!(
            "Widget '{}' is already {}",
            app_id,
            if reusable { "reusable" } else { "not reusable" }
        )));
    }
    if reusable {
        return Ok(());
    }
    let origin_exists = match app.origin_thread_id {
        Some(thread_id) => thread_exists(pool, thread_id).await.map_err(ReadFailed)?,
        None => false,
    };
    if !origin_exists {
        return Err(Refused(format!(
            "Widget '{}' was made in a thread that no longer exists, so it must stay reusable. \
             Make an app from it to keep it somewhere of its own.",
            app_id
        )));
    }
    Ok(())
}

/// Write the one thread event a checked widget change consists of. No file
/// and no commit: unpinning is never destructive.
pub async fn emit_widget_change(
    event_bus: &EventBus,
    change: WidgetChange,
    app_id: &str,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
) -> Result<(), BoxError> {
    event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: change.event(app_id.to_string()),
            meta: EventMeta::with_actor(actor),
        })
        .await
        .map_err(|e| format!("recording the widget change: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(n: u128, event_type: &str, app_id: &str) -> WidgetEventRow {
        WidgetEventRow {
            event_id: Uuid::from_u128(n),
            event_type: event_type.to_string(),
            app_id: app_id.to_string(),
        }
    }

    #[test]
    fn thread_widgets_keep_first_shown_order_and_the_newest_showing() {
        let widgets = fold_thread_widgets(&[
            row(1, "WidgetShown", "fare-grid"),
            row(2, "WidgetShown", "currency"),
            row(3, "WidgetShown", "fare-grid"),
        ]);
        assert_eq!(
            widgets,
            vec![
                FoldedWidget {
                    app_id: "fare-grid".into(),
                    pinned: false,
                    shown_event_id: Uuid::from_u128(3),
                },
                FoldedWidget {
                    app_id: "currency".into(),
                    pinned: false,
                    shown_event_id: Uuid::from_u128(2),
                },
            ]
        );
    }

    #[test]
    fn a_shown_widget_is_not_pinned() {
        let widgets = fold_thread_widgets(&[row(1, "WidgetShown", "fare-grid")]);
        assert!(!widgets[0].pinned);
    }

    #[test]
    fn pin_and_unpin_flip_the_chip_and_showing_again_changes_neither() {
        let pinned = fold_thread_widgets(&[
            row(1, "WidgetShown", "fare-grid"),
            row(2, "WidgetPinned", "fare-grid"),
        ]);
        assert!(pinned[0].pinned);

        let unpinned = fold_thread_widgets(&[
            row(1, "WidgetShown", "fare-grid"),
            row(2, "WidgetPinned", "fare-grid"),
            row(3, "WidgetUnpinned", "fare-grid"),
        ]);
        assert!(!unpinned[0].pinned);

        let shown_again = fold_thread_widgets(&[
            row(1, "WidgetShown", "fare-grid"),
            row(2, "WidgetPinned", "fare-grid"),
            row(3, "WidgetShown", "fare-grid"),
        ]);
        assert!(shown_again[0].pinned);
        assert_eq!(shown_again[0].shown_event_id, Uuid::from_u128(3));
    }

    /// Rows written before the rename read as a pin and an unpin.
    #[test]
    fn legacy_restored_and_hidden_rows_read_as_pin_and_unpin() {
        let pinned = fold_thread_widgets(&[
            row(1, "WidgetShown", "fare-grid"),
            row(2, "WidgetRestored", "fare-grid"),
        ]);
        assert!(pinned[0].pinned);

        let unpinned = fold_thread_widgets(&[
            row(1, "WidgetShown", "fare-grid"),
            row(2, "WidgetRestored", "fare-grid"),
            row(3, "WidgetHidden", "fare-grid"),
        ]);
        assert!(!unpinned[0].pinned);
    }

    #[test]
    fn the_thread_widgets_query_reads_current_and_legacy_names() {
        let mut names = STORED_WIDGET_EVENT_TYPES.clone();
        names.sort_unstable();
        assert_eq!(
            names,
            [
                "WidgetHidden",
                "WidgetPinned",
                "WidgetRestored",
                "WidgetShown",
                "WidgetUnpinned"
            ]
        );
    }

    async fn reusable_widget(manager: &AppManager, bus: &EventBus, id: &str, origin: Uuid) {
        manager
            .create_app(
                bus,
                id,
                "Fare grid",
                "",
                "<h1>w",
                crate::core::NewAppKind::Widget {
                    origin_thread_id: origin,
                },
            )
            .await
            .unwrap();
        manager
            .set_widget_reusable(bus, id, true, None)
            .await
            .unwrap();
    }

    /// Stopping reuse hands a widget back to its origin thread. With that
    /// thread deleted nothing could show or remove it, so it is refused.
    #[tokio::test]
    async fn stop_reusing_needs_a_living_origin_thread() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();
        let living = Uuid::new_v4();
        crate::test_support::seed_thread_event(
            &bus,
            living,
            ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
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
        crate::test_support::seed_thread_event(
            &bus,
            living,
            ThreadEvent::WidgetShown {
                app_id: "kept".into(),
            },
        )
        .await;
        reusable_widget(&manager, &bus, "kept", living).await;
        reusable_widget(&manager, &bus, "orphan", Uuid::new_v4()).await;

        check_set_reusable(&pool, &manager, "kept", false)
            .await
            .expect("its origin thread exists");
        let refused = check_set_reusable(&pool, &manager, "orphan", false).await;
        assert!(
            matches!(&refused, Err(WidgetCheckFailed::Refused(r)) if r.contains("no longer exists")),
            "got: {refused:?}"
        );
        assert!(matches!(
            check_set_reusable(&pool, &manager, "kept", true).await,
            Err(WidgetCheckFailed::Refused(_))
        ));

        // The read returns the event, with the manifest's name and reveal.
        let widgets = thread_widgets(&pool, &manager, living).await.unwrap();
        assert_eq!(widgets.len(), 1);
        assert_eq!(widgets[0].name, "Fare grid");
        assert_eq!(widgets[0].reveal, AppReveal::OnLoad);
        assert!(widgets[0].reusable);

        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[test]
    fn a_pin_or_unpin_before_any_showing_is_ignored() {
        assert!(fold_thread_widgets(&[
            row(1, "WidgetPinned", "fare-grid"),
            row(2, "WidgetUnpinned", "fare-grid"),
        ])
        .is_empty());
    }
}
