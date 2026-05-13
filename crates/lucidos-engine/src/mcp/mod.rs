pub mod client;
pub mod types;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::core::{McpServer, McpServerStore};
use crate::engine::thread_events::ActorMode;
use crate::llm::provider::ToolDefinition;
use client::McpClient;
use types::McpTool;

/// Running server state.
struct RunningServer {
    client: McpClient,
    server_config: McpServer,
}

/// Manages MCP server lifecycle: start, stop, tool discovery, tool calls.
pub struct McpManager {
    /// Currently running servers keyed by server id.
    running: Arc<Mutex<HashMap<String, RunningServer>>>,
    pool: sqlx::PgPool,
}

/// Status of an MCP server for display.
#[derive(Debug, Clone, serde::Serialize)]
pub struct McpServerStatus {
    pub id: String,
    pub name: String,
    pub running: bool,
    pub auto_approve: bool,
    pub tool_count: usize,
    pub tools: Vec<String>,
}

impl McpManager {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self {
            running: Arc::new(Mutex::new(HashMap::new())),
            pool,
        }
    }

    /// Register a new MCP server (saves to DB and connects).
    pub async fn setup_server(
        &self,
        id: &str,
        name: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let server = McpServerStore::insert(&self.pool, id, name, command, args, env).await?;

        // Try to connect immediately
        match self.start_server_internal(&server).await {
            Ok(tool_count) => Ok(format!(
                "MCP server '{}' is set up and connected with {} tools available.",
                name, tool_count
            )),
            Err(e) => {
                // Server is saved but failed to connect — that's OK, user can start later
                Ok(format!(
                    "MCP server '{}' is registered but failed to connect: {}. \
                     Make sure the server command is correct and any required application (e.g. Blender) is running. \
                     Use start_mcp_server to try again.",
                    name, e
                ))
            }
        }
    }

    /// Start a registered server.
    pub async fn start_server(
        &self,
        id: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let server = McpServerStore::get(&self.pool, id)
            .await?
            .ok_or_else(|| format!("MCP server '{}' not found", id))?;

        // Check if already running
        {
            let running = self.running.lock().await;
            if let Some(entry) = running.get(id) {
                return Ok(format!(
                    "MCP server '{}' is already running with {} tools.",
                    server.name,
                    entry.client.tools.len()
                ));
            }
        }

        let tool_count = self.start_server_internal(&server).await?;
        Ok(format!(
            "MCP server '{}' started with {} tools available.",
            server.name, tool_count
        ))
    }

    /// Internal: spawn and connect to an MCP server.
    async fn start_server_internal(
        &self,
        server: &McpServer,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        // MCP servers are invoked as part of LLM tool calls today, so default to Agent.
        // If engine-internal MCP usage is added, that call site should pass ActorMode::Engine.
        let client =
            McpClient::connect(&server.command, &server.args, &server.env, ActorMode::Agent)
                .await?;
        let tool_count = client.tools.len();

        let mut running = self.running.lock().await;
        running.insert(
            server.id.clone(),
            RunningServer {
                client,
                server_config: server.clone(),
            },
        );

        Ok(tool_count)
    }

    /// Stop a running server.
    pub async fn stop_server(
        &self,
        id: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let mut running = self.running.lock().await;
        if let Some(mut entry) = running.remove(id) {
            entry.client.shutdown().await;
            Ok(format!(
                "MCP server '{}' stopped.",
                entry.server_config.name
            ))
        } else {
            Ok(format!("MCP server '{}' is not running.", id))
        }
    }

    /// Remove a server (stop + delete from DB).
    pub async fn remove_server(
        &self,
        id: &str,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        // Stop if running
        {
            let mut running = self.running.lock().await;
            if let Some(mut entry) = running.remove(id) {
                entry.client.shutdown().await;
            }
        }

        if McpServerStore::delete(&self.pool, id).await? {
            Ok(format!("MCP server '{}' removed.", id))
        } else {
            Ok(format!("MCP server '{}' not found.", id))
        }
    }

    /// List all configured servers with their status.
    pub async fn list_servers(
        &self,
    ) -> Result<Vec<McpServerStatus>, Box<dyn std::error::Error + Send + Sync>> {
        let servers = McpServerStore::list(&self.pool).await?;
        let running = self.running.lock().await;

        let mut statuses = Vec::new();
        for server in servers {
            let is_running = running.contains_key(&server.id);
            let (tool_count, tools) = if let Some(entry) = running.get(&server.id) {
                let names: Vec<String> =
                    entry.client.tools.iter().map(|t| t.name.clone()).collect();
                (names.len(), names)
            } else {
                (0, Vec::new())
            };

            statuses.push(McpServerStatus {
                id: server.id,
                name: server.name,
                running: is_running,
                auto_approve: server.auto_approve,
                tool_count,
                tools,
            });
        }

        Ok(statuses)
    }

    /// Set auto_approve for a server.
    pub async fn set_auto_approve(
        &self,
        id: &str,
        auto_approve: bool,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        McpServerStore::set_auto_approve(&self.pool, id, auto_approve).await?;

        // Update in-memory config too
        let mut running = self.running.lock().await;
        if let Some(entry) = running.get_mut(id) {
            entry.server_config.auto_approve = auto_approve;
        }

        let action = if auto_approve { "enabled" } else { "disabled" };
        Ok(format!("Auto-approve {} for MCP server '{}'.", action, id))
    }

    /// Call an MCP tool. Starts the server on-demand if not running.
    /// Returns (result_string, server_name, auto_approve).
    pub async fn call_tool(
        &self,
        server_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<(String, String, bool), Box<dyn std::error::Error + Send + Sync>> {
        // Ensure server is running (on-demand start)
        {
            let running = self.running.lock().await;
            if !running.contains_key(server_id) {
                drop(running);
                // Start the server
                let server = McpServerStore::get(&self.pool, server_id)
                    .await?
                    .ok_or_else(|| format!("MCP server '{}' not found", server_id))?;
                self.start_server_internal(&server).await?;
            }
        }

        let mut running = self.running.lock().await;
        let entry = running
            .get_mut(server_id)
            .ok_or_else(|| format!("MCP server '{}' not running after start attempt", server_id))?;

        let server_name = entry.server_config.name.clone();
        let auto_approve = entry.server_config.auto_approve;

        let result = entry.client.call_tool(tool_name, arguments).await?;

        Ok((result, server_name, auto_approve))
    }

    /// Get tool definitions for all running servers, namespaced as mcp__{server_id}__{tool_name}.
    pub async fn get_tool_definitions(&self) -> Vec<ToolDefinition> {
        let running = self.running.lock().await;
        let mut tools = Vec::new();

        for (server_id, entry) in running.iter() {
            for mcp_tool in &entry.client.tools {
                tools.push(mcp_tool_to_definition(
                    server_id,
                    &entry.server_config.name,
                    mcp_tool,
                ));
            }
        }

        tools
    }

    /// Get tool definitions for stopped servers (name + description only, no params).
    /// Used to inform the LLM that these servers exist and can be started.
    pub async fn get_stopped_server_summaries(&self) -> Vec<String> {
        let servers = match McpServerStore::list(&self.pool).await {
            Ok(s) => s,
            Err(e) => {
                log!("[MCP] Failed to list servers for stopped summary: {}", e);
                return Vec::new();
            }
        };
        let running = self.running.lock().await;

        let mut summaries = Vec::new();
        for server in servers {
            if !running.contains_key(&server.id) {
                summaries.push(format!(
                    "- {} (id: {}) — stopped. Call start_mcp_server to activate.",
                    server.name, server.id
                ));
            }
        }

        summaries
    }

    /// Parse a namespaced tool name like "mcp__blender__create_object"
    /// into (server_id, tool_name).
    pub fn parse_mcp_tool_name(name: &str) -> Option<(String, String)> {
        let rest = name.strip_prefix("mcp__")?;
        let sep = rest.find("__")?;
        let server_id = &rest[..sep];
        let tool_name = &rest[sep + 2..];
        if server_id.is_empty() || tool_name.is_empty() {
            return None;
        }
        Some((server_id.to_string(), tool_name.to_string()))
    }
}

