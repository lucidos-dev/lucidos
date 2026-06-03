//! LLM-facing schemas for the change tools (`list_changes`, `apply_change`).
//!
//! These are the in-thread mirror of the `lucidos changes list` /
//! `lucidos changes apply` CLI subcommands — the chat Lucidos Agent's
//! first-class way to inspect and apply *changes* (coding-agent-proposed
//! branches awaiting the Apply button) instead of shelling out to the CLI
//! via `run_bash`. Handlers live in `engine::tools`.

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

pub(super) fn changes_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::LIST_CHANGES.to_string(),
            description: "List pending and recently-applied changes — the coding-agent-proposed branches that are waiting on the Apply button (or were recently applied). Returns JSON `{ \"pending\": [...], \"applied\": [...], \"total_pending\": N }`, the same shape as `GET /api/v1/changes` and the `lucidos changes list` CLI. Each change carries `id`, `branch_name`, `description`, `file_count`, `requires_restart`, `thread_id`, and `thread_title`. Use this to find a pending change's `id` before calling `apply_change` — do NOT scan `ChangeProposed` events for it; this tool gives it directly. Read-only.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDefinition {
            name: tn::APPLY_CHANGE.to_string(),
            description: "Apply a pending change — merge the coding-agent-proposed branch into the workspace's main, exactly as clicking the Apply button does. Get the `change_id` from `list_changes` (`.pending[].id`). Returns the typed apply result JSON: `status` (`applied` | `noop` | `hardening` | `conflict`), `applied_commit` / `previous_commit` SHAs, `commits_applied`, `files_changed`, and `restart_required`.\n\nONLY call this when the user has asked you to apply the change — it merges code into main on their behalf. A Lucidos-source change runs `/harden` and may need an engine restart (Lucidos shows the user a restart toast; do NOT promise the changes are live immediately — the user triggers the restart). An app change ff-merges with no restart. If `status` is `conflict` or `hardening`, the merge is NOT final yet: a recovery / hardening thread is running — tell the user to watch that thread (`conflict_thread_id` / `review_thread_id`) rather than assuming success.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "change_id": {
                        "type": "string",
                        "description": "UUID of the pending change to apply. Get it from `list_changes` (`.pending[].id`)."
                    }
                },
                "required": ["change_id"]
            }),
        },
    ]
}
