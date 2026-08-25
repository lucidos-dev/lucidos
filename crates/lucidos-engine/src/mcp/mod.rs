pub mod client;
pub mod types;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

use crate::core::{McpServer, McpServerStore};
use crate::engine::context::{estimate_tokens_from_chars, tool_definitions_chars};
use crate::engine::thread_events::{ActorMode, MessageOrigin};
use crate::llm::provider::ToolDefinition;
use client::McpClient;
use types::McpTool;

/// Running server state.
struct RunningServer {
    /// The stdio connection, locked per server. JSON-RPC over one pipe pair is
    /// sequential, so two calls to ONE server take turns. Calls to different
    /// servers never meet, and nothing but a call locks this.
    client: Arc<Mutex<McpClient>>,
    /// What the server advertised at connect, readable without the client
    /// lock. A running server's manifest never changes, so this is the very
    /// list `client.tools` holds. Keeping it out here is what lets tool
    /// assembly read it while a call is in flight.
    tools: Arc<[McpTool]>,
    /// The registry row as it stood when the process started, kept in step for
    /// the fields the request path reads: `auto_approve` and `disabled_tools`.
    /// `tools` on it is NOT maintained, because a running server's tools are
    /// the snapshot above and a stopped one's come off a fresh DB row.
    server_config: McpServer,
}

/// What one dispatch takes off the registry, so the guard is released before
/// anything waits on the server.
///
/// Deliberately not the whole [`RunningServer`]: only immutable handles and the
/// two display values a call reports back. Whether a tool is switched off is a
/// live read at dispatch, never a field of this.
struct CallTarget {
    client: Arc<Mutex<McpClient>>,
    tools: Arc<[McpTool]>,
    server_name: String,
    auto_approve: bool,
}

impl RunningServer {
    fn target(&self) -> CallTarget {
        CallTarget {
            client: Arc::clone(&self.client),
            tools: Arc::clone(&self.tools),
            server_name: self.server_config.name.clone(),
            auto_approve: self.server_config.auto_approve,
        }
    }
}

/// What the model is told when it calls a tool the user switched off.
fn disabled_tool_refusal(server_id: &str, wire_name: &str) -> String {
    format!(
        "MCP tool '{}' is switched off for server '{}' and was NOT run. \
         Do not retry it. Tell the user it is disabled, and let them \
         re-enable it in Settings if they want it back.",
        wire_name, server_id
    )
}

/// Manages MCP server lifecycle: start, stop, tool discovery, tool calls.
pub struct McpManager {
    /// Currently running servers keyed by server id.
    ///
    /// The guard covers map work only, never an await into a server process.
    /// It used to be held across the tool call itself. One slow server then
    /// stalled tool assembly for every thread in the workspace, and two
    /// servers could never work at once.
    running: Arc<RwLock<HashMap<String, RunningServer>>>,
    pool: sqlx::PgPool,
    /// Registry mutations announce through here. Held rather than passed per
    /// call because `McpServerStore`'s mutators require it: registering a
    /// server changes the agent's tool surface, and that is not the caller's
    /// choice to skip (see `core::announced_surfaces`).
    event_bus: crate::engine::event_bus::EventBus,
}

/// Where the tool list in an [`McpServerStatus`] came from.
///
/// `Cache` and `NeverObserved` are deliberately distinct. A server nobody has
/// connected to has an empty manifest. Reporting that as a zero-cost server
/// states something the engine does not know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpToolsSource {
    /// Read off the running process.
    Live,
    /// The manifest cached at the last successful connect. Its age is
    /// [`McpServerStatus::tools_observed_at`].
    Cache,
    /// No manifest has ever been observed, so the tool list is unknown.
    NeverObserved,
}

/// One tool of one server, with what it costs the request that carries it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct McpToolStatus {
    /// The server's own spelling, which is not always what the model is shown.
    pub name: String,
    /// The name the model is offered. `None` when no usable one exists, so the
    /// tool can never be called and costs nothing.
    pub wire_name: Option<String>,
    pub description: Option<String>,
    /// Switched off by the user, so it is absent from every request.
    pub disabled: bool,
    pub chars: usize,
    pub tokens: usize,
}

/// Status of an MCP server for display.
///
/// `chars` and `tokens` are what the ENABLED tools cost, whether or not the
/// server is currently up: for a stopped one that answers "what would this cost
/// if I switched it on". Whether the workspace is paying it right now is
/// `running`, and [`McpCostTotals`] splits the two.
#[derive(Debug, Clone, serde::Serialize)]
pub struct McpServerStatus {
    pub id: String,
    pub name: String,
    pub running: bool,
    pub auto_approve: bool,
    /// False when the stored id cannot ride a wire tool name, so no tool on
    /// this server can ever be called. Starting it is refused, and Remove is
    /// the only thing to offer.
    pub dispatchable: bool,
    pub tools_source: McpToolsSource,
    pub tools_observed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub tools: Vec<McpToolStatus>,
    pub chars: usize,
    pub tokens: usize,
    /// What the switched-off tools would add back.
    pub disabled_chars: usize,
    pub disabled_tokens: usize,
}

/// What the registered MCP servers cost the workspace, split by whether the
/// workspace is paying it.
///
/// Every token figure is [`estimate_tokens_from_chars`] of the matching char
/// figure, never a sum of per-tool tokens: the ratio is integer division, so
/// summing rounded parts drifts from the whole.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct McpCostTotals {
    pub servers: usize,
    pub running_servers: usize,
    /// Tools in every request right now.
    pub tools: usize,
    pub chars: usize,
    pub tokens: usize,
    /// What the stopped servers would add if switched on. Excludes servers
    /// whose id cannot dispatch, since those can never be switched on.
    pub stopped_tools: usize,
    pub stopped_chars: usize,
    pub stopped_tokens: usize,
    /// What the switched-off tools would add back, across every server.
    pub disabled_tools: usize,
    pub disabled_chars: usize,
    pub disabled_tokens: usize,
}

impl McpCostTotals {
    /// Roll a status list up. Kept beside the per-server figures so the header
    /// and the rows can never disagree about one workspace.
    pub fn of(servers: &[McpServerStatus]) -> Self {
        let mut totals = Self {
            servers: servers.len(),
            ..Self::default()
        };
        for server in servers {
            // A stopped server whose id cannot ride the wire contributes to no
            // figure here. It can never be started, so neither its enabled
            // tools nor its disabled ones are cost anyone could ever pay back.
            // It is still counted in `servers`, because it is still registered
            // and the page still has to show it a Remove button.
            if !server.running && !server.dispatchable {
                continue;
            }

            let enabled_count = server
                .tools
                .iter()
                .filter(|t| !t.disabled && t.wire_name.is_some())
                .count();
            if server.running {
                totals.running_servers += 1;
                totals.tools += enabled_count;
                totals.chars += server.chars;
            } else {
                totals.stopped_tools += enabled_count;
                totals.stopped_chars += server.chars;
            }
            totals.disabled_tools += server.tools.iter().filter(|t| t.disabled).count();
            totals.disabled_chars += server.disabled_chars;
        }
        totals.tokens = estimate_tokens_from_chars(totals.chars);
        totals.stopped_tokens = estimate_tokens_from_chars(totals.stopped_chars);
        totals.disabled_tokens = estimate_tokens_from_chars(totals.disabled_chars);
        totals
    }
}

/// What a start attempt resolved to. Starting an already-running server is not
/// an error, and the two read differently to the user, so the caller is told
/// which happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpStartOutcome {
    AlreadyRunning { tool_count: usize },
    Started { tool_count: usize },
}

impl McpStartOutcome {
    pub fn tool_count(self) -> usize {
        match self {
            Self::AlreadyRunning { tool_count } | Self::Started { tool_count } => tool_count,
        }
    }
}

/// What a stop attempt resolved to. Stopping something already stopped is not
/// an error, so this is a fact about the process, not a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpStopOutcome {
    Stopped { name: String },
    WasNotRunning,
}

