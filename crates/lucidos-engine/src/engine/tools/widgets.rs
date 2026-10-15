//! Handler for the grouped `widgets` tool: the agent's path to every action
//! on a thread's widgets and its shelf (ADRs 0402, 0407). The schema is built
//! from the capability parity manifest; the HTTP routes in `api/widgets.rs`
//! share the same checks in `engine::widgets`.

use uuid::Uuid;

use super::LucidosEngine;
use crate::engine::home_board;
use crate::engine::thread_events::{ArrangedTile, LayoutClass};
use crate::engine::widgets::{self, WidgetChange, WidgetInstance};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const ACTIONS: &str = "list, thread, pin, unpin, make_reusable, stop_reusing, board, arrange";

impl LucidosEngine {
    pub(crate) async fn execute_widgets(
        &self,
        args: &serde_json::Value,
        thread_id: Uuid,
    ) -> Result<String, BoxError> {
        let action = args["action"].as_str().map(str::trim).unwrap_or("");
        match action {
            "list" => self.widgets_list(),
            "thread" => self.widgets_thread(args, thread_id).await,
            "pin" => {
                let shown_event_id = super::parse_optional_uuid(args, "shown_event_id")?;
                self.widgets_change(WidgetChange::Pin { shown_event_id }, args, thread_id)
                    .await
            }
            "unpin" => {
                self.widgets_change(WidgetChange::Unpin, args, thread_id)
                    .await
            }
            "make_reusable" => self.widgets_set_reusable(args, thread_id, true).await,
            "stop_reusing" => self.widgets_set_reusable(args, thread_id, false).await,
            "board" => self.widgets_board(layout(args)?).await,
            "arrange" => self.widgets_arrange(args, thread_id).await,
            "" => Err(format!("action is required (one of: {ACTIONS})").into()),
            other => Err(format!("unknown action '{other}'. Use one of: {ACTIONS}.").into()),
        }
    }

    fn widgets_list(&self) -> Result<String, BoxError> {
        let widgets = self.app_manager.list_reusable_widgets()?;
        if widgets.is_empty() {
            return Ok("No reusable widgets.".to_string());
        }
        Ok(widgets
            .iter()
            .map(|w| format!("- {} (id: `{}`): {}", w.name, w.id, w.description))
            .collect::<Vec<_>>()
            .join("\n"))
    }

