//! The two *dashboards*, `phone` and `desktop`, each with its own tiles
//! (ADR 0496).
//!
//! No column caches them. Each read folds two sources, in order:
//!
//! - **The seed**: the home thread's old pins and `HomeBoardArranged`
//!   snapshots, folded as they were before each dashboard had its own tiles.
//!   Each dashboard starts with its layout's tiles, minus the hidden ones.
//!   Nothing writes those events any more, so the seed never moves.
//! - **The dashboard events**, `DashboardTileAdded`, `DashboardTileRemoved`
//!   and `DashboardArranged`, applied on top.
//!
//! A tile keys on its *widget instance*, as a shelf chip does.

use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;

use crate::core::apps::WidgetSize;
use crate::core::AppManager;
use crate::engine::event_bus::{BusEvent, EventBus, SystemEvent};
use crate::engine::thread_events::{
    ArrangedTile, Dashboard, LegacyArrangedTile, MessageOrigin, ThreadEvent,
};
use crate::engine::widgets::{
    self, InstanceKey, ThreadWidget, WidgetCheckFailed, WidgetInstance, WidgetParams,
    STORED_WIDGET_EVENT_TYPES,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// One tile as the events alone say it.
#[derive(Debug, Clone, PartialEq)]
pub struct FoldedTile {
    pub app_id: String,
    pub params: Option<WidgetParams>,
    pub label: Option<String>,
    /// `None` until an arrange gives one: the widget's default size.
    pub size: Option<WidgetSize>,
}

impl FoldedTile {
    fn new(app_id: &str, params: Option<WidgetParams>, label: Option<String>) -> Self {
        Self {
            app_id: app_id.to_string(),
            params: params.filter(|p| !p.is_empty()),
            label,
            size: None,
        }
    }

    fn key(&self) -> InstanceKey {
        InstanceKey::new(&self.app_id, self.params.as_ref())
    }
}

/// Both dashboards' tiles, in order.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct FoldedDashboards {
    pub phone: Vec<FoldedTile>,
    pub desktop: Vec<FoldedTile>,
}

impl FoldedDashboards {
    pub fn tiles(&self, dashboard: Dashboard) -> &[FoldedTile] {
        match dashboard {
            Dashboard::Phone => &self.phone,
            Dashboard::Desktop => &self.desktop,
        }
    }

    fn tiles_mut(&mut self, dashboard: Dashboard) -> &mut Vec<FoldedTile> {
        match dashboard {
            Dashboard::Phone => &mut self.phone,
            Dashboard::Desktop => &mut self.desktop,
        }
    }

    fn holds(&self, dashboard: Dashboard, key: &InstanceKey) -> bool {
        self.tiles(dashboard).iter().any(|t| &t.key() == key)
    }

    fn add(&mut self, dashboard: Dashboard, tile: FoldedTile) {
        if !self.holds(dashboard, &tile.key()) {
            self.tiles_mut(dashboard).push(tile);
        }
    }

    fn remove(&mut self, dashboard: Dashboard, key: &InstanceKey) {
        self.tiles_mut(dashboard).retain(|t| &t.key() != key);
    }

    /// The snapshot's tiles first, in its order. A tile it leaves out keeps
    /// its place after them, and an entry naming no tile is ignored.
    fn arrange(&mut self, dashboard: Dashboard, tiles: &[ArrangedTile]) {
        let mut previous = std::mem::take(self.tiles_mut(dashboard));
        let mut next = Vec::with_capacity(previous.len());
        for arranged in tiles {
            let key = InstanceKey::new(&arranged.app_id, arranged.params.as_ref());
            if let Some(at) = previous.iter().position(|t| t.key() == key) {
                let mut tile = previous.remove(at);
                tile.size = Some(arranged.size);
                next.push(tile);
            }
        }
        next.append(&mut previous);
        *self.tiles_mut(dashboard) = next;
    }

    fn apply(&mut self, event: &SystemEvent) {
        match event {
            SystemEvent::DashboardTileAdded {
                dashboard,
                app_id,
                params,
                label,
                ..
            } => self.add(
                *dashboard,
                FoldedTile::new(app_id, params.clone(), label.clone()),
            ),
            SystemEvent::DashboardTileRemoved {
                dashboard,
                app_id,
                params,
                ..
            } => self.remove(*dashboard, &InstanceKey::new(app_id, params.as_ref())),
            SystemEvent::DashboardArranged {
                dashboard, tiles, ..
            } => self.arrange(*dashboard, tiles),
            _ => {}
        }
    }
}

