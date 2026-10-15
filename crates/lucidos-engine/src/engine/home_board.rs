//! The *Home board*: Home's pinned widgets as tiles, arranged per *layout
//! class* (ADR 0432).
//!
//! The tiles are exactly the home thread's pinned widget instances. No second
//! store holds them: the board folds the home thread's widget events and its
//! `HomeBoardArranged` snapshots on read, in thread order.
//!
//! - A pin appends a tile at the widget's default size, on every layout.
//! - An unpin removes the tile from every layout.
//! - An arrange replaces one layout's list with the snapshot's pinned
//!   instances, in its order. A pinned instance the snapshot leaves out keeps
//!   its place after them, with its size and hidden flag.
//! - The phone layout copies desktop until a phone arrangement exists.

use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use crate::core::apps::WidgetSize;
use crate::core::AppManager;
use crate::engine::event_bus::{BusEvent, EventBus};
use crate::engine::thread_events::{
    ArrangedTile, EventMeta, LayoutClass, MessageOrigin, ThreadEvent,
};
use crate::engine::widgets::{
    self, InstanceKey, ThreadWidget, WidgetCheckFailed, WidgetParams, STORED_WIDGET_EVENT_TYPES,
};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// One tile in a layout as the events alone say it.
#[derive(Debug, Clone, PartialEq)]
pub struct FoldedTile {
    pub app_id: String,
    pub params: Option<WidgetParams>,
    /// `None` until an arrange gives one: the widget's default size.
    pub size: Option<WidgetSize>,
    pub hidden: bool,
}

impl FoldedTile {
    fn key(&self) -> InstanceKey {
        InstanceKey::new(&self.app_id, self.params.as_ref())
    }
}

/// The two layouts the events build. `phone` is `None` until arranged.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct FoldedBoard {
    pub desktop: Vec<FoldedTile>,
    pub phone: Option<Vec<FoldedTile>>,
}

impl FoldedBoard {
    pub fn layout(&self, layout: LayoutClass) -> &[FoldedTile] {
        match (layout, &self.phone) {
            (LayoutClass::Phone, Some(phone)) => phone,
            _ => &self.desktop,
        }
    }

    fn lists(&mut self) -> impl Iterator<Item = &mut Vec<FoldedTile>> {
        std::iter::once(&mut self.desktop).chain(self.phone.as_mut())
    }

    fn is_pinned(&self, key: &InstanceKey) -> bool {
        self.desktop.iter().any(|t| &t.key() == key)
    }

    fn pin(&mut self, app_id: &str, params: Option<WidgetParams>) {
        let key = InstanceKey::new(app_id, params.as_ref());
        if self.is_pinned(&key) {
            return;
        }
        let tile = FoldedTile {
            app_id: app_id.to_string(),
            params: params.filter(|p| !p.is_empty()),
            size: None,
            hidden: false,
        };
        for list in self.lists() {
            list.push(tile.clone());
        }
    }

    fn unpin(&mut self, key: &InstanceKey) {
        for list in self.lists() {
            list.retain(|t| &t.key() != key);
        }
    }

    fn arrange(&mut self, layout: LayoutClass, tiles: &[ArrangedTile]) {
        let previous = self.layout(layout).to_vec();
        let mut next: Vec<FoldedTile> = Vec::with_capacity(previous.len());
        for tile in tiles {
            let params = tile.params.clone().filter(|p| !p.is_empty());
            let key = InstanceKey::new(&tile.app_id, params.as_ref());
            if !self.is_pinned(&key) || next.iter().any(|t| t.key() == key) {
                continue;
            }
            next.push(FoldedTile {
                app_id: tile.app_id.clone(),
                params,
                size: Some(tile.size),
                hidden: tile.hidden,
            });
        }
        let omitted: Vec<FoldedTile> = previous
            .into_iter()
            .filter(|t| !next.iter().any(|n| n.key() == t.key()))
            .collect();
        next.extend(omitted);
        match layout {
            LayoutClass::Desktop => self.desktop = next,
            LayoutClass::Phone => self.phone = Some(next),
        }
    }
}

/// Fold the home thread's widget and board events, in thread order. A row
/// that parses as no such event is skipped.
pub fn fold_home_board(events: &[ThreadEvent]) -> FoldedBoard {
    let mut board = FoldedBoard::default();
    for event in events {
        match event {
            ThreadEvent::WidgetPinned { app_id, params, .. } => {
                board.pin(app_id, params.clone());
            }
            ThreadEvent::WidgetUnpinned { app_id, params, .. } => {
                board.unpin(&InstanceKey::new(app_id, params.as_ref()));
            }
            ThreadEvent::HomeBoardArranged { layout, tiles } => board.arrange(*layout, tiles),
            _ => {}
        }
    }
    board
}

