//! LLM-facing schemas for MCP server management tools
//! (setup/list/start/stop/remove server).


use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

/// Tool definitions for MCP server management.
pub fn get_mcp_tools() -> Vec<ToolDefinition> {
    vec![
        ToolDefinition {
            name: tn::SETUP_MCP_SERVER.to_string(),
            description: "Register and connect a new MCP (Model Context Protocol) server. The server process is spawned and tools are discovered automatically. Use web_search first to find the right package and install command for the MCP server the user wants.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Unique identifier for this server (e.g., 'blender-mcp', 'roblox-studio'). Use lowercase with hyphens."
                    },
                    "name": {
                        "type": "string",
                        "description": "Human-readable name (e.g., 'Blender MCP', 'Roblox Studio MCP')"
                    },
                    "command": {
                        "type": "string",
                        "description": "Command to run the MCP server (e.g., 'npx', 'uvx', 'node')"
                    },
                    "args": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Arguments for the command (e.g., ['blender-mcp'] for 'uvx blender-mcp')"
                    },
                    "env": {
                        "type": "object",
                        "description": "Optional environment variables for the server process",
                        "additionalProperties": { "type": "string" }
                    }
                },
                "required": ["id", "name", "command", "args"]
            }),
        },
        ToolDefinition {
            name: tn::LIST_MCP_SERVERS.to_string(),
            description: "List all configured MCP servers with their status (running/stopped) and available tools.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {}
            }),
        },
        ToolDefinition {
            name: tn::START_MCP_SERVER.to_string(),
            description: "Start a stopped MCP server by its ID.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Server ID to start"
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: tn::STOP_MCP_SERVER.to_string(),
            description: "Stop a running MCP server by its ID.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Server ID to stop"
                    }
                },
                "required": ["id"]
            }),
        },
        ToolDefinition {
            name: tn::REMOVE_MCP_SERVER.to_string(),
            description: "Remove an MCP server configuration (stops it first if running).".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Server ID to remove"
                    }
                },
                "required": ["id"]
            }),
        },
    ]
}