/// The home thread's board as it was before ADR 0496: one tile set, Home's
/// pins, and an arrangement per layout. The phone copies the desktop until a
/// phone arrangement exists.
#[derive(Default)]
struct LegacyBoard {
    desktop: Vec<(FoldedTile, bool)>,
    phone: Option<Vec<(FoldedTile, bool)>>,
}

impl LegacyBoard {
    fn layout(&self, layout: Dashboard) -> &[(FoldedTile, bool)] {
        match (layout, &self.phone) {
            (Dashboard::Phone, Some(phone)) => phone,
            _ => &self.desktop,
        }
    }

    fn lists(&mut self) -> impl Iterator<Item = &mut Vec<(FoldedTile, bool)>> {
        std::iter::once(&mut self.desktop).chain(self.phone.as_mut())
    }

    fn is_pinned(&self, key: &InstanceKey) -> bool {
        self.desktop.iter().any(|(t, _)| &t.key() == key)
    }

    fn pin(&mut self, tile: FoldedTile) {
        if self.is_pinned(&tile.key()) {
            return;
        }
        for list in self.lists() {
            list.push((tile.clone(), false));
        }
    }

    fn unpin(&mut self, key: &InstanceKey) {
        for list in self.lists() {
            list.retain(|(t, _)| &t.key() != key);
        }
    }

    fn arrange(&mut self, layout: Dashboard, tiles: &[LegacyArrangedTile]) {
        let mut previous = self.layout(layout).to_vec();
        let mut next = Vec::with_capacity(previous.len());
        for arranged in tiles {
            let key = InstanceKey::new(&arranged.app_id, arranged.params.as_ref());
            if let Some(at) = previous.iter().position(|(t, _)| t.key() == key) {
                let (mut tile, _) = previous.remove(at);
                tile.size = Some(arranged.size);
                next.push((tile, arranged.hidden));
            }
        }
        next.append(&mut previous);
        match layout {
            Dashboard::Desktop => self.desktop = next,
            Dashboard::Phone => self.phone = Some(next),
        }
    }
}

/// Each dashboard's starting tiles: its layout of the legacy board, minus the
/// tiles hidden there. `events` are the home thread's, in thread order.
pub fn seed_dashboards(events: &[ThreadEvent]) -> FoldedDashboards {
    let mut board = LegacyBoard::default();
    for event in events {
        match event {
            ThreadEvent::WidgetPinned {
                app_id,
                params,
                label,
                ..
            } => board.pin(FoldedTile::new(app_id, params.clone(), label.clone())),
            ThreadEvent::WidgetUnpinned { app_id, params, .. } => {
                board.unpin(&InstanceKey::new(app_id, params.as_ref()));
            }
            ThreadEvent::HomeBoardArranged { layout, tiles } => board.arrange(*layout, tiles),
            _ => {}
        }
    }
    let shown = |layout: Dashboard| -> Vec<FoldedTile> {
        board
            .layout(layout)
            .iter()
            .filter(|(_, hidden)| !hidden)
            .map(|(tile, _)| tile.clone())
            .collect()
    };
    FoldedDashboards {
        phone: shown(Dashboard::Phone),
        desktop: shown(Dashboard::Desktop),
    }
}

/// Both dashboards: the seed, then the dashboard events in order.
pub fn fold_dashboards(seed: &[ThreadEvent], events: &[SystemEvent]) -> FoldedDashboards {
    let mut dashboards = seed_dashboards(seed);
    for event in events {
        dashboards.apply(event);
    }
    dashboards
}

/// The home thread's events the seed folds. None before Home exists.
async fn seed_events(pool: &PgPool) -> Result<Vec<ThreadEvent>, BoxError> {
    let Some(home) = crate::engine::home_thread::home_thread_id(pool).await? else {
        return Ok(Vec::new());
    };
    let types: Vec<&str> = STORED_WIDGET_EVENT_TYPES
        .iter()
        .copied()
        .chain(["HomeBoardArranged"])
        .collect();
    let rows: Vec<(String, Value)> = sqlx::query_as(
        "SELECT event_type, payload FROM events \
         WHERE thread_id = $1 AND event_type = ANY($2) ORDER BY sequence",
    )
    .bind(home)
    .bind(&types)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(
            |(event_type, payload)| match ThreadEvent::from_stored(&event_type, payload) {
                Ok(event) => Some(event),
                Err(e) => {
                    log!("[Dashboard] skipped an unreadable {event_type} seed row: {e}");
                    None
                }
            },
        )
        .collect())
}

