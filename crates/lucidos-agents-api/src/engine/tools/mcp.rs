use super::super::LucidosEngine;
use crate::engine::command_guard;
use crate::engine::thread_events::{EventMeta, McpEnablementSource};
use crate::engine::tool_arguments::McpSetupRequest;
use crate::mcp::McpStopOutcome;

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

#[easy_ext::ext(McpExt)]
pub(crate) impl LucidosEngine {
    async fn execute_mcp_management_tool(
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
