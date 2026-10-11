//! A thread's widgets, its shelf, and the verbs that change them (ADRs 0402,
//! 0407, 0415).
//!
//! A thread's widgets are derived on read from its widget events plus each
//! widget's app manifest. No column caches them, so a reload, another device
//! or a compaction cannot lose them. Each verb checks first and then writes:
//! show, pin and unpin write one thread event and touch no file.
//!
//! Everything here keys on the *widget instance*: the widget plus its canonical
//! params. The label is not part of the key.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::{App, AppManager, AppReveal};
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{EventMeta, MessageOrigin, ThreadEvent};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// A widget's params for one place it shows: a JSON object.
pub type WidgetParams = BTreeMap<String, Value>;

/// The most bytes a widget's canonical params may take. They ride the frame
/// URL, so a large value belongs in a file the widget reads.
pub const WIDGET_PARAMS_MAX_BYTES: usize = 2048;

/// JSON with every object's keys sorted, so one set of params always gives one
/// string: the engine's instance identity. A whole-number float writes as an
/// integer, as JavaScript writes it, so params the frontend sends back name
/// the same instance.
pub fn canonical_widget_params(params: &WidgetParams) -> String {
    fn canonical(value: &Value) -> Value {
        match value {
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), canonical(v)))
                    .collect::<BTreeMap<_, _>>()
                    .into_iter()
                    .collect(),
            ),
            Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
            Value::Number(n) => match n.as_f64() {
                Some(f) if n.is_f64() && f.fract() == 0.0 && f.abs() < 9.0e15 => {
                    Value::from(f as i64)
                }
                _ => value.clone(),
            },
            other => other.clone(),
        }
    }
    let ordered: BTreeMap<&String, Value> = params.iter().map(|(k, v)| (k, canonical(v))).collect();
    serde_json::to_string(&ordered).unwrap_or_default()
}

/// One widget instance's identity: the widget and its canonical params.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceKey {
    pub app_id: String,
    pub params: String,
}

impl InstanceKey {
    pub fn new(app_id: &str, params: Option<&WidgetParams>) -> Self {
        Self {
            app_id: app_id.to_string(),
            params: canonical_widget_params(params.unwrap_or(&WidgetParams::new())),
        }
    }
}

/// One widget instance in a thread, shown or pinned. Only a pinned one has a
/// chip on the shelf; the transcript's card reads every shown one.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThreadWidget {
    pub app_id: String,
    /// The instance's params. Absent means none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<WidgetParams>,
    /// The instance's chip label, newest first. Absent means the name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub name: String,
    /// The validated `App::icon`, a path inside the widget's folder.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub reusable: bool,
    /// The plugin that ships this widget. Such a widget offers no Stop reusing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin_plugin_id: Option<String>,
    /// A *built-in widget*, which no menu may change (ADR 0415).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub built_in: bool,
    /// When the frame's cover lifts, from the manifest, as for any app.
    pub reveal: AppReveal,
    /// Pinned to the shelf, so it has a chip there. The thread's card shows
    /// either way.
    pub pinned: bool,
    /// The newest `WidgetShown` for this instance in this thread: where "Show
    /// in thread" scrolls to. Absent for an instance only ever pinned, as
    /// from an embed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shown_event_id: Option<Uuid>,
}

/// A widget event as the fold reads it, in thread order.
#[derive(Debug, Clone)]
pub struct WidgetEventRow {
    pub event_id: Uuid,
    pub event_type: String,
    pub app_id: String,
    pub params: Option<WidgetParams>,
    pub label: Option<String>,
}

/// What the events alone say about one widget instance in a thread.
#[derive(Debug, Clone, PartialEq)]
pub struct FoldedWidget {
    pub app_id: String,
    pub params: Option<WidgetParams>,
    pub label: Option<String>,
    pub pinned: bool,
    pub shown_event_id: Option<Uuid>,
}

impl FoldedWidget {
    fn key(&self) -> InstanceKey {
        InstanceKey::new(&self.app_id, self.params.as_ref())
    }
}