    async fn widgets_thread(
        &self,
        args: &serde_json::Value,
        caller: Uuid,
    ) -> Result<String, BoxError> {
        let thread_id = target_thread(args, caller)?;
        let shown = widgets::thread_widgets(&self.pool, &self.app_manager, thread_id).await?;
        if shown.is_empty() {
            return Ok("No widget was shown or pinned in this thread.".to_string());
        }
        Ok(shown
            .iter()
            .map(|w| {
                let mut line = format!(
                    "- {} (id: `{}`)",
                    w.label.as_ref().unwrap_or(&w.name),
                    w.app_id
                );
                if let Some(params) = &w.params {
                    line.push_str(&format!(
                        ", params {}",
                        widgets::canonical_widget_params(params)
                    ));
                }
                if w.pinned {
                    line.push_str(", pinned to the shelf");
                }
                if w.reusable {
                    line.push_str(", reusable");
                }
                line
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }

    async fn widgets_change(
        &self,
        change: WidgetChange,
        args: &serde_json::Value,
        caller: Uuid,
    ) -> Result<String, BoxError> {
        let instance = WidgetInstance::from_args(args)?;
        let app_id = instance.app_id.clone();
        let thread_id = target_thread(args, caller)?;
        let plan = widgets::check_widget_change(
            &self.pool,
            &self.app_manager,
            change,
            &instance,
            thread_id,
        )
        .await
        .map_err(|e| e.to_string())?;
        let made_reusable = widgets::apply_widget_change(
            &self.app_manager,
            &self.event_bus,
            plan,
            change,
            instance,
            thread_id,
            Some(super::agent_tool_actor(caller)),
        )
        .await?;
        Ok(match change {
            WidgetChange::Pin { .. } if made_reusable => format!(
                "Made widget '{app_id}' reusable and pinned it to Home, \
                 so it stays on the Home board if its thread is deleted."
            ),
            WidgetChange::Pin { .. } => {
                format!("Pinned widget '{app_id}' to thread {thread_id}'s shelf.")
            }
            WidgetChange::Unpin => {
                format!("Unpinned widget '{app_id}' from thread {thread_id}'s shelf.")
            }
        })
    }

    async fn widgets_set_reusable(
        &self,
        args: &serde_json::Value,
        caller: Uuid,
        reusable: bool,
    ) -> Result<String, BoxError> {
        let app_id = app_id(args)?;
        widgets::check_set_reusable(&self.pool, &self.app_manager, app_id, reusable)
            .await
            .map_err(|e| e.to_string())?;
        self.app_manager
            .set_widget_reusable(
                &self.event_bus,
                app_id,
                reusable,
                Some(super::agent_tool_actor(caller)),
            )
            .await?;
        Ok(if reusable {
            format!("Widget '{app_id}' is reusable: any thread can show it.")
        } else {
            format!("Widget '{app_id}' is no longer reusable: its origin thread owns it again.")
        })
    }
}

impl LucidosEngine {
    async fn widgets_board(&self, layout: LayoutClass) -> Result<String, BoxError> {
        let board = home_board::home_board(&self.pool, &self.app_manager, layout).await?;
        Ok(describe_board(&board))
    }

    async fn widgets_arrange(
        &self,
        args: &serde_json::Value,
        caller: Uuid,
    ) -> Result<String, BoxError> {
        let layout = layout(args)?;
        let tiles: Vec<ArrangedTile> =
            serde_json::from_value(args["tiles"].clone()).map_err(|e| {
                format!("tiles must be a list of {{app_id, params?, size, hidden?}}: {e}")
            })?;
        let home = home_board::check_arrangement(&self.pool, &self.app_manager, &tiles)
            .await
            .map_err(|e| e.to_string())?;
        home_board::emit_arrangement(
            &self.event_bus,
            home,
            layout,
            tiles,
            Some(super::agent_tool_actor(caller)),
        )
        .await?;
        let board = home_board::home_board(&self.pool, &self.app_manager, layout).await?;
        Ok(format!(
            "Arranged the Home board.\n{}",
            describe_board(&board)
        ))
    }
}

fn layout(args: &serde_json::Value) -> Result<LayoutClass, BoxError> {
    Ok(LayoutClass::from_arg(
        args["layout"].as_str().unwrap_or(""),
    )?)
}

/// One line per tile, in board order, with the sizes each widget takes.
fn describe_board(board: &home_board::HomeBoard) -> String {
    if board.tiles.is_empty() {
        return format!(
            "The {} Home board is empty: nothing is pinned to Home.",
            board.layout.as_str()
        );
    }
    let mut out = format!("The {} Home board, in order:", board.layout.as_str());
    for tile in &board.tiles {
        let w = &tile.widget;
        let sizes: Vec<&str> = w.sizes.iter().map(|s| s.as_str()).collect();
        out.push_str(&format!(
            "\n- {} (id: `{}`), {}, takes {}",
            w.label.as_ref().unwrap_or(&w.name),
            w.app_id,
            tile.size.as_str(),
            sizes.join(", ")
        ));
        if let Some(params) = &w.params {
            out.push_str(&format!(
                ", params {}",
                widgets::canonical_widget_params(params)
            ));
        }
        if tile.hidden {
            out.push_str(", hidden");
        }
    }
    out
}

fn app_id(args: &serde_json::Value) -> Result<&str, BoxError> {
    args["app_id"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "app_id is required".into())
}

/// The thread a widget action targets: the caller's own unless named.
fn target_thread(args: &serde_json::Value, caller: Uuid) -> Result<Uuid, BoxError> {
    match &args["thread_id"] {
        serde_json::Value::Null => Ok(caller),
        serde_json::Value::String(raw) => Ok(crate::api::resolve_thread_id_arg(raw, Some(caller))?),
        other => Err(format!("thread_id must be a string, not {other}").into()),
    }
}

/// The actions the handler recognises. A test pins them to the manifest.
#[cfg(test)]
pub(crate) fn recognized_actions() -> Vec<&'static str> {
    ACTIONS.split(", ").collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability_manifest::domain_for_tool;

    #[test]
    fn handler_actions_match_manifest() {
        let mut manifest = domain_for_tool("widgets")
            .expect("widgets domain")
            .actions();
        manifest.sort();
        let mut handled = recognized_actions();
        handled.sort();
        assert_eq!(
            manifest, handled,
            "widgets handler drifted from the manifest"
        );
    }

    /// Invariant: a new pin starts at its widget's default size, so a pin
    /// takes no size on any surface (ADR 0432).
    #[test]
    fn no_pin_surface_takes_a_size() {
        let domain = domain_for_tool("widgets").expect("widgets domain");
        let op = domain
            .operations
            .iter()
            .find(|o| o.action == "pin")
            .expect("the op exists");
        assert!(op.args.iter().all(|a| !a.name.contains("size")));
        assert!(op.llm_schema.is_none(), "pin derives from its args");
        let instance =
            WidgetInstance::from_args(&serde_json::json!({"app_id": "clock", "size": "large"}))
                .unwrap();
        assert_eq!(
            instance.app_id, "clock",
            "a stray size is ignored, never stored"
        );
    }

    #[test]
    fn a_widget_action_targets_the_callers_thread_unless_named() {
        let caller = Uuid::from_u128(7);
        let other = Uuid::from_u128(9);
        assert_eq!(
            target_thread(&serde_json::json!({}), caller).unwrap(),
            caller
        );
        assert_eq!(
            target_thread(&serde_json::json!({"thread_id": "current"}), caller).unwrap(),
            caller
        );
        assert_eq!(
            target_thread(&serde_json::json!({"thread_id": other.to_string()}), caller).unwrap(),
            other
        );
        assert!(target_thread(&serde_json::json!({"thread_id": 3}), caller).is_err());
    }
}
