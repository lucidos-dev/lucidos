//! Generic dispatch for the manifest-grouped LLM tools (`triggers`,
//! `trigger_groups`, `preferences`, …). Each reads the `action` discriminator,
//! maps it to the legacy flat tool name via the capability parity manifest, and
//! delegates to the EXISTING per-verb handler — no handler-logic rewrite. The
//! flat tool names stay wired in `execute_tool` as back-compat aliases, so
//! cached prompts/threads that still call `create_trigger` etc. keep working.
//!
//! The grouped tool's `llm_schema` (in `capability_manifest`) deliberately keeps
//! the same arg shape the flat tool used, so the delegated handler reads `args`
//! unchanged.

use super::{LucidosEngine, ToolOutcome};
use crate::engine::tool_arguments::grouped_legacy_name;
use crate::engine::tools::preferences::PreferencesExt;
use crate::engine::tools::scheduler::SchedulerExt;
use crate::llm::tool_names as tn;

#[easy_ext::ext(GroupedExt)]
pub(crate) impl LucidosEngine {
    /// Grouped `triggers` / `trigger_groups` tools → the existing scheduler
    /// handler (it already dispatches on the flat tool name).
    async fn execute_scheduler_grouped(
        &self,
        tool: &str,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> ToolOutcome {
        let legacy = grouped_legacy_name(tool, args)?;
        super::to_outcome(self.execute_scheduler_tool(legacy, args, thread_id).await)
    }

    /// Grouped `preferences` tool → the existing preferences handler. `device_id`
    /// is the calling device from the dispatch context (the agent never passes
    /// one), exactly as the flat get_preferences/set_preference path received it.
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
        // `run` has no retired flat predecessor, but the `triggers` domain
        // dispatches every action through this map, so the manifest still has
        // to name a handler key or the action is rejected as unknown.
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
        // Phase 5 grouped tools resolve through the same path (execute_tool's
        // top-level normalization → the existing flat-alias arm).
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
