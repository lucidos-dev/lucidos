use std::collections::HashMap;

use crate::capability_manifest;
use crate::engine::command_guard;
use crate::llm::tool_names as tn;

pub fn dispatch_name<'a>(name: &'a str, args: &serde_json::Value) -> Result<&'a str, String> {
    match name {
        tn::MCP
        | tn::PLUGINS
        | tn::EVENTS
        | tn::CHANGES
        | tn::THREADS
        | tn::THREAD_QUEUE
        | tn::MEMORY
        | tn::RECALL => grouped_legacy_name(name, args),
        tn::EXECUTE_INTENT => Err(execute_intent_refusal(args)),
        other => Ok(other),
    }
}

fn execute_intent_refusal(args: &serde_json::Value) -> String {
    let intent_id = args["intent_id"]
        .as_str()
        .or_else(|| args["prompt_id"].as_str())
        .unwrap_or("<intent id>");
    format!(
        "Error: execute_intent is retired. Call load_knowhow(id: \"{intent_id}\") and do its \
         steps in this turn."
    )
}

pub fn grouped_legacy_name(tool: &str, args: &serde_json::Value) -> Result<&'static str, String> {
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

pub struct McpSetupRequest<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub command: &'a str,
    pub args: Vec<String>,
    pub env: HashMap<String, crate::core::McpEnvValue>,
}

impl<'a> McpSetupRequest<'a> {
    pub(crate) fn launch_spec(&self) -> crate::core::LaunchSpec<'_> {
        crate::core::LaunchSpec {
            command: self.command,
            args: &self.args,
            env: &self.env,
        }
    }

    pub fn parse(args: &'a serde_json::Value) -> Result<Self, String> {
        let id = args["id"].as_str().unwrap_or("");
        let name = args["name"].as_str().unwrap_or("");
        let command = args["command"].as_str().unwrap_or("");
        if id.is_empty() || name.is_empty() || command.is_empty() {
            return Err("id, name, and command are required".to_string());
        }
        // INVARIANT: The guard reads the spawn as a shell line, where `A=b` before the
        // command is an assignment. A command containing `=` would let the
        // guard classify the next word as the command instead.
        if command.contains('=') {
            return Err(format!(
                "the command '{command}' contains '=', which an MCP server command cannot \
                 contain. Move the file to a path without '=', or start it through an \
                 interpreter (for example node or python) given as the command."
            ));
        }
        let tool_args: Vec<String> = args
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        // INVARIANT: A malformed `env` must NOT degrade to "no env vars". Models often
        // write a number (`{"PORT": 3000}`). Dropping the whole map then
        // started the server without its API key, and the failure surfaced
        // later as an opaque upstream auth error.
        let env: HashMap<String, crate::core::McpEnvValue> = match args.get("env") {
            None => HashMap::new(),
            Some(v) if v.is_null() => HashMap::new(),
            Some(v) => serde_json::from_value(v.clone()).map_err(|e| {
                format!(
                    "each 'env' value must be a quoted string (e.g. {{\"PORT\": \"3000\"}}) or a credential name as {{\"credential\": \"<name>\"}}: {}",
                    e
                )
            })?,
        };
        if let Some(bad) = env.keys().find(|k| !command_guard::is_env_var_name(k)) {
            return Err(format!(
                "'{bad}' is not a valid environment variable name. Use letters, digits and \
                 '_', not starting with a digit."
            ));
        }
        Ok(Self {
            id,
            name,
            command,
            args: tool_args,
            env,
        })
    }
}

/// INVARIANT: The packages a `run_python` / `run_python_background` call installs before
/// it runs. The command guard reads the same list, so it judges what pip gets.
pub fn requested_packages(args: &serde_json::Value) -> Vec<&str> {
    args.get("packages")
        .and_then(|v| v.as_array())
        .map(|list| list.iter().filter_map(|p| p.as_str()).collect())
        .unwrap_or_default()
}
