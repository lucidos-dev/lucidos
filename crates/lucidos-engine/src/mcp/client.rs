use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

use super::types::*;
use crate::core::user_path::augmented_user_path;
use crate::core::{
    argv_secret_values, inherited_secret_env, redact_argv, redact_subprocess_output,
    subprocess_secret_values,
};
use crate::engine::thread_events::ActorMode;

/// INVARIANT: The most one JSON-RPC frame may carry, its newline excluded.
///
/// A frame past this is a framing fault, not a big result, so the call errors
/// instead of truncating. A cut frame parses as nothing. Its tail is then read
/// as further garbage frames. The caller sees "failed to parse" where the real
/// cause is a missing newline. The figure is generous next to any real tool
/// result, a base64 image included. It is also the ceiling on what one call in
/// flight can hold in memory.
pub(super) const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

const MAX_STDERR_CHUNK_BYTES: usize = 8 * 1024;

fn mcp_client_name(mode: ActorMode) -> &'static str {
    match mode {
        ActorMode::Agent => "Lucidos Agent",
        ActorMode::Engine => "Lucidos Engine",
        ActorMode::Human => "Lucidos",
    }
}

fn spawn_log_line(server_label: &str, command: &str, args: &[String]) -> String {
    format!(
        "[MCP] Spawning '{server_label}': {command} {}",
        redact_argv(args)
    )
}

fn launch_secret_values(args: &[String], env: &HashMap<String, String>) -> Vec<String> {
    let env_pairs: Vec<(String, String)> = env
        .iter()
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect();
    let mut secrets = subprocess_secret_values(&env_pairs, inherited_secret_env());
    secrets.extend(argv_secret_values(args));
    secrets
}

#[derive(Debug, PartialEq, Eq)]
enum CappedRead {
    Line,
    Eof,
    OverCap,
}

/// INVARIANT: Read one newline-terminated line into `buf`, refusing to grow past `cap`.
///
/// `read_line` has no cap. A server holding its newline back could therefore
/// grow the buffer for a whole 30s deadline and take the engine down with it.
/// Reading through `fill_buf` puts the check on every chunk instead.
async fn read_capped_line<R: AsyncBufRead + Unpin + ?Sized>(
    reader: &mut R,
    cap: usize,
    buf: &mut Vec<u8>,
) -> std::io::Result<CappedRead> {
    loop {
        // INVARIANT: Checked at the top, so `room` below can never underflow, whatever
        // buffer a caller hands in.
        if buf.len() > cap {
            return Ok(CappedRead::OverCap);
        }
        let room = cap + 1 - buf.len();
        let (found, used) = {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                let ended = if buf.is_empty() {
                    CappedRead::Eof
                } else {
                    CappedRead::Line
                };
                return Ok(ended);
            }
            let window = &available[..room.min(available.len())];
            match window.iter().position(|b| *b == b'\n') {
                Some(i) => {
                    buf.extend_from_slice(&window[..=i]);
                    (true, i + 1)
                }
                None => {
                    buf.extend_from_slice(window);
                    (false, window.len())
                }
            }
        };
        reader.consume(used);
        if found {
            return Ok(CappedRead::Line);
        }
    }
}

async fn next_stderr_chunk<R: AsyncBufRead + Unpin>(reader: &mut R) -> Option<Vec<u8>> {
    let mut chunk = Vec::new();
    match read_capped_line(reader, MAX_STDERR_CHUNK_BYTES, &mut chunk).await {
        Ok(CappedRead::Line) | Ok(CappedRead::OverCap) => Some(chunk),
        Ok(CappedRead::Eof) | Err(_) => None,
    }
}

pub struct McpClient {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: AtomicU64,
    server_label: String,
    pub tools: Arc<[McpTool]>,
    pub server_name: Option<String>,
}