/// Every dashboard event, in order. A row that does not parse is logged and
/// skipped.
async fn dashboard_events(pool: &PgPool) -> Result<Vec<SystemEvent>, BoxError> {
    let rows: Vec<(uuid::Uuid, Value)> = sqlx::query_as(
        "SELECT id, payload FROM events WHERE aggregate = 'dashboard' ORDER BY sequence",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, payload)| match SystemEvent::from_stored(payload) {
            Ok(event) => Some(event),
            Err(e) => {
                log!("[Dashboard] skipped unreadable dashboard event {id}: {e}");
                None
            }
        })
        .collect())
}

/// Both dashboards, as the events say them.
pub async fn folded_dashboards(pool: &PgPool) -> Result<FoldedDashboards, BoxError> {
    Ok(fold_dashboards(
        &seed_events(pool).await?,
        &dashboard_events(pool).await?,
    ))
}

/// One tile on a dashboard: the widget, as a shelf chip reads it, at the size
/// it shows at.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DashboardTile {
    #[serde(flatten)]
    pub widget: ThreadWidget,
    /// Always one the widget declares: an undeclared stored size fits the
    /// nearest smaller declared one, else the nearest larger.
    pub size: WidgetSize,
}

/// One dashboard's tiles, in order.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DashboardView {
    pub dashboard: Dashboard,
    pub tiles: Vec<DashboardTile>,
}

/// One dashboard. A tile whose widget no longer lists drops off.
pub async fn dashboard(
    pool: &PgPool,
    app_manager: &AppManager,
    dashboard: Dashboard,
) -> Result<DashboardView, BoxError> {
    let folded = folded_dashboards(pool).await?;
    let mut tiles = Vec::new();
    for tile in folded.tiles(dashboard) {
        let Some(app) = widgets::listed_widget(app_manager, &tile.app_id)? else {
            continue;
        };
        let size = tile
            .size
            .map_or(WidgetSize::default_of(&app.sizes), |s| s.fit(&app.sizes));
        tiles.push(DashboardTile {
            widget: ThreadWidget {
                pinned: true,
                ..ThreadWidget::of(app, tile.params.clone(), tile.label.clone())
            },
            size,
        });
    }
    Ok(DashboardView { dashboard, tiles })
}

/// A dashboard that holds this widget, with any params, or `None`.
pub async fn dashboard_holding(pool: &PgPool, app_id: &str) -> Result<Option<Dashboard>, BoxError> {
    let folded = folded_dashboards(pool).await?;
    Ok(Dashboard::ALL
        .into_iter()
        .find(|d| folded.tiles(*d).iter().any(|t| t.app_id == app_id)))
}

/// Adding a tile to a dashboard, or removing one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardChange {
    Add,
    Remove,
}

/// What a checked add must do besides writing its event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DashboardChangePlan {
    /// A tile outlives the thread that made its widget, so a widget that
    /// belongs to a thread becomes reusable first.
    pub make_reusable: bool,
}

/// Whether a tile can be added or removed, and what an add must do first.
pub async fn check_dashboard_change(
    pool: &PgPool,
    app_manager: &AppManager,
    change: DashboardChange,
    dashboard: Dashboard,
    instance: &WidgetInstance,
) -> Result<DashboardChangePlan, WidgetCheckFailed> {
    use WidgetCheckFailed::{ReadFailed, Refused};
    let folded = folded_dashboards(pool).await.map_err(ReadFailed)?;
    let held = folded.holds(
        dashboard,
        &InstanceKey::new(&instance.app_id, instance.params.as_ref()),
    );
    match change {
        DashboardChange::Add => {
            let app = widgets::widget(app_manager, &instance.app_id)?;
            widgets::check_widget_params(&app, instance.params.as_ref()).map_err(Refused)?;
            if held {
                return Err(Refused(format!(
                    "Widget '{}' with these params is already on the {} dashboard.",
                    instance.app_id,
                    dashboard.as_str()
                )));
            }
            Ok(DashboardChangePlan {
                make_reusable: !app.reusable,
            })
        }
        DashboardChange::Remove if held => Ok(DashboardChangePlan::default()),
        DashboardChange::Remove => Err(Refused(format!(
            "Widget '{}' with these params is not on the {} dashboard.",
            instance.app_id,
            dashboard.as_str()
        ))),
    }
}