impl McpManager {
    pub fn new(pool: sqlx::PgPool, event_bus: crate::engine::event_bus::EventBus) -> Self {
        Self {
            running: Arc::new(RwLock::new(HashMap::new())),
            pool,
            event_bus,
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
        let server = McpServerStore::register(
            &self.pool,
            &self.event_bus,
            id,
            name,
            command,
            args,
            env,
            None,
        )
        .await?;

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
                     If the command was not found, give an absolute path to it or install its \
                     interpreter (node/npx/uvx/python) in a standard location — a packaged build \
                     searches only a minimal PATH plus the common install dirs. \
                     Use start_mcp_server to try again.",
                    name, e
                ))
            }
        }
    }

    /// Start a registered server by id.
    pub async fn start_server(
        &self,
        id: &str,
    ) -> Result<McpStartOutcome, Box<dyn std::error::Error + Send + Sync>> {
        let server = McpServerStore::get(&self.pool, id)
            .await?
            .ok_or_else(|| format!("MCP server '{}' not found", id))?;
        self.start_loaded(&server).await
    }

    /// Start a server whose row the caller already has.
    ///
    /// The HTTP routes load the row first so they can answer 404 and "this id
    /// cannot ride the wire" separately from a connect failure. Re-reading it
    /// here would let a removal between the two turn a 404 into a 502.
    pub async fn start_loaded(
        &self,
        server: &McpServer,
    ) -> Result<McpStartOutcome, Box<dyn std::error::Error + Send + Sync>> {
        let already_running = self
            .running
            .read()
            .await
            .get(&server.id)
            .map(|entry| entry.tools.len());
        if let Some(tool_count) = already_running {
            return Ok(McpStartOutcome::AlreadyRunning { tool_count });
        }

        let tool_count = self.start_server_internal(server).await?;
        Ok(McpStartOutcome::Started { tool_count })
    }

    /// Internal: spawn and connect to an MCP server, then cache what it
    /// advertised.
    ///
    /// Validates the id here and not only at registration, because a stored row
    /// is not proof that it passed. Only a running server advertises tools, so
    /// this is the gate that keeps every advertised tool dispatchable. Letting
    /// an unusable id run gives the model tools no call can reach.
    ///
    /// The manifest is cached only once `connect` has returned, which is what
    /// leaves a failed start reporting the LAST good manifest instead of
    /// nothing. It emits no event: a cache refresh is an observation, and the
    /// row's `tools_observed_at` stamp is the freshness signal the settings
    /// page reads. See `core::announced_surfaces`.
    async fn start_server_internal(
        &self,
        server: &McpServer,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        crate::core::mcp_servers::validate_server_id(&server.id)?;
        // MCP servers are invoked as part of LLM tool calls today, so default to Agent.
        // If engine-internal MCP usage is added, that call site should pass ActorMode::Engine.
        let client = McpClient::connect(
            &server.id,
            &server.command,
            &server.args,
            &server.env,
            ActorMode::Agent,
        )
        .await?;
        let tools = Arc::clone(&client.tools);
        let tool_count = tools.len();

        if let Err(e) = McpServerStore::set_tools(&self.pool, &server.id, &tools).await {
            // The server IS up, so this is not a start failure. It only means
            // the page will keep quoting the previous manifest.
            log!(
                "[MCP] Failed to cache the tool manifest for '{}': {}",
                server.id,
                e
            );
        }

        self.running.write().await.insert(
            server.id.clone(),
            RunningServer {
                client: Arc::new(Mutex::new(client)),
                tools,
                server_config: server.clone(),
            },
        );

        Ok(tool_count)
    }

    /// Stop a running server.
    ///
    /// The entry leaves the registry first, and the process is killed after the
    /// guard is gone: a shutdown waits on the process, and nothing else may
    /// queue behind that.
    pub async fn stop_server(
        &self,
        id: &str,
    ) -> Result<McpStopOutcome, Box<dyn std::error::Error + Send + Sync>> {
        let removed = self.running.write().await.remove(id);
        match removed {
            Some(RunningServer {
                client,
                server_config,
                ..
            }) => {
                client.lock().await.shutdown().await;
                Ok(McpStopOutcome::Stopped {
                    name: server_config.name,
                })
            }
            None => Ok(McpStopOutcome::WasNotRunning),
        }
    }

    /// Remove a server: stop the process first, then delete the row.
    ///
    /// Returns whether a row was actually deleted. The caller needs that fact:
    /// reporting "removed" for an id that never existed is a silent success,
    /// and behind a DELETE route it is a 200 that did nothing.
    pub async fn remove_server(
        &self,
        id: &str,
        actor: Option<MessageOrigin>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        // Stop first. Deleting the row while the process runs would leave it
        // orphaned, with nothing left that names it.
        let removed = self.running.write().await.remove(id);
        if let Some(entry) = removed {
            entry.client.lock().await.shutdown().await;
        }

        McpServerStore::unregister(&self.pool, &self.event_bus, id, actor).await
    }

    /// Replace which of a server's tools are switched off, by wire name.
    /// Returns the stored set, or `None` when no such server exists.
    pub async fn set_disabled_tools(
        &self,
        id: &str,
        disabled_tools: &[String],
        actor: Option<MessageOrigin>,
    ) -> Result<Option<Vec<String>>, Box<dyn std::error::Error + Send + Sync>> {
        let stored = McpServerStore::set_disabled_tools(
            &self.pool,
            &self.event_bus,
            id,
            disabled_tools,
            actor,
        )
        .await?;

        // Mirror it onto the running snapshot, which is what the per-request
        // `get_tool_definitions` reads. Without this a tool switched off while
        // the server is up keeps riding every request until the next restart.
        if let Some(stored) = &stored {
            let mut running = self.running.write().await;
            if let Some(entry) = running.get_mut(id) {
                entry.server_config.disabled_tools = stored.clone();
            }
        }

        Ok(stored)
    }

    /// List all configured servers with their status and what they cost.
    ///
    /// Tools come from the running process when there is one and from the
    /// cached manifest when there is not, and `tools_source` says which. A
    /// stopped server used to report zero tools, which reads identically to a
    /// server that genuinely has none.
    pub async fn list_servers(
        &self,
    ) -> Result<Vec<McpServerStatus>, Box<dyn std::error::Error + Send + Sync>> {
        let servers = McpServerStore::list(&self.pool).await?;
        let running = self.running.read().await;

        Ok(servers
            .into_iter()
            .map(|server| {
                let running_entry = running.get(&server.id);
                let (tools, tools_source) = match running_entry {
                    Some(entry) => (&*entry.tools, McpToolsSource::Live),
                    None if server.tools_observed_at.is_none() => {
                        (&[][..], McpToolsSource::NeverObserved)
                    }
                    None => (server.tools.as_slice(), McpToolsSource::Cache),
                };
                server_status(&server, tools, tools_source, running_entry.is_some())
            })
            .collect())
    }

    /// Set auto_approve for a server.
    /// `actor` is the device that asked. Auto-approve decides whether this
    /// server's tool calls prompt at all. The `McpServerUpdated` row is
    /// therefore the audit trail for a widened grant, and it named nobody: the
    /// actor was hardcoded `None` here.
    pub async fn set_auto_approve(
        &self,
        id: &str,
        auto_approve: bool,
        actor: Option<crate::engine::thread_events::MessageOrigin>,
    ) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        McpServerStore::set_auto_approve(&self.pool, &self.event_bus, id, auto_approve, actor)
            .await?;

        // Update in-memory config too
        let mut running = self.running.write().await;
        if let Some(entry) = running.get_mut(id) {
            entry.server_config.auto_approve = auto_approve;
        }

        let action = if auto_approve { "enabled" } else { "disabled" };
        Ok(format!("Auto-approve {} for MCP server '{}'.", action, id))
    }

    /// Why this call must not reach the process, or `None` to go ahead.
    ///
    /// Read live off the registry, never off a snapshot. Every condition here
    /// is the user's, and every one can move while the call waits its turn: a
    /// call can sit on the client lock for the whole MCP timeout. Answering
    /// from what was true on arrival is how a Stop, a Remove or a switched-off
    /// tool lands in that window and is ignored.
    async fn dispatch_refusal(
        &self,
        server_id: &str,
        wire_name: &str,
        client: &Arc<Mutex<McpClient>>,
    ) -> Option<String> {
        let running = self.running.read().await;
        let Some(entry) = running.get(server_id) else {
            return Some(format!(
                "MCP server '{}' was stopped or removed, so tool '{}' was NOT run. \
                 Do not retry it. Tell the user, and let them start the server \
                 again if they want it back.",
                server_id, wire_name
            ));
        };
        if !Arc::ptr_eq(&entry.client, client) {
            return Some(format!(
                "MCP server '{}' was restarted, so tool '{}' was NOT run: the \
                 process it was called against is gone. Retry it.",
                server_id, wire_name
            ));
        }
        entry
            .server_config
            .disabled_tools
            .iter()
            .any(|d| d == wire_name)
            .then(|| disabled_tool_refusal(server_id, wire_name))
    }

    /// Call an MCP tool. Starts the server on-demand if not running.
    /// Returns (result_string, server_name, auto_approve).
    pub async fn call_tool(
        &self,
        server_id: &str,
        tool_name: &str,
        arguments: serde_json::Value,
    ) -> Result<(String, String, bool), Box<dyn std::error::Error + Send + Sync>> {
        // Take a handle and let the registry go. Each lookup is its own
        // statement so the guard is dropped at the semicolon: holding a read
        // guard across the on-demand start below would deadlock against the
        // write the start ends with.
        let existing = self
            .running
            .read()
            .await
            .get(server_id)
            .map(RunningServer::target);
        let target = match existing {
            Some(target) => target,
            None => {
                let server = McpServerStore::get(&self.pool, server_id)
                    .await?
                    .ok_or_else(|| format!("MCP server '{}' not found", server_id))?;
                self.start_server_internal(&server).await?;
                self.running
                    .read()
                    .await
                    .get(server_id)
                    .map(RunningServer::target)
                    .ok_or_else(|| {
                        format!("MCP server '{}' not running after start attempt", server_id)
                    })?
            }
        };

        // Dispatch is the gate, not the definition list. Omitting a disabled
        // tool from the next request is what makes the switch cheap. It is not
        // what enforces it: a call the model already generated is still in
        // flight, and a resumed turn carries the old definitions. Refusing here
        // makes switching a tool off take effect on the call.
        //
        // Asked twice, and the second one is the gate. This is the cheap
        // refusal, so an already-off tool never queues behind a slow call just
        // to be turned away at the end of it.
        let wire_name = format!("mcp__{}__{}", server_id, tool_name);
        if let Some(refusal) = self
            .dispatch_refusal(server_id, &wire_name, &target.client)
            .await
        {
            return Err(refusal.into());
        }

        // `tool_name` is the wire name the model was shown, which is not always
        // what the server calls the tool. Resolve before dispatching.
        let tool = resolve_wire_tool_name(server_id, &target.tools, tool_name)
            .map(|t| t.name.clone())
            .ok_or_else(|| {
                let available: Vec<String> = wire_tool_names(server_id, &target.tools)
                    .into_iter()
                    .flatten()
                    .collect();
                format!(
                    "MCP server '{}' has no tool named '{}'. Available: {}",
                    server_id,
                    tool_name,
                    available.join(", ")
                )
            })?;

        // The one lock held across the call, and it covers this server alone.
        let result = {
            let mut client = target.client.lock().await;
            // The real gate, asked once this call has the server to itself.
            // Waiting for it can take the whole MCP timeout, and whatever the
            // user did in that window has to win.
            if let Some(refusal) = self
                .dispatch_refusal(server_id, &wire_name, &target.client)
                .await
            {
                return Err(refusal.into());
            }
            client.call_tool(&tool, arguments).await?
        };

        Ok((result, target.server_name, target.auto_approve))
    }

    /// Get tool definitions for all running servers, namespaced as mcp__{server_id}__{tool_name}.
    ///
    /// Runs once per LLM call, off the manifest snapshots, so it never waits on
    /// a tool call in flight.
    pub async fn get_tool_definitions(&self) -> Vec<ToolDefinition> {
        let running = self.running.read().await;
        let mut tools = Vec::new();

        for (server_id, entry) in running.iter() {
            for offer in tool_offers(
                server_id,
                &entry.server_config.name,
                &entry.tools,
                &entry.server_config.disabled_tools,
            ) {
                if offer.wire_name.is_none() {
                    log!(
                        "[MCP] Tool '{}' on server '{}' has no name that fits the tool-name limit, not offering it",
                        offer.tool.name,
                        server_id
                    );
                    continue;
                }
                if let Some(definition) = offer.into_offered() {
                    tools.push(definition);
                }
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
        let running = self.running.read().await;

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

/// One tool of one server, resolved against everything that decides whether a
/// request carries it.
///
/// The single place the request path and the cost report agree on what a server
/// contributes. Two functions answering that separately is how a header total
/// starts disagreeing with what was actually sent.
struct ToolOffer<'a> {
    tool: &'a McpTool,
    wire_name: Option<String>,
    /// Switched off by the user.
    disabled: bool,
    /// What this tool WOULD add to a request under its wire name, disabled or
    /// not. `None` when it has no usable name, so it can never be offered and
    /// costs nothing either way.
    definition: Option<ToolDefinition>,
}

impl ToolOffer<'_> {
    /// The definition a request actually carries, if any.
    ///
    /// Consuming rather than borrowing, because the request path is the hot
    /// caller: `get_tool_definitions` runs per LLM call over every tool of
    /// every running server, and handing back a reference would make it clone
    /// each one.
    fn into_offered(self) -> Option<ToolDefinition> {
        if self.disabled {
            return None;
        }
        self.definition
    }

    /// Chars this tool contributes when it is offered.
    fn chars(&self) -> usize {
        self.definition
            .as_ref()
            .map_or(0, |d| tool_definitions_chars(std::slice::from_ref(d)))
    }
}

/// Resolve every tool of one server into an offer.
fn tool_offers<'a>(
    server_id: &str,
    server_name: &str,
    tools: &'a [McpTool],
    disabled_tools: &[String],
) -> Vec<ToolOffer<'a>> {
    wire_tool_names(server_id, tools)
        .into_iter()
        .zip(tools)
        .map(|(wire_name, tool)| {
            let disabled = wire_name
                .as_ref()
                .is_some_and(|n| disabled_tools.iter().any(|d| d == n));
            let definition = wire_name
                .clone()
                .map(|n| mcp_tool_to_definition(n, server_name, tool));
            ToolOffer {
                tool,
                wire_name,
                disabled,
                definition,
            }
        })
        .collect()
}

