//! Handler for the grouped `widgets` tool: the agent's path to every action
//! on a thread's widgets and its shelf (ADRs 0402, 0407). The schema is built
//! from the capability parity manifest; the HTTP routes in `api/widgets.rs`
//! share the same checks in `engine::widgets`.

use uuid::Uuid;

use super::LucidosEngine;
use crate::engine::widgets::{self, WidgetChange, WidgetInstance};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

const ACTIONS: &str = "list, thread, show, pin, unpin, make_reusable, stop_reusing";

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
            "show" => {
                self.widgets_change(WidgetChange::Show, args, thread_id)
                    .await
            }
            "pin" => {
                self.widgets_change(WidgetChange::Pin, args, thread_id)
                    .await
            }
            "unpin" => {
                self.widgets_change(WidgetChange::Unpin, args, thread_id)
                    .await
            }
            "make_reusable" => self.widgets_set_reusable(args, thread_id, true).await,
            "stop_reusing" => self.widgets_set_reusable(args, thread_id, false).await,
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
            WidgetChange::Show => format!("Showed widget '{app_id}' in thread {thread_id}."),
            WidgetChange::Pin => format!("Pinned widget '{app_id}' to thread {thread_id}'s shelf."),
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