/// Carry out a checked change: make the widget reusable when the plan says
/// so, then write its event. Answers whether it made the widget reusable.
pub async fn apply_dashboard_change(
    app_manager: &AppManager,
    event_bus: &EventBus,
    plan: DashboardChangePlan,
    change: DashboardChange,
    dashboard: Dashboard,
    instance: WidgetInstance,
    actor: Option<MessageOrigin>,
) -> Result<bool, BoxError> {
    // A second add racing the first finds the widget reusable already.
    let make_reusable = plan.make_reusable && !app_manager.get_app(&instance.app_id)?.reusable;
    if make_reusable {
        app_manager
            .set_widget_reusable(event_bus, &instance.app_id, true, actor.clone())
            .await?;
    }
    let WidgetInstance {
        app_id,
        params,
        label,
    } = instance;
    let params = params.filter(|p| !p.is_empty());
    let event = match change {
        DashboardChange::Add => SystemEvent::DashboardTileAdded {
            dashboard,
            app_id,
            params,
            label,
            actor,
        },
        DashboardChange::Remove => SystemEvent::DashboardTileRemoved {
            dashboard,
            app_id,
            params,
            actor,
        },
    };
    event_bus
        .emit(BusEvent::System(event))
        .await
        .map_err(|e| format!("recording the dashboard change: {e}"))?;
    Ok(make_reusable)
}

/// Whether this arrangement can be written: every tile is on this dashboard,
/// named once, at a size the widget declares.
pub async fn check_arrangement(
    pool: &PgPool,
    app_manager: &AppManager,
    dashboard: Dashboard,
    tiles: &[ArrangedTile],
) -> Result<(), WidgetCheckFailed> {
    use WidgetCheckFailed::{ReadFailed, Refused};
    let folded = folded_dashboards(pool).await.map_err(ReadFailed)?;
    let mut seen: Vec<InstanceKey> = Vec::with_capacity(tiles.len());
    for tile in tiles {
        let key = InstanceKey::new(&tile.app_id, tile.params.as_ref());
        if !folded.holds(dashboard, &key) {
            return Err(Refused(format!(
                "Widget '{}' with these params is not on the {} dashboard. Add it first.",
                tile.app_id,
                dashboard.as_str()
            )));
        }
        if seen.contains(&key) {
            return Err(Refused(format!(
                "Widget '{}' with these params is named twice.",
                tile.app_id
            )));
        }
        seen.push(key);
        let app = widgets::widget(app_manager, &tile.app_id)?;
        if !app.sizes.contains(&tile.size) {
            let declared: Vec<&str> = app.sizes.iter().map(|s| s.as_str()).collect();
            return Err(Refused(format!(
                "Widget '{}' does not take the size '{}'. It takes: {}.",
                tile.app_id,
                tile.size.as_str(),
                declared.join(", ")
            )));
        }
    }
    Ok(())
}

/// Write the one event a checked arrangement consists of.
pub async fn emit_arrangement(
    event_bus: &EventBus,
    dashboard: Dashboard,
    tiles: Vec<ArrangedTile>,
    actor: Option<MessageOrigin>,
) -> Result<(), BoxError> {
    let tiles = tiles
        .into_iter()
        .map(|t| ArrangedTile {
            params: t.params.filter(|p| !p.is_empty()),
            ..t
        })
        .collect();
    event_bus
        .emit(BusEvent::System(SystemEvent::DashboardArranged {
            dashboard,
            tiles,
            actor,
        }))
        .await
        .map_err(|e| format!("recording the dashboard arrangement: {e}"))?;
    Ok(())
}

/// The dashboard a caller means when it names none: the one its device
/// shows. `None` when there is no device, or it is not registered.
pub async fn device_dashboard(pool: &PgPool, device_id: Option<&str>) -> Option<Dashboard> {
    let device_id = device_id?;
    match crate::core::devices::DeviceStore::user_agent(pool, device_id).await {
        Ok(ua) => ua.map(|ua| Dashboard::for_user_agent(&ua)),
        Err(e) => {
            log!("[Dashboard] reading device {device_id}'s user agent failed: {e}");
            None
        }
    }
}

/// A caller's `dashboard` argument, else its device's dashboard.
pub async fn resolve_dashboard(
    pool: &PgPool,
    raw: Option<&str>,
    device_id: Option<&str>,
) -> Result<Dashboard, String> {
    match raw.map(str::trim).filter(|r| !r.is_empty()) {
        Some(raw) => Dashboard::from_arg(raw),
        None => device_dashboard(pool, device_id).await.ok_or_else(|| {
            "dashboard is required here: phone or desktop. No device names one.".to_string()
        }),
    }
}

#[cfg(test)]
#[path = "dashboard_tests.rs"]
mod tests;