/// Build a server's status from the tool list `tools_source` says to use.
fn server_status(
    server: &McpServer,
    tools: &[McpTool],
    tools_source: McpToolsSource,
    running: bool,
) -> McpServerStatus {
    let offers = tool_offers(&server.id, &server.name, tools, &server.disabled_tools);

    let chars: usize = offers
        .iter()
        .filter(|o| !o.disabled)
        .map(ToolOffer::chars)
        .sum();
    let disabled_chars: usize = offers
        .iter()
        .filter(|o| o.disabled)
        .map(ToolOffer::chars)
        .sum();

    McpServerStatus {
        id: server.id.clone(),
        name: server.name.clone(),
        running,
        auto_approve: server.auto_approve,
        dispatchable: crate::core::mcp_servers::validate_server_id(&server.id).is_ok(),
        tools_source,
        tools_observed_at: server.tools_observed_at,
        tools: offers
            .iter()
            .map(|o| McpToolStatus {
                name: o.tool.name.clone(),
                wire_name: o.wire_name.clone(),
                description: o.tool.description.clone(),
                disabled: o.disabled,
                chars: o.chars(),
                tokens: estimate_tokens_from_chars(o.chars()),
            })
            .collect(),
        chars,
        tokens: estimate_tokens_from_chars(chars),
        disabled_chars,
        disabled_tokens: estimate_tokens_from_chars(disabled_chars),
    }
}

/// FNV-1a over the name, as eight hex digits. Deliberately not `DefaultHasher`,
/// whose output is unspecified across Rust releases: a wire name has to mean
/// the same tool after an upgrade.
fn stable_tag(name: &str) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in name.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{:08x}", hash)
}

