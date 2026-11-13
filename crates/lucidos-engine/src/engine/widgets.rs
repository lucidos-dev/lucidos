//! The checks and verbs a widget instance goes through (ADRs 0402, 0415).
//!
//! Each verb checks first and then writes. Sizing a thread's cards writes one
//! thread event and touches no file.
//!
//! Everything here keys on the *widget instance*: the widget plus its canonical
//! params. The label is not part of the key.

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::apps::WidgetSize;
use crate::core::{App, AppManager};
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

/// The widget a stored event names, or `None` when it no longer lists: its
/// folder is gone, or its manifest does not parse, as for the apps list. Any
/// other read failure fails, since it says nothing about whether it exists.
pub(crate) fn listed_widget(
    app_manager: &AppManager,
    app_id: &str,
) -> Result<Option<App>, BoxError> {
    match app_manager.get_app(app_id) {
        Ok(app) => Ok(Some(app)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
            log!("[Widgets] Skipping {app_id}: {e}");
            Ok(None)
        }
        Err(e) => Err(format!("reading widget '{app_id}': {e}").into()),
    }
}

/// The widget this id names, or why it cannot be used. Only a missing folder
/// or manifest means there is no such widget.
pub(crate) fn widget(app_manager: &AppManager, app_id: &str) -> Result<App, WidgetCheckFailed> {
    let app = existing_app(app_manager, app_id, "widget")?;
    if !app.is_widget() {
        return Err(Refused(format!(
            "'{}' is an app, not a widget. Open an app with navigate_ui.",
            app_id
        )));
    }
    Ok(app)
}

/// What a dashboard tile can be: a widget, or an app that shows as a shortcut.
/// Only a missing folder or manifest means there is no such thing.
pub(crate) fn tile_app(app_manager: &AppManager, app_id: &str) -> Result<App, WidgetCheckFailed> {
    existing_app(app_manager, app_id, "widget or app")
}

fn existing_app(
    app_manager: &AppManager,
    app_id: &str,
    what: &str,
) -> Result<App, WidgetCheckFailed> {
    app_manager.get_app(app_id).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput => {
            Refused(format!("No {what} '{app_id}' exists"))
        }
        _ => ReadFailed(format!("reading {what} '{app_id}': {e}").into()),
    })
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
            "Widget '{}' belongs to another thread. Make it reusable first, then embed it here.",
            app.id
        )));
    }
    Ok(())
}

/// Whether a widget can be embedded in this thread with these params, with
/// no thread lookup. A question card runs it on each
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
/// its tile reads.
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

/// Whether a widget instance's cards in this thread can take this size: the
/// thread exists, it may hold the widget, and the widget declares the size.
pub async fn check_card_size(
    pool: &PgPool,
    app_manager: &AppManager,
    instance: &WidgetInstance,
    size: WidgetSize,
    thread_id: Uuid,
) -> Result<(), WidgetCheckFailed> {
    let app = widget(app_manager, &instance.app_id)?;
    if !thread_exists(pool, thread_id).await.map_err(ReadFailed)? {
        return Err(Refused(format!("No thread {} exists", thread_id)));
    }
    check_widget_owner(&app, thread_id)?;
    check_widget_params(&app, instance.params.as_ref()).map_err(Refused)?;
    check_declared_size(&app, size).map_err(Refused)
}

/// Whether the widget draws at this size: one its manifest declares.
pub fn check_declared_size(app: &App, size: WidgetSize) -> Result<(), String> {
    if app.sizes.contains(&size) {
        return Ok(());
    }
    let declared: Vec<&str> = app.sizes.iter().map(|s| s.as_str()).collect();
    Err(format!(
        "Widget '{}' does not draw at {}. It takes: {}.",
        app.id,
        size.as_str(),
        declared.join(", ")
    ))
}