/// Fold a thread's widget events into its instances, ordered by first
/// appearance. Only a pin puts an instance on the shelf and only an unpin
/// takes it off; showing again changes neither. A pin alone adds the
/// instance, since an embed records no showing. An unpin of an instance never
/// seen is ignored. The newest label given wins.
pub fn fold_thread_widgets(rows: &[WidgetEventRow]) -> Vec<FoldedWidget> {
    let mut widgets: Vec<FoldedWidget> = Vec::new();
    for row in rows {
        let Some(change) = WidgetChange::from_event_type(&row.event_type) else {
            continue;
        };
        let key = InstanceKey::new(&row.app_id, row.params.as_ref());
        let existing = widgets.iter_mut().find(|w| w.key() == key);
        let widget = match (change, existing) {
            (WidgetChange::Unpin, None) => continue,
            (_, Some(widget)) => widget,
            (_, None) => {
                widgets.push(FoldedWidget {
                    app_id: row.app_id.clone(),
                    params: row.params.clone().filter(|p| !p.is_empty()),
                    label: None,
                    pinned: false,
                    shown_event_id: None,
                });
                widgets.last_mut().expect("just pushed")
            }
        };
        match change {
            WidgetChange::Show => widget.shown_event_id = Some(row.event_id),
            WidgetChange::Pin => widget.pinned = true,
            WidgetChange::Unpin => widget.pinned = false,
        }
        if row.label.is_some() && change != WidgetChange::Unpin {
            widget.label = row.label.clone();
        }
    }
    widgets
}

async fn widget_event_rows(
    pool: &PgPool,
    thread_id: Uuid,
) -> Result<Vec<WidgetEventRow>, BoxError> {
    type Row = (Uuid, String, Option<String>, Option<Value>, Option<String>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, event_type, payload->>'app_id', payload->'params', payload->>'label' \
         FROM events WHERE thread_id = $1 AND event_type = ANY($2) ORDER BY sequence",
    )
    .bind(thread_id)
    .bind(STORED_WIDGET_EVENT_TYPES.as_slice())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(event_id, event_type, app_id, params, label)| {
            Some(WidgetEventRow {
                event_id,
                event_type,
                app_id: app_id?,
                params: params.and_then(|p| serde_json::from_value(p).ok()),
                label,
            })
        })
        .collect())
}

/// Every widget instance in a thread, unpinned ones included. A widget whose
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
        let origin_plugin_id = app.origin_plugin().map(str::to_string);
        widgets.push(ThreadWidget {
            app_id: widget.app_id,
            params: widget.params,
            label: widget.label,
            name: app.name,
            icon: app.icon,
            reusable: app.reusable,
            origin_plugin_id,
            built_in: app.built_in,
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

/// Whether these params fit what the widget's manifest declares: no unknown
/// name, every required name present, and within the size cap.
pub fn check_widget_params(app: &App, params: Option<&WidgetParams>) -> Result<(), String> {
    let empty = WidgetParams::new();
    let params = params.unwrap_or(&empty);
    if let Some(unknown) = params.keys().find(|k| !app.params.contains_key(*k)) {
        let declared = if app.params.is_empty() {
            "none".to_string()
        } else {
            app.params.keys().cloned().collect::<Vec<_>>().join(", ")
        };
        return Err(format!(
            "Widget '{}' takes no param '{unknown}'. It takes: {declared}.",
            app.id
        ));
    }
    if let Some((missing, _)) = app
        .params
        .iter()
        .find(|(name, p)| p.required && !params.contains_key(*name))
    {
        return Err(format!("Widget '{}' needs the param '{missing}'.", app.id));
    }
    let size = canonical_widget_params(params).len();
    if size > WIDGET_PARAMS_MAX_BYTES {
        return Err(format!(
            "Widget '{}' params take {size} bytes, over the {WIDGET_PARAMS_MAX_BYTES}-byte cap. \
             Put large data in a file and pass its path.",
            app.id
        ));
    }
    Ok(())
}

/// Whether this thread may hold this widget: a reusable one anywhere, any
/// other only in the thread that made it.
fn check_widget_owner(app: &App, thread_id: Uuid) -> Result<(), WidgetCheckFailed> {
    if !app.reusable && app.origin_thread() != Some(thread_id) {
        return Err(Refused(format!(
            "Widget '{}' belongs to another thread. Make it reusable first, then show it here.",
            app.id
        )));
    }
    Ok(())
}

/// Whether a widget can be embedded in this thread with these params: the
/// checks a show runs, with no thread lookup. A question card runs it on each
/// option's widget before it is asked (ADR 0415).
pub fn check_widget_embed(
    app_manager: &AppManager,
    app_id: &str,
    params: Option<&WidgetParams>,
    thread_id: Uuid,
) -> Result<(), WidgetCheckFailed> {
    let app = widget(app_manager, app_id)?;
    check_widget_owner(&app, thread_id)?;
    check_widget_params(&app, params).map_err(Refused)
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

    fn event(self, instance: WidgetInstance) -> ThreadEvent {
        let WidgetInstance {
            app_id,
            params,
            label,
        } = instance;
        let params = params.filter(|p| !p.is_empty());
        match self {
            Self::Show => ThreadEvent::WidgetShown {
                app_id,
                params,
                label,
            },
            Self::Pin => ThreadEvent::WidgetPinned {
                app_id,
                params,
                label,
            },
            Self::Unpin => ThreadEvent::WidgetUnpinned {
                app_id,
                params,
                label,
            },
        }
    }
}

/// The widget instance a request names, as it arrives: `app_id`, and an
/// optional `params` object and `label`.
#[derive(Debug, Clone, Deserialize)]
pub struct WidgetInstanceArgs {
    pub app_id: String,
    #[serde(default)]
    pub params: Option<WidgetParams>,
    #[serde(default)]
    pub label: Option<String>,
}

impl TryFrom<WidgetInstanceArgs> for WidgetInstance {
    type Error = String;

    /// Trim the id and label; a blank label means none.
    fn try_from(args: WidgetInstanceArgs) -> Result<Self, String> {
        let app_id = args.app_id.trim();
        if app_id.is_empty() {
            return Err("app_id is required".into());
        }
        Ok(Self {
            app_id: app_id.to_string(),
            params: args.params,
            label: args
                .label
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty()),
        })
    }
}

/// The widget instance a verb names: the widget, its params, and the label
/// its chip reads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WidgetInstance {
    pub app_id: String,
    pub params: Option<WidgetParams>,
    pub label: Option<String>,
}

