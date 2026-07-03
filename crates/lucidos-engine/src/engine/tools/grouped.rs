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
use crate::capability_manifest;
use crate::llm::tool_names as tn;

/// Resolve a grouped tool call's `action` to the legacy flat tool name its
/// handler delegates to (e.g. `triggers` + action `create` → `create_trigger`).
/// Returns a ready-to-surface `Err` string when `action` is missing or unknown.
pub(crate) fn grouped_legacy_name(
    tool: &str,
    args: &serde_json::Value,
) -> Result<&'static str, String> {
    let domain = capability_manifest::domain_for_tool(tool)
        .ok_or_else(|| format!("Error: unknown grouped tool '{}'", tool))?;
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty());
    match action {
        Some(a) => domain.legacy_tool_for_action(a).ok_or_else(|| {
            format!(
                "Error: unknown action '{}' for '{}'. Use one of: {}.",
                a,
                tool,
                domain.actions().join(", ")
            )
        }),
        None => Err(format!(
            "Error: action is required for '{}' (one of: {}).",
            tool,
            domain.actions().join(", ")
        )),
    }
}

impl LucidosEngine {
    /// Grouped `triggers` / `trigger_groups` tools → the existing scheduler
    /// handler (it already dispatches on the flat tool name).
    pub(crate) async fn execute_scheduler_grouped(
        &self,
        tool: &str,
        args: &serde_json::Value,
    ) -> ToolOutcome {
        let legacy = grouped_legacy_name(tool, args)?;
        super::to_outcome(self.execute_scheduler_tool(legacy, args).await)
    }

    /// Grouped `preferences` tool → the existing preferences handler. `device_id`
    /// is the calling device from the dispatch context (the agent never passes
    /// one), exactly as the flat get_preferences/set_preference path received it.
    pub(crate) async fn execute_preferences_grouped(
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