impl McpClient {
    pub async fn connect(
        sandbox: &crate::runtime::agent_sandbox::Sandbox,
        server_label: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        mode: ActorMode,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        log_legacy!("{}", spawn_log_line(server_label, command, args));

        let mut cmd = sandbox.command(command)?;
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        crate::runtime::spawn_env::isolate_in_process_group(&mut cmd);

        if !env.contains_key("PATH") {
            cmd.env(
                "PATH",
                augmented_user_path(
                    std::env::var_os("PATH"),
                    std::env::var_os("HOME").map(PathBuf::from).as_deref(),
                ),
            );
        }

        for (k, v) in env {
            cmd.env(k, v);
        }

        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound && !command.contains('/') {
                format!(
                    "Failed to spawn MCP server '{command}': {e}. '{command}' was not found on \
                     PATH — a packaged build only searches /usr/bin:/bin:/usr/sbin:/sbin plus \
                     common install dirs (Homebrew, npm, ~/.local/bin). Use an absolute path to \
                     the command, or install its interpreter (node/npx/uvx/python) in a standard \
                     location."
                )
            } else {
                format!("Failed to spawn MCP server '{command}': {e}")
            }
        })?;

        let stdin = child
            .stdin
            .take()
            .ok_or("Failed to capture MCP server stdin")?;
        let stdout = child
            .stdout
            .take()
            .ok_or("Failed to capture MCP server stdout")?;

        if let Some(stderr) = child.stderr.take() {
            let label = server_label.to_string();
            let secrets = launch_secret_values(args, env);
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                while let Some(chunk) = next_stderr_chunk(&mut reader).await {
                    let text = String::from_utf8_lossy(&chunk);
                    let trimmed = text.trim_end();
                    if !trimmed.is_empty() {
                        log_legacy!(
                            "[MCP:stderr] [{}] {}",
                            label,
                            redact_subprocess_output(trimmed, &secrets)
                        );
                    }
                }
            });
        }

        let mut client = Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: AtomicU64::new(1),
            server_label: server_label.to_string(),
            tools: Vec::new().into(),
            server_name: None,
        };

        client.handshake(mode).await?;

        client.discover_tools().await?;

        Ok(client)
    }

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

        if let Some(err) = &response.error {
            return Err(format!(
                "MCP server '{}' failed initialize: {} (code {})",
                self.server_label, err.message, err.code
            )
            .into());
        }
        let Some(result) = response.result else {
            return Err(format!(
                "MCP server '{}' answered initialize with neither a result nor an error",
                self.server_label
            )
            .into());
        };
        if let Ok(init) = serde_json::from_value::<InitializeResult>(result) {
            log_legacy!(
                "[MCP] '{}' speaks protocol {}",
                self.server_label,
                init.protocol_version
            );
            if let Some(info) = &init.server_info {
                self.server_name = info.name.clone();
                log_legacy!(
                    "[MCP] '{}' is {} v{}",
                    self.server_label,
                    info.name.as_deref().unwrap_or("unknown"),
                    info.version.as_deref().unwrap_or("?")
                );
            }
        }

        self.notify("notifications/initialized").await?;

        Ok(())
    }

    /// INVARIANT: Call tools/list and cache the results.
    ///
    /// A `tools/list` that FAILED is a failure, never a server with no tools.
    /// The two used to read the same: the error was logged and the connect
    /// carried on, so the page showed a healthy server offering nothing. A
    /// probe that could not run is unknown, never a no. An empty `tools` array
    /// is the other thing, and it still succeeds.
    async fn discover_tools(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let response = self.request("tools/list", None).await?;

        if let Some(err) = &response.error {
            return Err(format!(
                "MCP server '{}' failed tools/list: {} (code {}). It offers no callable \
                 tools until that answers, so it was not started.",
                self.server_label, err.message, err.code
            )
            .into());
        }
        let Some(result) = response.result else {
            return Err(format!(
                "MCP server '{}' answered tools/list with neither a result nor an error",
                self.server_label
            )
            .into());
        };

        let list: ToolsListResult = serde_json::from_value(result).map_err(|e| {
            format!(
                "MCP server '{}' sent a tools/list Lucidos could not parse: {}",
                self.server_label, e
            )
        })?;
        log_legacy!(
            "[MCP] '{}' advertised {} tools",
            self.server_label,
            list.tools.len()
        );
        self.tools = list.tools.into();

        Ok(())
    }

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
                    ToolCallContent::Unknown => {
                        crate::log_legacy!(
                            "[MCP] Server '{}' tool '{}' returned an unsupported content block",
                            self.server_label,
                            name
                        );
                        if !output.is_empty() {
                            output.push('\n');
                        }
                        output.push_str("[Unsupported content]");
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

    async fn write_frame<T: serde::Serialize>(
        &mut self,
        frame: &T,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let mut line = serde_json::to_string(frame)?;
        line.push('\n');
        let named = |e: std::io::Error| {
            format!(
                "Failed to write to MCP server '{}': {}",
                self.server_label, e
            )
        };
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(&named)?;
        self.stdin.flush().await.map_err(&named)?;
        Ok(())
    }

    async fn request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse, Box<dyn std::error::Error + Send + Sync>> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.write_frame(&JsonRpcRequest::new(id, method, params))
            .await?;

        let timeout = tokio::time::Duration::from_secs(30);
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            let mut buf: Vec<u8> = Vec::new();
            let read_result = tokio::time::timeout_at(
                deadline,
                read_capped_line(&mut self.reader, MAX_FRAME_BYTES, &mut buf),
            )
            .await;

            match read_result {
                Ok(Ok(CappedRead::Eof)) => {
                    return Err(format!(
                        "MCP server '{}' closed stdout (process exited)",
                        self.server_label
                    )
                    .into());
                }
                Ok(Ok(CappedRead::OverCap)) => {
                    // INVARIANT: Finding where this frame ends means reading the rest of
                    // it, which is the unbounded read the cap exists to stop.
                    // So the stream cannot be resynchronised. The process is
                    // killed rather than left to answer the next call with the
                    // tail of the one that broke it.
                    self.kill_now();
                    return Err(format!(
                        "MCP server '{}' sent more than {} bytes with no newline. That breaks \
                         JSON-RPC framing, so the frame was refused and the connection closed. \
                         It is a fault in the server, not a large tool result.",
                        self.server_label, MAX_FRAME_BYTES
                    )
                    .into());
                }
                Ok(Ok(CappedRead::Line)) => {
                    let trimmed = buf.trim_ascii();
                    if trimmed.is_empty() {
                        continue;
                    }

                    if let Ok(resp) = serde_json::from_slice::<JsonRpcResponse>(trimmed) {
                        if resp.id == Some(id) {
                            return Ok(resp);
                        }
                    }
                }
                Ok(Err(e)) => {
                    return Err(format!(
                        "Failed to read from MCP server '{}': {}",
                        self.server_label, e
                    )
                    .into());
                }
                Err(_) => {
                    return Err(format!(
                        "MCP request '{}' to server '{}' timed out after {}s",
                        method,
                        self.server_label,
                        timeout.as_secs()
                    )
                    .into());
                }
            }
        }
    }

    async fn notify(
        &mut self,
        method: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.write_frame(&JsonRpcNotification::new(method)).await
    }

    /// INVARIANT: Whether the server process has exited.
    ///
    /// A probe that could not run is unknown, never a "no", so an `Err` from
    /// `try_wait` answers `false`. Reading unknown as "exited" would drop a
    /// server that is still serving calls, and the caller cannot get it back
    /// mid-turn. The other way costs one stale entry until the next probe.
    pub fn has_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// INVARIANT: Kill the server and every process it started, giving them a grace to
    /// clean up first.
    ///
    /// The server leads its own process group, so signalling the group reaches
    /// its descendants. `Child::id` answers `None` once the child is reaped,
    /// which is exactly when its pid may be recycled and must not be signalled.
    pub async fn shutdown(&mut self) {
        if let Some(pid) = self.child.id() {
            crate::runtime::spawn_env::graceful_kill_child_process_group(
                pid,
                crate::runtime::claude_code::GROUP_TEARDOWN_GRACE,
            )
            .await;
        }
        let _ = self.child.start_kill();
        match tokio::time::timeout(std::time::Duration::from_secs(3), self.child.wait()).await {
            Ok(_) => {}
            Err(_) => {
                let _ = self.child.kill().await;
            }
        }
    }

    fn kill_now(&mut self) {
        if let Some(pid) = self.child.id() {
            crate::runtime::spawn_env::kill_child_process_group_now(pid);
        }
        let _ = self.child.start_kill();
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        self.kill_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spawn_log_line_never_carries_an_argument_secret() {
        let args: Vec<String> = ["-y", "search-server", "--api-key", "opaque-secret-value"]
            .map(String::from)
            .to_vec();
        let line = spawn_log_line("search", "npx", &args);
        assert_eq!(
            line,
            "[MCP] Spawning 'search': npx -y search-server --api-key [REDACTED]"
        );
    }

    #[test]
    fn a_stderr_line_echoing_a_launch_secret_is_masked() {
        let args: Vec<String> = ["--api-key", "opaque-argv-secret"]
            .map(String::from)
            .to_vec();
        let env = HashMap::from([(
            "SEARCH_API_KEY".to_string(),
            "opaque-env-secret".to_string(),
        )]);
        let secrets = launch_secret_values(&args, &env);
        assert_eq!(
            redact_subprocess_output("key opaque-argv-secret, env opaque-env-secret", &secrets),
            "key [REDACTED], env [REDACTED]"
        );
    }

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

    #[tokio::test]
    async fn read_capped_line_refuses_a_frame_over_the_cap() {
        let cap = 16;

        let at_cap = format!("{}\n", "x".repeat(cap));
        let mut reader = BufReader::new(at_cap.as_bytes());
        let mut buf = Vec::new();
        assert_eq!(
            read_capped_line(&mut reader, cap, &mut buf).await.unwrap(),
            CappedRead::Line
        );
        assert_eq!(buf.len(), cap + 1, "the newline is not part of the frame");

        let over_cap = format!("{}\n", "x".repeat(cap + 1));
        let mut reader = BufReader::new(over_cap.as_bytes());
        let mut buf = Vec::new();
        assert_eq!(
            read_capped_line(&mut reader, cap, &mut buf).await.unwrap(),
            CappedRead::OverCap
        );
        assert!(
            buf.len() <= cap + 1,
            "the buffer must stop at the cap, not follow the server: {}",
            buf.len()
        );
    }

    #[tokio::test]
    async fn read_capped_line_ends_a_newline_free_flood() {
        let cap = 64;
        let flood = "y".repeat(cap * 50);
        let mut reader = BufReader::new(flood.as_bytes());
        let mut buf = Vec::new();
        assert_eq!(
            read_capped_line(&mut reader, cap, &mut buf).await.unwrap(),
            CappedRead::OverCap
        );
        assert!(buf.len() <= cap + 1);
    }

    #[tokio::test]
    async fn read_capped_line_reports_a_partial_tail_then_end_of_stream() {
        let mut reader = BufReader::new("first\nsecond".as_bytes());

        let mut buf = Vec::new();
        assert_eq!(
            read_capped_line(&mut reader, 64, &mut buf).await.unwrap(),
            CappedRead::Line
        );
        assert_eq!(buf, b"first\n");

        let mut buf = Vec::new();
        assert_eq!(
            read_capped_line(&mut reader, 64, &mut buf).await.unwrap(),
            CappedRead::Line
        );
        assert_eq!(buf, b"second");

        let mut buf = Vec::new();
        assert_eq!(
            read_capped_line(&mut reader, 64, &mut buf).await.unwrap(),
            CappedRead::Eof
        );
        assert!(buf.is_empty());
    }

    #[tokio::test]
    async fn stderr_chunks_are_bounded() {
        let flood = "y".repeat(MAX_STDERR_CHUNK_BYTES * 2 + 5);
        let input = format!("{}\nshort\n", flood);
        let mut reader = BufReader::new(input.as_bytes());

        let mut chunks: Vec<Vec<u8>> = Vec::new();
        while let Some(chunk) = next_stderr_chunk(&mut reader).await {
            assert!(
                chunk.len() <= MAX_STDERR_CHUNK_BYTES + 1,
                "a chunk grew past the cap: {}",
                chunk.len()
            );
            chunks.push(chunk);
        }

        assert!(chunks.len() >= 3, "the flood is logged in pieces");
        assert_eq!(
            chunks.concat(),
            input.as_bytes(),
            "chunking must lose nothing"
        );
    }
}