impl WidgetInstance {
    /// Read an instance from a tool's JSON arguments.
    pub fn from_args(args: &Value) -> Result<Self, String> {
        let args: WidgetInstanceArgs = serde_json::from_value(args.clone())
            .map_err(|e| format!("bad widget arguments: {e}"))?;
        args.try_into()
    }

    fn key(&self) -> InstanceKey {
        InstanceKey::new(&self.app_id, self.params.as_ref())
    }

    /// How a reply names the instance: its label, else its id.
    fn describe(&self) -> String {
        match &self.label {
            Some(label) => format!("'{}' ({label})", self.app_id),
            None => format!("'{}'", self.app_id),
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
    instance: &WidgetInstance,
    thread_id: Uuid,
) -> Result<(), WidgetCheckFailed> {
    let app = widget(app_manager, &instance.app_id)?;
    if !thread_exists(pool, thread_id).await.map_err(ReadFailed)? {
        return Err(Refused(format!("No thread {} exists", thread_id)));
    }
    // A new instance must be one this thread may hold. One the thread already
    // holds stays pinnable, even if its widget stopped being reusable since.
    let check_new = || -> Result<(), WidgetCheckFailed> {
        check_widget_owner(&app, thread_id)?;
        check_widget_params(&app, instance.params.as_ref()).map_err(Refused)
    };
    if change == WidgetChange::Show {
        return check_new();
    }
    let widgets = fold_thread_widgets(
        &widget_event_rows(pool, thread_id)
            .await
            .map_err(ReadFailed)?,
    );
    let key = instance.key();
    let pinned = widgets.iter().find(|w| w.key() == key).map(|w| w.pinned);
    if change == WidgetChange::Pin && pinned.is_none() {
        check_new()?;
    }
    match (change, pinned) {
        (WidgetChange::Pin, Some(true)) => Err(Refused(format!(
            "Widget {} is already pinned to this thread's shelf",
            instance.describe()
        ))),
        (WidgetChange::Unpin, None) => Err(Refused(format!(
            "Widget {} was never shown or pinned in this thread",
            instance.describe()
        ))),
        (WidgetChange::Unpin, Some(false)) => Err(Refused(format!(
            "Widget {} is not pinned to this thread's shelf",
            instance.describe()
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
    app_manager.refuse_built_in(app_id).map_err(Refused)?;
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
    if let Some(plugin) = app.origin_plugin() {
        return Err(Refused(format!(
            "Widget '{}' ships with the plugin '{}', so it stays reusable. \
             Uninstall the plugin to remove it.",
            app_id, plugin
        )));
    }
    let origin_exists = match app.origin_thread() {
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
    instance: WidgetInstance,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
) -> Result<(), BoxError> {
    event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: change.event(instance),
            meta: EventMeta::with_actor(actor),
        })
        .await
        .map_err(|e| format!("recording the widget change: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(n: u128, event_type: &str, app_id: &str) -> WidgetEventRow {
        WidgetEventRow {
            event_id: Uuid::from_u128(n),
            event_type: event_type.to_string(),
            app_id: app_id.to_string(),
            params: None,
            label: None,
        }
    }

    fn params(value: Value) -> WidgetParams {
        serde_json::from_value(value).unwrap()
    }

    fn row_with(
        n: u128,
        event_type: &str,
        app_id: &str,
        p: Value,
        label: Option<&str>,
    ) -> WidgetEventRow {
        WidgetEventRow {
            params: Some(params(p)),
            label: label.map(str::to_string),
            ..row(n, event_type, app_id)
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
                    params: None,
                    label: None,
                    pinned: false,
                    shown_event_id: Some(Uuid::from_u128(3)),
                },
                FoldedWidget {
                    app_id: "currency".into(),
                    params: None,
                    label: None,
                    pinned: false,
                    shown_event_id: Some(Uuid::from_u128(2)),
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
        assert_eq!(shown_again[0].shown_event_id, Some(Uuid::from_u128(3)));
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

    fn plugin_widget(tmp: &std::path::Path, id: &str) {
        let dir = tmp.join("data/apps").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            serde_json::json!({
                "name": "Board", "kind": "widget", "reusable": true,
                "origin_plugin_id": "habit-tracker",
            })
            .to_string(),
        )
        .unwrap();
    }

    /// Invariant I14: a plugin widget stays reusable, shows in any thread, and
    /// tells the shelf it has no Stop reusing.
    #[tokio::test]
    async fn a_plugin_widget_stays_reusable_and_shows_anywhere() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();
        plugin_widget(tmp.path(), "board");
        let thread = Uuid::new_v4();
        crate::test_support::seed_thread_event(
            &bus,
            thread,
            ThreadEvent::MessageReceived {
                provider: None,
                voice_session_id: None,
                text: "show my habits".into(),
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
            thread,
            ThreadEvent::WidgetShown {
                app_id: "board".into(),
                params: None,
                label: None,
            },
        )
        .await;

        let refused = check_set_reusable(&pool, &manager, "board", false).await;
        assert!(
            matches!(&refused, Err(WidgetCheckFailed::Refused(r)) if r.contains("habit-tracker")),
            "got: {refused:?}"
        );
        let board = WidgetInstance {
            app_id: "board".into(),
            params: None,
            label: None,
        };
        check_widget_change(&pool, &manager, WidgetChange::Show, &board, thread)
            .await
            .expect("a plugin widget shows in any thread");
        let widgets = thread_widgets(&pool, &manager, thread).await.unwrap();
        assert_eq!(
            widgets[0].origin_plugin_id.as_deref(),
            Some("habit-tracker")
        );
        crate::test_support::teardown_test_db(&db_name).await;
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
                params: None,
                label: None,
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

    /// An embed records no showing, so its pin alone adds the instance. An
    /// unpin of an instance never seen still changes nothing (ADR 0415).
    #[test]
    fn a_pin_alone_adds_the_instance_and_a_stray_unpin_is_ignored() {
        let pinned = fold_thread_widgets(&[row(1, "WidgetPinned", "fare-grid")]);
        assert_eq!(pinned.len(), 1);
        assert!(pinned[0].pinned);
        assert_eq!(pinned[0].shown_event_id, None);

        assert!(fold_thread_widgets(&[row(1, "WidgetUnpinned", "fare-grid")]).is_empty());
    }

    /// Identity compares canonical params: the same set in any key order, at
    /// any depth, is one instance.
    #[test]
    fn reordered_params_are_one_instance() {
        let widgets = fold_thread_widgets(&[
            row_with(
                1,
                "WidgetShown",
                "chart",
                json!({"a": 1, "b": {"x": 1, "y": 2}}),
                None,
            ),
            row_with(
                2,
                "WidgetPinned",
                "chart",
                json!({"b": {"y": 2, "x": 1}, "a": 1}),
                None,
            ),
        ]);
        assert_eq!(widgets.len(), 1);
        assert!(widgets[0].pinned);
    }

    /// Two param sets of one widget are two instances, each with its own pin.
    #[test]
    fn two_instances_of_one_widget_pin_apart() {
        let widgets = fold_thread_widgets(&[
            row_with(1, "WidgetShown", "player", json!({"clip": "a.mp3"}), None),
            row_with(2, "WidgetShown", "player", json!({"clip": "b.mp3"}), None),
            row_with(3, "WidgetPinned", "player", json!({"clip": "b.mp3"}), None),
        ]);
        assert_eq!(widgets.len(), 2);
        assert!(!widgets[0].pinned);
        assert!(widgets[1].pinned);
    }

    /// A row from before params existed reads as `{}`. So it is the same
    /// instance as a new event with empty params, and legacy pins stay.
    #[test]
    fn a_legacy_row_with_no_params_reads_as_empty_params() {
        let widgets = fold_thread_widgets(&[
            row(1, "WidgetShown", "fare-grid"),
            row(2, "WidgetPinned", "fare-grid"),
            row_with(3, "WidgetShown", "fare-grid", json!({}), None),
        ]);
        assert_eq!(widgets.len(), 1);
        assert!(widgets[0].pinned);
        assert_eq!(widgets[0].params, None);
        assert_eq!(widgets[0].shown_event_id, Some(Uuid::from_u128(3)));
    }

    /// The label is not part of the key, and the newest one given wins.
    #[test]
    fn the_newest_label_wins_and_never_splits_an_instance() {
        let widgets = fold_thread_widgets(&[
            row_with(
                1,
                "WidgetShown",
                "player",
                json!({"clip": "a.mp3"}),
                Some("Marin"),
            ),
            row_with(
                2,
                "WidgetPinned",
                "player",
                json!({"clip": "a.mp3"}),
                Some("Marin, calm"),
            ),
            row_with(3, "WidgetShown", "player", json!({"clip": "a.mp3"}), None),
        ]);
        assert_eq!(widgets.len(), 1);
        assert_eq!(widgets[0].label.as_deref(), Some("Marin, calm"));
    }

    /// A legacy `WidgetShown { app_id }` still parses as it did.
    #[test]
    fn a_legacy_widget_shown_row_replays_unchanged() {
        let event =
            ThreadEvent::from_stored("WidgetShown", json!({"app_id": "fare-grid"})).unwrap();
        assert!(matches!(
            event,
            ThreadEvent::WidgetShown { ref app_id, params: None, label: None } if app_id == "fare-grid"
        ));
        let stored = serde_json::to_value(&event).unwrap();
        assert!(stored.get("params").is_none() && stored.get("label").is_none());
    }

    fn player() -> App {
        App {
            id: "player".into(),
            name: "Player".into(),
            description: String::new(),
            icon: None,
            reveal: AppReveal::OnLoad,
            kind: crate::core::AppKind::Widget,
            origin: None,
            reusable: true,
            params: BTreeMap::from([
                (
                    "clip".to_string(),
                    crate::core::apps::AppParam {
                        description: "What to play".into(),
                        required: true,
                    },
                ),
                (
                    "loop".to_string(),
                    crate::core::apps::AppParam {
                        description: String::new(),
                        required: false,
                    },
                ),
            ]),
            built_in: false,
        }
    }

    #[test]
    fn params_must_match_the_manifest() {
        let app = player();
        assert!(check_widget_params(&app, Some(&params(json!({"clip": "a.mp3"})))).is_ok());
        assert!(
            check_widget_params(&app, Some(&params(json!({"clip": "a", "loop": true})))).is_ok()
        );

        let unknown = check_widget_params(&app, Some(&params(json!({"clip": "a", "x": 1}))));
        assert!(unknown.unwrap_err().contains("no param 'x'"));
        let missing = check_widget_params(&app, None);
        assert!(missing.unwrap_err().contains("needs the param 'clip'"));
        let big = "x".repeat(WIDGET_PARAMS_MAX_BYTES);
        let over = check_widget_params(&app, Some(&params(json!({"clip": big}))));
        assert!(over.unwrap_err().contains("cap"));
    }

    #[test]
    fn instance_args_read_params_and_label_and_refuse_a_non_object() {
        let instance = WidgetInstance::from_args(
            &json!({"app_id": "player", "params": {"clip": "a.mp3"}, "label": " Marin "}),
        )
        .unwrap();
        assert_eq!(instance.params, Some(params(json!({"clip": "a.mp3"}))));
        assert_eq!(instance.label.as_deref(), Some("Marin"));
        assert!(WidgetInstance::from_args(&json!({"app_id": "p", "params": [1]})).is_err());
        assert!(WidgetInstance::from_args(&json!({"params": {}})).is_err());
    }

    /// JavaScript writes `1.0` as `1`, so a param read back by the frontend
    /// must name the instance the engine stored.
    #[test]
    fn a_whole_float_and_its_integer_are_one_instance() {
        assert_eq!(
            canonical_widget_params(&params(json!({"rate": 1.0, "n": [2.0, 2.5]}))),
            canonical_widget_params(&params(json!({"rate": 1, "n": [2, 2.5]})))
        );
    }

    #[test]
    fn canonical_params_sort_every_level() {
        assert_eq!(
            canonical_widget_params(&params(json!({"b": [{"z": 1, "a": 2}], "a": 1}))),
            r#"{"a":1,"b":[{"a":2,"z":1}]}"#
        );
    }
}
