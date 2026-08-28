use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout};

use super::types::*;
use crate::core::user_path::augmented_user_path;
use crate::engine::thread_events::ActorMode;

/// The most one JSON-RPC frame may carry, its newline excluded.
///
/// A frame past this is a framing fault, not a big result, so the call errors
/// instead of truncating. A cut frame parses as nothing. Its tail is then read
/// as further garbage frames. The caller sees "failed to parse" where the real
/// cause is a missing newline. The figure is generous next to any real tool
/// result, a base64 image included. It is also the ceiling on what one call in
/// flight can hold in memory.
pub(super) const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// The most one logged stderr line may carry.
///
/// Truncating here, unlike the frame above, because stderr is a log and not the
/// protocol. An over-long line is logged in pieces and the server keeps
/// running.
const MAX_STDERR_CHUNK_BYTES: usize = 8 * 1024;

/// Returns the MCP `client_info.name` value to use for a given actor mode.
/// MCP servers log this name to identify the calling client.
fn mcp_client_name(mode: ActorMode) -> &'static str {
    match mode {
        ActorMode::Agent => "Lucidos Agent",
        ActorMode::Engine => "Lucidos Engine",
        ActorMode::Human => "Lucidos",
    }
}

/// What one capped read off a server's pipe found.
#[derive(Debug, PartialEq, Eq)]
enum CappedRead {
    /// A line, its newline included when the stream had one.
    Line,
    /// The stream ended with nothing left to read.
    Eof,
    /// More than `cap` bytes arrived with no newline.
    OverCap,
}

