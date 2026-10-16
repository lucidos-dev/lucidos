//! Handler for the grouped `widgets` tool: the agent's path to every action
//! on a thread's widgets, its shelf and the dashboards (ADRs 0402, 0407,
//! 0496). The schema is built from the capability parity manifest; the HTTP
//! routes in `api/widgets.rs` share the same checks.

use uuid::Uuid;

use super::LucidosEngine;
use crate::engine::dashboard::{self, DashboardChange, DashboardView};
use crate::engine::thread_events::{ArrangedTile, Dashboard};
use crate::engine::widgets::{self, WidgetChange, WidgetInstance};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const ACTIONS: &str = "list, thread, pin, unpin, make_reusable, stop_reusing, dashboard, \
                       add_to_dashboard, remove_from_dashboard, arrange";

impl LucidosEngine {
    pub(crate) async fn execute_widgets(
        &self,
        args: &serde_json::Value,
        thread_id: Uuid,
        device_id: Option<&str>,
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
            "dashboard" => {
                let which = self.named_dashboard(args, thread_id, device_id).await?;
                let view = dashboard::dashboard(&self.pool, &self.app_manager, which).await?;
                Ok(describe_dashboard(&view))
            }
            "add_to_dashboard" => {
                self.widgets_dashboard_change(DashboardChange::Add, args, thread_id, device_id)
                    .await
            }
            "remove_from_dashboard" => {
                self.widgets_dashboard_change(DashboardChange::Remove, args, thread_id, device_id)
                    .await
            }
            "arrange" => self.widgets_arrange(args, thread_id, device_id).await,
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
        widgets::check_widget_change(&self.pool, &self.app_manager, change, &instance, thread_id)
            .await
            .map_err(|e| e.to_string())?;
        widgets::emit_widget_change(
            &self.event_bus,
            change,
            instance,
            thread_id,
            Some(super::agent_tool_actor(caller)),
        )
        .await?;
        Ok(match change {
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
    /// The dashboard the agent names, else the last used device's.
    async fn named_dashboard(
        &self,
        args: &serde_json::Value,
        thread_id: Uuid,
        device_id: Option<&str>,
    ) -> Result<Dashboard, BoxError> {
        let device = self.last_used_device(thread_id, device_id).await;
        Ok(
            dashboard::resolve_dashboard(&self.pool, args["dashboard"].as_str(), device.as_deref())
                .await?,
        )
    }

    async fn widgets_dashboard_change(
        &self,
        change: DashboardChange,
        args: &serde_json::Value,
        caller: Uuid,
        device_id: Option<&str>,
    ) -> Result<String, BoxError> {
        let which = self.named_dashboard(args, caller, device_id).await?;
        let instance = WidgetInstance::from_args(args)?;
        let app_id = instance.app_id.clone();
        let plan = dashboard::check_dashboard_change(
            &self.pool,
            &self.app_manager,
            change,
            which,
            &instance,
        )
        .await
        .map_err(|e| e.to_string())?;
        let made_reusable = dashboard::apply_dashboard_change(
            &self.app_manager,
            &self.event_bus,
            plan,
            change,
            which,
            instance,
            Some(super::agent_tool_actor(caller)),
        )
        .await?;
        let name = which.as_str();
        Ok(match change {
            DashboardChange::Add if made_reusable => format!(
                "Made widget '{app_id}' reusable and added it to the {name} dashboard, \
                 so it stays there if its thread is deleted."
            ),
            DashboardChange::Add => format!("Added widget '{app_id}' to the {name} dashboard."),
            DashboardChange::Remove => {
                format!("Removed widget '{app_id}' from the {name} dashboard.")
            }
        })
    }

    async fn widgets_arrange(
        &self,
        args: &serde_json::Value,
        caller: Uuid,
        device_id: Option<&str>,
    ) -> Result<String, BoxError> {
        let which = self.named_dashboard(args, caller, device_id).await?;
        let tiles: Vec<ArrangedTile> = serde_json::from_value(args["tiles"].clone())
            .map_err(|e| format!("tiles must be a list of {{app_id, params?, size}}: {e}"))?;
        dashboard::check_arrangement(&self.pool, &self.app_manager, which, &tiles)
            .await
            .map_err(|e| e.to_string())?;
        dashboard::emit_arrangement(
            &self.event_bus,
            which,
            tiles,
            Some(super::agent_tool_actor(caller)),
        )
        .await?;
        let view = dashboard::dashboard(&self.pool, &self.app_manager, which).await?;
        Ok(format!(
            "Arranged the {} dashboard.\n{}",
            which.as_str(),
            describe_dashboard(&view)
        ))
    }
}

/// One line per tile, in order, with the sizes each widget takes.
fn describe_dashboard(view: &DashboardView) -> String {
    let name = view.dashboard.as_str();
    if view.tiles.is_empty() {
        return format!("The {name} dashboard is empty.");
    }
    let mut out = format!("The {name} dashboard, in order:");
    for tile in &view.tiles {
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
    ACTIONS.split(',').map(str::trim).collect()
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