/// The wire name of every tool in `tools`, positionally aligned with it.
/// `None` for a tool that cannot be given one. See `docs/glossary.md`
/// § "Wire tool name" for what the scheme guarantees and why.
///
/// Three properties this function must keep, each load-bearing:
///
/// * Only the tool half is rewritten and budgeted. Truncating the composed
///   name can cut the `__` separator, leaving a name that parses to no server.
/// * A name is a pure function of the tool's own spelling and of whether any
///   sibling shares its rewrite. Nothing depends on list position.
/// * A name that would refer to two tools is `None` for both, never assigned
///   to one of them.
fn wire_tool_names(server_id: &str, tools: &[McpTool]) -> Vec<Option<String>> {
    let prefix = format!("mcp__{}__", server_id);
    let budget = crate::llm::validate::MAX_TOOL_NAME_LEN.saturating_sub(prefix.len());

    let bases: Vec<String> = tools
        .iter()
        .map(|t| crate::llm::validate::wire_safe_tool_name(&t.name))
        .collect();
    let mut base_claimants: HashMap<&str, usize> = HashMap::new();
    for base in &bases {
        *base_claimants.entry(base.as_str()).or_default() += 1;
    }

    // A base only one tool rewrites to stays bare, which is every tool on a
    // well-behaved server. A shared base tags EVERY claimant from its own
    // spelling, so the bare name goes unclaimed instead of landing on whoever
    // came first. Adding a colliding tool can then retire a name. It can never
    // hand that name to a different tool, which is what would carry a
    // persisted grant across.
    let candidates: Vec<Option<String>> = tools
        .iter()
        .zip(&bases)
        .map(|(tool, base)| {
            let suffix = if base_claimants[base.as_str()] == 1 {
                String::new()
            } else {
                format!("_{}", stable_tag(&tool.name))
            };
            let keep = budget.saturating_sub(suffix.len());
            (keep > 0).then(|| {
                format!(
                    "{}{}{}",
                    prefix,
                    &base[..base.floor_char_boundary(keep)],
                    suffix
                )
            })
        })
        .collect();

    // Truncation, or a tag collision, can still land two tools on one name.
    // Drop every claimant rather than pick one: picking would depend on list
    // order, and one name meaning two tools is the failure this all prevents.
    let mut name_claimants: HashMap<&str, usize> = HashMap::new();
    for name in candidates.iter().flatten() {
        *name_claimants.entry(name.as_str()).or_default() += 1;
    }
    candidates
        .iter()
        .map(|c| c.clone().filter(|n| name_claimants[n.as_str()] == 1))
        .collect()
}

/// The tool a wire name refers to, or `None` when the server offers no such
/// tool. `wire_tool_part` is what [`McpManager::parse_mcp_tool_name`] returned,
/// so re-composing the full wire name reproduces the exact string the model saw.
fn resolve_wire_tool_name<'a>(
    server_id: &str,
    tools: &'a [McpTool],
    wire_tool_part: &str,
) -> Option<&'a McpTool> {
    let wanted = format!("mcp__{}__{}", server_id, wire_tool_part);
    let position = wire_tool_names(server_id, tools)
        .iter()
        .position(|n| n.as_deref() == Some(wanted.as_str()))?;
    tools.get(position)
}

