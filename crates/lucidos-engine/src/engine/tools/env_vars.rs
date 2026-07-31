//! Grouped `env_vars` LLM tool (list / set / delete) — the manifest-declared
//! surface for user-managed, non-secret environment variables (Settings → System
//! → Environment variables). Mirrors `execute_notifications`: a dedicated handler
//! that dispatches on the `action` discriminator, because the `list`/`delete`
//! actions are brand-new (no retired flat predecessor) and so can't go through
//! the `grouped_legacy_name` normalization path.
//!
//! `set_environment_variable` is the retired standalone tool; it stays wired in
//! `execute_tool` as a back-compat alias and is treated here as `action = "set"`
//! (the `read_notifications` → `list` precedent). All three actions go through
//! `EnvironmentVariableStore` — the same source of truth the HTTP `/env-vars`
//! routes use — and the mutating ones emit their audit events (actor `None`:
//! agent-origin, no request headers in the tool path).

use super::{LucidosEngine, ToolOutcome};
use crate::core::environment_variables::validate_name;
use crate::core::EnvironmentVariableStore;
use crate::engine::event_bus::{BusEvent, SystemEvent};
use crate::llm::tool_names as tn;

impl LucidosEngine {
    /// Grouped `env_vars` tool dispatcher. `name` is the tool name as called so
    /// the retired `set_environment_variable` alias can default its action.
    pub(crate) async fn execute_env_vars(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> ToolOutcome {
        // The retired flat tool only ever set, and callers pass no `action`.
        let default_action = if name == tn::SET_ENVIRONMENT_VARIABLE {
            "set"
        } else {
            ""
        };
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(default_action);

        match action {
            "list" => super::to_outcome(self.env_vars_list().await),
            "set" => super::to_outcome(self.execute_environment_variable_tool(args).await),
            "delete" => super::to_outcome(self.env_vars_delete(args).await),
            "" => Err("Error: action is required (one of: list, set, delete)".to_string()),
            other => Err(format!(
                "Error: unknown action '{}'. Use one of: list, set, delete.",
                other
            )),
        }
    }

    /// `list` — every user env var as name+value pairs (non-secret by design).
    async fn env_vars_list(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let vars = match EnvironmentVariableStore::list(&self.pool).await {
            Ok(v) => v,
            Err(e) => {
                return Ok(format!(
                    "Error: Failed to list environment variables: {}",
                    e
                ))
            }
        };
        if vars.is_empty() {
            return Ok("No environment variables set.".to_string());
        }
        let items: Vec<serde_json::Value> = vars
            .iter()
            .map(|v| serde_json::json!({ "name": v.name, "value": v.value }))
            .collect();
        Ok(serde_json::to_string(&items).unwrap_or_default())
    }

    /// `delete` — remove one var by name. Mirrors the HTTP `delete_env_var`
    /// handler (delete via the store, emit `EnvironmentVariableDeleted`); a
    /// missing name is a clear no-op message, not an error.
    async fn env_vars_delete(
        &self,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let name = args["name"].as_str().unwrap_or("").trim();
        if name.is_empty() {
            return Ok("Error: name is required".to_string());
        }
        match EnvironmentVariableStore::delete(&self.pool, name).await {
            Ok(true) => {
                self.event_bus
                    .emit(BusEvent::System(SystemEvent::EnvironmentVariableDeleted {
                        name: name.to_string(),
                        actor: None,
                    }))
                    .await?;
                Ok(format!(
                    "[ACTION COMPLETED] Environment variable '{}' deleted. The change takes \
                     effect on newly spawned subprocesses — no restart needed.",
                    name
                ))
            }
            Ok(false) => Ok(format!("Environment variable '{}' not found.", name)),
            Err(e) => Ok(format!(
                "Error: Failed to delete environment variable: {}",
                e
            )),
        }
    }

    /// Handler for the `set` action (and the back-compat `set_environment_variable`
    /// alias) — define a user environment variable. Validates the name (shape +
    /// not engine-reserved), upserts, and emits `EnvironmentVariableSet` (value
    /// carried — these are non-secret). Takes effect on the next spawned
    /// subprocess; no restart.
    pub(crate) async fn execute_environment_variable_tool(
        &self,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let name = args["name"].as_str().unwrap_or("").trim();
        let value = args["value"].as_str().unwrap_or("");

        if name.is_empty() {
            return Ok("Error: name is required".to_string());
        }
        if let Err(rejection) = validate_name(name) {
            return Ok(format!("Error: {}", rejection.message(name)));
        }

        if let Err(e) = EnvironmentVariableStore::upsert(&self.pool, name, value).await {
            return Ok(format!("Error: Failed to save environment variable: {}", e));
        }

        self.event_bus
            .emit(BusEvent::System(SystemEvent::EnvironmentVariableSet {
                name: name.to_string(),
                value: value.to_string(),
                actor: None,
            }))
            .await?;

        Ok(format!(
            "[ACTION COMPLETED] Environment variable '{}' set. It is injected into newly \
             spawned subprocesses (run_bash, run_python, scheduled scripts, coding agents) — \
             no restart needed. Note these are NOT secret; for API keys/tokens use a credential.",
            name
        ))
    }
}