/// Write the one thread event a checked card size consists of.
pub async fn emit_card_size(
    event_bus: &EventBus,
    instance: WidgetInstance,
    size: WidgetSize,
    thread_id: Uuid,
    actor: Option<MessageOrigin>,
) -> Result<(), BoxError> {
    event_bus
        .emit(BusEvent::Thread {
            thread_id,
            event: ThreadEvent::InlineWidgetSized {
                app_id: instance.app_id,
                params: instance.params.filter(|p| !p.is_empty()),
                size,
            },
            meta: EventMeta::with_actor(actor),
        })
        .await
        .map_err(|e| format!("recording the card size: {e}"))?;
    Ok(())
}

/// The newest size picked for one widget instance's cards in a thread. A paged
/// thread snapshot carries these, since the `InlineWidgetSized` that set one
/// may sit behind the page.
#[derive(Debug, Clone, PartialEq, serde::Serialize, Deserialize)]
pub struct CardSize {
    pub sequence: i64,
    pub app_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<WidgetParams>,
    pub size: WidgetSize,
}

/// A thread's card sizes: the newest `InlineWidgetSized` per widget instance,
/// oldest first.
pub async fn thread_card_sizes(pool: &PgPool, thread_id: Uuid) -> Result<Vec<CardSize>, BoxError> {
    let rows: Vec<(i64, Value)> = sqlx::query_as(
        "SELECT sequence, payload FROM events \
         WHERE thread_id = $1 AND event_type = 'InlineWidgetSized' \
         ORDER BY sequence",
    )
    .bind(thread_id)
    .fetch_all(pool)
    .await?;
    let mut newest: BTreeMap<String, CardSize> = BTreeMap::new();
    for (sequence, mut payload) in rows {
        payload["sequence"] = sequence.into();
        let size: CardSize = serde_json::from_value(payload)
            .map_err(|e| format!("reading card size event {sequence}: {e}"))?;
        let key = format!(
            "{}?{}",
            size.app_id,
            canonical_widget_params(size.params.as_ref().unwrap_or(&WidgetParams::new()))
        );
        newest.insert(key, size);
    }
    let mut sizes: Vec<CardSize> = newest.into_values().collect();
    sizes.sort_by_key(|s| s.sequence);
    Ok(sizes)
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
    if let Some(dashboard) = crate::engine::dashboard::dashboard_holding(pool, app_id)
        .await
        .map_err(ReadFailed)?
    {
        return Err(Refused(format!(
            "Widget '{}' is on the {} dashboard. Remove it from there first, \
             or deleting its thread would empty the tile.",
            app_id,
            dashboard.as_str()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::AppReveal;
    use serde_json::json;

    fn params(value: Value) -> WidgetParams {
        serde_json::from_value(value).unwrap()
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
                    sizes: None,
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

    /// Invariant I14: a plugin widget stays reusable and shows in any thread.
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
                wake_mode: None,
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
        check_widget_embed(&manager, "board", None, thread)
            .expect("a plugin widget embeds in any thread");
        check_widget_embed(&manager, "board", None, Uuid::new_v4())
            .expect("even one it was never shown in");
        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// I6 and I10 of the one-render plan: a card size is checked like an
    /// embed. It refuses an undeclared size, and writes one
    /// `InlineWidgetSized` with the instance and the size.
    #[tokio::test]
    async fn a_card_size_is_checked_and_written_as_one_event() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let tmp = tempfile::tempdir().unwrap();
        let manager = AppManager::new(tmp.path()).unwrap();
        let thread = Uuid::new_v4();
        let other = Uuid::new_v4();
        for id in [thread, other] {
            crate::test_support::seed_thread_event(
                &bus,
                id,
                ThreadEvent::MessageReceived {
                    provider: None,
                    voice_session_id: None,
                    wake_mode: None,
                    text: "show fares".into(),
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
        manager
            .create_app(
                &bus,
                "fare-grid",
                "Fare grid",
                "",
                "<h1>w",
                crate::core::NewAppKind::Widget {
                    origin_thread_id: thread,
                    sizes: Some(vec![WidgetSize::Medium, WidgetSize::Large]),
                },
            )
            .await
            .unwrap();
        let grid = WidgetInstance {
            app_id: "fare-grid".into(),
            ..Default::default()
        };

        let undeclared = check_card_size(&pool, &manager, &grid, WidgetSize::Small, thread).await;
        assert!(
            matches!(&undeclared, Err(Refused(r)) if r.contains("does not draw at small")),
            "got: {undeclared:?}"
        );
        let elsewhere = check_card_size(&pool, &manager, &grid, WidgetSize::Large, other).await;
        assert!(
            matches!(&elsewhere, Err(Refused(r)) if r.contains("belongs to another thread")),
            "got: {elsewhere:?}"
        );
        check_card_size(&pool, &manager, &grid, WidgetSize::Large, thread)
            .await
            .expect("a declared size in the widget's own thread");
        emit_card_size(&bus, grid, WidgetSize::Large, thread, None)
            .await
            .unwrap();

        let (event_type, payload): (String, Value) = sqlx::query_as(
            "SELECT event_type, payload FROM events \
             WHERE thread_id = $1 AND event_type = 'InlineWidgetSized'",
        )
        .bind(thread)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(event_type, "InlineWidgetSized");
        assert_eq!(payload["app_id"], "fare-grid");
        assert_eq!(payload["size"], "large");
        assert!(payload.get("params").is_none(), "empty params write none");
        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// I6 of the one-render plan, past the loaded page: a thread's card sizes
    /// are the newest pick per instance, keyed by canonical params.
    #[tokio::test]
    async fn card_sizes_are_the_newest_pick_per_instance() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let (bus, _rx) = EventBus::new(pool.clone());
        let thread = Uuid::new_v4();
        let other = Uuid::new_v4();
        let instance = |params: Option<Value>| WidgetInstance {
            app_id: "player".into(),
            params: params.map(|p| serde_json::from_value(p).unwrap()),
            label: None,
        };
        let picks = [
            (thread, None, WidgetSize::Medium),
            (
                thread,
                Some(serde_json::json!({"clip": "a.mp3", "loop": true})),
                WidgetSize::Small,
            ),
            (thread, None, WidgetSize::Large),
            (
                thread,
                Some(serde_json::json!({"loop": true, "clip": "a.mp3"})),
                WidgetSize::Medium,
            ),
            (other, None, WidgetSize::Small),
        ];
        for (tid, params, size) in picks {
            emit_card_size(&bus, instance(params), size, tid, None)
                .await
                .unwrap();
        }

        let sizes = thread_card_sizes(&pool, thread).await.unwrap();
        let read: Vec<(Option<String>, WidgetSize)> = sizes
            .iter()
            .map(|s| (s.params.as_ref().map(canonical_widget_params), s.size))
            .collect();
        assert_eq!(
            read,
            vec![
                (None, WidgetSize::Large),
                (
                    Some(r#"{"clip":"a.mp3","loop":true}"#.into()),
                    WidgetSize::Medium
                ),
            ],
            "the newest pick per instance, in sequence order, and no other thread's"
        );
        assert!(sizes[0].sequence < sizes[1].sequence);
        assert!(thread_card_sizes(&pool, Uuid::new_v4())
            .await
            .unwrap()
            .is_empty());
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

        crate::test_support::teardown_test_db(&db_name).await;
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
            sizes: WidgetSize::UNDECLARED.to_vec(),
            transparent: false,
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

    /// Identity compares canonical params: the same set in any key order, at
    /// any depth, is one instance.
    #[test]
    fn reordered_params_are_one_instance() {
        assert_eq!(
            InstanceKey::new(
                "chart",
                Some(&params(json!({"a": 1, "b": {"x": 1, "y": 2}})))
            ),
            InstanceKey::new(
                "chart",
                Some(&params(json!({"b": {"y": 2, "x": 1}, "a": 1})))
            ),
        );
        assert_eq!(
            InstanceKey::new("chart", None),
            InstanceKey::new("chart", Some(&params(json!({}))))
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
