//! The View menu's Show/Hide item per pane: Thread Drawer, Thread Pane,
//! Content Pane and Dashboard.
//!
//! The page owns the state and the menu mirrors it. Each app window's page
//! derives every item from the layout it draws and pushes it with
//! [`set_view_menu_state`]. macOS has one menu for the whole app, so the shell
//! keeps the last push per window and shows the FOCUSED window's. It re-applies
//! on each push from that window and on every focus change.
//!
//! A click goes back to the focused window only, as one
//! [`VIEW_MENU_ITEM_CLICKED_EVENT`] carrying the pane id. That page decides
//! what showing or hiding the pane means in its layout.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tauri::menu::MenuItem;
use tauri::{Emitter, Manager};

use crate::app_window;

/// The Tauri event a page hears when one of its View menu items is clicked.
/// Its payload is the pane id. Mirrored in `utils/tauri.ts`.
pub(crate) const VIEW_MENU_ITEM_CLICKED_EVENT: &str = "view-menu-item-clicked";

/// A pane with a View menu item. The serde id is the menu item id, the click
/// event's payload and the page's key, all at once.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ViewMenuPane {
    ThreadDrawer,
    ThreadPane,
    ContentPane,
    Dashboard,
}

impl ViewMenuPane {
    /// Menu order, top to bottom. `VIEW_MENU_PANES` in `utils/tauri.ts` lists
    /// the same ids in the same order.
    pub(crate) const ALL: [Self; 4] = [
        Self::ThreadDrawer,
        Self::ThreadPane,
        Self::ContentPane,
        Self::Dashboard,
    ];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::ThreadDrawer => "thread-drawer",
            Self::ThreadPane => "thread-pane",
            Self::ContentPane => "content-pane",
            Self::Dashboard => "dashboard",
        }
    }

    /// The pane's canonical name (`system-knowhow/glossary.md`), in title case.
    fn title(self) -> &'static str {
        match self {
            Self::ThreadDrawer => "Thread Drawer",
            Self::ThreadPane => "Thread Pane",
            Self::ContentPane => "Content Pane",
            Self::Dashboard => "Dashboard",
        }
    }

    pub(crate) fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|pane| pane.id() == id)
    }
}

/// One item as the page last described it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ViewMenuItemState {
    pane: ViewMenuPane,
    /// The pane is on screen, so the item offers to hide it.
    shown: bool,
    /// The window has room to show the pane.
    enabled: bool,
    /// The pane's current shortcut in muda's syntax, such as `Ctrl+Shift+1`.
    accelerator: Option<String>,
}

/// What a menu item displays.
#[derive(Debug, PartialEq, Eq)]
struct Rendered {
    text: String,
    enabled: bool,
    accelerator: Option<String>,
}

/// The item for `pane`, given what its window last pushed. A window that has
/// pushed nothing gets a disabled item: no page is there to hear a click.
fn render(pane: ViewMenuPane, pushed: Option<&[ViewMenuItemState]>) -> Rendered {
    let state = pushed.and_then(|items| items.iter().find(|item| item.pane == pane));
    let verb = if state.is_some_and(|s| s.shown) {
        "Hide"
    } else {
        "Show"
    };
    Rendered {
        text: format!("{verb} {}", pane.title()),
        enabled: state.is_some_and(|s| s.enabled),
        accelerator: state.and_then(|s| s.accelerator.clone()),
    }
}

/// The menu items and every window's last push. Managed state.
#[derive(Default)]
pub(crate) struct ViewMenu {
    /// In [`ViewMenuPane::ALL`] order. Unset where the shell builds no View
    /// menu items, which makes every apply a no-op.
    items: OnceLock<Vec<MenuItem<tauri::Wry>>>,
    pushed: Mutex<BTreeMap<String, Vec<ViewMenuItemState>>>,
}

/// Build the items, disabled until a page pushes, and keep their handles.
pub(crate) fn build_items(app: &tauri::App) -> tauri::Result<Vec<MenuItem<tauri::Wry>>> {
    let items = ViewMenuPane::ALL
        .into_iter()
        .map(|pane| {
            let rendered = render(pane, None);
            MenuItem::with_id(app, pane.id(), rendered.text, false, None::<&str>)
        })
        .collect::<tauri::Result<Vec<_>>>()?;
    let _ = app.state::<ViewMenu>().items.set(items.clone());
    Ok(items)
}

/// Show `label`'s last push on the menu.
pub(crate) fn apply_for(app: &tauri::AppHandle, label: &str) {
    let view_menu = app.state::<ViewMenu>();
    let Some(items) = view_menu.items.get() else {
        return;
    };
    let pushed = view_menu.pushed.lock().unwrap();
    let pushed = pushed.get(label).map(Vec::as_slice);
    for (pane, item) in ViewMenuPane::ALL.into_iter().zip(items) {
        let rendered = render(pane, pushed);
        let applied = item
            .set_text(&rendered.text)
            .and_then(|()| item.set_enabled(rendered.enabled))
            .and_then(|()| item.set_accelerator(rendered.accelerator.as_deref()));
        if let Err(e) = applied {
            eprintln!("[Tauri] View menu item {}: {e}", pane.id());
        }
    }
}