async fn home_board_events(pool: &PgPool, home: Uuid) -> Result<Vec<ThreadEvent>, BoxError> {
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
        .filter_map(|(event_type, payload)| ThreadEvent::from_stored(&event_type, payload).ok())
        .collect())
}

async fn home_thread(pool: &PgPool) -> Result<Uuid, BoxError> {
    crate::engine::home_thread::home_thread_id(pool)
        .await?
        .ok_or_else(|| "This workspace has no home thread yet".into())
}

/// One tile on the Home board: the widget, as the shelf reads it, at the size
/// it shows at on this layout.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HomeBoardTile {
    #[serde(flatten)]
    pub widget: ThreadWidget,
    /// Always one the widget declares: an undeclared stored size fits the
    /// nearest smaller declared one, else the nearest larger.
    pub size: WidgetSize,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

/// The Home board of one layout class.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HomeBoard {
    pub thread_id: Uuid,
    pub layout: LayoutClass,
    pub tiles: Vec<HomeBoardTile>,
}

/// The Home board of `layout`, in board order. A tile whose widget is gone
/// drops off, as it does from the thread's widgets.
pub async fn home_board(
    pool: &PgPool,
    app_manager: &AppManager,
    layout: LayoutClass,
) -> Result<HomeBoard, BoxError> {
    let home = home_thread(pool).await?;
    let folded = fold_home_board(&home_board_events(pool, home).await?);
    let widgets = widgets::thread_widgets(pool, app_manager, home).await?;
    let tiles = folded
        .layout(layout)
        .iter()
        .filter_map(|tile| {
            let key = tile.key();
            let widget = widgets
                .iter()
                .find(|w| InstanceKey::new(&w.app_id, w.params.as_ref()) == key)?
                .clone();
            let size = tile
                .size
                .map_or(WidgetSize::default_of(&widget.sizes), |s| {
                    s.fit(&widget.sizes)
                });
            Some(HomeBoardTile {
                widget,
                size,
                hidden: tile.hidden,
            })
        })
        .collect();
    Ok(HomeBoard {
        thread_id: home,
        layout,
        tiles,
    })
}

/// Whether this arrangement can be written: every tile names a widget
/// instance pinned to Home, once, at a size the widget declares.
pub async fn check_arrangement(
    pool: &PgPool,
    app_manager: &AppManager,
    tiles: &[ArrangedTile],
) -> Result<Uuid, WidgetCheckFailed> {
    use WidgetCheckFailed::{ReadFailed, Refused};
    let home = home_thread(pool).await.map_err(ReadFailed)?;
    let board = fold_home_board(&home_board_events(pool, home).await.map_err(ReadFailed)?);
    let mut seen: Vec<InstanceKey> = Vec::with_capacity(tiles.len());
    for tile in tiles {
        let key = InstanceKey::new(&tile.app_id, tile.params.as_ref());
        if !board.is_pinned(&key) {
            return Err(Refused(format!(
                "Widget '{}' with these params is not pinned to Home. Pin it first.",
                tile.app_id
            )));
        }
        if seen.contains(&key) {
            return Err(Refused(format!(
                "Widget '{}' with these params is named twice.",
                tile.app_id
            )));
        }
        seen.push(key);
        let app = app_manager
            .get_app(&tile.app_id)
            .map_err(|e| match e.kind() {
                std::io::ErrorKind::NotFound => {
                    Refused(format!("No widget '{}' exists", tile.app_id))
                }
                _ => ReadFailed(format!("reading widget '{}': {e}", tile.app_id).into()),
            })?;
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
    Ok(home)
}

/// Write the one event a checked arrangement consists of.
pub async fn emit_arrangement(
    event_bus: &EventBus,
    home: Uuid,
    layout: LayoutClass,
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
        .emit(BusEvent::Thread {
            thread_id: home,
            event: ThreadEvent::HomeBoardArranged { layout, tiles },
            meta: EventMeta::with_actor(actor),
        })
        .await
        .map_err(|e| format!("recording the Home board arrangement: {e}"))?;
    Ok(())
}

#[cfg(test)]
#[path = "home_board_tests.rs"]
mod tests;