/// Convert an MCP tool to a Lucidos ToolDefinition under its wire name.
fn mcp_tool_to_definition(
    namespaced_name: String,
    server_name: &str,
    tool: &McpTool,
) -> ToolDefinition {
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

        let def = mcp_tool_to_definition(
            "mcp__blender__create_object".to_string(),
            "Blender MCP",
            &tool,
        );
        assert_eq!(def.name, "mcp__blender__create_object");
        assert!(def.description.contains("[Blender MCP]"));
        assert!(def.description.contains("Create a 3D object"));
    }

    // -----------------------------------------------------------------------
    // Wire names
    // -----------------------------------------------------------------------

    fn tools_named(names: &[&str]) -> Vec<McpTool> {
        names
            .iter()
            .map(|n| McpTool {
                name: n.to_string(),
                description: None,
                input_schema: None,
            })
            .collect()
    }

    /// Every tool the model is offered must resolve back to the tool the server
    /// actually has. Asserted for the whole list, so a rewrite that loses a
    /// tool or aliases two of them fails here.
    fn assert_round_trips(server_id: &str, tools: &[McpTool]) {
        let wire = wire_tool_names(server_id, tools);
        assert_eq!(wire.len(), tools.len());
        for (name, tool) in wire.iter().zip(tools) {
            let name = name
                .as_deref()
                .unwrap_or_else(|| panic!("{:?} got no wire name", tool.name));
            assert!(
                crate::llm::validate::is_wire_safe_tool_name(name),
                "{:?} would be rejected by the Messages API",
                name
            );
            // Parse the way dispatch does, rather than by stripping a known
            // prefix: a name that survives the API but splits wrong is exactly
            // the failure this guards.
            let (parsed_server, part) = McpManager::parse_mcp_tool_name(name)
                .unwrap_or_else(|| panic!("{:?} does not parse back", name));
            assert_eq!(
                parsed_server, server_id,
                "{:?} parsed to another server",
                name
            );
            let resolved = resolve_wire_tool_name(server_id, tools, &part)
                .unwrap_or_else(|| panic!("{:?} resolved to nothing", name));
            assert_eq!(
                resolved.name, tool.name,
                "{:?} resolved to the wrong tool",
                name
            );
        }
    }

    /// The reported wedge: Backstage names its tools with dots, so every
    /// request carrying them was rejected whole and the workspace was dead
    /// until the engine restarted.
    #[test]
    fn backstage_dotted_tool_names_become_wire_safe() {
        let tools = tools_named(&[
            "catalog.get-catalog-model-description",
            "catalog.get-catalog-entity",
            "scaffolder.list-scaffolder-tasks",
            "auth.who-am-i",
            "search.query",
            "techdocs.search-techdocs",
        ]);

        assert_eq!(
            wire_tool_names("backstage", &tools)[1].as_deref(),
            Some("mcp__backstage__catalog_get-catalog-entity")
        );
        assert_round_trips("backstage", &tools);
    }

    /// The rewrite is lossy, so two distinct tools can reduce to one name.
    /// Without the suffix, calling one would silently run the other.
    #[test]
    fn colliding_names_stay_distinct_and_each_resolves_to_itself() {
        let tools = tools_named(&["a.b", "a_b", "a-b", "a b"]);
        let wire = wire_tool_names("srv", &tools);

        let unique: std::collections::HashSet<&Option<String>> = wire.iter().collect();
        assert_eq!(unique.len(), wire.len(), "wire names must be unique");
        // `a-b` is the only tool rewriting to its base, so it stays bare. The
        // three sharing `a_b` are each tagged from their own spelling, and the
        // bare `a_b` belongs to none of them.
        assert_eq!(wire[2].as_deref(), Some("mcp__srv__a-b"));
        for i in [0, 1, 3] {
            let name = wire[i].as_deref().unwrap();
            assert!(name.starts_with("mcp__srv__a_b_"), "{name}");
            assert_ne!(name, "mcp__srv__a_b");
        }
        assert_round_trips("srv", &tools);
    }

    /// A server may return the same tools in a different order next session.
    /// If the bare name moved to the other claimant, a persisted "always
    /// allow" grant would authorize a tool the user never approved.
    #[test]
    fn a_reordered_tool_list_gives_every_tool_the_same_name() {
        let forward = tools_named(&["a.b", "a_b", "a b", "z"]);
        let reversed = tools_named(&["z", "a b", "a_b", "a.b"]);

        let named = |tools: &[McpTool]| -> Vec<(String, Option<String>)> {
            let mut pairs: Vec<(String, Option<String>)> = wire_tool_names("srv", tools)
                .into_iter()
                .zip(tools)
                .map(|(name, tool)| (tool.name.clone(), name))
                .collect();
            pairs.sort();
            pairs
        };
        assert_eq!(named(&forward), named(&reversed));
        assert_round_trips("srv", &forward);
        assert_round_trips("srv", &reversed);
    }

    /// A server update that adds a colliding tool must never move an existing
    /// name onto it. A persisted "always allow" keys on the name, so a moved
    /// name silently authorizes a tool the user never saw. Retiring the name
    /// is fine: a grant for it then matches nothing.
    #[test]
    fn adding_a_colliding_tool_never_hands_its_name_to_the_newcomer() {
        let before = tools_named(&["a_b", "unrelated"]);
        let after = tools_named(&["a_b", "unrelated", "a b"]);

        let bare = "mcp__srv__a_b";
        assert_eq!(wire_tool_names("srv", &before)[0].as_deref(), Some(bare));

        let grown = wire_tool_names("srv", &after);
        assert!(
            grown.iter().flatten().all(|n| n != bare),
            "the bare name must be retired, never reassigned: {grown:?}"
        );
        assert_eq!(
            wire_tool_names("srv", &after)[1].as_deref(),
            Some("mcp__srv__unrelated"),
            "a tool outside the collision is untouched"
        );
        assert_round_trips("srv", &after);
    }

    #[test]
    fn an_over_long_tool_name_is_truncated_and_still_resolves() {
        let long = "t".repeat(400);
        let tools = tools_named(&[&long, &format!("{}x", long)]);
        assert_round_trips("srv", &tools);
    }

    /// The longest id registration accepts leaves one character for the tool
    /// half. Truncating the COMPOSED name to fit a `_10` suffix would cut the
    /// `__` separator. That name passes the API and then parses back to no
    /// server. Budgeting the tool half instead leaves every name either
    /// dispatchable or absent.
    #[test]
    fn a_maximal_server_id_never_yields_a_name_that_loses_the_separator() {
        let server_id = "a".repeat(crate::core::mcp_servers::MAX_SERVER_ID_LEN);
        let tools = tools_named(&["one.tool", "two.tool", "three.tool"]);
        let wire = wire_tool_names(&server_id, &tools);

        // One character of budget, so the whole name is a first letter.
        // `two.tool` and `three.tool` both truncate to `t`, and neither can be
        // told apart within the budget, so both are dropped.
        assert_eq!(wire[0].as_deref(), Some(&*format!("mcp__{server_id}__o")));
        assert_eq!(wire[1], None);
        assert_eq!(wire[2], None);

        let named = format!("mcp__{server_id}__o");
        assert!(crate::llm::validate::is_wire_safe_tool_name(&named));
        assert_eq!(
            McpManager::parse_mcp_tool_name(&named),
            Some((server_id.clone(), "o".to_string())),
            "the separator must survive"
        );
        assert_eq!(
            resolve_wire_tool_name(&server_id, &tools, "o").map(|t| t.name.as_str()),
            Some("one.tool")
        );
    }

    /// Regression guard: a server whose names were already legal must keep the
    /// exact names it had, or every stored `mcp-allowed-tools` grant for it
    /// stops matching.
    #[test]
    fn an_already_safe_tool_list_keeps_its_existing_names() {
        let tools = tools_named(&["channels_list", "conversations_history", "users_search"]);
        let wire = wire_tool_names("slack", &tools);
        assert_eq!(
            wire.iter().map(|n| n.as_deref()).collect::<Vec<_>>(),
            vec![
                Some("mcp__slack__channels_list"),
                Some("mcp__slack__conversations_history"),
                Some("mcp__slack__users_search"),
            ]
        );
        assert_round_trips("slack", &tools);
    }

    #[test]
    fn resolve_rejects_a_name_no_tool_claims() {
        let tools = tools_named(&["catalog.get-catalog-entity"]);
        assert!(resolve_wire_tool_name("backstage", &tools, "nope").is_none());
        // The server's own spelling is NOT the wire name, and only the wire
        // name is a valid request: it is the only one a model was ever shown.
        assert!(
            resolve_wire_tool_name("backstage", &tools, "catalog.get-catalog-entity").is_none()
        );
    }

    // -----------------------------------------------------------------------
    // Context cost
    // -----------------------------------------------------------------------

    /// A tool with a real description and schema, so the cost figures are not
    /// all the flat per-definition overhead.
    fn priced_tools(names: &[&str]) -> Vec<McpTool> {
        names
            .iter()
            .map(|n| McpTool {
                name: n.to_string(),
                description: Some(format!("Does {n}, at some length, for the model to read.")),
                input_schema: Some(serde_json::json!({
                    "type": "object",
                    "properties": { "target": { "type": "string", "description": n } },
                    "required": ["target"],
                })),
            })
            .collect()
    }

    fn server_row(id: &str, disabled: &[&str]) -> McpServer {
        McpServer {
            id: id.to_string(),
            name: format!("{id} server"),
            command: "cmd".to_string(),
            args: Vec::new(),
            env: HashMap::new(),
            auto_approve: false,
            created_at: chrono::Utc::now(),
            tools: Vec::new(),
            tools_observed_at: None,
            disabled_tools: disabled.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// The number the page shows must be the number the request pays. Both come
    /// from `tool_definitions_chars` over the same definitions, and this is the
    /// guard against a second ratio appearing on the reporting side.
    #[test]
    fn per_server_cost_equals_tool_definitions_chars_over_the_same_definitions() {
        let tools = priced_tools(&["alpha", "beta", "gamma"]);
        let server = server_row("srv", &[]);
        let status = server_status(&server, &tools, McpToolsSource::Live, true);

        // Independently: the definitions this server actually contributes.
        let definitions: Vec<ToolDefinition> = tool_offers("srv", &server.name, &tools, &[])
            .into_iter()
            .filter_map(|o| o.definition)
            .collect();
        assert_eq!(definitions.len(), 3);
        assert_eq!(status.chars, tool_definitions_chars(&definitions));
        assert_eq!(status.tokens, estimate_tokens_from_chars(status.chars));
        assert!(status.chars > 0, "a real schema is not free");

        // Per tool, against the same helper over one definition.
        for (tool_status, definition) in status.tools.iter().zip(&definitions) {
            assert_eq!(
                tool_status.chars,
                tool_definitions_chars(std::slice::from_ref(definition))
            );
            assert_eq!(
                tool_status.tokens,
                estimate_tokens_from_chars(tool_status.chars)
            );
        }
        // The parts sum to the whole in chars. Tokens deliberately do not: the
        // ratio is integer division, so the server figure is computed from the
        // server's chars rather than by adding rounded per-tool figures.
        assert_eq!(
            status.tools.iter().map(|t| t.chars).sum::<usize>(),
            status.chars
        );
    }

    /// The disabled set is the lever, so it has to move cost out of the
    /// per-request total AND out of the definitions the request carries.
    #[test]
    fn a_disabled_tool_leaves_the_definitions_and_moves_to_its_own_subtotal() {
        let tools = priced_tools(&["alpha", "beta"]);
        let all_on = server_status(&server_row("srv", &[]), &tools, McpToolsSource::Live, true);

        let server = server_row("srv", &["mcp__srv__beta"]);
        let status = server_status(&server, &tools, McpToolsSource::Live, true);

        assert_eq!(
            status.tools.iter().filter(|t| t.disabled).count(),
            1,
            "only the named tool is off"
        );
        assert!(
            status.tools[0].wire_name.as_deref() == Some("mcp__srv__alpha")
                && !status.tools[0].disabled
        );
        assert!(status.tools[1].disabled);

        // The cost moved rather than vanishing, so the switch visibly pays.
        assert_eq!(status.chars + status.disabled_chars, all_on.chars);
        assert_eq!(status.disabled_chars, all_on.tools[1].chars);
        assert!(status.chars < all_on.chars);

        // And the definition itself is gone from what a request would carry.
        let offered: Vec<String> = tool_offers("srv", &server.name, &tools, &server.disabled_tools)
            .into_iter()
            .filter_map(|o| o.into_offered().map(|d| d.name))
            .collect();
        assert_eq!(offered, vec!["mcp__srv__alpha".to_string()]);
    }

    /// "Never observed" and "observed, and it has nothing" both show an empty
    /// tool list. Reporting the first as zero cost states something the engine
    /// does not know.
    #[test]
    fn a_never_observed_server_is_not_a_zero_cost_one() {
        let unobserved = server_status(
            &server_row("a", &[]),
            &[],
            McpToolsSource::NeverObserved,
            false,
        );
        let mut row = server_row("b", &[]);
        row.tools_observed_at = Some(chrono::Utc::now());
        let empty = server_status(&row, &[], McpToolsSource::Cache, false);

        assert_eq!(unobserved.tools_source, McpToolsSource::NeverObserved);
        assert!(unobserved.tools_observed_at.is_none());
        assert_eq!(empty.tools_source, McpToolsSource::Cache);
        assert!(empty.tools_observed_at.is_some());

        // Both report zero, which is exactly why the source has to be carried.
        assert_eq!(unobserved.chars, 0);
        assert_eq!(empty.chars, 0);

        // Neither counts toward what switching servers on would cost.
        let totals = McpCostTotals::of(&[unobserved, empty]);
        assert_eq!(totals.servers, 2);
        assert_eq!(totals.running_servers, 0);
        assert_eq!(totals.stopped_tokens, 0);
    }

    /// A server whose stored id cannot ride a wire tool name can never be
    /// started, so its tools must not be counted as available.
    #[test]
    fn an_undispatchable_server_is_excluded_from_the_switch_on_total() {
        let tools = priced_tools(&["alpha", "beta"]);
        let mut bad = server_row("back.stage", &["mcp__back.stage__beta"]);
        bad.tools_observed_at = Some(chrono::Utc::now());
        let bad = server_status(&bad, &tools, McpToolsSource::Cache, false);

        let mut good = server_row("slack", &[]);
        good.tools_observed_at = Some(chrono::Utc::now());
        let good = server_status(&good, &tools, McpToolsSource::Cache, false);

        assert!(!bad.dispatchable);
        assert!(good.dispatchable);
        assert!(
            bad.disabled_chars > 0,
            "the row itself still reports what it holds"
        );

        let totals = McpCostTotals::of(&[bad.clone(), good.clone()]);
        assert_eq!(totals.servers, 2, "it is still a registered server");
        assert_eq!(
            totals.stopped_chars, good.chars,
            "only the server that CAN be switched on is counted"
        );
        assert_eq!(totals.stopped_tools, 2);
        assert_eq!(
            totals.disabled_chars, 0,
            "re-enabling a tool on an unusable server gives nothing back, so it \
             is not in the disabled subtotal either"
        );
        assert_eq!(totals.disabled_tools, 0);
    }

    /// A running server pays now; a stopped one only would. The header splits
    /// them, and a disabled tool is in neither.
    #[test]
    fn totals_split_what_is_paid_from_what_would_be() {
        let tools = priced_tools(&["alpha", "beta"]);
        let up = server_status(
            &server_row("up", &["mcp__up__beta"]),
            &tools,
            McpToolsSource::Live,
            true,
        );
        let mut down_row = server_row("down", &[]);
        down_row.tools_observed_at = Some(chrono::Utc::now());
        let down = server_status(&down_row, &tools, McpToolsSource::Cache, false);

        let totals = McpCostTotals::of(&[up.clone(), down.clone()]);
        assert_eq!(totals.servers, 2);
        assert_eq!(totals.running_servers, 1);
        assert_eq!(totals.tools, 1, "one enabled tool on the running server");
        assert_eq!(totals.chars, up.chars);
        assert_eq!(totals.tokens, estimate_tokens_from_chars(up.chars));
        assert_eq!(totals.stopped_tools, 2);
        assert_eq!(totals.stopped_chars, down.chars);
        assert_eq!(totals.disabled_tools, 1);
        assert_eq!(totals.disabled_chars, up.disabled_chars);
    }

    // -----------------------------------------------------------------------
    // Against a live server
    //
    // `McpClient::connect` spawns a real process, so "running" is only testable
    // with one. The stub is a shell script answering the two handshake calls,
    // the same shape as the Codex driver stubs in `runtime/codex_tests`.
    // -----------------------------------------------------------------------

    const STUB_SERVER: &str = r#"#!/bin/sh
echo $$ > "$STUB_PID_FILE"
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2024-11-05","serverInfo":{"name":"stub","version":"1"}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      if [ -n "$STUB_TOOLS_LIST_ERROR" ]; then
        printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32000,"message":"%s"}}\n' "$id" "$STUB_TOOLS_LIST_ERROR"
      else
        printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":%s}}\n' "$id" "$STUB_TOOLS"
      fi
      ;;
    *'"method":"tools/call"'*)
      : > "$STUB_CALL_STARTED_FILE"
      if [ -n "$STUB_CALL_DELAY" ]; then
        sleep "$STUB_CALL_DELAY"
      fi
      if [ -n "$STUB_CALL_FLOOD_BYTES" ]; then
        head -c "$STUB_CALL_FLOOD_BYTES" /dev/zero | tr '\000' 'x'
      else
        printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"stub ran"}]}}\n' "$id"
      fi
      ;;
  esac
