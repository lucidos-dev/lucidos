use super::super::LucidosEngine;
use crate::engine::command_guard;
use crate::engine::thread_events::{EventMeta, McpEnablementSource};
use crate::mcp::McpStopOutcome;
use std::collections::HashMap;

/// A `setup_mcp_server` call's arguments, read once for the tool and once for
/// the command guard. Both read them here, so the guard classifies exactly the
/// process the tool spawns.
pub(crate) struct McpSetupRequest<'a> {
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

    pub(crate) fn parse(args: &'a serde_json::Value) -> Result<Self, String> {
        let id = args["id"].as_str().unwrap_or("");
        let name = args["name"].as_str().unwrap_or("");
        let command = args["command"].as_str().unwrap_or("");
        if id.is_empty() || name.is_empty() || command.is_empty() {
            return Err("id, name, and command are required".to_string());
        }
        // The guard reads the spawn as a shell line, where `A=b` before the
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
        // A malformed `env` must NOT degrade to "no env vars". Models often
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
        // A name holding `=` sets a different variable than it names: the
        // process reads the name `PATH=/x` as `PATH`.
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

/// A server's launch line, whether it is approved, and what the guard's static
/// pass makes of it, as one `list_mcp_servers` line. `line` is masked.
fn launch_audit_line(line: Option<&str>, approved: bool, root: &std::path::Path) -> String {
    let Some(line) = line else {
        return "  Launch: unknown, the engine's working directory is unknown\n".to_string();
    };
    let approved = if approved { "approved" } else { "not approved" };
    let verdict = command_guard::static_classify(
        crate::llm::tool_names::RUN_BASH,
        &serde_json::json!({ "command": line }),
        command_guard::CommandSite::Workspace(root),
    );
    let verdict = match verdict {
        command_guard::StaticVerdict::Settled(command_guard::RiskLane::Safe) => "safe".to_string(),
        command_guard::StaticVerdict::Settled(command_guard::RiskLane::Catastrophic) => {
            "catastrophic, the guard refuses it".to_string()
        }
        command_guard::StaticVerdict::Settled(_) | command_guard::StaticVerdict::NeedsJudge(_) => {
            "needs a judgment at start".to_string()
        }
        command_guard::StaticVerdict::SettledIrreversible(category) => {
            format!("may perform {}", category.reason())
        }
    };
    format!("  Launch ({approved}, static check: {verdict}): {line}\n")
}

impl LucidosEngine {
    /// `thread_id` is the calling thread: `enable`, `disable`, `start` and
    /// `setup` act on its own enabled servers (ADR 0430).
    pub(crate) async fn execute_mcp_management_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
        thread_id: uuid::Uuid,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match name {
            "setup_mcp_server" => {
                let setup = match McpSetupRequest::parse(args) {
                    Ok(setup) => setup,
                    Err(e) => return Ok(format!("Error: {e}")),
                };
                self.mcp_manager
                    .setup_server(
                        thread_id,
                        setup.id,
                        setup.name,
                        setup.command,
                        &setup.args,
                        &setup.env,
                    )
                    .await
                    .map_err(|e| format!("Failed to set up MCP server: {}", e).into())
            }

            "list_mcp_servers" => {
                let statuses = self.mcp_manager.list_servers().await?;
                if statuses.is_empty() {
                    return Ok("No MCP servers configured.".to_string());
                }
                let enabled = self.mcp_manager.enabled_servers(thread_id).await;

                let mut output = String::from("MCP Servers:\n");
                for s in &statuses {
                    let status = match (s.running, enabled.contains(&s.id)) {
                        (true, true) => "running, enabled in this thread",
                        (true, false) => "running",
                        (false, true) => "stopped, enabled in this thread",
                        (false, false) => "stopped",
                    };
                    let approve = if s.auto_approve { ", auto-approve" } else { "" };
                    let unusable = if s.dispatchable {
                        ""
                    } else {
                        ", id cannot be used on the wire (remove it)"
                    };
                    output.push_str(&format!(
                        "\n- {} (id: {}): {}{}{}\n",
                        s.name, s.id, status, approve, unusable
                    ));
                    output.push_str(&launch_audit_line(
                        s.launch_line.as_deref(),
                        s.launch_approved,
                        self.workspace_path(),
                    ));

                    // Wire names, because that is what a call has to use. The
                    // server's own spelling is shown only where it differs.
                    let prefix = format!("mcp__{}__", s.id);
                    let offered: Vec<String> = s
                        .tools
                        .iter()
                        .filter(|t| !t.disabled)
                        .filter_map(|t| {
                            let wire = t.wire_name.as_deref()?;
                            Some(match wire.strip_prefix(&prefix) {
                                Some(part) if part != t.name => format!("{} ({})", wire, t.name),
                                _ => wire.to_string(),
                            })
                        })
                        .collect();
                    if !offered.is_empty() {
                        // Only a thread that enables a server pays for its
                        // tools (ADR 0430). The source says how old the list is,
                        // since a cached one can be stale.
                        let cost = if s.running {
                            format!("live, ~{} tokens in each thread that enables it", s.tokens)
                        } else {
                            format!(
                                "last observed, ~{} tokens in each thread that enables it",
                                s.tokens
                            )
                        };
                        output.push_str(&format!(
                            "  Tools ({}, {}): {}\n",
                            offered.len(),
                            cost,
                            offered.join(", ")
                        ));
                    } else if s.tools_source == crate::mcp::McpToolsSource::NeverObserved {
                        output.push_str("  Tools: never observed, start it to find out\n");
                    }

                    let disabled: Vec<&str> = s
                        .tools
                        .iter()
                        .filter(|t| t.disabled)
                        .filter_map(|t| t.wire_name.as_deref())
                        .collect();
                    if !disabled.is_empty() {
                        output.push_str(&format!(
                            "  Disabled ({}, ~{} tokens saved): {}\n",
                            disabled.len(),
                            s.disabled_tokens,
                            disabled.join(", ")
                        ));
                    }
                }
                Ok(output)
            }

            // `start` from a thread enables the server there too, so the
            // agent's habit of starting what it needs keeps working.
            "start_mcp_server" | "enable_mcp_server" => {
                let id = args["id"].as_str().unwrap_or("");
                if id.is_empty() {
                    return Ok("Error: id is required".to_string());
                }
                match self
                    .mcp_manager
                    .enable_for_thread(thread_id, id, McpEnablementSource::Agent, EventMeta::NONE)
                    .await
                {
                    Ok(outcome) => Ok(outcome.describe(id)),
                    Err(e) => Err(format!("Failed to enable MCP server: {}", e).into()),
                }
            }

            "disable_mcp_server" => {
                let id = args["id"].as_str().unwrap_or("");
                if id.is_empty() {
                    return Ok("Error: id is required".to_string());
                }
                match self
                    .mcp_manager
                    .disable_for_thread(thread_id, id, McpEnablementSource::Agent, EventMeta::NONE)
                    .await
                {
                    Ok(true) => Ok(format!(
                        "MCP server '{}' is disabled in this thread. Its tools leave this \
                         thread's next step. The process keeps running for other threads.",
                        id
                    )),
                    Ok(false) => Ok(format!(
                        "MCP server '{}' was not enabled in this thread.",
                        id
                    )),
                    Err(e) => Err(format!("Failed to disable MCP server: {}", e).into()),
                }
            }

            "stop_mcp_server" => {
                let id = args["id"].as_str().unwrap_or("");
                if id.is_empty() {
                    return Ok("Error: id is required".to_string());
                }
                match self
                    .mcp_manager
                    .stop_everywhere(id, Some(super::agent_tool_actor(thread_id)))
                    .await
                {
                    Ok(Some(McpStopOutcome::Stopped { name })) => Ok(format!(
                        "MCP server '{}' stopped, and it is switched off in every thread.",
                        name
                    )),
                    Ok(Some(McpStopOutcome::WasNotRunning)) => Ok(format!(
                        "MCP server '{}' was not running. It is now switched off in every thread.",
                        id
                    )),
                    Ok(None) => Ok(format!("MCP server '{}' not found.", id)),
                    Err(e) => Err(format!("Failed to stop MCP server: {}", e).into()),
                }
            }

            "remove_mcp_server" => {
                let id = args["id"].as_str().unwrap_or("");
                if id.is_empty() {
                    return Ok("Error: id is required".to_string());
                }
                match self
                    .mcp_manager
                    .remove_server(id, Some(super::agent_tool_actor(thread_id)))
                    .await
                {
                    Ok(true) => Ok(format!("MCP server '{}' removed.", id)),
                    Ok(false) => Ok(format!("MCP server '{}' not found.", id)),
                    Err(e) => Err(format!("Failed to remove MCP server: {}", e).into()),
                }
            }

            _ => Err(format!("Unknown MCP management tool: {}", name).into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{launch_audit_line, McpSetupRequest};
    use serde_json::json;

    #[test]
    fn the_launch_audit_line_states_approval_and_the_static_verdict() {
        let ws = tempfile::tempdir().unwrap();
        let root = ws.path();
        let line = launch_audit_line(Some("npx -y server"), false, root);
        assert!(line.contains("not approved"), "{line}");
        assert!(line.contains("needs a judgment at start"), "{line}");
        assert!(line.ends_with("npx -y server\n"), "{line}");
        let line = launch_audit_line(Some("echo hi"), true, root);
        assert!(line.contains("(approved, static check: safe)"), "{line}");
        let line = launch_audit_line(Some("rm -rf /"), false, root);
        assert!(line.contains("catastrophic"), "{line}");
        assert!(launch_audit_line(None, false, root).contains("unknown"));
    }

    fn refusal(args: serde_json::Value) -> String {
        match McpSetupRequest::parse(&args) {
            Ok(_) => panic!("{args} should be refused"),
            Err(e) => e,
        }
    }

    #[test]
    fn parse_reads_the_spawn_the_tool_runs() {
        let args = json!({
            "id": "fs", "name": "Files", "command": "npx",
            "args": ["-y", "server", 7], "env": { "MODE": "ro" },
        });
        let setup = McpSetupRequest::parse(&args).unwrap();
        assert_eq!(
            (setup.id, setup.name, setup.command),
            ("fs", "Files", "npx")
        );
        // A non-string argument is dropped, by the guard and the spawn alike.
        assert_eq!(setup.args, ["-y", "server"]);
        assert_eq!(setup.env.len(), 1);
    }

    #[test]
    fn parse_refuses_what_the_guard_could_misread() {
        assert!(refusal(json!({ "id": "a", "name": "A" })).contains("required"));
        assert!(refusal(json!({ "id": "a", "name": "A", "command": "./x=y" })).contains("'='"));
        let bad_name =
            json!({ "id": "a", "name": "A", "command": "cat", "env": { "PATH=/x": "" } });
        assert!(refusal(bad_name).contains("not a valid environment variable name"));
        let number = json!({ "id": "a", "name": "A", "command": "cat", "env": { "PORT": 3000 } });
        assert!(refusal(number).contains("quoted string"));
    }
}