/// Convert an MCP tool to a Lucidos ToolDefinition with namespaced name.
fn mcp_tool_to_definition(server_id: &str, server_name: &str, tool: &McpTool) -> ToolDefinition {
    let namespaced_name = format!("mcp__{}__{}", server_id, tool.name);
    let description = format!(
        "[{}] {}",
        server_name,
        tool.description.as_deref().unwrap_or("No description")
    );
    let parameters = tool.input_schema.clone().unwrap_or_else(|| {
        serde_json::json!({
            "type": "object",
            "properties": {}
        })
    });

    ToolDefinition {
        name: namespaced_name,
        description,
        parameters,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mcp_tool_name_valid() {
        let result = McpManager::parse_mcp_tool_name("mcp__blender__create_object");
        assert_eq!(
            result,
            Some(("blender".to_string(), "create_object".to_string()))
        );
    }

    #[test]
    fn parse_mcp_tool_name_with_underscores() {
        let result = McpManager::parse_mcp_tool_name("mcp__roblox_studio__run_code");
        assert_eq!(
            result,
            Some(("roblox_studio".to_string(), "run_code".to_string()))
        );
    }

    #[test]
    fn parse_mcp_tool_name_not_mcp() {
        assert!(McpManager::parse_mcp_tool_name("read_file").is_none());
        assert!(McpManager::parse_mcp_tool_name("mcp_blender_create").is_none());
    }

    #[test]
    fn parse_mcp_tool_name_empty_parts() {
        assert!(McpManager::parse_mcp_tool_name("mcp____tool").is_none());
        assert!(McpManager::parse_mcp_tool_name("mcp__server__").is_none());
    }

    #[test]
    fn mcp_tool_to_definition_basic() {
        let tool = McpTool {
            name: "create_object".to_string(),
            description: Some("Create a 3D object".to_string()),
            input_schema: Some(serde_json::json!({
                "type": "object",
                "properties": { "name": { "type": "string" } }
            })),
        };

        let def = mcp_tool_to_definition("blender", "Blender MCP", &tool);
        assert_eq!(def.name, "mcp__blender__create_object");
        assert!(def.description.contains("[Blender MCP]"));
        assert!(def.description.contains("Create a 3D object"));
    }
}
