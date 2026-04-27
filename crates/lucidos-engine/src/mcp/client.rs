use std::collections::HashMap;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

use super::types::*;
use crate::engine::thread_events::ActorMode;

/// Returns the MCP `client_info.name` value to use for a given actor mode.
/// MCP servers log this name to identify the calling client.
pub fn mcp_client_name(mode: ActorMode) -> &'static str {
    match mode {
        ActorMode::Agent => "Lucidos Agent",
        ActorMode::Engine => "Lucidos Engine",
        ActorMode::Human => "Lucidos",
    }
}

/// An active MCP client connected to a server process via stdio.
pub struct McpClient {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: AtomicU64,
    /// Tools discovered from the server.
    pub tools: Vec<McpTool>,
    /// Server info from the initialize response.
    pub server_name: Option<String>,
}

impl McpClient {
    /// Spawn an MCP server process and complete the handshake.
    pub async fn connect(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        mode: ActorMode,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        log!("[MCP] Spawning: {} {}", command, args.join(" "));

        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (k, v) in env {
            cmd.env(k, v);
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn MCP server '{}': {}", command, e))?;

        let stdin = child
            .stdin
            .take()
            .ok_or("Failed to capture MCP server stdin")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("Failed to capture MCP server stdout")?;

        // Spawn a task to drain stderr and log it
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line).await {
                        Ok(0) => break,
                        Ok(_) => {
                            let trimmed = line.trim_end();
                            if !trimmed.is_empty() {
                                log!("[MCP:stderr] {}", trimmed);
                            }
                        }
                        Err(_) => break,
                    }
                }
            });
        }

        let mut client = Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: AtomicU64::new(1),
            tools: Vec::new(),
            server_name: None,
        };

        // Perform MCP handshake
        client.handshake(mode).await?;

        // Discover tools
        client.discover_tools().await?;

        Ok(client)
    }

    /// Send JSON-RPC initialize + initialized notification.
    async fn handshake(
        &mut self,
        mode: ActorMode,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let params = InitializeParams {
            protocol_version: "2024-11-05".to_string(),
            capabilities: ClientCapabilities {},
            client_info: ClientInfo {
                name: mcp_client_name(mode).to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };

        let response = self
            .request("initialize", Some(serde_json::to_value(&params)?))
            .await?;

        if let Some(result) = response.result {
            if let Ok(init) = serde_json::from_value::<InitializeResult>(result) {
                log!("[MCP] Server protocol: {}", init.protocol_version);
                if let Some(info) = &init.server_info {
                    self.server_name = info.name.clone();
                    log!(
                        "[MCP] Server: {} v{}",
                        info.name.as_deref().unwrap_or("unknown"),
                        info.version.as_deref().unwrap_or("?")
                    );
                }
            }
        } else if let Some(err) = &response.error {
            return Err(
                format!("MCP initialize failed: {} (code {})", err.message, err.code).into(),
            );
        }

        // Send initialized notification
        self.notify("notifications/initialized").await?;

        Ok(())
    }

    /// Call tools/list and cache the results.
    async fn discover_tools(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let response = self.request("tools/list", None).await?;

        if let Some(result) = response.result {
            let list: ToolsListResult = serde_json::from_value(result)
                .map_err(|e| format!("Failed to parse tools/list: {}", e))?;
            log!("[MCP] Discovered {} tools", list.tools.len());
            self.tools = list.tools;
        } else if let Some(err) = &response.error {
            log!("[MCP] tools/list failed: {}", err.message);
        }

        Ok(())
    }

    /// Call an MCP tool and return the result as a string.
    pub async fn call_tool(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        let params = ToolCallParams {
            name: name.to_string(),
            arguments,
        };

        let response = self
            .request("tools/call", Some(serde_json::to_value(&params)?))
            .await?;

        if let Some(err) = &response.error {
            return Err(format!(
                "MCP tool call '{}' failed: {} (code {})",
                name, err.message, err.code
            )
            .into());
        }

        if let Some(result) = response.result {
            let call_result: ToolCallResult = serde_json::from_value(result)
                .map_err(|e| format!("Failed to parse tool call result: {}", e))?;

            let mut output = String::new();
            for content in &call_result.content {
                match content {
                    ToolCallContent::Text { text } => {
                        if !output.is_empty() {
                            output.push('\n');
                        }
                        output.push_str(text);
                    }
                    ToolCallContent::Image { data, mime_type } => {
                        if !output.is_empty() {
                            output.push('\n');
                        }
                        output.push_str(&format!("[Image: {} ({} bytes)]", mime_type, data.len()));
                    }
                    ToolCallContent::Resource { .. } => {
                        if !output.is_empty() {
                            output.push('\n');
                        }
                        output.push_str("[Resource content]");
                    }
                }
            }

            if call_result.is_error {
                return Err(format!("MCP tool '{}' returned error: {}", name, output).into());
            }

            Ok(output)
        } else {
            Err(format!("MCP tool '{}': no result in response", name).into())
        }
    }

    /// Send a JSON-RPC request and wait for the response.
    async fn request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse, Box<dyn std::error::Error + Send + Sync>> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let req = JsonRpcRequest::new(id, method, params);

        let mut line = serde_json::to_string(&req)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;

        // Read lines until we get a response with our id.
        // MCP servers may send notifications interleaved with responses.
        let timeout = tokio::time::Duration::from_secs(30);
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let mut buf = String::new();
            let read_result =
                tokio::time::timeout_at(deadline, self.reader.read_line(&mut buf)).await;

            match read_result {
                Ok(Ok(0)) => {
                    return Err("MCP server closed stdout (process exited)".into());
                }
                Ok(Ok(_)) => {
                    let trimmed = buf.trim();
                    if trimmed.is_empty() {
                        continue;
                    }

                    // Try to parse as a response
                    if let Ok(resp) = serde_json::from_str::<JsonRpcResponse>(trimmed) {
                        if resp.id == Some(id) {
                            return Ok(resp);
                        }
                        // Response for different id — skip (shouldn't happen in sequential use)
                    }
                    // Could be a notification or other message — skip
                }
                Ok(Err(e)) => {
                    return Err(format!("Failed to read from MCP server: {}", e).into());
                }
                Err(_) => {
                    return Err(format!(
                        "MCP request '{}' timed out after {}s",
                        method,
                        timeout.as_secs()
                    )
                    .into());
                }
            }
        }
    }

    /// Send a JSON-RPC notification (no response expected).
    async fn notify(
        &mut self,
        method: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let notif = JsonRpcNotification::new(method);
        let mut line = serde_json::to_string(&notif)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;
        self.stdin.flush().await?;
        Ok(())
    }

    /// Kill the server process.
    pub async fn shutdown(&mut self) {
        let _ = self.child.start_kill();
        match tokio::time::timeout(std::time::Duration::from_secs(3), self.child.wait()).await {
            Ok(_) => {}
            Err(_) => {
                let _ = self.child.kill().await;
            }
        }
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_client_name_returns_lucidos_agent_for_agent_mode() {
        assert_eq!(mcp_client_name(ActorMode::Agent), "Lucidos Agent");
    }

    #[test]
    fn mcp_client_name_returns_lucidos_engine_for_engine_mode() {
        assert_eq!(mcp_client_name(ActorMode::Engine), "Lucidos Engine");
    }

    #[test]
    fn mcp_client_name_returns_lucidos_for_human_mode() {
        assert_eq!(mcp_client_name(ActorMode::Human), "Lucidos");
    }
}