/// Read one newline-terminated line into `buf`, refusing to grow past `cap`.
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
        // Checked at the top, so `room` below can never underflow, whatever
        // buffer a caller hands in.
        if buf.len() > cap {
            return Ok(CappedRead::OverCap);
        }
        // One byte over the cap, so a line of exactly `cap` bytes still has
        // room for the newline that ends it.
        let room = cap + 1 - buf.len();
        let (found, used) = {
            let available = reader.fill_buf().await?;
            if available.is_empty() {
                // A stream that ends mid-line still delivered that line. The
                // next call answers `Eof`.
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

/// The next stderr line to log, or `None` once the stream is done.
///
/// A line past [`MAX_STDERR_CHUNK_BYTES`] comes back in pieces rather than
/// erroring. A server that floods stderr therefore costs bounded memory, and
/// every byte still reaches the log.
async fn next_stderr_chunk<R: AsyncBufRead + Unpin>(reader: &mut R) -> Option<Vec<u8>> {
    let mut chunk = Vec::new();
    match read_capped_line(reader, MAX_STDERR_CHUNK_BYTES, &mut chunk).await {
        Ok(CappedRead::Line) | Ok(CappedRead::OverCap) => Some(chunk),
        Ok(CappedRead::Eof) | Err(_) => None,
    }
}

/// An active MCP client connected to a server process via stdio.
pub struct McpClient {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<ChildStdout>,
    next_id: AtomicU64,
    /// The registry id of the server this talks to. Every error names it, so a
    /// workspace running several servers can say which one misbehaved.
    server_label: String,
    /// Tools discovered from the server. Behind an `Arc` because the manager
    /// keeps a snapshot of it outside this struct's lock.
    pub tools: Arc<[McpTool]>,
    /// Server info from the initialize response.
    pub server_name: Option<String>,
}

impl McpClient {
    /// Spawn an MCP server process and complete the handshake.
    ///
    /// `server_label` is the registry id, carried so every later error can name
    /// the server that produced it.
    pub async fn connect(
        server_label: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        mode: ActorMode,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        log!(
            "[MCP] Spawning '{}': {} {}",
            server_label,
            command,
            args.join(" ")
        );

        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // Augment PATH with common interpreter / package-manager bin dirs so a
        // user-configured server like `npx` / `uvx` / `node` resolves even if
        // the engine's own PATH is a service manager's minimal one (startup
        // already augments the process PATH via `augment_process_path`; this
        // per-spawn call keeps the guarantee local and is a deduped no-op
        // then). Skip when the caller pinned PATH explicitly in `env` (don't
        // override their choice).
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
            // A NotFound on a bare (non-absolute) command is almost always the
            // PATH issue above — name it so the user can fix it, rather than a
            // bare ENOENT. (`setup_server` carries the same guidance.)
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

        // Spawn a task to drain stderr and log it, in bounded pieces.
        if let Some(stderr) = child.stderr.take() {
            let label = server_label.to_string();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stderr);
                while let Some(chunk) = next_stderr_chunk(&mut reader).await {
                    let text = String::from_utf8_lossy(&chunk);
                    let trimmed = text.trim_end();
                    if !trimmed.is_empty() {
                        log!("[MCP:stderr] [{}] {}", label, trimmed);
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

        if let Some(err) = &response.error {
            return Err(format!(
                "MCP server '{}' failed initialize: {} (code {})",
                self.server_label, err.message, err.code
            )
            .into());
        }
        // Neither a result nor an error is a broken answer, not a quiet yes.
        let Some(result) = response.result else {
            return Err(format!(
                "MCP server '{}' answered initialize with neither a result nor an error",
                self.server_label
            )
            .into());
        };
        // The result itself is read leniently: a server may omit fields we do
        // not need, and none of them gate the connection.
        if let Ok(init) = serde_json::from_value::<InitializeResult>(result) {
            log!(
                "[MCP] '{}' speaks protocol {}",
                self.server_label,
                init.protocol_version
            );
            if let Some(info) = &init.server_info {
                self.server_name = info.name.clone();
                log!(
                    "[MCP] '{}' is {} v{}",
                    self.server_label,
                    info.name.as_deref().unwrap_or("unknown"),
                    info.version.as_deref().unwrap_or("?")
                );
            }
        }

        // Send initialized notification
        self.notify("notifications/initialized").await?;

        Ok(())
    }

    /// Call tools/list and cache the results.
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
        log!(
            "[MCP] '{}' advertised {} tools",
            self.server_label,
            list.tools.len()
        );
        self.tools = list.tools.into();

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
                    // A content kind this engine has no arm for. The model sees
                    // a placeholder, so log which server and tool sent it: that
                    // is the only trace of what the model was denied.
                    ToolCallContent::Unknown => {
                        crate::log!(
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

    /// Write one newline-terminated JSON-RPC frame to the server's stdin.
    ///
    /// Both writers go through here so the failure names the server. A bare
    /// "Broken pipe" says nothing, and this pipe now breaks on purpose: an
    /// over-cap frame kills the process, and the next call is what finds out.
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

    /// Send a JSON-RPC request and wait for the response.
    async fn request(
        &mut self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<JsonRpcResponse, Box<dyn std::error::Error + Send + Sync>> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        self.write_frame(&JsonRpcRequest::new(id, method, params))
            .await?;

        // Read lines until we get a response with our id.
        // MCP servers may send notifications interleaved with responses.
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
                    // Finding where this frame ends means reading the rest of
                    // it, which is the unbounded read the cap exists to stop.
                    // So the stream cannot be resynchronised. The process is
                    // killed rather than left to answer the next call with the
                    // tail of the one that broke it.
                    let _ = self.child.start_kill();
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

                    // Anything that is not our response is skipped: a
                    // notification, or a reply to an id we are not waiting on.
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

    /// Send a JSON-RPC notification (no response expected).
    async fn notify(
        &mut self,
        method: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.write_frame(&JsonRpcNotification::new(method)).await
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

    /// The cap is a limit on the frame, so both sides of the boundary have to
    /// be stated. A line of exactly the cap is legal; one byte more is a
    /// framing fault, and the buffer never grows past the cap either way.
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

    /// A server that never sends a newline is the reported hazard. The read has
    /// to end on its own rather than on the 30s deadline.
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

    /// A stream ending mid-line still delivered that line, and only the call
    /// after it reports the end.
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

    /// The stderr drain had the same unbounded read as the frame reader, with
    /// no deadline at all. It truncates rather than erroring, so every byte
    /// still reaches the log and nothing about the connection changes.
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