done
"#;

    struct StubServer {
        _dir: tempfile::TempDir,
        command: String,
        env: HashMap<String, String>,
        pid_file: std::path::PathBuf,
        call_started_file: std::path::PathBuf,
    }

    impl StubServer {
        fn new(tools: &[McpTool]) -> Self {
            let dir = tempfile::TempDir::new().expect("tempdir");
            let script = dir.path().join("mcp-stub.sh");
            std::fs::write(&script, STUB_SERVER).expect("write stub");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let pid_file = dir.path().join("stub.pid");
            let call_started_file = dir.path().join("call-started");
            let env = HashMap::from([
                (
                    "STUB_TOOLS".to_string(),
                    serde_json::to_string(tools).unwrap(),
                ),
                ("STUB_PID_FILE".to_string(), pid_file.display().to_string()),
                (
                    "STUB_CALL_STARTED_FILE".to_string(),
                    call_started_file.display().to_string(),
                ),
            ]);
            Self {
                _dir: dir,
                command: script.display().to_string(),
                env,
                pid_file,
                call_started_file,
            }
        }

        /// Sleep before answering every `tools/call`, so another server can be
        /// shown running while this one is busy.
        fn slow_to_answer(mut self, seconds: u32) -> Self {
            self.env
                .insert("STUB_CALL_DELAY".to_string(), seconds.to_string());
            self
        }

        /// Answer `tools/call` with a run of bytes and no newline, the shape a
        /// broken or hostile server uses to grow the reader's buffer.
        fn floods_on_call(mut self, bytes: usize) -> Self {
            self.env
                .insert("STUB_CALL_FLOOD_BYTES".to_string(), bytes.to_string());
            self
        }

        /// Fail `tools/list` with a JSON-RPC error, which is NOT the same as
        /// advertising no tools.
        fn fails_tools_list(mut self, message: &str) -> Self {
            self.env
                .insert("STUB_TOOLS_LIST_ERROR".to_string(), message.to_string());
            self
        }

        /// Whether the spawned process is still alive. `kill -0` only probes.
        fn is_alive(&self) -> bool {
            let Ok(pid) = std::fs::read_to_string(&self.pid_file) else {
                return false;
            };
            std::process::Command::new("kill")
                .args(["-0", pid.trim()])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        }

        /// Whether a `tools/call` has reached the process.
        fn call_started(&self) -> bool {
            self.call_started_file.exists()
        }

        /// Block until a `tools/call` reaches the process, so what follows is
        /// measured against a call genuinely in flight.
        async fn await_call_started(&self) {
            for _ in 0..250 {
                if self.call_started() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            panic!("the stub never received a tools/call");
        }

        /// Block until the process is gone, which `Drop for McpClient` sees to
        /// with `start_kill` on every path that abandons a client.
        async fn await_exit(&self) {
            for _ in 0..150 {
                if !self.is_alive() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            panic!("the stub outlived the client that spawned it");
        }
    }

    async fn manager_with(
        pool: &sqlx::PgPool,
        id: &str,
        stub: &StubServer,
    ) -> (McpManager, crate::engine::event_bus::EventBus) {
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        McpServerStore::register(pool, &bus, id, "Stub", &stub.command, &[], &stub.env, None)
            .await
            .unwrap();
        (McpManager::new(pool.clone(), bus.clone()), bus)
    }

    /// One manager over several stub servers, for the tests that need two
    /// processes at once.
    async fn manager_with_all(pool: &sqlx::PgPool, stubs: &[(&str, &StubServer)]) -> McpManager {
        let (bus, _rx) = crate::engine::event_bus::EventBus::new(pool.clone());
        for (id, stub) in stubs {
            McpServerStore::register(pool, &bus, id, id, &stub.command, &[], &stub.env, None)
                .await
                .unwrap();
        }
        McpManager::new(pool.clone(), bus)
    }

    /// The round trip the whole feature rests on: connect, cache what the
    /// server said, stop it, and still answer what it costs.
    #[tokio::test]
    async fn a_stopped_server_still_reports_the_manifest_it_last_advertised() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha", "beta"]));
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;

        // Before any connect, the tool list is unknown rather than empty.
        let before = &manager.list_servers().await.unwrap()[0];
        assert_eq!(before.tools_source, McpToolsSource::NeverObserved);
        assert!(!before.running);
        assert!(before.tools.is_empty());

        let outcome = manager.start_server("stub").await.unwrap();
        assert_eq!(outcome, McpStartOutcome::Started { tool_count: 2 });

        let live = manager.list_servers().await.unwrap().remove(0);
        assert!(live.running);
        assert_eq!(live.tools_source, McpToolsSource::Live);
        assert_eq!(live.tools.len(), 2);
        assert!(live.chars > 0);
        assert!(live.tools_observed_at.is_some());

        // Starting again is idempotent and reports so.
        assert_eq!(
            manager.start_server("stub").await.unwrap(),
            McpStartOutcome::AlreadyRunning { tool_count: 2 }
        );

        assert!(matches!(
            manager.stop_server("stub").await.unwrap(),
            McpStopOutcome::Stopped { .. }
        ));
        assert_eq!(
            manager.stop_server("stub").await.unwrap(),
            McpStopOutcome::WasNotRunning
        );

        let cached = manager.list_servers().await.unwrap().remove(0);
        assert!(!cached.running);
        assert_eq!(
            cached.tools_source,
            McpToolsSource::Cache,
            "the manifest outlives the process"
        );
        assert_eq!(cached.tools.len(), 2);
        assert_eq!(
            cached.chars, live.chars,
            "cost is the same whether it is read live or from the cache"
        );
        assert_eq!(cached.tools_observed_at, live.tools_observed_at);

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// A start that cannot spawn must not wipe what the page knows. A broken
    /// server that reads as costing nothing is worse than a stale figure.
    #[tokio::test]
    async fn a_failed_start_leaves_the_previous_manifest_intact() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha"]));
        let (manager, bus) = manager_with(&pool, "stub", &stub).await;

        manager.start_server("stub").await.unwrap();
        manager.stop_server("stub").await.unwrap();
        let good = McpServerStore::get(&pool, "stub").await.unwrap().unwrap();
        assert_eq!(good.tools.len(), 1);

        // Re-register the same id onto a command that does not exist.
        McpServerStore::register(
            &pool,
            &bus,
            "stub",
            "Stub",
            "/nonexistent/mcp-server",
            &[],
            &HashMap::new(),
            None,
        )
        .await
        .unwrap();
        assert!(manager.start_server("stub").await.is_err());

        let after = McpServerStore::get(&pool, "stub").await.unwrap().unwrap();
        assert_eq!(after.tools.len(), 1, "the last good manifest survives");
        assert_eq!(after.tools_observed_at, good.tools_observed_at);

        let status = manager.list_servers().await.unwrap().remove(0);
        assert!(!status.running);
        assert_eq!(status.tools_source, McpToolsSource::Cache);
        assert!(status.chars > 0, "a broken server is not a free one");

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Switching a tool off has to reach the definitions the NEXT request
    /// carries, not just the DB. The running snapshot is what that path reads.
    #[tokio::test]
    async fn disabling_a_tool_removes_it_from_a_running_server_s_definitions() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha", "beta"]));
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;
        manager.start_server("stub").await.unwrap();

        let offered = |defs: Vec<ToolDefinition>| -> Vec<String> {
            let mut names: Vec<String> = defs.into_iter().map(|d| d.name).collect();
            names.sort();
            names
        };
        assert_eq!(
            offered(manager.get_tool_definitions().await),
            vec![
                "mcp__stub__alpha".to_string(),
                "mcp__stub__beta".to_string()
            ]
        );

        manager
            .set_disabled_tools("stub", &["mcp__stub__beta".to_string()], None)
            .await
            .unwrap()
            .expect("the server exists");

        assert_eq!(
            offered(manager.get_tool_definitions().await),
            vec!["mcp__stub__alpha".to_string()],
            "a disabled tool must leave the request without a restart"
        );

        let status = manager.list_servers().await.unwrap().remove(0);
        assert!(status.tools.iter().any(|t| t.disabled));
        assert!(status.disabled_chars > 0);

        // Clearing the set brings it straight back.
        manager
            .set_disabled_tools("stub", &[], None)
            .await
            .unwrap()
            .expect("the server exists");
        assert_eq!(manager.get_tool_definitions().await.len(), 2);

        assert!(manager
            .set_disabled_tools("missing", &[], None)
            .await
            .unwrap()
            .is_none());

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Dropping a tool from the definitions is not enforcement. A call made
    /// before the switch is still in flight, and a resumed turn carries the old
    /// definitions, so dispatch has to refuse it too.
    #[tokio::test]
    async fn a_disabled_tool_is_refused_at_dispatch_not_only_omitted() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha", "beta"]));
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;
        manager.start_server("stub").await.unwrap();

        manager
            .set_disabled_tools("stub", &["mcp__stub__beta".to_string()], None)
            .await
            .unwrap()
            .expect("the server exists");

        // Calling it the way an in-flight tool call would, by the wire name the
        // model was shown before the switch.
        let refused = manager
            .call_tool("stub", "beta", serde_json::json!({}))
            .await
            .expect_err("a disabled tool must not dispatch");
        let message = refused.to_string();
        assert!(message.contains("mcp__stub__beta"), "{message}");
        assert!(message.contains("switched off"), "{message}");
        assert!(
            message.contains("NOT run"),
            "the model has to be told it did not happen: {message}"
        );

        // The refusal is about the switch, not the server: its sibling still
        // reaches the process and runs.
        let (result, server_name, _auto_approve) = manager
            .call_tool("stub", "alpha", serde_json::json!({}))
            .await
            .expect("an enabled tool still dispatches");
        assert_eq!(result, "stub ran");
        assert_eq!(server_name, "Stub");

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Remove stops the process before deleting the row. The other order leaves
    /// a live MCP server with nothing left that names it.
    #[tokio::test]
    async fn removing_a_running_server_stops_the_process_first() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha"]));
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;
        manager.start_server("stub").await.unwrap();
        assert!(stub.is_alive(), "the stub should be up before removal");

        assert!(manager.remove_server("stub", None).await.unwrap());

        assert!(!stub.is_alive(), "the process must not outlive its row");
        assert!(McpServerStore::get(&pool, "stub").await.unwrap().is_none());
        assert!(
            manager.get_tool_definitions().await.is_empty(),
            "a removed server must not keep offering tools"
        );
        assert!(manager.list_servers().await.unwrap().is_empty());

        // Removing again removes nothing, which is what the route turns into a
        // 404 rather than a silent success.
        assert!(!manager.remove_server("stub", None).await.unwrap());

        crate::test_support::teardown_test_db(&db_name).await;
    }

    // -----------------------------------------------------------------------
    // The third-party boundary
    // -----------------------------------------------------------------------

    /// One slow MCP server must not stall the workspace. The global lock was
    /// held across the call. A second server could not run, and tool assembly
    /// for every thread queued behind whatever was in flight.
    #[tokio::test]
    async fn two_mcp_servers_run_tool_calls_concurrently() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let slow = StubServer::new(&priced_tools(&["alpha"])).slow_to_answer(3);
        let quick = StubServer::new(&priced_tools(&["beta"]));
        let manager = manager_with_all(&pool, &[("slow", &slow), ("quick", &quick)]).await;
        manager.start_server("slow").await.unwrap();
        manager.start_server("quick").await.unwrap();

        let budget = std::time::Duration::from_millis(1500);
        let busy = manager.call_tool("slow", "alpha", serde_json::json!({}));
        let meanwhile = async {
            slow.await_call_started().await;
            let answered = tokio::time::timeout(
                budget,
                manager.call_tool("quick", "beta", serde_json::json!({})),
            )
            .await
            .expect("a call to another server must not wait for the slow one")
            .expect("the quick server answers");
            let definitions = tokio::time::timeout(budget, manager.get_tool_definitions())
                .await
                .expect("tool assembly must not wait for a call in flight");
            (answered, definitions)
        };
        let (slow_result, (answered, definitions)) = tokio::join!(busy, meanwhile);

        assert_eq!(answered.0, "stub ran");
        assert_eq!(definitions.len(), 2, "both servers are still offered");
        assert_eq!(
            slow_result.expect("the slow server answers in the end").0,
            "stub ran"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Two calls to ONE server share a single stdio pipe pair, so they have to
    /// take turns. Each must still get its own answer.
    #[tokio::test]
    async fn two_calls_to_one_server_take_turns() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha", "beta"])).slow_to_answer(1);
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;
        manager.start_server("stub").await.unwrap();

        let started = std::time::Instant::now();
        let (first, second) = tokio::join!(
            manager.call_tool("stub", "alpha", serde_json::json!({})),
            manager.call_tool("stub", "beta", serde_json::json!({})),
        );
        let elapsed = started.elapsed();

        assert_eq!(first.expect("the first call answers").0, "stub ran");
        assert_eq!(second.expect("the second call answers").0, "stub ran");
        assert!(
            elapsed >= std::time::Duration::from_millis(1800),
            "the two must queue rather than share the pipe: {elapsed:?}"
        );

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// The switch is read at dispatch, not when the call arrived. A call can
    /// queue behind another on the same server for the whole MCP timeout. A
    /// snapshot taken on arrival would let it run after the user said no.
    #[tokio::test]
    async fn a_tool_switched_off_while_a_call_queues_is_still_refused() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha", "beta"])).slow_to_answer(3);
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;
        manager.start_server("stub").await.unwrap();

        let busy = manager.call_tool("stub", "alpha", serde_json::json!({}));
        let meanwhile = async {
            stub.await_call_started().await;
            // Queued behind the call in flight, and switched off while it
            // waits. It passes the arrival check and must fail the dispatch
            // one.
            let queued = manager.call_tool("stub", "beta", serde_json::json!({}));
            let switch_off = async {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                manager
                    .set_disabled_tools("stub", &["mcp__stub__beta".to_string()], None)
                    .await
                    .unwrap()
                    .expect("the server exists");
            };
            let (queued, ()) = tokio::join!(queued, switch_off);
            queued
        };
        let (busy, queued) = tokio::join!(busy, meanwhile);

        assert_eq!(
            busy.expect("the call in flight still answers").0,
            "stub ran"
        );
        let refused = queued.expect_err("a tool switched off mid-queue must not run");
        assert!(refused.to_string().contains("switched off"), "{refused}");

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// Stop is the user's word, and a call queued behind a slow one must not
    /// outlive it. The queued call wakes before the shutdown does, since it
    /// reached the client lock first, so only the live check turns it away.
    #[tokio::test]
    async fn a_call_queued_when_the_server_is_stopped_is_refused() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let stub = StubServer::new(&priced_tools(&["alpha", "beta"])).slow_to_answer(3);
        let (manager, _bus) = manager_with(&pool, "stub", &stub).await;
        manager.start_server("stub").await.unwrap();

        let busy = manager.call_tool("stub", "alpha", serde_json::json!({}));
        let meanwhile = async {
            stub.await_call_started().await;
            let queued = manager.call_tool("stub", "beta", serde_json::json!({}));
            let stop = async {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                manager.stop_server("stub").await.unwrap()
            };
            tokio::join!(queued, stop)
        };
        let (busy, (queued, stopped)) = tokio::join!(busy, meanwhile);

        assert_eq!(
            busy.expect("the call already in flight still answers").0,
            "stub ran"
        );
        let refused = queued.expect_err("a call queued past a Stop must not run");
        assert!(
            refused.to_string().contains("stopped or removed"),
            "{refused}"
        );
        assert!(matches!(stopped, McpStopOutcome::Stopped { .. }));
        assert!(!stub.is_alive(), "Stop still reaches the process");

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// A server that emits bytes without a newline used to grow the read buffer
    /// for the whole 30s deadline. The cap ends the call instead, and the
    /// message names the server and the limit.
    #[tokio::test]
    async fn an_oversized_frame_errors_instead_of_growing() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let flooder = StubServer::new(&priced_tools(&["alpha"]))
            .floods_on_call(client::MAX_FRAME_BYTES + 1_000_000);
        let (manager, _bus) = manager_with(&pool, "flood", &flooder).await;
        manager.start_server("flood").await.unwrap();

        let error = tokio::time::timeout(
            std::time::Duration::from_secs(20),
            manager.call_tool("flood", "alpha", serde_json::json!({})),
        )
        .await
        .expect("the cap has to fire well before the 30s deadline")
        .expect_err("an over-long frame is a protocol fault, not a result");

        let message = error.to_string();
        assert!(
            message.contains("flood"),
            "the server must be named: {message}"
        );
        assert!(
            message.contains(&client::MAX_FRAME_BYTES.to_string()),
            "the limit must be stated: {message}"
        );
        assert!(message.contains("newline"), "{message}");

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// A `tools/list` that failed is a failure, never a server with no tools.
    /// Reporting it as empty left a healthy-looking row contributing nothing.
    #[tokio::test]
    async fn a_failed_tools_list_is_a_start_failure_not_an_empty_server() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let broken = StubServer::new(&[]).fails_tools_list("catalog backend is down");
        let (manager, bus) = manager_with(&pool, "broken", &broken).await;

        let error = manager
            .start_server("broken")
            .await
            .expect_err("a probe that could not run is not a start");
        let message = error.to_string();
        assert!(message.contains("tools/list"), "{message}");
        assert!(message.contains("catalog backend is down"), "{message}");
        assert!(
            message.contains("broken"),
            "the server must be named: {message}"
        );

        // The abandoned client takes its process with it. Nothing calls
        // `shutdown` on this path, so `Drop` is what has to do it.
        broken.await_exit().await;

        // Nothing is left running, and the page still reads unknown rather
        // than "no tools".
        let status = manager.list_servers().await.unwrap().remove(0);
        assert!(!status.running);
        assert_eq!(status.tools_source, McpToolsSource::NeverObserved);
        assert!(status.tools.is_empty());
        assert!(manager.get_tool_definitions().await.is_empty());

        // A server that genuinely advertises nothing is the other thing, and
        // it starts.
        let empty = StubServer::new(&[]);
        McpServerStore::register(
            &pool,
            &bus,
            "empty",
            "Empty",
            &empty.command,
            &[],
            &empty.env,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            manager.start_server("empty").await.unwrap(),
            McpStartOutcome::Started { tool_count: 0 }
        );
        let empty_status = manager
            .list_servers()
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.id == "empty")
            .expect("the empty server is listed");
        assert!(empty_status.running);
        assert_eq!(empty_status.tools_source, McpToolsSource::Live);
        assert!(empty_status.tools.is_empty());

        crate::test_support::teardown_test_db(&db_name).await;
    }

    /// A tool with no usable wire name is never offered, so it costs nothing
    /// and cannot be disabled. Reporting a price for it would inflate a total
    /// no request ever pays.
    #[test]
    fn a_tool_with_no_wire_name_costs_nothing() {
        let server_id = "a".repeat(crate::core::mcp_servers::MAX_SERVER_ID_LEN);
        let tools = priced_tools(&["one.tool", "two.tool", "three.tool"]);
        let status = server_status(
            &server_row(&server_id, &[]),
            &tools,
            McpToolsSource::Live,
            true,
        );

        assert!(status.tools[0].wire_name.is_some());
        assert!(status.tools[0].chars > 0);
        for nameless in &status.tools[1..] {
            assert!(nameless.wire_name.is_none());
            assert_eq!(nameless.chars, 0);
            assert!(!nameless.disabled);
        }
        assert_eq!(status.chars, status.tools[0].chars);
    }
}