/// A window is gone for good, so drop its last push.
pub(crate) fn forget(app: &tauri::AppHandle, label: &str) {
    app.state::<ViewMenu>().pushed.lock().unwrap().remove(label);
}

/// Send a click to the focused app window, the one whose state the menu shows.
pub(crate) fn route_click(app: &tauri::AppHandle, pane: ViewMenuPane) {
    if let Some(webview) = app_window::source_webview(app) {
        let _ = app.emit_to(webview.label(), VIEW_MENU_ITEM_CLICKED_EVENT, pane);
    }
}

/// Record the CALLING window's View menu state, and show it now if that window
/// is the one the menu speaks for.
#[tauri::command]
pub(crate) fn set_view_menu_state(window: tauri::Window, items: Vec<ViewMenuItemState>) {
    let app = window.app_handle();
    let label = window.label();
    app.state::<ViewMenu>()
        .pushed
        .lock()
        .unwrap()
        .insert(label.to_string(), items);
    if app_window::source_webview(app).is_some_and(|webview| webview.label() == label) {
        apply_for(app, label);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(pane: ViewMenuPane, shown: bool, enabled: bool) -> ViewMenuItemState {
        ViewMenuItemState {
            pane,
            shown,
            enabled,
            accelerator: None,
        }
    }

    #[test]
    fn a_shown_pane_offers_hide_and_a_hidden_one_offers_show() {
        let pushed = [
            state(ViewMenuPane::ThreadDrawer, true, true),
            state(ViewMenuPane::Dashboard, false, true),
        ];
        assert_eq!(
            render(ViewMenuPane::ThreadDrawer, Some(&pushed)).text,
            "Hide Thread Drawer"
        );
        assert_eq!(
            render(ViewMenuPane::Dashboard, Some(&pushed)).text,
            "Show Dashboard"
        );
    }

    #[test]
    fn a_window_that_pushed_nothing_gets_disabled_show_items() {
        for pane in ViewMenuPane::ALL {
            let rendered = render(pane, None);
            assert!(rendered.text.starts_with("Show "), "{pane:?}");
            assert!(!rendered.enabled, "{pane:?}");
            assert_eq!(rendered.accelerator, None, "{pane:?}");
        }
    }

    #[test]
    fn render_carries_the_enabled_flag_and_the_accelerator() {
        let mut drawer = state(ViewMenuPane::ThreadDrawer, false, false);
        drawer.accelerator = Some("Ctrl+Shift+1".to_string());
        let rendered = render(ViewMenuPane::ThreadDrawer, Some(&[drawer]));
        assert_eq!(
            rendered,
            Rendered {
                text: "Show Thread Drawer".to_string(),
                enabled: false,
                accelerator: Some("Ctrl+Shift+1".to_string()),
            }
        );
    }

    #[test]
    fn every_pane_id_round_trips_and_matches_its_serde_id() {
        for pane in ViewMenuPane::ALL {
            assert_eq!(ViewMenuPane::from_id(pane.id()), Some(pane));
            assert_eq!(
                serde_json::to_value(pane).unwrap(),
                serde_json::Value::String(pane.id().to_string())
            );
        }
        assert_eq!(ViewMenuPane::from_id("quit_lucidos"), None);
    }

    #[test]
    fn the_page_payload_deserializes() {
        let items: Vec<ViewMenuItemState> = serde_json::from_str(
            r#"[{"pane":"content-pane","shown":true,"enabled":true,"accelerator":"Ctrl+Shift+3"},
                {"pane":"dashboard","shown":false,"enabled":false,"accelerator":null}]"#,
        )
        .unwrap();
        assert_eq!(
            render(ViewMenuPane::ContentPane, Some(&items)).accelerator,
            Some("Ctrl+Shift+3".to_string())
        );
        assert!(!render(ViewMenuPane::Dashboard, Some(&items)).enabled);
    }

    /// The page spells the pane ids and the event name as its own literals.
    /// A rename on one side must fail here, not leave a silent menu.
    #[test]
    fn the_typescript_mirror_matches() {
        let ts =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/utils/tauri.ts"))
                .expect("read src/utils/tauri.ts");
        let event = format!(
            "export const VIEW_MENU_ITEM_CLICKED_EVENT = '{VIEW_MENU_ITEM_CLICKED_EVENT}';"
        );
        assert!(ts.contains(&event), "utils/tauri.ts must carry `{event}`");
        let ids = ViewMenuPane::ALL
            .iter()
            .map(|pane| format!("'{}'", pane.id()))
            .collect::<Vec<_>>()
            .join(", ");
        let panes = format!("export const VIEW_MENU_PANES = [{ids}] as const;");
        assert!(ts.contains(&panes), "utils/tauri.ts must carry `{panes}`");
    }
}
