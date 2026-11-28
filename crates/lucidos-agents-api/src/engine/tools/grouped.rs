use super::{LucidosEngine, ToolOutcome};
use crate::engine::tool_arguments::grouped_legacy_name;
use crate::engine::tools::preferences::PreferencesExt;
use crate::engine::tools::scheduler::SchedulerExt;
use crate::llm::tool_names as tn;

#[easy_ext::ext(GroupedExt)]
pub(crate) impl LucidosEngine {
    async fn execute_scheduler_grouped(
        &self,
        tool: &str,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        let legacy = grouped_legacy_name(tool, args)?;
        super::to_outcome(self.execute_scheduler_tool(legacy, args, thread_id).await)
    }

    async fn execute_preferences_grouped(
        &self,
        args: &serde_json::Value,
        device_id: Option<&str>,
    ) -> ToolOutcome {
        let legacy = grouped_legacy_name(tn::PREFERENCES, args)?;
        super::to_outcome(self.execute_preferences_tool(legacy, args, device_id).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn resolves_grouped_actions_to_legacy_names() {
        assert_eq!(
            grouped_legacy_name("triggers", &json!({"action": "create"})).unwrap(),
            "create_trigger"
        );
        assert_eq!(
            grouped_legacy_name("triggers", &json!({"action": "pause"})).unwrap(),
            "pause_trigger"
        );
        assert_eq!(
            grouped_legacy_name("triggers", &json!({"action": "run"})).unwrap(),
            "run_trigger"
        );
        assert_eq!(
            grouped_legacy_name("trigger_groups", &json!({"action": "reorder"})).unwrap(),
            "reorder_trigger_groups"
        );
        assert_eq!(
            grouped_legacy_name("preferences", &json!({"action": "set"})).unwrap(),
            "set_preference"
        );
        assert_eq!(
            grouped_legacy_name("mcp", &json!({"action": "setup"})).unwrap(),
            "setup_mcp_server"
        );
        assert_eq!(
            grouped_legacy_name("plugins", &json!({"action": "register_marketplace"})).unwrap(),
            "register_plugin_marketplace"
        );
    }

    #[test]
    fn missing_or_unknown_action_errors() {
        let missing = grouped_legacy_name("triggers", &json!({})).unwrap_err();
        assert!(missing.contains("action is required"));
        let unknown = grouped_legacy_name("triggers", &json!({"action": "nope"})).unwrap_err();
        assert!(unknown.contains("unknown action"));
    }
}
