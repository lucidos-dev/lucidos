use super::super::LucidosEngine;
use crate::mcp::{McpStartOutcome, McpStopOutcome};
use std::collections::HashMap;

impl LucidosEngine {
    pub(crate) async fn execute_mcp_management_tool(
        &self,
        name: &str,
        args: &serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        match name {
            "setup_mcp_server" => {
                let id = args["id"].as_str().unwrap_or("");
                let server_name = args["name"].as_str().unwrap_or("");
                let command = args["command"].as_str().unwrap_or("");
                let tool_args: Vec<String> = args
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                // A malformed `env` must NOT degrade to "no env vars". LLMs
                // routinely emit non-string values (`{"PORT": 3000}`), which fails
                // `from_value`; the old `.ok().unwrap_or_default()` then dropped
                // EVERY variable, so the server was registered and started without
                // its API key and the failure surfaced much later as an opaque
                // upstream auth error. Reject it here where the cause is obvious.
                let env: HashMap<String, String> = match args.get("env") {
                    None => HashMap::new(),
                    Some(v) if v.is_null() => HashMap::new(),
                    Some(v) => match serde_json::from_value(v.clone()) {
                        Ok(m) => m,
                        Err(e) => {
                            return Ok(format!(
                                "Error: 'env' must be an object of string to string pairs (quote every value, e.g. {{\"PORT\": \"3000\"}}): {}",
                                e
                            ))
                        }
                    },
                };

                if id.is_empty() || server_name.is_empty() || command.is_empty() {
                    return Ok("Error: id, name, and command are required".to_string());
                }

                self.mcp_manager
                    .setup_server(id, server_name, command, &tool_args, &env)
                    .await
                    .map_err(|e| format!("Failed to set up MCP server: {}", e).into())
            }

            "list_mcp_servers" => {
                let statuses = self.mcp_manager.list_servers().await?;
                if statuses.is_empty() {
                    return Ok("No MCP servers configured.".to_string());
                }

                let mut output = String::from("MCP Servers:\n");
                for s in &statuses {
                    let status = if s.running { "running" } else { "stopped" };
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
                        // A stopped server pays nothing today, so its figure is
                        // conditional and has to read that way. The source says
                        // how old the list is, since a cached one can be stale.
                        let cost = if s.running {
                            format!("live, ~{} tokens per request", s.tokens)
                        } else {
                            format!("last observed, ~{} tokens if started", s.tokens)
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

            "start_mcp_server" => {
                let id = args["id"].as_str().unwrap_or("");
                if id.is_empty() {
                    return Ok("Error: id is required".to_string());
                }
                match self.mcp_manager.start_server(id).await {
                    Ok(McpStartOutcome::AlreadyRunning { tool_count }) => Ok(format!(
                        "MCP server '{}' is already running with {} tools.",
                        id, tool_count
                    )),
                    Ok(McpStartOutcome::Started { tool_count }) => Ok(format!(
                        "MCP server '{}' started with {} tools available.",
                        id, tool_count
                    )),
                    Err(e) => Err(format!("Failed to start MCP server: {}", e).into()),
                }
            }

            "stop_mcp_server" => {
                let id = args["id"].as_str().unwrap_or("");
                if id.is_empty() {
                    return Ok("Error: id is required".to_string());
                }
                match self.mcp_manager.stop_server(id).await {
                    Ok(McpStopOutcome::Stopped { name }) => {
                        Ok(format!("MCP server '{}' stopped.", name))
                    }
                    Ok(McpStopOutcome::WasNotRunning) => {
                        Ok(format!("MCP server '{}' is not running.", id))
                    }
                    Err(e) => Err(format!("Failed to stop MCP server: {}", e).into()),
                }
            }

            "remove_mcp_server" => {
                let id = args["id"].as_str().unwrap_or("");
                if id.is_empty() {
                    return Ok("Error: id is required".to_string());
                }
                // No actor: this handler is not given the calling thread, the
                // same as every other mutation in this file. The HTTP route
                // stamps the device.
                match self.mcp_manager.remove_server(id, None).await {
                    Ok(true) => Ok(format!("MCP server '{}' removed.", id)),
                    Ok(false) => Ok(format!("MCP server '{}' not found.", id)),
                    Err(e) => Err(format!("Failed to remove MCP server: {}", e).into()),
                }
            }

            _ => Ok(format!("Unknown MCP management tool: {}", name)),
        }
    }
}
