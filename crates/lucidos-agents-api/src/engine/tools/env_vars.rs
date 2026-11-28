use super::{LucidosEngine, ToolOutcome};
use crate::core::environment_variables::validate_agent_name;
use crate::core::EnvironmentVariableStore;
use crate::llm::tool_names as tn;

#[easy_ext::ext(EnvVarsExt)]
pub(crate) impl LucidosEngine {
    async fn execute_env_vars(&self, name: &str, args: &serde_json::Value) -> ToolOutcome {
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
        // INVARIANT: An empty string here would reach the LLM stamped as a successful
        // `list`, i.e. "no environment variables", which is the one answer that
        // is never true at this point.
        match serde_json::to_string(&items) {
            Ok(json) => Ok(json),
            Err(e) => Ok(format!(
                "Error: Failed to render {} environment variable(s): {}",
                items.len(),
                e
            )),
        }
    }

    async fn env_vars_delete(
        &self,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let name = args["name"].as_str().unwrap_or("").trim();
        if name.is_empty() {
            return Ok("Error: name is required".to_string());
        }
        match EnvironmentVariableStore::delete(&self.pool, &self.event_bus, name, None).await {
            Ok(true) => Ok(format!(
                "[ACTION COMPLETED] Environment variable '{}' deleted. Newly spawned \
                     subprocesses stop seeing it with no restart. The engine's own process \
                     environment is unchanged until the next engine restart.",
                name
            )),
            Ok(false) => Ok(format!("Environment variable '{}' not found.", name)),
            Err(e) => Ok(format!(
                "Error: Failed to delete environment variable: {}",
                e
            )),
        }
    }

    async fn execute_environment_variable_tool(
        &self,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        Ok(set_env_var(&self.pool, &self.event_bus, args).await)
    }
}

async fn set_env_var(
    pool: &sqlx::PgPool,
    event_bus: &crate::engine::event_bus::EventBus,
    args: &serde_json::Value,
) -> String {
    let name = args["name"].as_str().unwrap_or("").trim();
    let value = args["value"].as_str().unwrap_or("");

    if name.is_empty() {
        return "Error: name is required".to_string();
    }
    if let Err(rejection) = validate_agent_name(name) {
        return format!("Error: {}", rejection.message(name));
    }

    if let Err(e) = EnvironmentVariableStore::upsert(pool, event_bus, name, value, None).await {
        return format!("Error: Failed to save environment variable: {}", e);
    }

    format!(
        "[ACTION COMPLETED] Environment variable '{}' set. It is injected into newly \
         spawned subprocesses (run_bash, run_python, scheduled scripts, coding agents) with \
         no restart. The engine loads its own process environment from this store only at \
         startup, so a variable the engine itself reads needs an engine restart. Note these \
         are NOT secret; for API keys/tokens use a credential.",
        name
    )
}

#[cfg(test)]
mod tests {
    use super::set_env_var;
    use crate::core::EnvironmentVariableStore;
    use crate::engine::event_bus::EventBus;

    #[tokio::test]
    async fn an_agent_cannot_store_a_code_injecting_name() {
        let (pool, db) = crate::test_support::setup_test_db().await;
        let (bus, _callback_rx) = EventBus::new(pool.clone());

        for name in [
            "BASH_ENV",
            "LD_PRELOAD",
            "DYLD_INSERT_LIBRARIES",
            "NODE_OPTIONS",
            "GIT_SSH_COMMAND",
        ] {
            let args = serde_json::json!({ "name": name, "value": "/tmp/x" });
            let out = set_env_var(&pool, &bus, &args).await;
            assert!(out.starts_with("Error:"), "{name} was accepted: {out}");
            assert!(
                EnvironmentVariableStore::get(&pool, name)
                    .await
                    .expect("get")
                    .is_none(),
                "{name} was stored"
            );
        }

        let args = serde_json::json!({ "name": "MY_FLAG", "value": "1" });
        let out = set_env_var(&pool, &bus, &args).await;
        assert!(out.starts_with("[ACTION COMPLETED]"), "{out}");

        crate::test_support::teardown_test_db(&db).await;
    }
}
