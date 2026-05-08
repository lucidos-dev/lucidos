use super::super::LucidosEngine;
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
                let env: HashMap<String, String> = args
                    .get("env")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();

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
                    output.push_str(&format!(
                        "\n- {} (id: {}) — {}{}\n",
                        s.name, s.id, status, approve
                    ));
                    if s.running && !s.tools.is_empty() {
                        output.push_str(&format!(
                            "  Tools ({}): {}\n",
                            s.tool_count,
                            s.tools.join(", ")
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
                self.mcp_manager
                    .start_server(id)
                    .await
                    .map_err(|e| format!("Failed to start MCP server: {}", e).into())
            }

            "stop_mcp_server" => {
                let id = args["id"].as_str().unwrap_or("");
                if id.is_empty() {
                    return Ok("Error: id is required".to_string());
                }
                self.mcp_manager
                    .stop_server(id)
                    .await
                    .map_err(|e| format!("Failed to stop MCP server: {}", e).into())
            }

            "remove_mcp_server" => {
                let id = args["id"].as_str().unwrap_or("");
                if id.is_empty() {
                    return Ok("Error: id is required".to_string());
                }
                self.mcp_manager
                    .remove_server(id)
                    .await
                    .map_err(|e| format!("Failed to remove MCP server: {}", e).into())
            }

            _ => Ok(format!("Unknown MCP management tool: {}", name)),
        }
    }
}
