use serde::{Deserialize, Serialize};

/// JSON-RPC 2.0 request.
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: u64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
}

impl JsonRpcRequest {
    pub fn new(id: u64, method: &str, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method: method.to_string(),
            params,
        }
    }
}

/// JSON-RPC 2.0 notification (no id, no params — Lucidos only sends
/// `notifications/initialized` which has no parameters).
#[derive(Debug, Serialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: &'static str,
    pub method: String,
}

impl JsonRpcNotification {
    pub fn new(method: &str) -> Self {
        Self {
            jsonrpc: "2.0",
            method: method.to_string(),
        }
    }
}

/// JSON-RPC 2.0 response (success or error).
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    pub id: Option<u64>,
    pub result: Option<serde_json::Value>,
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    pub data: Option<serde_json::Value>,
}

/// MCP `initialize` request params.
#[derive(Debug, Serialize)]
pub struct InitializeParams {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: ClientCapabilities,
    #[serde(rename = "clientInfo")]
    pub client_info: ClientInfo,
}

#[derive(Debug, Serialize)]
pub struct ClientCapabilities {}

#[derive(Debug, Serialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// MCP `initialize` response result.
#[derive(Debug, Deserialize)]
pub struct InitializeResult {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: String,
    pub capabilities: Option<serde_json::Value>,
    #[serde(rename = "serverInfo")]
    pub server_info: Option<ServerInfo>,
}

#[derive(Debug, Deserialize)]
pub struct ServerInfo {
    pub name: Option<String>,
    pub version: Option<String>,
}

/// MCP tool definition from `tools/list`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "inputSchema", default)]
    pub input_schema: Option<serde_json::Value>,
}

/// MCP `tools/list` response result.
#[derive(Debug, Deserialize)]
pub struct ToolsListResult {
    pub tools: Vec<McpTool>,
}

/// MCP `tools/call` request params.
#[derive(Debug, Serialize)]
pub struct ToolCallParams {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// MCP `tools/call` response result.
#[derive(Debug, Deserialize)]
pub struct ToolCallResult {
    pub content: Vec<ToolCallContent>,
    #[serde(rename = "isError", default)]
    pub is_error: bool,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ToolCallContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
    #[serde(rename = "resource")]
    Resource { resource: serde_json::Value },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_jsonrpc_request() {
        let req = JsonRpcRequest::new(1, "initialize", Some(serde_json::json!({"test": true})));
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"method\":\"initialize\""));
    }

    #[test]
    fn deserialize_jsonrpc_response_success() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, Some(1));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn deserialize_jsonrpc_response_error() {
        let json =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[test]
    fn deserialize_mcp_tool() {
        let json = r#"{"name":"create_object","description":"Create a 3D object","inputSchema":{"type":"object","properties":{"name":{"type":"string"}}}}"#;
        let tool: McpTool = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "create_object");
        assert_eq!(tool.description.as_deref(), Some("Create a 3D object"));
        assert!(tool.input_schema.is_some());
    }

    #[test]
    fn deserialize_tool_call_result() {
        let json = r#"{"content":[{"type":"text","text":"Object created"}],"isError":false}"#;
        let result: ToolCallResult = serde_json::from_str(json).unwrap();
        assert!(!result.is_error);
        assert_eq!(result.content.len(), 1);
        match &result.content[0] {
            ToolCallContent::Text { text } => assert_eq!(text, "Object created"),
            _ => panic!("Expected text content"),
        }
    }
}
